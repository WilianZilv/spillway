//! External hooks — how an application adds its own rules without spillway
//! knowing anything about them.
//!
//! * **validate** (before moving): sealed files wait in `Deciding`; they are
//!   POSTed in micro-batches — a batch leaves when it reaches `batch_max` files
//!   or when its oldest file has waited `batch_wait_ms`, with up to `inflight`
//!   batches at once. The answer per file is move / drop / hold. Errors,
//!   timeouts and omitted files follow `on_error` (fail-open by default).
//! * **moved** (after moving): every file that lands on a destination is
//!   appended to an on-disk outbox first, then delivered in batches with
//!   retries until the receiver answers 2xx — at-least-once, survives crashes.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::config::ValidateHookCfg;
use crate::master::Core;
use crate::proto::now_ms;
use crate::ram::{Applied, Decision, DecisionItem};
use crate::stats::{Counters, Labels};

// ---------------------------------------------------------------- shared stats

#[derive(Debug, Clone, Default, Serialize)]
pub struct HookHealth {
    pub calls: u64,
    pub errors: u64,
    pub files: u64,
    pub last_ok_ms: i64,
    pub last_error: Option<(i64, String)>,
    pub failing: bool,
    pub ewma_ms: f64,
    pub max_batch: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ValidateStats {
    #[serde(flatten)]
    pub health: HookHealth,
    pub moved: u64,
    pub dropped: u64,
    pub held: u64,
    pub fallback: u64,
    pub stale: u64,
}

#[derive(Default)]
pub struct HookStats {
    pub validate: Mutex<ValidateStats>,
    pub moved: Mutex<HookHealth>,
}

fn note_ok(h: &mut HookHealth, files: usize, took: Duration) {
    h.calls += 1;
    h.files += files as u64;
    h.last_ok_ms = now_ms();
    h.max_batch = h.max_batch.max(files);
    let ms = took.as_secs_f64() * 1000.0;
    h.ewma_ms = if h.ewma_ms == 0.0 { ms } else { h.ewma_ms * 0.8 + ms * 0.2 };
}

fn note_err(h: &mut HookHealth, err: &str) {
    h.calls += 1;
    h.errors += 1;
    h.last_error = Some((now_ms(), err.to_string()));
}

// ---------------------------------------------------------------- validate

#[derive(Serialize)]
struct VFile<'a> {
    id: String,
    path: &'a str,
    size: u64,
    mtime_ms: i64,
    mp4: serde_json::Value,
}

#[derive(Serialize)]
struct VReq<'a> {
    batch: String,
    files: Vec<VFile<'a>>,
}

#[derive(Deserialize)]
struct VResp {
    #[serde(default)]
    decisions: Vec<VDecision>,
}

#[derive(Deserialize)]
struct VDecision {
    id: String,
    action: String,
    #[serde(default)]
    labels: Option<Labels>,
    #[serde(default)]
    retry_after_ms: Option<u64>,
    #[serde(default)]
    reason: Option<String>,
}

fn ms(t: std::time::SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

static BATCH_SEQ: AtomicU64 = AtomicU64::new(1);

pub async fn validator_loop(core: Arc<Core>) {
    let inflight = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(Notify::new());
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            _ = done.notified() => {}
        }
        let cfg = core.cfg.read().hooks.validate.clone();
        if cfg.url.is_empty() {
            if core.ram.release_deciding() > 0 {
                core.wake.notify_one();
            }
            continue;
        }
        let shutting = core.shutting_down.load(Ordering::Relaxed);
        loop {
            if inflight.load(Ordering::Relaxed) >= cfg.inflight {
                break;
            }
            let (ready, oldest) = core.ram.deciding_ready(cfg.batch_max);
            if ready == 0 {
                break;
            }
            let waited = oldest.map_or(Duration::ZERO, |o| o.elapsed());
            if ready < cfg.batch_max && waited < Duration::from_millis(cfg.batch_wait_ms) && !shutting {
                break;
            }
            let items = core.ram.take_for_decision(cfg.batch_max);
            if items.is_empty() {
                break;
            }
            inflight.fetch_add(1, Ordering::Relaxed);
            let (core2, cfg2, inflight2, done2) = (core.clone(), cfg.clone(), inflight.clone(), done.clone());
            tokio::spawn(async move {
                send_batch(&core2, &cfg2, items).await;
                inflight2.fetch_sub(1, Ordering::Relaxed);
                done2.notify_one();
            });
        }
    }
}

