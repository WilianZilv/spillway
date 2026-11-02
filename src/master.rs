//! Master: owns the RAM tier and the FUSE mount, seals and parses finished
//! files, load-balances them to destinations, runs the thief and serves the
//! panel. See README for the full picture.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tokio::sync::{Notify, Semaphore};

use crate::cluster::{Cluster, DestRt};
use crate::config::{save_atomic, MasterConfig};
use crate::hooks::{HookStats, MovedEvent, Outbox};
use crate::loc::LocCache;
use crate::node::PutAttrs;
use crate::proto::{now_ms, Entry, Kind, NodeError};
use crate::ram::{Commit, Job, Ram};
use crate::stats::{Counters, FileMeta, Labels, RecentFile, Sample, Stats};

#[derive(Debug, Clone, Default, Serialize)]
pub struct ThiefState {
    pub active: bool,
    pub since_ms: i64,
    pub episode_files: u64,
    pub episode_bytes: u64,
    pub stalled: bool,
    pub activations: u64,
}

pub struct Core {
    pub cfg: RwLock<MasterConfig>,
    cfg_path: PathBuf,
    /// Secrets that came from the environment: (path, value in the file).
    /// The file keeps its own value; the env one is never written to disk.
    pub env_secrets: Mutex<Vec<(&'static str, String)>>,
    pub ram: Ram,
    pub cluster: Arc<Cluster>,
    pub stats: Arc<Stats>,
    pub locs: LocCache,
    pub wake: Notify,
    /// Shared client for hooks (keep-alive).
    pub http: reqwest::Client,
    pub hooks: HookStats,
    /// Durable queue of the moved hook (None if its directory could not be opened).
    pub outbox: Option<Arc<Outbox>>,
    pub thief: Mutex<ThiefState>,
    pub shutting_down: AtomicBool,
    parse_sem: Arc<Semaphore>,
    /// Serializes config edits (read-modify-validate-save).
    pub cfg_edit: tokio::sync::Mutex<()>,
}

impl Core {
    pub fn new(cfg: MasterConfig, cfg_path: PathBuf) -> Arc<Self> {
        let stats = Arc::new(Stats::new());
        let cluster = Cluster::new(
            cfg.cluster.local_node_id.clone(),
            cfg.cluster.token.clone(),
            Duration::from_secs_f64(cfg.cluster.node_timeout_secs),
            cfg.mover.fail_streak,
            stats.clone(),
        );
        let outbox = match Outbox::open(&cfg.hooks.moved.outbox_dir) {
            Ok(o) => {
                if o.len() > 0 {
                    stats.event("warn", "outbox_recovered", format!("{} eventos do hook moved pendentes de uma execução anterior", o.len()));
                }
                Some(Arc::new(o))
            }
            Err(e) => {
                stats.event("error", "outbox_unavailable", format!("Outbox do hook moved indisponível em {}: {e}", cfg.hooks.moved.outbox_dir.display()));
                None
            }
        };
        let core = Arc::new(Core {
            ram: Ram::new(cfg.ram.spool_dir.clone()),
            cfg: RwLock::new(cfg),
            cfg_path,
            env_secrets: Mutex::new(Vec::new()),
            cluster,
            stats,
            locs: LocCache::default(),
            wake: Notify::new(),
            http: reqwest::Client::builder().pool_max_idle_per_host(16).tcp_nodelay(true).build().expect("http client"),
            hooks: HookStats::default(),
            outbox,
            thief: Mutex::new(ThiefState::default()),
            shutting_down: AtomicBool::new(false),
            parse_sem: Arc::new(Semaphore::new(8)),
            cfg_edit: tokio::sync::Mutex::new(()),
        });
        let cfg = core.cfg.read().clone();
        core.apply_runtime(&cfg);
        core
    }

    /// Pushes config values into the running components.
    fn apply_runtime(&self, cfg: &MasterConfig) {
        self.cluster.set_tuning(cfg.cluster.token.clone(), Duration::from_secs_f64(cfg.cluster.node_timeout_secs), cfg.mover.fail_streak);
        self.cluster.apply_dests(&cfg.destinations);
        self.cluster.local.fsync.store(cfg.mover.fsync, Ordering::Relaxed);
    }

