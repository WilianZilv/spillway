//! Counters, time series, per-label aggregates, recent files and the event log
//! that feed the dashboard. Spillway knows nothing about what a file *is*:
//! grouping comes from the labels a validate hook returns.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Serialize;

use crate::mp4::Mp4Info;
use crate::proto::now_ms;

const SERIES_FINE: usize = 900; // 15 min of 1 s samples
const SERIES_COARSE: usize = 1440; // 24 h of 1 min samples
const MAX_EVENTS: usize = 500;
const MAX_RECENT: usize = 200;
const MAX_GROUPS: usize = 50_000;

/// Free-form key/value tags attached by the validate hook (e.g. camera, tenant).
pub type Labels = BTreeMap<String, String>;

pub const DURATION_EDGES: [f64; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 300.0, 600.0, 1800.0];
pub const SIZE_EDGES_MB: [f64; 9] = [0.0, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0];

#[derive(Default)]
pub struct Counters {
    pub ingest_bytes: AtomicU64,
    pub files_created: AtomicU64,
    pub files_sealed: AtomicU64,
    pub zero_deleted: AtomicU64,
    pub invalid_dropped: AtomicU64,
    pub moved_files: AtomicU64,
    pub moved_bytes: AtomicU64,
    pub move_failures: AtomicU64,
    pub thief_files: AtomicU64,
    pub thief_bytes: AtomicU64,
    pub read_ram_bytes: AtomicU64,
    pub read_dest_bytes: AtomicU64,
    pub write_rejected: AtomicU64,
    pub hook_dropped: AtomicU64,
    pub hook_dropped_bytes: AtomicU64,
    pub outbox_errors: AtomicU64,
}