async fn call_validate(core: &Core, cfg: &ValidateHookCfg, items: &[DecisionItem]) -> Result<VResp, String> {
    let req = VReq {
        batch: format!("v-{:06}", BATCH_SEQ.fetch_add(1, Ordering::Relaxed)),
        files: items
            .iter()
            .map(|i| VFile {
                id: i.id(),
                path: &i.rel,
                size: i.size,
                mtime_ms: ms(i.mtime),
                mp4: i.meta.as_ref().map_or(serde_json::Value::Null, |m| m.mp4_json()),
            })
            .collect(),
    };
    let mut rb = core.http.post(&cfg.url).timeout(Duration::from_millis(cfg.timeout_ms)).json(&req);
    if !cfg.token.is_empty() {
        rb = rb.bearer_auth(&cfg.token);
    }
    let resp = rb.send().await.map_err(|e| if e.is_timeout() { format!("timeout após {} ms", cfg.timeout_ms) } else { format!("conexão: {e}") })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    resp.json::<VResp>().await.map_err(|e| format!("resposta inválida: {e}"))
}

async fn send_batch(core: &Arc<Core>, cfg: &ValidateHookCfg, items: Vec<DecisionItem>) {
    let t0 = Instant::now();
    let res = call_validate(core, cfg, &items).await;
    let fallback = || if cfg.on_error == "hold" { Decision::Hold(Duration::from_millis(cfg.hold_retry_ms)) } else { Decision::Move(None) };

    let mut answers: HashMap<String, VDecision> = HashMap::new();
    let ok = match res {
        Ok(r) => {
            for d in r.decisions {
                answers.insert(d.id.clone(), d);
            }
            let recovered = {
                let mut s = core.hooks.validate.lock();
                let was = s.health.failing;
                note_ok(&mut s.health, items.len(), t0.elapsed());
                s.health.failing = false;
                was
            };
            if recovered {
                core.stats.event("info", "hook_validate_ok", "Hook validate respondendo de novo");
            }
            true
        }
        Err(e) => {
            let first = {
                let mut s = core.hooks.validate.lock();
                note_err(&mut s.health, &e);
                let first = !s.health.failing;
                s.health.failing = true;
                first
            };
            if first {
                let what = if cfg.on_error == "hold" { "arquivos ficam na RAM até ele voltar" } else { "arquivos seguem para os discos (fail-open)" };
                core.stats.event("warn", "hook_validate_error", format!("Hook validate falhou ({e}); {what}"));
            }
            false
        }
    };

    let (mut moved, mut dropped, mut held, mut fell, mut stale) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for item in &items {
        let (decision, reason, labels) = match answers.remove(&item.id()) {
            Some(d) => match d.action.as_str() {
                "move" => (Decision::Move(d.labels.clone()), None, d.labels),
                "drop" => (Decision::Drop, d.reason, d.labels),
                "hold" => (Decision::Hold(Duration::from_millis(d.retry_after_ms.unwrap_or(cfg.hold_retry_ms))), None, None),
                _ => {
                    fell += 1;
                    (fallback(), None, None)
                }
            },
            None => {
                if ok {
                    fell += 1; // the hook skipped this file
                }
                (fallback(), None, None)
            }
        };
        match core.ram.apply_decision(item, decision) {
            Applied::Moved => moved += 1,
            Applied::Held => held += 1,
            Applied::Stale => stale += 1,
            Applied::Dropped(f) => {
                dropped += 1;
                core.stats.note_dropped(f.size, labels.as_ref());
                core.recent_dropped(&item.rel, f.size, item.meta.as_deref(), reason, labels);
            }
        }
    }
    if !ok {
        fell = items.len() as u64;
    }
    {
        let mut s = core.hooks.validate.lock();
        s.moved += moved;
        s.dropped += dropped;
        s.held += held;
        s.fallback += fell;
        s.stale += stale;
    }
    if moved > 0 {
        core.wake.notify_one();
    }
}