    /// Validates, applies and persists a new configuration.
    pub fn replace_config(&self, next: MasterConfig) -> Result<()> {
        next.validate()?;
        let prev = self.cfg.read().clone();
        if next.ram.thief_max_gb != prev.ram.thief_max_gb || next.ram.hard_limit_pct != prev.ram.hard_limit_pct {
            if next.ram.manage_tmpfs {
                if next.ram.hard_limit_bytes() < self.ram.used() {
                    bail!(
                        "new RAM limit ({:.2} GiB hard) is below what is currently in RAM ({:.2} GiB)",
                        next.ram.hard_limit_bytes() as f64 / crate::config::GIB,
                        self.ram.used() as f64 / crate::config::GIB
                    );
                }
                resize_tmpfs(&next.ram.spool_dir, next.ram.tmpfs_bytes())?;
            }
        }
        for removed in prev.destinations.iter().filter(|d| !next.destinations.iter().any(|n| n.id == d.id)) {
            self.locs.invalidate_dest(&removed.id);
        }
        let mut to_disk = next.clone();
        for (path, file_value) in self.env_secrets.lock().iter() {
            *to_disk.secret_mut(path) = file_value.clone();
        }
        save_atomic(&self.cfg_path, &to_disk)?;
        *self.cfg.write() = next.clone();
        self.apply_runtime(&next);
        self.locs.clear_negative();
        self.wake.notify_one();
        Ok(())
    }

    // ------------------------------------------------------------------
    // Sealing: closed + quiet files are parsed and queued.
    // ------------------------------------------------------------------

