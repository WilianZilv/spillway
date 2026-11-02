//! Master-side view of nodes and destinations: health state machine, balancer
//! integration and a uniform data API that dispatches either to the embedded
//! local node (in-process) or to a remote node over HTTP.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use crate::balancer::{self, Candidate, Swrr};
use crate::config::DestCfg;
use crate::node::{NodeService, PutAttrs};
use crate::proto::{now_ms, DestAssign, Entry, Heartbeat, HeartbeatResp, NodeError, ProbeReport, VERSION};
use crate::stats::Stats;

/// A destination with less room than this before its usage limit counts as full.
const MIN_ROOM: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DestStatus {
    /// Receiving new files.
    Healthy,
    /// Usage above max_use_pct: readable, not writable.
    Full,
    /// Too many consecutive transfer failures: waits for the next good probe.
    Suspended,
    /// Probe failing (missing, read-only, not a mountpoint, hung...).
    Error,
    /// Owning node not reachable.
    Offline,
    /// No probe result yet.
    Pending,
    Draining,
    Disabled,
}

impl DestStatus {
    pub fn writable(self) -> bool {
        self == DestStatus::Healthy
    }
    /// Destinations queried for lookups/reads. Erroring/offline ones are skipped
    /// so a dead NFS never stalls the mount.
    pub fn readable(self) -> bool {
        matches!(self, DestStatus::Healthy | DestStatus::Full | DestStatus::Suspended | DestStatus::Draining)
    }
}

pub struct DestRt {
    pub cfg: RwLock<DestCfg>,
    pub inflight: AtomicU32,
    pub fail_streak: AtomicU32,
    /// Unix ms when suspended; 0 when not suspended.
    pub suspended_at: AtomicI64,
    pub moved_files: AtomicU64,
    pub moved_bytes: AtomicU64,
    pub failures: AtomicU64,
    pub read_bytes: AtomicU64,
    pub ewma_mbps: Mutex<f64>,
    pub last_transfer_ms: Mutex<f64>,
    pub last_error: Mutex<Option<(i64, String)>>,
    pub last_status: Mutex<DestStatus>,
    /// Circuit breaker: after an I/O timeout the destination is skipped by
    /// lookups and the balancer until this unix-ms instant.
    pub tripped_until: AtomicI64,
    /// Unix ms of the last successful transfer.
    pub last_ok_ms: AtomicI64,
    /// Bytes of transfers currently running into this destination.
    pub inflight_bytes: AtomicU64,
    /// (finished_at_ms, bytes) of recent transfers, to discount space the last probe has not seen yet.
    recent_writes: Mutex<std::collections::VecDeque<(i64, u64)>>,
    pub status_since_ms: AtomicI64,
    /// Current assignment generation (see `DestAssign::gen`).
    gen: AtomicU64,
}

/// Assignment generations: monotonic and seeded from the clock, so they stay
/// unique across master restarts too.
fn next_gen() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = now_ms().max(1) as u64;
    let mut cur = LAST.load(Ordering::Relaxed);
    loop {
        let next = cur.max(now - 1) + 1;
        match LAST.compare_exchange_weak(cur, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(seen) => cur = seen,
        }
    }
}

impl DestRt {
    fn new(cfg: DestCfg) -> Arc<Self> {
        Arc::new(DestRt {
            cfg: RwLock::new(cfg),
            inflight: AtomicU32::new(0),
            fail_streak: AtomicU32::new(0),
            suspended_at: AtomicI64::new(0),
            moved_files: AtomicU64::new(0),
            moved_bytes: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            read_bytes: AtomicU64::new(0),
            ewma_mbps: Mutex::new(0.0),
            last_transfer_ms: Mutex::new(0.0),
            last_error: Mutex::new(None),
            last_status: Mutex::new(DestStatus::Pending),
            status_since_ms: AtomicI64::new(now_ms()),
            inflight_bytes: AtomicU64::new(0),
            tripped_until: AtomicI64::new(0),
            last_ok_ms: AtomicI64::new(0),
            recent_writes: Mutex::new(std::collections::VecDeque::new()),
            gen: AtomicU64::new(next_gen()),
        })
    }
    pub fn id(&self) -> String {
        self.cfg.read().id.clone()
    }