// ---------------------------------------------------------------- moved: outbox

/// One file that landed on a destination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovedEvent {
    pub id: String,
    pub path: String,
    pub dest: String,
    pub node: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub moved_at_ms: i64,
    pub mp4: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Labels>,
}

const SEGMENT_EVENTS: usize = 2000;
const SEGMENT_SECS: u64 = 10;

/// File side of the outbox. Only touched from blocking threads: under a busy
/// disk a write can stall for seconds, and nothing on the async runtime may
/// wait on that (it once froze the whole master during a burst).
struct OutboxIo {
    /// Segment files on disk.
    segments: BTreeSet<u64>,
    cur: u64,
    cur_file: Option<File>,
    cur_count: usize,
    cur_opened: Instant,
}

/// Undelivered events (segment number, JSON line, when queued). Memory only,
/// so the delivery loop and the panel can read it without touching the disk.
type Pending = VecDeque<(u64, String, Instant)>;

/// Append-only segment files (`seg-<n>.jsonl`) holding undelivered events. A
/// segment is deleted once every event in it has been acknowledged; on start
/// all remaining segments are replayed (duplicates are possible, losses are not).
pub struct Outbox {
    dir: PathBuf,
    // lock order: io, then pending
    io: Mutex<OutboxIo>,
    pending: Mutex<Pending>,
    pub notify: Notify,
}

impl Outbox {
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let mut segs: Vec<u64> = fs::read_dir(dir)?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter_map(|n| n.strip_prefix("seg-").and_then(|r| r.strip_suffix(".jsonl")).and_then(|x| x.parse().ok()))
            .collect();
        segs.sort_unstable();
        let mut pending = VecDeque::new();
        let now = Instant::now();
        for s in &segs {
            let f = File::open(dir.join(format!("seg-{s}.jsonl")))?;
            for line in BufReader::new(f).lines() {
                let line = line?;
                // a torn last line from a crash is skipped
                if serde_json::from_str::<MovedEvent>(&line).is_ok() {
                    pending.push_back((*s, line, now));
                }
            }
        }
        let cur = segs.last().map_or(1, |s| s + 1);
        Ok(Outbox {
            dir: dir.to_path_buf(),
            io: Mutex::new(OutboxIo { segments: segs.into_iter().collect(), cur, cur_file: None, cur_count: 0, cur_opened: now }),
            pending: Mutex::new(pending),
            notify: Notify::new(),
        })
    }

    fn seg_path(&self, n: u64) -> PathBuf {
        self.dir.join(format!("seg-{n}.jsonl"))
    }

    /// Durably records an event before it is delivered. Blocking: call it
    /// from a blocking thread (see `record_moved`).
    pub fn push(&self, ev: &MovedEvent) -> std::io::Result<()> {
        let line = serde_json::to_string(ev).map_err(std::io::Error::other)?;
        let mut g = self.io.lock();
        let mut closed = None;
        if g.cur_file.is_some() && (g.cur_count >= SEGMENT_EVENTS || g.cur_opened.elapsed() >= Duration::from_secs(SEGMENT_SECS)) {
            closed = g.cur_file.take();
            g.cur += 1;
            g.cur_count = 0;
        }
        if g.cur_file.is_none() {
            let n = g.cur;
            let f = OpenOptions::new().create(true).append(true).open(self.seg_path(n))?;
            g.cur_file = Some(f);
            g.segments.insert(n);
            g.cur_opened = Instant::now();
        }
        let cur = g.cur;
        // a single write() of the whole line: it reaches the kernel even if we crash right after
        g.cur_file.as_mut().unwrap().write_all(format!("{line}\n").as_bytes())?;
        g.cur_count += 1;
        // queued while the file lock is held, so `ack` never sees an event on
        // disk that is missing from the queue
        self.pending.lock().push_back((cur, line, Instant::now()));
        drop(g);
        self.notify.notify_one();
        if let Some(f) = closed {
            let _ = f.sync_data(); // outside the lock: other pushes keep going
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.pending.lock().len()
    }

    /// Size of the next batch and how long its oldest event has waited.
    fn ready(&self, max: usize) -> (usize, Duration) {
        let p = self.pending.lock();
        (p.len().min(max), p.front().map_or(Duration::ZERO, |e| e.2.elapsed()))
    }

    fn peek(&self, max: usize) -> Vec<String> {
        self.pending.lock().iter().take(max).map(|p| p.1.clone()).collect()
    }

    /// The first `n` pending events were delivered: forget them and delete
    /// fully delivered segments. Blocking (file deletes): call it from a
    /// blocking thread.
    fn ack(&self, n: usize) {
        let mut g = self.io.lock();
        let (keep_from, empty) = {
            let mut p = self.pending.lock();
            for _ in 0..n {
                p.pop_front();
            }
            (p.front().map_or(g.cur, |e| e.0), p.is_empty())
        };
        let cur_open = if g.cur_file.is_some() { Some(g.cur) } else { None };
        let done: Vec<u64> = g.segments.iter().copied().filter(|s| *s < keep_from && Some(*s) != cur_open).collect();
        for s in done {
            let _ = fs::remove_file(self.seg_path(s));
            g.segments.remove(&s);
        }
        // everything delivered: the open segment can go too
        if empty {
            if let Some(cur) = cur_open {
                g.cur_file = None;
                let _ = fs::remove_file(self.seg_path(cur));
                g.segments.remove(&cur);
                g.cur += 1;
                g.cur_count = 0;
            }
        }
    }

    #[cfg(test)]
    pub fn segments_on_disk(&self) -> usize {
        self.io.lock().segments.len()
    }
}