    async fn sealer_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let settle = {
                let s = self.cfg.read().ram.settle_secs;
                if self.shutting_down.load(Ordering::Relaxed) {
                    s.min(0.5)
                } else {
                    s
                }
            };
            let ready = self.ram.take_settled(Duration::from_secs_f64(settle), 4096);
            for (rel, version, size) in ready {
                if size == 0 {
                    if self.ram.discard_if(&rel, version).is_some() {
                        Counters::add(&self.stats.c.zero_deleted, 1);
                        self.recent(&rel, 0, "empty", None, None, None);
                    }
                    continue;
                }
                let permit = self.parse_sem.clone().acquire_owned().await.unwrap();
                let core = self.clone();
                tokio::spawn(async move {
                    let path = core.ram.spool_path(&rel);
                    let meta = tokio::task::spawn_blocking(move || {
                        let t0 = Instant::now();
                        let mp4 = crate::mp4::parse_file(&path);
                        FileMeta {
                            mp4_error_code: mp4.as_ref().err().map(|e| e.code()),
                            mp4: mp4.map_err(|e| e.to_string()),
                            parse_ms: t0.elapsed().as_secs_f64() * 1000.0,
                        }
                    })
                    .await;
                    drop(permit);
                    let Ok(meta) = meta else { return };
                    let meta = Arc::new(meta);
                    if meta.mp4.is_err() && core.cfg.read().mp4.drop_invalid {
                        if core.ram.discard_if(&rel, version).is_some() {
                            core.stats.note_sealed(size, &meta);
                            Counters::add(&core.stats.c.invalid_dropped, 1);
                            core.recent(&rel, size, "invalid", None, None, Some(&meta));
                        }
                        return;
                    }
                    let decide = !core.cfg.read().hooks.validate.url.is_empty();
                    if core.ram.finish_seal(&rel, version, meta.clone(), decide) {
                        core.stats.note_sealed(size, &meta);
                        core.wake.notify_one();
                    }
                });
            }
        }
    }

    // ------------------------------------------------------------------
    // Dispatcher: queued files → destinations (one transfer task per file).
    // ------------------------------------------------------------------

    async fn dispatcher_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            }
            self.dispatch_now();
        }
    }

    fn dispatch_now(self: &Arc<Self>) {
        let mut spins = 0;
        let pressure = {
            let c = self.cfg.read();
            self.shutting_down.load(Ordering::Relaxed)
                || self.ram.used() as f64 >= c.ram.thief_max_bytes() as f64 * c.mover.pressure_pct / 100.0
        };
        while let Some((rel, avoid, size)) = self.ram.peek_next() {
            let Some(dest) = self.cluster.pick(avoid.as_deref(), pressure, size) else { break };
            let dest_id = dest.id();
            let Some(job) = self.ram.claim(&rel, &dest_id) else {
                self.cluster.release(&dest, size);
                spins += 1;
                if spins > 1000 {
                    break;
                }
                continue;
            };
            let core = self.clone();
            tokio::spawn(async move { core.transfer(job, dest, dest_id).await });
        }
    }

    async fn transfer(self: Arc<Self>, job: Job, dest: Arc<DestRt>, dest_id: String) {
        let (min_rate, avoid_secs) = {
            let c = self.cfg.read();
            (c.mover.min_rate_mbps.max(0.1), c.mover.avoid_secs)
        };
        let timeout = Duration::from_secs_f64(30.0 + job.size as f64 / (min_rate * 1024.0 * 1024.0));
        let mtime_ms = job.mtime.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let attrs = PutAttrs { mtime_ms, mode: job.mode, uid: job.uid, gid: job.gid };
        let t0 = Instant::now();
        let res = self.cluster.put(&dest, &job.rel, self.ram.spool_path(&job.rel), job.size, attrs, timeout).await;
        let elapsed = t0.elapsed();
        self.cluster.release(&dest, job.size);

        let res = match res {
            Ok(n) if n != job.size => Err(NodeError::Io(format!("size mismatch: wrote {n} of {} bytes", job.size))),
            other => other,
        };
        match res {
            Ok(_) => match self.ram.commit(&job, &dest_id) {
                Commit::Done => {
                    self.cluster.note_ok(&dest, job.size, elapsed);
                    Counters::add(&self.stats.c.moved_files, 1);
                    Counters::add(&self.stats.c.moved_bytes, job.size);
                    let name = job.rel.rsplit('/').next().unwrap_or("").to_string();
                    self.locs.put(&job.rel, &dest_id, Entry { name, kind: Kind::File, size: job.size, mtime_ms, mode: job.mode, uid: job.uid, gid: job.gid });
                    self.recent(&job.rel, job.size, "moved", Some(&dest_id), Some(elapsed), job.meta.as_deref());
                    self.stats.note_moved(job.size, job.meta.as_deref(), job.labels.as_deref());
                    let ev = MovedEvent {
                        id: format!("{}:{}", job.fid, job.version),
                        path: job.rel.clone(),
                        dest: dest_id.clone(),
                        node: dest.cfg.read().node.clone(),
                        size: job.size,
                        mtime_ms,
                        moved_at_ms: now_ms(),
                        mp4: job.meta.as_ref().map_or(serde_json::Value::Null, |m| m.mp4_json()),
                        labels: job.labels.as_deref().cloned(),
                    };
                    crate::hooks::record_moved(&self, ev).await;
                    if let Some(origin) = job.origin.as_deref().filter(|o| *o != dest_id) {
                        // copy-up finished elsewhere: remove the outdated copy
                        if let Some(od) = self.cluster.dest(origin) {
                            let _ = self.cluster.delete(&od, &job.rel).await;
                        }
                    }
                }
                Commit::Deleted => {
                    self.cluster.note_ok(&dest, job.size, elapsed);
                    let _ = self.cluster.delete(&dest, &job.rel).await;
                    self.locs.invalidate(&job.rel);
                }
                Commit::Stale => {
                    self.cluster.note_ok(&dest, job.size, elapsed);
                    // The RAM version is newer; drop the outdated copy unless it was the original location.
                    if job.origin.as_deref() != Some(dest_id.as_str()) {
                        let _ = self.cluster.delete(&dest, &job.rel).await;
                    }
                    self.locs.invalidate(&job.rel);
                }
            },
            Err(NodeError::UnknownDest) => {
                // The node does not serve this destination (yet): the master's
                // view is ahead of the node's. Not the disk's fault, so no
                // failure and no growing backoff; just stay away until it is confirmed.
                tracing::debug!(file = %job.rel, dest = %dest_id, "node does not serve the destination yet; requeued");
                self.ram.requeue(&job, &dest_id, Duration::from_millis(500), Duration::from_secs(2));
            }
            Err(e) => {
                let spool_gone = !self.ram.exists_file(&job.rel);
                if !spool_gone {
                    self.cluster.note_fail(&dest, &e.to_string());
                    Counters::add(&self.stats.c.move_failures, 1);
                    let backoff = Duration::from_millis((500u64 << job.attempts.min(6)).min(30_000));
                    self.ram.requeue(&job, &dest_id, backoff, Duration::from_secs_f64(avoid_secs));
                    tracing::debug!(file = %job.rel, dest = %dest_id, error = %e, attempt = job.attempts, "transfer failed");
                }
            }
        }
        self.wake.notify_one();
    }

    /// A file refused by the validate hook, for the "recent files" list.
    pub fn recent_dropped(&self, rel: &str, size: u64, meta: Option<&FileMeta>, reason: Option<String>, labels: Option<Labels>) {
        self.stats.recent_push(RecentFile {
            t_ms: now_ms(),
            path: rel.to_string(),
            size,
            outcome: "dropped",
            dest: None,
            transfer_ms: None,
            duration_sec: meta.and_then(|m| m.duration()),
            codec: meta.and_then(|m| m.codec()),
            resolution: meta.and_then(|m| m.resolution()),
            parse_error: meta.and_then(|m| m.mp4.as_ref().err().cloned()),
            reason,
            labels,
        });
    }

    fn recent(&self, rel: &str, size: u64, outcome: &'static str, dest: Option<&str>, took: Option<Duration>, meta: Option<&FileMeta>) {
        self.stats.recent_push(RecentFile {
            t_ms: now_ms(),
            path: rel.to_string(),
            size,
            outcome,
            dest: dest.map(|d| d.to_string()),
            transfer_ms: took.map(|d| d.as_secs_f64() * 1000.0),
            duration_sec: meta.and_then(|m| m.duration()),
            codec: meta.and_then(|m| m.codec()),
            resolution: meta.and_then(|m| m.resolution()),
            parse_error: meta.and_then(|m| m.mp4.as_ref().err().cloned()),
            reason: None,
            labels: None,
        });
    }

    // ------------------------------------------------------------------
    // Thief: above the RAM ceiling, discard the oldest queued files until
    // usage is back under the resume mark. Movers keep running meanwhile.
    // ------------------------------------------------------------------

    async fn thief_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            if self.shutting_down.load(Ordering::Relaxed) {
                continue;
            }
            self.thief_tick();
        }
    }

    pub fn thief_tick(&self) {
        let (max, resume) = {
            let c = self.cfg.read();
            (c.ram.thief_max_bytes(), c.ram.thief_resume_bytes())
        };
        let used = self.ram.used();
        {
            let mut t = self.thief.lock();
            if !t.active {
                if used < max {
                    return;
                }
                *t = ThiefState { active: true, since_ms: now_ms(), activations: t.activations + 1, ..Default::default() };
                drop(t);
                self.stats.event(
                    "critical",
                    "thief_active",
                    format!("Thief ativado: RAM em {} atingiu o teto de {}; descartando os arquivos pendentes mais antigos", fmt_bytes(used), fmt_bytes(max)),
                );
            }
        }
        let mut n = 0u64;
        let mut b = 0u64;
        let mut stalled = false;
        while self.ram.used() > resume && n < 20_000 {
            match self.ram.steal_oldest() {
                Some((rel, f)) => {
                    n += 1;
                    b += f.size;
                    self.stats.note_lost(f.size, f.labels.as_deref());
                    self.recent(&rel, f.size, "stolen", None, None, f.meta.as_deref());
                }
                None => {
                    stalled = true;
                    break;
                }
            }
        }
        Counters::add(&self.stats.c.thief_files, n);
        Counters::add(&self.stats.c.thief_bytes, b);
        let mut t = self.thief.lock();
        t.episode_files += n;
        t.episode_bytes += b;
        if stalled && !t.stalled {
            t.stalled = true;
            drop(t);
            self.stats.event(
                "warn",
                "thief_stalled",
                format!("Thief sem o que descartar: RAM em {}, mas os arquivos ainda estão abertos ou em cópia", fmt_bytes(self.ram.used())),
            );
            return;
        }
        if self.ram.used() <= resume {
            let (files, bytes) = (t.episode_files, t.episode_bytes);
            t.active = false;
            t.stalled = false;
            drop(t);
            if files > 0 {
                self.stats.event("warn", "thief_standby", format!("Thief em espera: {files} arquivos ({}) descartados neste episódio", fmt_bytes(bytes)));
            } else {
                self.stats.event("info", "thief_standby", "Thief em espera: a RAM voltou ao normal sem descartar nada");
            }
        }
    }

    // ------------------------------------------------------------------
    // Housekeeping
    // ------------------------------------------------------------------

    async fn maintain_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        let mut last_prune = Instant::now();
        let mut panics_seen = 0;
        loop {
            tick.tick().await;
            self.cluster.maintain();
            let panics = crate::fs::FS_PANICS.load(Ordering::Relaxed);
            if panics > panics_seen {
                self.stats.event(
                    "error",
                    "fs_panic",
                    format!("{} operação(ões) do mount falharam por erro interno e responderam EIO; o mount segue no ar (detalhes no log)", panics - panics_seen),
                );
                panics_seen = panics;
            }
            if last_prune.elapsed() > Duration::from_secs(60) {
                last_prune = Instant::now();
                let ram = &self.ram;
                let n = ram.prune_empty_dirs(Duration::from_secs(600));
                if n > 0 {
                    tracing::debug!(dirs = n, "pruned empty spool directories");
                }
            }
        }
    }

    async fn probe_loop(self: Arc<Self>) {
        self.cluster.local.clone().probe_loop().await
    }

    async fn sampler_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let c = &self.stats.c;
        let mut last = Instant::now();
        let mut prev = [0u64; 4];
        let mut prev_dest: std::collections::HashMap<String, u64> = Default::default();
        loop {
            tick.tick().await;
            let dt = last.elapsed().as_secs_f64().max(0.001);
            last = Instant::now();
            let now = [
                Counters::get(&c.ingest_bytes),
                Counters::get(&c.moved_bytes),
                Counters::get(&c.read_ram_bytes) + Counters::get(&c.read_dest_bytes),
                Counters::get(&c.thief_bytes),
            ];
            let rate = |i: usize| (now[i].saturating_sub(prev[i])) as f64 / dt;
            let snap = self.ram.snapshot();
            let mut dest_bps = std::collections::BTreeMap::new();
            for d in self.cluster.dests() {
                let id = d.id();
                let mb = d.moved_bytes.load(Ordering::Relaxed);
                let p = prev_dest.insert(id.clone(), mb).unwrap_or(mb);
                dest_bps.insert(id, mb.saturating_sub(p) as f64 / dt);
            }
            self.stats.push_sample(Sample {
                t_ms: now_ms(),
                ram_used: snap.used_bytes,
                ram_files: snap.files,
                queued: snap.queued,
                inflight: snap.inflight,
                ingest_bps: rate(0),
                egress_bps: rate(1),
                read_bps: rate(2),
                thief_bps: rate(3),
                dest_bps,
            });
            prev = now;
        }
    }

    pub fn start_background(self: &Arc<Self>) {
        tokio::spawn(self.clone().sealer_loop());
        tokio::spawn(self.clone().dispatcher_loop());
        tokio::spawn(self.clone().thief_loop());
        tokio::spawn(self.clone().maintain_loop());
        tokio::spawn(self.clone().probe_loop());
        tokio::spawn(self.clone().sampler_loop());
        tokio::spawn(crate::hooks::validator_loop(self.clone()));
        if let Some(o) = &self.outbox {
            tokio::spawn(crate::hooks::moved_loop(self.clone(), o.clone()));
        }
    }

    /// Shutdown: keep moving for up to `shutdown_flush_secs`. Whatever is left
    /// (e.g. files still open) stays in the tmpfs spool and is recovered on restart.
    pub async fn drain(&self, limit: Duration) {
        self.shutting_down.store(true, Ordering::Relaxed);
        let t0 = Instant::now();
        let mut last_log = Instant::now();
        loop {
            let (files, open) = self.ram.pending();
            if files == 0 {
                tracing::info!("RAM fully flushed to destinations");
                // give the moved hook a moment; whatever is left stays in the outbox
                let t1 = Instant::now();
                while self.outbox.as_ref().is_some_and(|o| o.len() > 0) && t1.elapsed() < Duration::from_secs(5) && t0.elapsed() < limit {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                return;
            }
            if t0.elapsed() >= limit {
                tracing::warn!(files, open, "shutdown flush timeout: remaining files stay in the spool and are recovered on restart");
                return;
            }
            if !self.cluster.any_writable() && t0.elapsed() > Duration::from_secs(5) {
                tracing::warn!(files, "no writable destination: leaving files in the spool");
                return;
            }
            if last_log.elapsed() > Duration::from_secs(2) {
                last_log = Instant::now();
                tracing::info!(files, open, "flushing RAM before exit");
            }
            self.wake.notify_one();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

pub fn fmt_bytes(n: u64) -> String {
    let f = n as f64;
    if f >= 1024.0 * 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} TiB", f / (1024.0f64.powi(4)))
    } else if f >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GiB", f / (1024.0f64.powi(3)))
    } else if f >= 1024.0 * 1024.0 {
        format!("{:.1} MiB", f / (1024.0 * 1024.0))
    } else if f >= 1024.0 {
        format!("{:.1} KiB", f / 1024.0)
    } else {
        format!("{n} B")
    }
}