    /// Free space as of now: the probe's view minus what was written after it
    /// and what is being written right now. Probes run every ~2 s, which is a
    /// lot of data at RAM speeds — without this a burst can overshoot max_use.
    pub fn effective_free(&self, r: &ProbeReport) -> u64 {
        let mut q = self.recent_writes.lock();
        let horizon = now_ms() - 60_000;
        while q.front().is_some_and(|(t, _)| *t < horizon) {
            q.pop_front();
        }
        // the probe's statvfs happens up to probe_ms before at_ms
        let seen_until = r.at_ms - r.probe_ms as i64 - 250;
        let unseen: u64 = q.iter().filter(|(t, _)| *t > seen_until).map(|(_, b)| *b).sum();
        r.free_bytes.saturating_sub(unseen).saturating_sub(self.inflight_bytes.load(Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub id: String,
    pub url: String,
    pub hostname: String,
    pub version: String,
    pub local: bool,
    pub started_at_ms: i64,
    pub last_seen_ms: i64,
    pub connected_since_ms: i64,
    #[serde(skip)]
    pub last_seen: Instant,
    #[serde(skip)]
    pub reports: Vec<ProbeReport>,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DestView {
    pub id: String,
    pub node: String,
    pub path: String,
    pub weight: f64,
    pub max_use_pct: f64,
    pub max_inflight: u32,
    pub enabled: bool,
    pub draining: bool,
    pub expect_mount: bool,
    pub status: DestStatus,
    pub status_since_ms: i64,
    pub reason: Option<String>,
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub use_pct: f64,
    pub is_mount: Option<bool>,
    pub probe_ms: Option<f64>,
    pub probe_age_s: Option<f64>,
    pub share: f64,
    pub inflight: u32,
    pub fail_streak: u32,
    pub moved_files: u64,
    pub moved_bytes: u64,
    pub failures: u64,
    pub read_bytes: u64,
    pub ewma_mbps: f64,
    pub last_transfer_ms: f64,
    pub last_error: Option<(i64, String)>,
}

pub struct Cluster {
    pub local: Arc<NodeService>,
    pub local_id: RwLock<String>,
    http: reqwest::Client,
    token: RwLock<String>,
    nodes: RwLock<HashMap<String, NodeInfo>>,
    dests: RwLock<Vec<Arc<DestRt>>>,
    swrr: Mutex<Swrr>,
    node_timeout: RwLock<Duration>,
    fail_streak_limit: AtomicU32,
    stats: Arc<Stats>,
    started: i64,
}

impl Cluster {
    pub fn new(local_id: String, token: String, node_timeout: Duration, fail_streak: u32, stats: Arc<Stats>) -> Arc<Self> {
        let local = NodeService::new(local_id.clone());
        let c = Arc::new(Cluster {
            local,
            local_id: RwLock::new(local_id),
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .pool_max_idle_per_host(64)
                .tcp_nodelay(true)
                .build()
                .expect("http client"),
            token: RwLock::new(token),
            nodes: RwLock::new(HashMap::new()),
            dests: RwLock::new(Vec::new()),
            swrr: Mutex::new(Swrr::default()),
            node_timeout: RwLock::new(node_timeout),
            fail_streak_limit: AtomicU32::new(fail_streak),
            stats,
            started: now_ms(),
        });
        c.touch_local();
        c
    }

    pub fn set_tuning(&self, token: String, node_timeout: Duration, fail_streak: u32) {
        *self.token.write() = token;
        *self.node_timeout.write() = node_timeout;
        self.fail_streak_limit.store(fail_streak, Ordering::Relaxed);
    }

    pub fn token(&self) -> String {
        self.token.read().clone()
    }

    fn touch_local(&self) {
        let id = self.local_id.read().clone();
        let mut nodes = self.nodes.write();
        let reports = self.local.reports();
        let n = nodes.entry(id.clone()).or_insert_with(|| NodeInfo {
            id: id.clone(),
            url: String::new(),
            hostname: crate::node::hostname(),
            version: VERSION.into(),
            local: true,
            started_at_ms: self.started,
            last_seen_ms: now_ms(),
            connected_since_ms: self.started,
            last_seen: Instant::now(),
            reports: Vec::new(),
            online: true,
        });
        n.last_seen = Instant::now();
        n.last_seen_ms = now_ms();
        n.reports = reports;
        n.online = true;
    }

    /// Applies the configured destination list (keeps runtime counters of unchanged ids).
    pub fn apply_dests(&self, cfgs: &[DestCfg]) {
        let mut dests = self.dests.write();
        let mut next = Vec::with_capacity(cfgs.len());
        for c in cfgs {
            match dests.iter().find(|d| d.cfg.read().id == c.id) {
                Some(d) => {
                    let (changed_location, reassigned) = {
                        let old = d.cfg.read();
                        let moved = old.node != c.node || old.path != c.path;
                        (moved, moved || old.expect_mount != c.expect_mount || (!old.enabled && c.enabled))
                    };
                    if reassigned {
                        // reports about the previous assignment no longer count
                        d.gen.store(next_gen(), Ordering::Relaxed);
                    }
                    *d.cfg.write() = c.clone();
                    if changed_location {
                        d.suspended_at.store(0, Ordering::Relaxed);
                        d.fail_streak.store(0, Ordering::Relaxed);
                    }
                    next.push(d.clone());
                }
                None => next.push(DestRt::new(c.clone())),
            }
        }
        *dests = next;
        drop(dests);
        let local_id = self.local_id.read().clone();
        self.local.set_dests(self.assignments_for(&local_id));
    }

    pub fn assignments_for(&self, node: &str) -> Vec<DestAssign> {
        self.dests
            .read()
            .iter()
            .filter_map(|d| {
                let c = d.cfg.read();
                (c.node == node && c.enabled).then(|| DestAssign {
                    id: c.id.clone(),
                    path: c.path.clone(),
                    expect_mount: c.expect_mount,
                    gen: d.gen.load(Ordering::Relaxed),
                })
            })
            .collect()
    }

    pub fn heartbeat(&self, mut hb: Heartbeat) -> Result<HeartbeatResp, &'static str> {
        // translate the node's clock into ours (clock skew between servers)
        if hb.sent_at_ms > 0 {
            let skew = now_ms() - hb.sent_at_ms;
            for r in hb.reports.iter_mut() {
                r.at_ms += skew;
            }
        }
        if !crate::config::valid_id(&hb.node_id) {
            return Err("invalid node id");
        }
        if hb.node_id == *self.local_id.read() {
            return Err("node id collides with the master's local node");
        }
        let now = now_ms();
        let mut nodes = self.nodes.write();
        let n = nodes.entry(hb.node_id.clone()).or_insert_with(|| NodeInfo {
            id: hb.node_id.clone(),
            url: hb.url.clone(),
            hostname: hb.hostname.clone(),
            version: hb.version.clone(),
            local: false,
            started_at_ms: hb.started_at_ms,
            last_seen_ms: now,
            connected_since_ms: now,
            last_seen: Instant::now(),
            reports: Vec::new(),
            online: false,
        });
        if !n.online || n.started_at_ms != hb.started_at_ms {
            n.connected_since_ms = now;
        }
        n.url = hb.url.trim_end_matches('/').to_string();
        n.hostname = hb.hostname;
        n.version = hb.version;
        n.started_at_ms = hb.started_at_ms;
        n.last_seen = Instant::now();
        n.last_seen_ms = now;
        n.reports = hb.reports;
        drop(nodes);
        Ok(HeartbeatResp { dests: self.assignments_for(&hb.node_id) })
    }

    pub fn forget_node(&self, id: &str) -> bool {
        if id == *self.local_id.read() {
            return false;
        }
        self.nodes.write().remove(id).is_some()
    }

    pub fn dests(&self) -> Vec<Arc<DestRt>> {
        self.dests.read().clone()
    }

    pub fn dest(&self, id: &str) -> Option<Arc<DestRt>> {
        self.dests.read().iter().find(|d| d.cfg.read().id == id).cloned()
    }

    fn node_online(&self, n: &NodeInfo) -> bool {
        n.local || n.last_seen.elapsed() <= *self.node_timeout.read()
    }

    /// The node's latest report for `d`. A report from an older assignment
    /// (sent before a disable / re-enable or path change reached the node)
    /// is ignored: the node may no longer serve that destination, so it stays
    /// Pending until the node confirms the current one.
    fn report_for(&self, d: &DestRt, c: &DestCfg) -> (bool, Option<ProbeReport>) {
        let gen = d.gen.load(Ordering::Relaxed);
        let current = |r: &ProbeReport| r.gen == gen || r.gen == 0;
        if c.node == *self.local_id.read() {
            return (true, self.local.report(&c.id).filter(current));
        }
        let nodes = self.nodes.read();
        match nodes.get(&c.node) {
            None => (false, None),
            Some(n) => (self.node_online(n), n.reports.iter().find(|r| r.dest_id == c.id && current(r)).cloned()),
        }
    }

    /// Status of a destination and the reason when it is not healthy.
    pub fn status(&self, d: &DestRt) -> (DestStatus, Option<String>, Option<ProbeReport>) {
        let c = d.cfg.read().clone();
        if !c.enabled {
            return (DestStatus::Disabled, None, None);
        }
        let (online, rep) = self.report_for(d, &c);
        if !online {
            return (DestStatus::Offline, Some(format!("node '{}' não está conectado", c.node)), rep);
        }
        let Some(r) = rep else { return (DestStatus::Pending, Some("aguardando o primeiro teste".into()), None) };
        let tripped = d.tripped_until.load(Ordering::Relaxed);
        if tripped > now_ms() {
            let left = (tripped - now_ms()) as f64 / 1000.0;
            return (DestStatus::Error, Some(format!("sem resposta (timeout de I/O); nova tentativa em {left:.0} s")), Some(r));
        }
        // A slow probe under heavy write load (journal flushes) is not an
        // outage while transfers keep succeeding.
        let busy_but_working = r.hung && now_ms() - d.last_ok_ms.load(Ordering::Relaxed) < 10_000;
        if !r.ok && !busy_but_working {
            return (DestStatus::Error, r.error.clone(), Some(r));
        }
        let susp = d.suspended_at.load(Ordering::Relaxed);
        if susp > 0 {
            if r.at_ms > susp {
                // A probe that started after the suspension succeeded: resume.
                d.suspended_at.store(0, Ordering::Relaxed);
                d.fail_streak.store(0, Ordering::Relaxed);
            } else {
                let err = d.last_error.lock().as_ref().map(|e| e.1.clone());
                return (DestStatus::Suspended, err, Some(r));
            }
        }
        let free = d.effective_free(&r);
        let use_pct = if r.total_bytes > 0 { 100.0 * (1.0 - free as f64 / r.total_bytes as f64) } else { 100.0 };
        // Bytes that still fit before max_use_pct. Below a minimum the destination
        // could not take a typical segment anyway — report it as full instead of
        // "healthy but never chosen".
        let room = free as f64 - (1.0 - c.max_use_pct / 100.0) * r.total_bytes as f64;
        if use_pct >= c.max_use_pct {
            return (DestStatus::Full, Some(format!("{use_pct:.1}% usado (limite {:.0}%)", c.max_use_pct)), Some(r));
        }
        if room < MIN_ROOM as f64 {
            let left = crate::master::fmt_bytes(room.max(0.0) as u64);
            return (DestStatus::Full, Some(format!("só {left} até o limite de {:.0}% ({use_pct:.1}% usado)", c.max_use_pct)), Some(r));
        }
        if c.draining {
            return (DestStatus::Draining, None, Some(r));
        }
        (DestStatus::Healthy, None, Some(r))
    }

    fn candidates(&self) -> (Vec<Arc<DestRt>>, Vec<Candidate>) {
        let mut ds = Vec::new();
        let mut cs = Vec::new();
        for d in self.dests() {
            let (st, _, rep) = self.status(&d);
            if !st.writable() {
                continue;
            }
            let c = d.cfg.read();
            cs.push(Candidate {
                id: c.id.clone(),
                free_bytes: rep.map_or(0, |r| d.effective_free(&r)),
                weight_mult: c.weight,
                inflight: d.inflight.load(Ordering::Relaxed),
                max_inflight: c.max_inflight,
            });
            drop(c);
            ds.push(d);
        }
        (ds, cs)
    }

    /// Reserves a transfer slot on the next destination. Release with `release`.
    /// Reserves a transfer slot (and `size` bytes) on the next destination. Release with `release`.
    pub fn pick(&self, avoid: Option<&str>, work_conserving: bool, size: u64) -> Option<Arc<DestRt>> {
        let (mut ds, mut cs) = self.candidates();
        // a file must fit without pushing the destination past its usage limit
        let mut i = 0;
        while i < ds.len() {
            let c = ds[i].cfg.read().max_use_pct;
            let total = self.report_for(&ds[i], &ds[i].cfg.read()).1.map_or(0, |r| r.total_bytes) as f64;
            let after = total - cs[i].free_bytes as f64 + size as f64;
            if total > 0.0 && 100.0 * after / total >= c {
                ds.remove(i);
                cs.remove(i);
            } else {
                i += 1;
            }
        }
        let idx = self.swrr.lock().pick(&cs, avoid, work_conserving)?;
        let d = ds[idx].clone();
        d.inflight.fetch_add(1, Ordering::Relaxed);
        d.inflight_bytes.fetch_add(size, Ordering::Relaxed);
        Some(d)
    }

    pub fn any_writable(&self) -> bool {
        self.dests().iter().any(|d| self.status(d).0.writable())
    }

    pub fn release(&self, d: &DestRt, size: u64) {
        d.inflight.fetch_sub(1, Ordering::Relaxed);
        d.inflight_bytes.fetch_sub(size, Ordering::Relaxed);
    }

    pub fn note_ok(&self, d: &DestRt, bytes: u64, elapsed: Duration) {
        d.recent_writes.lock().push_back((now_ms(), bytes));
        d.last_ok_ms.store(now_ms(), Ordering::Relaxed);
        d.fail_streak.store(0, Ordering::Relaxed);
        d.moved_files.fetch_add(1, Ordering::Relaxed);
        d.moved_bytes.fetch_add(bytes, Ordering::Relaxed);
        let ms = elapsed.as_secs_f64() * 1000.0;
        *d.last_transfer_ms.lock() = ms;
        if bytes >= 64 * 1024 && ms > 0.0 {
            let mbps = bytes as f64 / (1024.0 * 1024.0) / (ms / 1000.0);
            let mut e = d.ewma_mbps.lock();
            *e = if *e == 0.0 { mbps } else { *e * 0.9 + mbps * 0.1 };
        }
    }

    pub fn note_fail(&self, d: &DestRt, err: &str) {
        d.failures.fetch_add(1, Ordering::Relaxed);
        *d.last_error.lock() = Some((now_ms(), err.to_string()));
        let streak = d.fail_streak.fetch_add(1, Ordering::Relaxed) + 1;
        let limit = self.fail_streak_limit.load(Ordering::Relaxed);
        if streak >= limit && d.suspended_at.load(Ordering::Relaxed) == 0 {
            d.suspended_at.store(now_ms(), Ordering::Relaxed);
            self.stats.event(
                "warn",
                "dest_suspended",
                format!("Destino '{}' suspenso após {streak} falhas seguidas ({err}); volta no próximo teste bem-sucedido", d.id()),
            );
        }
    }

    /// Periodic housekeeping: refresh the local node's reports, mark nodes
    /// offline and emit events on status transitions.
    pub fn maintain(&self) {
        self.touch_local();
        {
            let timeout = *self.node_timeout.read();
            let mut nodes = self.nodes.write();
            for n in nodes.values_mut() {
                let online = n.local || n.last_seen.elapsed() <= timeout;
                if online != n.online {
                    n.online = online;
                    if online {
                        n.connected_since_ms = now_ms();
                        self.stats.event("info", "node_online", format!("Node '{}' conectado ({}, {})", n.id, n.hostname, n.url));
                    } else {
                        self.stats.event("error", "node_offline", format!("Node '{}' parou de enviar heartbeats", n.id));
                    }
                }
            }
        }
        let mut any_writable = false;
        let dests = self.dests();
        for d in &dests {
            let (st, reason, _) = self.status(d);
            any_writable |= st.writable();
            let mut last = d.last_status.lock();
            if *last != st {
                let id = d.id();
                let why = reason.map(|r| format!(": {r}")).unwrap_or_default();
                let (level, code, msg) = match st {
                    DestStatus::Healthy => ("info", "dest_healthy", format!("Destino '{id}' saudável")),
                    DestStatus::Full => ("warn", "dest_full", format!("Destino '{id}' cheio{why}")),
                    DestStatus::Suspended => ("warn", "dest_suspended", format!("Destino '{id}' suspenso{why}")),
                    DestStatus::Error => ("error", "dest_error", format!("Destino '{id}' com falha{why}")),
                    DestStatus::Offline => ("error", "dest_offline", format!("Destino '{id}' offline{why}")),
                    DestStatus::Pending => ("info", "dest_pending", format!("Destino '{id}' aguardando o primeiro teste")),
                    DestStatus::Draining => ("info", "dest_draining", format!("Destino '{id}' em só leitura")),
                    DestStatus::Disabled => ("info", "dest_disabled", format!("Destino '{id}' desativado")),
                };
                // Pending at startup is expected; don't spam it.
                if !(st == DestStatus::Pending && *last == DestStatus::Pending) {
                    self.stats.event(level, code, msg);
                }
                *last = st;
                d.status_since_ms.store(now_ms(), Ordering::Relaxed);
            }
        }
        static ALL_DOWN: AtomicU32 = AtomicU32::new(0);
        let was = ALL_DOWN.load(Ordering::Relaxed) == 1;
        if !any_writable && !dests.is_empty() && !was {
            ALL_DOWN.store(1, Ordering::Relaxed);
            self.stats.event("critical", "no_writable_dest", "Nenhum destino pode receber arquivos: a RAM está enchendo");
        } else if any_writable && was {
            ALL_DOWN.store(0, Ordering::Relaxed);
            self.stats.event("info", "writable_dest_back", "Destinos disponíveis de novo: esvaziando o acúmulo da RAM");
        }
    }

    pub fn nodes(&self) -> Vec<NodeInfo> {
        let mut v: Vec<NodeInfo> = self.nodes.read().values().cloned().collect();
        for n in v.iter_mut() {
            n.online = self.node_online(n);
        }
        v.sort_by(|a, b| b.local.cmp(&a.local).then(a.id.cmp(&b.id)));
        v
    }

    pub fn views(&self) -> Vec<DestView> {
        let dests = self.dests();
        let (_, cands) = self.candidates();
        let shares = balancer::shares(&cands);
        dests
            .iter()
            .map(|d| {
                let (st, reason, rep) = self.status(d);
                let c = d.cfg.read().clone();
                let share = cands.iter().position(|x| x.id == c.id).map_or(0.0, |i| shares[i]);
                let (free, total) = rep.as_ref().map_or((0, 0), |r| (d.effective_free(r), r.total_bytes));
                DestView {
                    status: st,
                    status_since_ms: d.status_since_ms.load(Ordering::Relaxed),
                    reason,
                    free_bytes: free,
                    total_bytes: total,
                    use_pct: if total > 0 { 100.0 * (1.0 - free as f64 / total as f64) } else { 0.0 },
                    is_mount: rep.as_ref().and_then(|r| r.is_mount),
                    probe_ms: rep.as_ref().map(|r| r.probe_ms),
                    probe_age_s: rep.as_ref().map(|r| (now_ms() - r.at_ms) as f64 / 1000.0),
                    share,
                    inflight: d.inflight.load(Ordering::Relaxed),
                    fail_streak: d.fail_streak.load(Ordering::Relaxed),
                    moved_files: d.moved_files.load(Ordering::Relaxed),
                    moved_bytes: d.moved_bytes.load(Ordering::Relaxed),
                    failures: d.failures.load(Ordering::Relaxed),
                    read_bytes: d.read_bytes.load(Ordering::Relaxed),
                    ewma_mbps: *d.ewma_mbps.lock(),
                    last_transfer_ms: *d.last_transfer_ms.lock(),
                    last_error: d.last_error.lock().clone(),
                    id: c.id,
                    node: c.node,
                    path: c.path,
                    weight: c.weight,
                    max_use_pct: c.max_use_pct,
                    max_inflight: c.max_inflight,
                    enabled: c.enabled,
                    draining: c.draining,
                    expect_mount: c.expect_mount,
                }
            })
            .collect()
    }

    /// Destinations to search on lookups, in configured order.
    pub fn readable(&self) -> Vec<Arc<DestRt>> {
        self.dests().into_iter().filter(|d| self.status(d).0.readable()).collect()
    }

    pub fn is_readable(&self, id: &str) -> bool {
        self.dest(id).is_some_and(|d| self.status(&d).0.readable())
    }

    // ---------------- data plane ----------------

    /// Opens the circuit breaker on errors that mean "this destination is not answering".
    /// `op` names the call that failed, for the event (stat / list / read).
    fn trip_on(&self, d: &DestRt, op: &str, e: &NodeError) {
        let unresponsive = match e {
            NodeError::Timeout | NodeError::Busy => true,
            NodeError::Io(m) => m.starts_with("network"),
            _ => false,
        };
        if !unresponsive {
            return;
        }
        let was = d.tripped_until.swap(now_ms() + 5_000, Ordering::Relaxed);
        if was < now_ms() {
            self.stats.event("warn", "dest_unresponsive", format!("Destino '{}' não respondeu ao {op} ({e}); fora das buscas por 5 s", d.id()));
        }
    }

    fn route(&self, d: &DestRt) -> Result<(String, Option<String>), NodeError> {
        let c = d.cfg.read();
        if c.node == *self.local_id.read() {
            return Ok((c.id.clone(), None));
        }
        let nodes = self.nodes.read();
        let n = nodes.get(&c.node).ok_or(NodeError::Io(format!("node '{}' unknown", c.node)))?;
        if !self.node_online(n) {
            return Err(NodeError::Io(format!("node '{}' offline", c.node)));
        }
        Ok((c.id.clone(), Some(n.url.clone())))
    }

    async fn remote_err(resp: reqwest::Response) -> NodeError {
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        match body.get("error").and_then(|e| e.as_str()) {
            Some("not_found") => NodeError::NotFound,
            Some("unknown_dest") => NodeError::UnknownDest,
            Some("bad_path") => NodeError::BadPath,
            Some("timeout") => NodeError::Timeout,
            Some("busy") => NodeError::Busy,
            _ => NodeError::Io(body.get("message").and_then(|m| m.as_str()).map_or_else(|| format!("http {status}"), |m| m.to_string())),
        }
    }

    fn net_err(e: reqwest::Error) -> NodeError {
        if e.is_timeout() {
            NodeError::Timeout
        } else {
            NodeError::Io(format!("network: {e}"))
        }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: String, timeout: Duration) -> Result<T, NodeError> {
        let r = self.http.get(url).bearer_auth(self.token()).timeout(timeout).send().await.map_err(Self::net_err)?;
        if !r.status().is_success() {
            return Err(Self::remote_err(r).await);
        }
        r.json().await.map_err(Self::net_err)
    }

    pub async fn stat(&self, d: &DestRt, rel: &str) -> Result<Option<Entry>, NodeError> {
        let (id, url) = self.route(d)?;
        let r = match url {
            None => self.local.stat(&id, rel).await,
            Some(u) => self.get_json(format!("{u}/v1/dest/{id}/stat?path={}", enc(rel)), Duration::from_millis(1500)).await,
        };
        if let Err(e) = &r {
            self.trip_on(d, "stat", e);
        }
        r
    }

    pub async fn list(&self, d: &DestRt, rel: &str) -> Result<Option<Vec<Entry>>, NodeError> {
        let (id, url) = self.route(d)?;
        let r = match url {
            None => self.local.list(&id, rel).await,
            Some(u) => self.get_json(format!("{u}/v1/dest/{id}/list?path={}", enc(rel)), Duration::from_secs(10)).await,
        };
        if let Err(e) = &r {
            self.trip_on(d, "list", e);
        }
        r
    }

    pub async fn read(&self, d: &DestRt, rel: &str, offset: u64, len: u32) -> Result<Bytes, NodeError> {
        let r = self.read_inner(d, rel, offset, len).await;
        if let Err(e) = &r {
            self.trip_on(d, "read", e);
        }
        r
    }

    async fn read_inner(&self, d: &DestRt, rel: &str, offset: u64, len: u32) -> Result<Bytes, NodeError> {
        let (id, url) = self.route(d)?;
        let b = match url {
            None => self.local.read(&id, rel, offset, len).await?,
            Some(u) => {
                let r = self
                    .http
                    .get(format!("{u}/v1/dest/{id}/read?path={}&offset={offset}&len={len}", enc(rel)))
                    .bearer_auth(self.token())
                    .timeout(Duration::from_secs(15))
                    .send()
                    .await
                    .map_err(Self::net_err)?;
                if !r.status().is_success() {
                    return Err(Self::remote_err(r).await);
                }
                r.bytes().await.map_err(Self::net_err)?
            }
        };
        d.read_bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
        Ok(b)
    }

    pub async fn delete(&self, d: &DestRt, rel: &str) -> Result<(), NodeError> {
        let (id, url) = self.route(d)?;
        match url {
            None => self.local.delete(&id, rel).await,
            Some(u) => {
                let r = self
                    .http
                    .delete(format!("{u}/v1/dest/{id}/file?path={}", enc(rel)))
                    .bearer_auth(self.token())
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await
                    .map_err(Self::net_err)?;
                if r.status().is_success() {
                    Ok(())
                } else {
                    Err(Self::remote_err(r).await)
                }
            }
        }
    }

    /// Transfers a spool file into the destination; returns bytes written.
    pub async fn put(&self, d: &DestRt, rel: &str, src: PathBuf, size: u64, attrs: PutAttrs, timeout: Duration) -> Result<u64, NodeError> {
        let (id, url) = self.route(d)?;
        match url {
            None => self.local.put_from_path(&id, rel, src, attrs, timeout).await,
            Some(u) => {
                let file = tokio::fs::File::open(&src).await.map_err(|e| NodeError::Io(format!("spool: {e}")))?;
                let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(file, 1024 * 1024));
                let r = self
                    .http
                    .put(format!(
                        "{u}/v1/dest/{id}/file?path={}&mtime_ms={}&mode={}&uid={}&gid={}&size={size}",
                        enc(rel),
                        attrs.mtime_ms,
                        attrs.mode,
                        attrs.uid,
                        attrs.gid
                    ))
                    .bearer_auth(self.token())
                    .header("content-length", size)
                    .timeout(timeout)
                    .body(body)
                    .send()
                    .await
                    .map_err(Self::net_err)?;
                if !r.status().is_success() {
                    return Err(Self::remote_err(r).await);
                }
                let v: serde_json::Value = r.json().await.map_err(Self::net_err)?;
                Ok(v.get("size").and_then(|s| s.as_u64()).unwrap_or(0))
            }
        }
    }
}

/// Percent-encodes a relative path for a query string.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cluster() -> Arc<Cluster> {
        Cluster::new("local".into(), "t".into(), Duration::from_secs(6), 3, Arc::new(Stats::new()))
    }