pub async fn moved_loop(core: Arc<Core>, outbox: Arc<Outbox>) {
    let mut backoff = Duration::from_millis(500);
    loop {
        let cfg = core.cfg.read().hooks.moved.clone();
        if cfg.url.is_empty() || outbox.len() == 0 {
            tokio::select! {
                _ = outbox.notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
            continue;
        }
        let (n, waited) = outbox.ready(cfg.batch_max);
        let wait = Duration::from_millis(cfg.batch_wait_ms);
        if n < cfg.batch_max && waited < wait && !core.shutting_down.load(Ordering::Relaxed) {
            tokio::select! {
                _ = outbox.notify.notified() => {}
                _ = tokio::time::sleep(wait - waited) => {}
            }
            continue;
        }
        let lines = outbox.peek(cfg.batch_max);
        let body = format!("{{\"batch\":\"m-{:06}\",\"files\":[{}]}}", BATCH_SEQ.fetch_add(1, Ordering::Relaxed), lines.join(","));
        let t0 = Instant::now();
        let mut rb = core.http.post(&cfg.url).timeout(Duration::from_millis(cfg.timeout_ms)).header("content-type", "application/json").body(body);
        if !cfg.token.is_empty() {
            rb = rb.bearer_auth(&cfg.token);
        }
        let res = match rb.send().await {
            Ok(r) if r.status().is_success() => Ok(()),
            Ok(r) => Err(format!("HTTP {}", r.status())),
            Err(e) => Err(if e.is_timeout() { format!("timeout após {} ms", cfg.timeout_ms) } else { format!("conexão: {e}") }),
        };
        match res {
            Ok(()) => {
                let (o, n) = (outbox.clone(), lines.len());
                let _ = tokio::task::spawn_blocking(move || o.ack(n)).await;
                backoff = Duration::from_millis(500);
                let recovered = {
                    let mut h = core.hooks.moved.lock();
                    let was = h.failing;
                    note_ok(&mut h, lines.len(), t0.elapsed());
                    h.failing = false;
                    was
                };
                if recovered {
                    core.stats.event("info", "hook_moved_ok", format!("Hook moved respondendo de novo; {} eventos na fila", outbox.len()));
                }
            }
            Err(e) => {
                let first = {
                    let mut h = core.hooks.moved.lock();
                    note_err(&mut h, &e);
                    let first = !h.failing;
                    h.failing = true;
                    first
                };
                if first {
                    core.stats.event("warn", "hook_moved_error", format!("Hook moved falhou ({e}); eventos guardados no outbox até ele voltar"));
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

/// Records a moved file for the moved hook (no-op when the hook is off).
/// Writes the event to the outbox on a blocking thread (the disk may stall).
pub async fn record_moved(core: &Core, ev: MovedEvent) {
    if core.cfg.read().hooks.moved.url.is_empty() {
        return;
    }
    let Some(outbox) = core.outbox.clone() else { return };
    let path = ev.path.clone();
    let res = tokio::task::spawn_blocking(move || outbox.push(&ev)).await.unwrap_or_else(|e| Err(std::io::Error::other(e)));
    if let Err(e) = res {
        Counters::add(&core.stats.c.outbox_errors, 1);
        tracing::error!(error = %e, file = %path, "could not write the moved-hook outbox");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(i: u64) -> MovedEvent {
        MovedEvent {
            id: format!("{i}:1"),
            path: format!("a/{i}.mp4"),
            dest: "d1".into(),
            node: "local".into(),
            size: i,
            mtime_ms: 0,
            moved_at_ms: 0,
            mp4: serde_json::json!({"ok": true}),
            labels: None,
        }
    }

    #[test]
    fn concurrent_pushes_and_acks_lose_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let ob = Arc::new(Outbox::open(dir.path()).unwrap());
        let writers: Vec<_> = (0..8u64)
            .map(|w| {
                let ob = ob.clone();
                std::thread::spawn(move || {
                    for i in 0..500 {
                        ob.push(&ev(w * 1000 + i)).unwrap();
                    }
                })
            })
            .collect();
        // deliver while writers are still going
        let mut delivered = 0;
        while delivered < 4000 {
            let n = ob.peek(64).len();
            if n == 0 {
                std::thread::yield_now();
                continue;
            }
            ob.ack(n);
            delivered += n;
        }
        writers.into_iter().for_each(|t| t.join().unwrap());
        assert_eq!(ob.len(), 0);
        assert_eq!(ob.segments_on_disk(), 0, "fully delivered segments are deleted");
        // nothing on disk is replayed after a restart
        assert_eq!(Outbox::open(dir.path()).unwrap().len(), 0);
    }

    #[test]
    fn queue_reads_do_not_wait_for_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let ob = Arc::new(Outbox::open(dir.path()).unwrap());
        ob.push(&ev(1)).unwrap();
        // a writer stuck on the disk holds the file lock...
        let io = ob.io.lock();
        // ...and the delivery loop / panel can still read the queue
        assert_eq!(ob.len(), 1);
        assert_eq!(ob.ready(10).0, 1);
        assert_eq!(ob.peek(10).len(), 1);
        drop(io);
    }

    #[test]
    fn outbox_survives_restart_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        {
            let o = Outbox::open(dir.path()).unwrap();
            for i in 0..5 {
                o.push(&ev(i)).unwrap();
            }
            assert_eq!(o.len(), 5);
            o.ack(2);
            assert_eq!(o.len(), 3);
        } // "crash": nothing flushed explicitly
        let o = Outbox::open(dir.path()).unwrap();
        // the partially delivered segment is replayed whole: at-least-once
        assert_eq!(o.len(), 5);
        let first: MovedEvent = serde_json::from_str(&o.peek(1)[0]).unwrap();
        assert_eq!(first.id, "0:1");
        o.push(&ev(9)).unwrap();
        assert!(o.segments_on_disk() >= 2, "new events go to a new segment");
        o.ack(6);
        assert_eq!(o.len(), 0);
        assert_eq!(o.segments_on_disk(), 0);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "delivered segments are deleted");
    }

    #[test]
    fn torn_line_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("seg-3.jsonl"), format!("{}\n{{\"id\":\"9:1\",\"pa", serde_json::to_string(&ev(1)).unwrap())).unwrap();
        let o = Outbox::open(dir.path()).unwrap();
        assert_eq!(o.len(), 1);
        o.push(&ev(2)).unwrap();
        assert!(dir.path().join("seg-4.jsonl").exists());
    }

    #[test]
    fn batch_body_is_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let o = Outbox::open(dir.path()).unwrap();
        o.push(&ev(1)).unwrap();
        o.push(&ev(2)).unwrap();
        let body = format!("{{\"batch\":\"m-1\",\"files\":[{}]}}", o.peek(10).join(","));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["files"].as_array().unwrap().len(), 2);
    }
}