// ----------------------------------------------------------------------
// Mount helpers
// ----------------------------------------------------------------------

/// (mountpoint, fstype) pairs from /proc/self/mountinfo.
fn mounts() -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo") else { return Vec::new() };
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(' ').collect();
            let mp = f.get(4)?;
            let dash = f.iter().position(|x| *x == "-")?;
            let fstype = f.get(dash + 1)?;
            Some((unescape_mount(mp), fstype.to_string()))
        })
        .collect()
}

fn unescape_mount(s: &str) -> String {
    s.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")
}

pub fn mount_type(path: &Path) -> Option<String> {
    let p = path.to_string_lossy().trim_end_matches('/').to_string();
    let p = if p.is_empty() { "/".to_string() } else { p };
    mounts().into_iter().rev().find(|(mp, _)| *mp == p).map(|(_, t)| t)
}

fn c_path(p: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    Ok(std::ffi::CString::new(p.as_os_str().as_bytes())?)
}

pub fn ensure_spool(cfg: &MasterConfig) -> Result<()> {
    let dir = &cfg.ram.spool_dir;
    std::fs::create_dir_all(dir).with_context(|| format!("creating spool dir {}", dir.display()))?;
    if !cfg.ram.manage_tmpfs {
        return Ok(());
    }
    let size = cfg.ram.tmpfs_bytes();
    match mount_type(dir).as_deref() {
        Some("tmpfs") => {
            // Reuse (keeps files from a previous run) and make sure the size matches.
            resize_tmpfs(dir, size)?;
            tracing::info!(dir = %dir.display(), size = %fmt_bytes(size), "reusing existing tmpfs spool");
        }
        _ => {
            if std::fs::read_dir(dir)?.next().is_some() {
                bail!("spool dir {} is not a tmpfs and is not empty; refusing to mount over it", dir.display());
            }
            let src = std::ffi::CString::new("spillway-ram")?;
            let fstype = std::ffi::CString::new("tmpfs")?;
            let opts = std::ffi::CString::new(format!("size={size},mode=0755"))?;
            let target = c_path(dir)?;
            let rc = unsafe { libc::mount(src.as_ptr(), target.as_ptr(), fstype.as_ptr(), libc::MS_NOATIME | libc::MS_NODEV | libc::MS_NOSUID, opts.as_ptr() as *const _) };
            if rc != 0 {
                bail!("mounting tmpfs on {}: {}", dir.display(), std::io::Error::last_os_error());
            }
            tracing::info!(dir = %dir.display(), size = %fmt_bytes(size), "mounted tmpfs spool");
        }
    }
    Ok(())
}