impl Counters {
    pub fn add(c: &AtomicU64, n: u64) {
        c.fetch_add(n, Ordering::Relaxed);
    }
    pub fn get(c: &AtomicU64) -> u64 {
        c.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub seq: u64,
    pub t_ms: i64,
    pub level: &'static str,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Sample {
    pub t_ms: i64,
    pub ram_used: u64,
    pub ram_files: u64,
    pub queued: u64,
    pub inflight: u64,
    pub ingest_bps: f64,
    pub egress_bps: f64,
    pub read_bps: f64,
    pub thief_bps: f64,
    pub dest_bps: BTreeMap<String, f64>,
}

/// Aggregate for one label value (e.g. key "camera", value "3f2a…").
#[derive(Debug, Clone, Serialize, Default)]
pub struct GroupStat {
    pub key: String,
    pub value: String,
    /// Files moved to a destination.
    pub files: u64,
    pub bytes: u64,
    pub duration_sec: f64,
    pub last_ms: i64,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub parse_fail: u64,
    /// Discarded by the thief.
    pub lost_files: u64,
    pub lost_bytes: u64,
    /// Refused by the validate hook.
    pub dropped_files: u64,
    pub dropped_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecentFile {
    pub t_ms: i64,
    pub path: String,
    pub size: u64,
    pub outcome: &'static str,
    pub dest: Option<String>,
    pub transfer_ms: Option<f64>,
    pub duration_sec: Option<f64>,
    pub codec: Option<String>,
    pub resolution: Option<String>,
    pub parse_error: Option<String>,
    /// Why the hook dropped it, when it did.
    pub reason: Option<String>,
    pub labels: Option<Labels>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Mp4Stats {
    pub ok: u64,
    pub failed: u64,
    pub fail_reasons: BTreeMap<String, u64>,
    pub fragmented: u64,
    pub progressive: u64,
    pub codecs: BTreeMap<String, u64>,
    pub resolutions: BTreeMap<String, u64>,
    pub duration_hist: Vec<u64>,
    pub size_hist: Vec<u64>,
    pub duration_total: f64,
    pub bytes_parsed: u64,
}

/// Per-file analysis result attached when a file is sealed.
#[derive(Debug, Clone, Serialize)]
pub struct FileMeta {
    pub mp4: Result<Mp4Info, String>,
    pub mp4_error_code: Option<&'static str>,
    pub parse_ms: f64,
}

impl FileMeta {
    pub fn duration(&self) -> Option<f64> {
        self.mp4.as_ref().ok().map(|i| i.duration_sec)
    }
    pub fn codec(&self) -> Option<String> {
        self.mp4.as_ref().ok().and_then(|i| i.video().map(|v| v.codec.clone()))
    }
    pub fn resolution(&self) -> Option<String> {
        self.mp4.as_ref().ok().and_then(|i| i.video().filter(|v| v.width > 0).map(|v| format!("{}x{}", v.width, v.height)))
    }

    /// What hooks receive under "mp4": the parse result, or why it failed.
    pub fn mp4_json(&self) -> serde_json::Value {
        match &self.mp4 {
            Ok(info) => {
                let mut v = serde_json::to_value(info).unwrap_or_default();
                v["ok"] = serde_json::Value::Bool(true);
                v
            }
            Err(msg) => serde_json::json!({ "ok": false, "error": self.mp4_error_code.unwrap_or("unknown"), "message": msg }),
        }
    }
}

pub struct Stats {
    pub started_ms: i64,
    pub c: Counters,
    events: Mutex<VecDeque<Event>>,
    event_seq: AtomicU64,
    fine: Mutex<VecDeque<Sample>>,
    coarse: Mutex<VecDeque<Sample>>,
    coarse_acc: Mutex<Vec<Sample>>,
    pub mp4: Mutex<Mp4Stats>,
    groups: Mutex<HashMap<(String, String), GroupStat>>,
    recent: Mutex<VecDeque<RecentFile>>,
}

fn bucket(edges: &[f64], v: f64) -> usize {
    let mut i = 0;
    while i + 1 < edges.len() && v >= edges[i + 1] {
        i += 1;
    }
    i
}

impl Stats {
    pub fn new() -> Self {
        Stats {
            started_ms: now_ms(),
            c: Counters::default(),
            events: Mutex::new(VecDeque::new()),
            event_seq: AtomicU64::new(0),
            fine: Mutex::new(VecDeque::new()),
            coarse: Mutex::new(VecDeque::new()),
            coarse_acc: Mutex::new(Vec::new()),
            mp4: Mutex::new(Mp4Stats {
                duration_hist: vec![0; DURATION_EDGES.len()],
                size_hist: vec![0; SIZE_EDGES_MB.len()],
                ..Default::default()
            }),
            groups: Mutex::new(HashMap::new()),
            recent: Mutex::new(VecDeque::new()),
        }
    }

    pub fn event(&self, level: &'static str, code: &'static str, message: impl Into<String>) {
        let message = message.into();
        match level {
            "error" | "critical" => tracing::error!(code, "{message}"),
            "warn" => tracing::warn!(code, "{message}"),
            _ => tracing::info!(code, "{message}"),
        }
        let seq = self.event_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let mut ev = self.events.lock();
        ev.push_back(Event { seq, t_ms: now_ms(), level, code, message });
        while ev.len() > MAX_EVENTS {
            ev.pop_front();
        }
    }

    pub fn events(&self, after: u64) -> Vec<Event> {
        self.events.lock().iter().filter(|e| e.seq > after).cloned().collect()
    }

    pub fn push_sample(&self, s: Sample) {
        {
            let mut f = self.fine.lock();
            f.push_back(s.clone());
            while f.len() > SERIES_FINE {
                f.pop_front();
            }
        }
        let mut acc = self.coarse_acc.lock();
        acc.push(s);
        let first = acc[0].t_ms;
        if acc.last().unwrap().t_ms - first >= 60_000 {
            let n = acc.len() as f64;
            let mut avg = Sample { t_ms: acc.last().unwrap().t_ms, ..Default::default() };
            for x in acc.iter() {
                avg.ram_used = avg.ram_used.max(x.ram_used);
                avg.ram_files = avg.ram_files.max(x.ram_files);
                avg.queued = avg.queued.max(x.queued);
                avg.inflight = avg.inflight.max(x.inflight);
                avg.ingest_bps += x.ingest_bps / n;
                avg.egress_bps += x.egress_bps / n;
                avg.read_bps += x.read_bps / n;
                avg.thief_bps += x.thief_bps / n;
                for (k, v) in &x.dest_bps {
                    *avg.dest_bps.entry(k.clone()).or_insert(0.0) += v / n;
                }
            }
            acc.clear();
            let mut c = self.coarse.lock();
            c.push_back(avg);
            while c.len() > SERIES_COARSE {
                c.pop_front();
            }
        }
    }

    /// `coarse` = 24 h at 1 min resolution, otherwise 15 min at 1 s.
    pub fn series(&self, coarse: bool) -> Vec<Sample> {
        if coarse {
            self.coarse.lock().iter().cloned().collect()
        } else {
            self.fine.lock().iter().cloned().collect()
        }
    }

    pub fn last_sample(&self) -> Option<Sample> {
        self.fine.lock().back().cloned()
    }

    /// Records the analysis of a sealed file.
    pub fn note_sealed(&self, size: u64, meta: &FileMeta) {
        Counters::add(&self.c.files_sealed, 1);
        let mut m = self.mp4.lock();
        m.size_hist[bucket(&SIZE_EDGES_MB, size as f64 / (1024.0 * 1024.0))] += 1;
        match &meta.mp4 {
            Ok(info) => {
                m.ok += 1;
                m.bytes_parsed += size;
                m.duration_total += info.duration_sec;
                m.duration_hist[bucket(&DURATION_EDGES, info.duration_sec)] += 1;
                if info.fragmented {
                    m.fragmented += 1;
                } else {
                    m.progressive += 1;
                }
                if let Some(c) = meta.codec() {
                    *m.codecs.entry(c).or_insert(0) += 1;
                }
                if let Some(r) = meta.resolution() {
                    *m.resolutions.entry(r).or_insert(0) += 1;
                }
            }
            Err(_) => {
                m.failed += 1;
                *m.fail_reasons.entry(meta.mp4_error_code.unwrap_or("unknown").to_string()).or_insert(0) += 1;
            }
        }
    }

    /// Applies `f` to the aggregate of every label the file carries.
    fn each_group(&self, labels: Option<&Labels>, mut f: impl FnMut(&mut GroupStat)) {
        let Some(labels) = labels else { return };
        let mut g = self.groups.lock();
        for (k, v) in labels {
            let key = (k.clone(), v.clone());
            if g.len() >= MAX_GROUPS && !g.contains_key(&key) {
                continue;
            }
            let e = g.entry(key).or_insert_with(|| GroupStat { key: k.clone(), value: v.clone(), ..Default::default() });
            e.last_ms = now_ms();
            f(e);
        }
    }

    /// A file reached a destination.
    pub fn note_moved(&self, size: u64, meta: Option<&FileMeta>, labels: Option<&Labels>) {
        self.each_group(labels, |e| {
            e.files += 1;
            e.bytes += size;
            match meta.map(|m| &m.mp4) {
                Some(Ok(info)) => {
                    e.duration_sec += info.duration_sec;
                    if let Some(v) = info.video() {
                        e.codec = v.codec.clone();
                        e.width = v.width;
                        e.height = v.height;
                    }
                }
                Some(Err(_)) => e.parse_fail += 1,
                None => {}
            }
        });
    }

    /// The thief discarded a file.
    pub fn note_lost(&self, size: u64, labels: Option<&Labels>) {
        self.each_group(labels, |e| {
            e.lost_files += 1;
            e.lost_bytes += size;
        });
    }

    /// The validate hook refused a file.
    pub fn note_dropped(&self, size: u64, labels: Option<&Labels>) {
        Counters::add(&self.c.hook_dropped, 1);
        Counters::add(&self.c.hook_dropped_bytes, size);
        self.each_group(labels, |e| {
            e.dropped_files += 1;
            e.dropped_bytes += size;
        });
    }

    /// Groups of one label key (or all), most recently active first.
    pub fn groups(&self, key: Option<&str>) -> Vec<GroupStat> {
        let mut v: Vec<GroupStat> = self.groups.lock().values().filter(|g| key.is_none_or(|k| g.key == k)).cloned().collect();
        v.sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then_with(|| a.value.cmp(&b.value)));
        v
    }

    /// Label keys seen so far, with how many distinct values each has.
    pub fn group_keys(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for (k, _) in self.groups.lock().keys() {
            *out.entry(k.clone()).or_insert(0) += 1;
        }
        out
    }

    pub fn recent_push(&self, r: RecentFile) {
        let mut q = self.recent.lock();
        q.push_back(r);
        while q.len() > MAX_RECENT {
            q.pop_front();
        }
    }

    pub fn recent(&self) -> Vec<RecentFile> {
        self.recent.lock().iter().rev().cloned().collect()
    }
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets() {
        assert_eq!(bucket(&DURATION_EDGES, 0.0), 0);
        assert_eq!(bucket(&DURATION_EDGES, 29.9), 0);
        assert_eq!(bucket(&DURATION_EDGES, 30.0), 1);
        assert_eq!(bucket(&DURATION_EDGES, 5000.0), 7);
        assert_eq!(bucket(&SIZE_EDGES_MB, 0.2), 0);
        assert_eq!(bucket(&SIZE_EDGES_MB, 3.0), 3);
    }

    #[test]
    fn events_ring_and_cursor() {
        let s = Stats::new();
        for i in 0..(MAX_EVENTS + 10) {
            s.event("info", "t", format!("e{i}"));
        }
        let all = s.events(0);
        assert_eq!(all.len(), MAX_EVENTS);
        assert_eq!(all.last().unwrap().seq, (MAX_EVENTS + 10) as u64);
        assert_eq!(s.events(all.last().unwrap().seq - 2).len(), 2);
    }

    #[test]
    fn groups_follow_labels() {
        let s = Stats::new();
        let mut l = Labels::new();
        l.insert("camera".into(), "c1".into());
        l.insert("tenant".into(), "t9".into());
        s.note_moved(100, None, Some(&l));
        s.note_moved(50, None, Some(&l));
        s.note_dropped(7, Some(&l));
        s.note_lost(3, Some(&l));
        s.note_moved(10, None, None); // unlabelled: counted nowhere
        let cams = s.groups(Some("camera"));
        assert_eq!(cams.len(), 1);
        let c = &cams[0];
        assert_eq!((c.value.as_str(), c.files, c.bytes, c.dropped_files, c.lost_bytes), ("c1", 2, 150, 1, 3));
        assert_eq!(s.group_keys().into_iter().collect::<Vec<_>>(), vec![("camera".to_string(), 1), ("tenant".to_string(), 1)]);
        assert_eq!(s.groups(None).len(), 2);
    }

    #[test]
    fn coarse_series_rolls_up_per_minute() {
        let s = Stats::new();
        for i in 0..=61 {
            s.push_sample(Sample { t_ms: i * 1000, ingest_bps: 10.0, ram_used: i as u64, ..Default::default() });
        }
        let c = s.series(true);
        assert_eq!(c.len(), 1);
        assert!((c[0].ingest_bps - 10.0).abs() < 1e-9);
        assert_eq!(c[0].ram_used, 60);
        assert_eq!(s.series(false).len(), 62);
    }
}