    fn dcfg(id: &str, path: &str) -> DestCfg {
        DestCfg { id: id.into(), path: path.into(), ..Default::default() }
    }

    #[tokio::test]
    async fn local_dest_goes_healthy_after_probe() {
        let dir = tempfile::tempdir().unwrap();
        let c = cluster();
        c.apply_dests(&[dcfg("a", dir.path().to_str().unwrap())]);
        let d = c.dest("a").unwrap();
        c.maintain();
        assert_eq!(c.status(&d).0, DestStatus::Pending);
        c.local.probe_all_now().await;
        c.maintain();
        assert_eq!(c.status(&d).0, DestStatus::Healthy);
        assert!(c.pick(None, false, 1).is_some());
    }

    #[tokio::test]
    async fn fail_streak_suspends_until_next_probe() {
        let dir = tempfile::tempdir().unwrap();
        let c = cluster();
        c.apply_dests(&[dcfg("a", dir.path().to_str().unwrap())]);
        c.local.probe_all_now().await;
        let d = c.dest("a").unwrap();
        for _ in 0..3 {
            c.note_fail(&d, "boom");
        }
        assert_eq!(c.status(&d).0, DestStatus::Suspended);
        assert!(c.pick(None, true, 1).is_none());
        tokio::time::sleep(Duration::from_millis(5)).await;
        c.local.probe_all_now().await;
        c.maintain();
        assert_eq!(c.status(&d).0, DestStatus::Healthy);
        assert_eq!(d.fail_streak.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn full_and_disabled_and_remote_offline() {
        let dir = tempfile::tempdir().unwrap();
        let c = cluster();
        let mut full = dcfg("full", dir.path().to_str().unwrap());
        full.max_use_pct = 1.0; // any real fs is above 1% used... unless empty; use 1% to be safe
        let mut off = dcfg("off", "/x");
        off.enabled = false;
        let mut remote = dcfg("r", "/y");
        remote.node = "far".into();
        c.apply_dests(&[full, off, remote]);
        c.local.probe_all_now().await;
        let st = |id: &str| c.status(&c.dest(id).unwrap()).0;
        assert!(matches!(st("full"), DestStatus::Full | DestStatus::Healthy));
        assert_eq!(st("off"), DestStatus::Disabled);
        assert_eq!(st("r"), DestStatus::Offline);
        // node joins and reports
        let hb = Heartbeat {
            node_id: "far".into(),
            url: "http://127.0.0.1:1".into(),
            hostname: "h".into(),
            version: "v".into(),
            started_at_ms: 1,
            reports: vec![ProbeReport { dest_id: "r".into(), ok: true, error: None, free_bytes: 10 << 30, total_bytes: 20 << 30, is_mount: Some(true), probe_ms: 1.0, at_ms: now_ms(), hung: false, gen: 0 }],
            sent_at_ms: now_ms(),
        };
        let resp = c.heartbeat(hb).unwrap();
        assert_eq!(resp.dests.len(), 1);
        assert_eq!(st("r"), DestStatus::Healthy);
        assert!(c.heartbeat(Heartbeat { node_id: "local".into(), url: String::new(), hostname: String::new(), version: String::new(), started_at_ms: 0, reports: vec![], sent_at_ms: 0 }).is_err());
    }

    #[test]
    fn reenabled_remote_waits_for_a_report_of_the_new_assignment() {
        let c = cluster();
        let mut r = dcfg("r", "/y");
        r.node = "far".into();
        c.apply_dests(&[r.clone()]);
        let st = || c.status(&c.dest("r").unwrap()).0;
        let report = |gen| ProbeReport { dest_id: "r".into(), ok: true, error: None, free_bytes: 10 << 30, total_bytes: 20 << 30, is_mount: Some(true), probe_ms: 1.0, at_ms: now_ms(), hung: false, gen };
        let hb = |reports| Heartbeat { node_id: "far".into(), url: "http://127.0.0.1:1".into(), hostname: "h".into(), version: "v".into(), started_at_ms: 1, reports, sent_at_ms: now_ms() };
        let g1 = c.heartbeat(hb(vec![])).unwrap().dests[0].gen;
        c.heartbeat(hb(vec![report(g1)])).unwrap();
        assert_eq!(st(), DestStatus::Healthy);

        // disable and re-enable before the node's next heartbeat: the report
        // it last sent is about the old assignment and must not count
        r.enabled = false;
        c.apply_dests(&[r.clone()]);
        assert!(c.assignments_for("far").is_empty());
        r.enabled = true;
        c.apply_dests(&[r.clone()]);
        assert_eq!(st(), DestStatus::Pending);
        assert!(c.pick(None, true, 1).is_none());
        let resp = c.heartbeat(hb(vec![report(g1)])).unwrap();
        let g2 = resp.dests[0].gen;
        assert!(g2 > g1);
        assert_eq!(st(), DestStatus::Pending);
        // the node confirms the new generation
        c.heartbeat(hb(vec![report(g2)])).unwrap();
        assert_eq!(st(), DestStatus::Healthy);
        assert!(c.pick(None, true, 1).is_some());

        // a weight change is not a reassignment
        r.weight = 2.0;
        c.apply_dests(&[r.clone()]);
        assert_eq!(c.assignments_for("far")[0].gen, g2);
        // nor does an older node (no gen in its reports) get stuck
        c.heartbeat(hb(vec![report(0)])).unwrap();
        assert_eq!(st(), DestStatus::Healthy);
    }

    #[test]
    fn generations_are_unique_and_increasing() {
        let mut last = 0;
        for _ in 0..10_000 {
            let g = next_gen();
            assert!(g > last);
            last = g;
        }
    }

    #[test]
    fn encodes_paths() {
        assert_eq!(enc("a b/ç.mp4"), "a%20b/%C3%A7.mp4");
        assert_eq!(enc("x&y=z"), "x%26y%3Dz");
    }
}