pub fn resize_tmpfs(dir: &Path, size: u64) -> Result<()> {
    if mount_type(dir).as_deref() != Some("tmpfs") {
        return Ok(());
    }
    let src = std::ffi::CString::new("spillway-ram")?;
    let fstype = std::ffi::CString::new("tmpfs")?;
    let opts = std::ffi::CString::new(format!("size={size}"))?;
    let target = c_path(dir)?;
    let rc = unsafe { libc::mount(src.as_ptr(), target.as_ptr(), fstype.as_ptr(), libc::MS_REMOUNT | libc::MS_NOATIME | libc::MS_NODEV | libc::MS_NOSUID, opts.as_ptr() as *const _) };
    if rc != 0 {
        bail!("resizing tmpfs {}: {}", dir.display(), std::io::Error::last_os_error());
    }
    Ok(())
}

/// Detaches a stale mount left by a crashed process ("Transport endpoint is not connected").
pub fn detach_stale(path: &Path) {
    if let Some(t) = mount_type(path) {
        if t.starts_with("fuse") {
            if let Ok(c) = c_path(path) {
                let rc = unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
                tracing::warn!(path = %path.display(), ok = rc == 0, "detached stale FUSE mount");
            }
        }
    }
}

pub fn lazy_unmount(path: &Path) {
    if let Ok(c) = c_path(path) {
        unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
    }
}

pub async fn run(cfg_path: PathBuf) -> Result<()> {
    let loaded = MasterConfig::load(&cfg_path)?;
    let cfg = loaded.cfg.clone();
    ensure_spool(&cfg)?;
    let core = Core::new(cfg.clone(), cfg_path);
    *core.env_secrets.lock() = loaded.env.clone();
    for (path, _) in &loaded.env {
        tracing::info!(setting = path, var = crate::config::env_var_for(path).unwrap_or("?"), "secret taken from the environment");
    }
    if !loaded.unknown.is_empty() {
        tracing::warn!(keys = ?loaded.unknown, "unknown keys in the config file (ignored)");
        core.stats.event("warn", "config_unknown_keys", format!("Chaves desconhecidas no arquivo de configuração foram ignoradas: {}", loaded.unknown.join(", ")));
    }
    let (files, bytes) = core.ram.recover(Duration::from_secs_f64(cfg.ram.settle_secs)).context("scanning spool")?;
    if files > 0 {
        core.stats.event("warn", "spool_recovered", format!("{files} arquivos ({}) recuperados da RAM de uma execução anterior", fmt_bytes(bytes)));
    }
    core.stats.event("info", "started", format!("spillway {} iniciado, mount em {}", crate::proto::VERSION, cfg.mount.path.display()));

    // First probe before accepting traffic so destinations are known-good.
    core.cluster.local.probe_all_now().await;
    core.cluster.maintain();
    core.start_background();

    let web = crate::web::serve(core.clone(), &cfg.web.listen).await?;

    detach_stale(&cfg.mount.path);
    std::fs::create_dir_all(&cfg.mount.path).ok();
    // a handle on the plain directory before the mount covers it: files a
    // recorder wrote there while we were down get rescued (see underlay.rs)
    let underlay = crate::underlay::open(&cfg.mount.path);
    let session = crate::fs::mount(core.clone(), tokio::runtime::Handle::current(), &cfg.mount)?;
    match underlay {
        Ok(dir) => {
            tokio::spawn(crate::underlay::underlay_loop(core.clone(), dir));
        }
        Err(e) => tracing::warn!(error = %e, "cannot watch the directory under the mount"),
    }
    tracing::info!(path = %cfg.mount.path.display(), "filesystem mounted");

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    tracing::info!("shutdown requested: unmounting (lazy) and flushing RAM");
    core.stats.event("info", "stopping", "Desligando: esvaziando a RAM para os destinos");
    lazy_unmount(&cfg.mount.path);
    let flush = Duration::from_secs_f64(core.cfg.read().mover.shutdown_flush_secs);
    core.drain(flush).await;
    drop(session);
    web.abort();
    Ok(())
}
