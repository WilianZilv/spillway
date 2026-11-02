//! Node: owns destination directories on one host. Runs health probes
//! ("doctor"), and executes stat/list/read/put/delete against them.
//!
//! Every filesystem call runs on the blocking pool behind a per-destination
//! semaphore and a timeout, so a hung NFS mount can only ever tie up its own
//! slots: callers get `Busy`/`Timeout` quickly instead of hanging.
//!
//! The same service backs the node embedded in the master (called in-process)
//! and the standalone `spillway node` process (called over HTTP, see `router`).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use bytes::Bytes;
use futures::StreamExt;
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;

use crate::proto::{now_ms, valid_rel, DestAssign, Entry, Kind, NodeError, ProbeReport, META_DIR};

const STAT_TIMEOUT: Duration = Duration::from_secs(1);
const LIST_TIMEOUT: Duration = Duration::from_secs(8);
const META_OP_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_EVERY: Duration = Duration::from_secs(2);
/// A probe still running after this long marks the destination as hung.
const HUNG_AFTER: Duration = Duration::from_secs(20);
const META_SLOTS: usize = 16;

pub struct LocalDest {
    pub assign: DestAssign,
    meta_sem: Arc<Semaphore>,
    report: Mutex<Option<ProbeReport>>,
    probing: AtomicBool,
    probe_started: Mutex<Option<Instant>>,
    /// Leftover partial transfers are removed by the first probe (which is hang-protected).
    cleaned: AtomicBool,
    /// st_dev of the destination root seen by the last good probe (expect_mount only).
    mount_dev: std::sync::atomic::AtomicU64,
}

impl LocalDest {
    fn root(&self) -> &Path {
        Path::new(&self.assign.path)
    }

    /// For expect_mount destinations: the device the root must still be on.
    fn guard(&self) -> Result<Option<u64>, NodeError> {
        if !self.assign.expect_mount {
            return Ok(None);
        }
        match self.mount_dev.load(Ordering::Relaxed) {
            0 => Err(NodeError::Io("destino ainda não verificado como ponto de montagem".into())),
            dev => Ok(Some(dev)),
        }
    }
}

/// Re-checked right before anything is created under the root: between two
/// probes an NFS/loop mount can vanish, and writing then would silently fill
/// the parent (root) filesystem.
fn mount_ok(root: &Path, guard: Option<u64>) -> Result<(), NodeError> {
    let Some(dev) = guard else { return Ok(()) };
    let m = fs::metadata(root).map_err(io_err)?;
    if m.dev() != dev {
        return Err(NodeError::Io("o destino deixou de ser o ponto de montagem esperado (recusando gravar)".into()));
    }
    Ok(())
}

pub struct NodeService {
    pub id: String,
    dests: RwLock<HashMap<String, Arc<LocalDest>>>,
    pub fsync: AtomicBool,
}

/// Metadata attributes applied to a file written into a destination.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct PutAttrs {
    pub mtime_ms: i64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

impl NodeService {
    pub fn new(id: String) -> Arc<Self> {
        Arc::new(NodeService { id, dests: RwLock::new(HashMap::new()), fsync: AtomicBool::new(false) })
    }

    /// Reconciles the set of destinations this node serves.
    pub fn set_dests(self: &Arc<Self>, wanted: Vec<DestAssign>) {
        let mut map = self.dests.write();
        let old = std::mem::take(&mut *map);
        for w in wanted {
            let prev = old.get(&w.id);
            if let Some(d) = prev.filter(|d| d.assign == w) {
                map.insert(w.id.clone(), d.clone());
                continue;
            }
            // A new generation of the same directory (re-enabled) is probed
            // afresh, but its tmp dir was already swept: sweeping it again
            // could delete a PUT still finishing under the old generation.
            let swept = prev.is_some_and(|d| d.assign.path == w.path && d.cleaned.load(Ordering::Relaxed));
            tracing::info!(node = %self.id, dest = %w.id, path = %w.path, "serving destination");
            let d = Arc::new(LocalDest {
                assign: w.clone(),
                meta_sem: Arc::new(Semaphore::new(META_SLOTS)),
                report: Mutex::new(None),
                probing: AtomicBool::new(false),
                probe_started: Mutex::new(None),
                cleaned: AtomicBool::new(swept),
                mount_dev: std::sync::atomic::AtomicU64::new(0),
            });
            map.insert(w.id.clone(), d);
        }
    }

    pub fn assigned(&self) -> Vec<DestAssign> {
        self.dests.read().values().map(|d| d.assign.clone()).collect()
    }

    pub fn reports(&self) -> Vec<ProbeReport> {
        let map = self.dests.read();
        let mut out = Vec::with_capacity(map.len());
        for d in map.values() {
            let mut r = d.report.lock().clone();
            // A probe that has been running far longer than its timeout means the mount is hung.
            if let Some(started) = *d.probe_started.lock() {
                if d.probing.load(Ordering::Relaxed) && started.elapsed() > HUNG_AFTER {
                    r = Some(ProbeReport {
                        dest_id: d.assign.id.clone(),
                        ok: false,
                        error: Some(format!("teste travado há {:.0} s (montagem pendurada?)", started.elapsed().as_secs_f64())),
                        free_bytes: r.as_ref().map_or(0, |r| r.free_bytes),
                        total_bytes: r.as_ref().map_or(0, |r| r.total_bytes),
                        is_mount: r.as_ref().and_then(|r| r.is_mount),
                        probe_ms: started.elapsed().as_secs_f64() * 1000.0,
                        at_ms: now_ms(),
                        hung: true,
                        gen: d.assign.gen,
                    });
                }
            }
            if let Some(r) = r {
                out.push(r);
            }
        }
        out
    }

    pub fn report(&self, dest: &str) -> Option<ProbeReport> {
        // reuse the hung-probe logic of reports()
        if !self.dests.read().contains_key(dest) {
            return None;
        }
        self.reports().into_iter().find(|r| r.dest_id == dest)
    }

    fn dest(&self, id: &str) -> Result<Arc<LocalDest>, NodeError> {
        self.dests.read().get(id).cloned().ok_or(NodeError::UnknownDest)
    }

    /// Runs `f` on the blocking pool, bounded by the destination's semaphore and a timeout.
    async fn guarded<T, F>(d: &LocalDest, timeout: Duration, f: F) -> Result<T, NodeError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, NodeError> + Send + 'static,
    {
        let permit = match tokio::time::timeout(timeout, d.meta_sem.clone().acquire_owned()).await {
            Ok(Ok(p)) => p,
            _ => return Err(NodeError::Busy),
        };
        let h = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            f()
        });
        match tokio::time::timeout(timeout, h).await {
            Err(_) => Err(NodeError::Timeout),
            Ok(Err(e)) => Err(NodeError::Io(format!("task failed: {e}"))),
            Ok(Ok(r)) => r,
        }
    }

    pub async fn stat(&self, dest: &str, rel: &str) -> Result<Option<Entry>, NodeError> {
        if !valid_rel(rel) {
            return Err(NodeError::BadPath);
        }
        let d = self.dest(dest)?;
        let p = d.root().join(rel);
        let name = rel.rsplit('/').next().unwrap_or("").to_string();
        Self::guarded(&d, STAT_TIMEOUT, move || match fs::metadata(&p) {
            Ok(m) => Ok(entry_from_meta(name, &m)),
            Err(e) if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ENOTDIR) => Ok(None),
            Err(e) => Err(io_err(e)),
        })
        .await
    }

    /// Lists a directory. `Ok(None)` when it does not exist on this destination.
    pub async fn list(&self, dest: &str, rel: &str) -> Result<Option<Vec<Entry>>, NodeError> {
        if !valid_rel(rel) {
            return Err(NodeError::BadPath);
        }
        let d = self.dest(dest)?;
        let p = d.root().join(rel);
        let at_root = rel.is_empty();
        Self::guarded(&d, LIST_TIMEOUT, move || {
            let rd = match fs::read_dir(&p) {
                Ok(rd) => rd,
                Err(e) if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ENOTDIR) => return Ok(None),
                Err(e) => return Err(io_err(e)),
            };
            let mut out = Vec::new();
            for ent in rd {
                let ent = ent.map_err(io_err)?;
                let Ok(name) = ent.file_name().into_string() else { continue };
                if at_root && name == META_DIR {
                    continue;
                }
                if let Ok(m) = ent.metadata() {
                    if let Some(e) = entry_from_meta(name, &m) {
                        out.push(e);
                    }
                }
            }
            Ok(Some(out))
        })
        .await
    }

    pub async fn read(&self, dest: &str, rel: &str, offset: u64, len: u32) -> Result<Bytes, NodeError> {
        if !valid_rel(rel) || rel.is_empty() {
            return Err(NodeError::BadPath);
        }
        let d = self.dest(dest)?;
        let p = d.root().join(rel);
        let len = len.min(8 * 1024 * 1024) as usize;
        Self::guarded(&d, META_OP_TIMEOUT * 2, move || {
            let mut f = File::open(&p).map_err(io_err)?;
            f.seek(SeekFrom::Start(offset)).map_err(io_err)?;
            let mut buf = vec![0u8; len];
            let mut got = 0;
            while got < len {
                match f.read(&mut buf[got..]) {
                    Ok(0) => break,
                    Ok(n) => got += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(io_err(e)),
                }
            }
            buf.truncate(got);
            Ok(Bytes::from(buf))
        })
        .await
    }

    pub async fn delete(&self, dest: &str, rel: &str) -> Result<(), NodeError> {
        if !valid_rel(rel) || rel.is_empty() {
            return Err(NodeError::BadPath);
        }
        let d = self.dest(dest)?;
        let p = d.root().join(rel);
        Self::guarded(&d, META_OP_TIMEOUT, move || match fs::remove_file(&p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(NodeError::NotFound),
            Err(e) => Err(io_err(e)),
        })
        .await
    }

    /// Copies a local file (the RAM spool) into the destination: temp file in
    /// META_DIR, copy (copy_file_range under the hood), attrs, optional fsync,
    /// then an atomic rename so readers never observe a partial file.
    pub async fn put_from_path(
        &self,
        dest: &str,
        rel: &str,
        src: PathBuf,
        attrs: PutAttrs,
        timeout: Duration,
    ) -> Result<u64, NodeError> {
        if !valid_rel(rel) || rel.is_empty() {
            return Err(NodeError::BadPath);
        }
        let d = self.dest(dest)?;
        let guard = d.guard()?;
        let root = d.root().to_path_buf();
        let rel = rel.to_string();
        let fsync = self.fsync.load(Ordering::Relaxed);
        let h = tokio::task::spawn_blocking(move || -> Result<u64, NodeError> {
            let (tmp, mut out) = open_tmp(&root, guard)?;
            let res = (|| {
                let mut input = File::open(&src).map_err(io_err)?;
                let n = io::copy(&mut input, &mut out).map_err(io_err)?;
                finish_put(&root, &rel, &tmp, out, attrs, fsync, guard)?;
                Ok(n)
            })();
            if res.is_err() {
                let _ = fs::remove_file(&tmp);
            }
            res
        });
        match tokio::time::timeout(timeout, h).await {
            Err(_) => Err(NodeError::Timeout),
            Ok(Err(e)) => Err(NodeError::Io(format!("task failed: {e}"))),
            Ok(Ok(r)) => r,
        }
    }

    /// Same as `put_from_path` but the bytes arrive as a stream (remote master).
    pub async fn put_stream<S, E>(&self, dest: &str, rel: &str, mut body: S, attrs: PutAttrs, expect: Option<u64>) -> Result<u64, NodeError>
    where
        S: futures::Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        if !valid_rel(rel) || rel.is_empty() {
            return Err(NodeError::BadPath);
        }
        let d = self.dest(dest)?;
        let guard = d.guard()?;
        let root = d.root().to_path_buf();
        let r2 = root.clone();
        let (tmp, std_file) = tokio::task::spawn_blocking(move || open_tmp(&r2, guard))
            .await
            .map_err(|e| NodeError::Io(e.to_string()))??;
        let mut out = tokio::fs::File::from_std(std_file);
        let mut n = 0u64;
        let res: Result<(), NodeError> = async {
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|e| NodeError::Io(format!("body: {e}")))?;
                out.write_all(&chunk).await.map_err(io_err)?;
                n += chunk.len() as u64;
            }
            out.flush().await.map_err(io_err)?;
            if let Some(exp) = expect {
                if exp != n {
                    return Err(NodeError::Io(format!("short body: got {n} of {exp} bytes")));
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = res {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
        let out = out.into_std().await;
        let fsync = self.fsync.load(Ordering::Relaxed);
        let rel = rel.to_string();
        let tmp2 = tmp.clone();
        let r = tokio::task::spawn_blocking(move || finish_put(&root, &rel, &tmp2, out, attrs, fsync, guard))
            .await
            .map_err(|e| NodeError::Io(e.to_string()))?;
        if r.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
        r.map(|_| n)
    }

    /// Probe loop: every destination is checked every PROBE_EVERY; a probe that
    /// is still stuck is not restarted (reports() flags it as hung instead).
    pub async fn probe_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(PROBE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            self.probe_all_now().await;
        }
    }

    /// Starts a probe on every idle destination and waits (bounded) for them.
    pub async fn probe_all_now(&self) {
        let dests: Vec<Arc<LocalDest>> = self.dests.read().values().cloned().collect();
        let mut handles = Vec::new();
        for d in dests {
            if d.probing.swap(true, Ordering::AcqRel) {
                continue;
            }
            *d.probe_started.lock() = Some(Instant::now());
            let d2 = d.clone();
            let h = tokio::task::spawn_blocking(move || {
                let t0 = Instant::now();
                if !d2.cleaned.swap(true, Ordering::AcqRel) {
                    let tmp = d2.root().join(META_DIR).join("tmp");
                    if tmp.is_dir() {
                        let _ = fs::remove_dir_all(&tmp);
                    }
                }
                let r = probe(&d2.assign);
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                let report = match r {
                    Ok((free, total, is_mount, dev)) => {
                        d2.mount_dev.store(if is_mount { dev } else { 0 }, Ordering::Relaxed);
                        ProbeReport {
                            dest_id: d2.assign.id.clone(),
                            ok: true,
                            error: None,
                            free_bytes: free,
                            total_bytes: total,
                            is_mount: Some(is_mount),
                            probe_ms: ms,
                            at_ms: now_ms(),
                            hung: false,
                            gen: d2.assign.gen,
                        }
                    }
                    Err((e, free, total, is_mount)) => ProbeReport {
                        dest_id: d2.assign.id.clone(),
                        ok: false,
                        error: Some(e),
                        free_bytes: free,
                        total_bytes: total,
                        is_mount,
                        probe_ms: ms,
                        at_ms: now_ms(),
                        hung: false,
                        gen: d2.assign.gen,
                    },
                };
                *d2.report.lock() = Some(report);
                d2.probing.store(false, Ordering::Release);
            });
            handles.push(h);
        }
        // all destinations in parallel, one bounded wait
        let _ = tokio::time::timeout(PROBE_TIMEOUT, futures::future::join_all(handles)).await;
    }
}

type ProbeErr = (String, u64, u64, Option<bool>);

/// Doctor: directory exists, statvfs sane, mountpoint (if required), needle write+fsync+readback.
fn probe(a: &DestAssign) -> Result<(u64, u64, bool, u64), ProbeErr> {
    let root = Path::new(&a.path);
    let meta = fs::metadata(root).map_err(|e| (format!("stat: {e}"), 0, 0, None))?;
    if !meta.is_dir() {
        return Err(("não é um diretório".into(), 0, 0, None));
    }
    let (free, total) = statvfs(root).map_err(|e| (format!("statvfs: {e}"), 0, 0, None))?;
    if total == 0 {
        return Err(("statvfs informa zero blocos".into(), 0, 0, None));
    }
    let is_mount = match root.parent() {
        None => true,
        Some(parent) => fs::metadata(parent).map(|pm| pm.dev() != meta.dev()).unwrap_or(false),
    };
    if a.expect_mount && !is_mount {
        return Err(("não é um ponto de montagem (recusando gravar no disco de cima)".into(), free, total, Some(false)));
    }
    let err = |e: String| (e, free, total, Some(is_mount));
    let dir = root.join(META_DIR);
    fs::create_dir_all(&dir).map_err(|e| err(format!("mkdir {META_DIR}: {e}")))?;
    let needle = dir.join("needle");
    let content = format!("spillway probe {}", now_ms());
    {
        let mut f = File::create(&needle).map_err(|e| err(format!("teste de escrita: {e}")))?;
        f.write_all(content.as_bytes()).map_err(|e| err(format!("teste de escrita: {e}")))?;
    }
    let back = fs::read(&needle).map_err(|e| err(format!("leitura do teste: {e}")))?;
    if back != content.as_bytes() {
        return Err(err("conteúdo do teste não confere na releitura".into()));
    }
    Ok((free, total, is_mount, meta.dev()))
}

pub fn statvfs(p: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let frsize = if st.f_frsize > 0 { st.f_frsize } else { st.f_bsize } as u64;
    Ok((st.f_bavail as u64 * frsize, st.f_blocks as u64 * frsize))
}

fn open_tmp(root: &Path, guard: Option<u64>) -> Result<(PathBuf, File), NodeError> {
    mount_ok(root, guard)?;
    let dir = root.join(META_DIR).join("tmp");
    fs::create_dir_all(&dir).map_err(io_err)?;
    let name = format!("{:016x}.part", rand::random::<u64>());
    let tmp = dir.join(name);
    let f = File::create(&tmp).map_err(io_err)?;
    Ok((tmp, f))
}

fn finish_put(root: &Path, rel: &str, tmp: &Path, out: File, a: PutAttrs, fsync: bool, guard: Option<u64>) -> Result<(), NodeError> {
    if fsync {
        out.sync_all().map_err(io_err)?;
    }
    let mtime = UNIX_EPOCH + Duration::from_millis(a.mtime_ms.max(0) as u64);
    let _ = out.set_modified(mtime);
    let _ = out.set_permissions(fs::Permissions::from_mode(a.mode & 0o7777));
    let _ = std::os::unix::fs::fchown(&out, Some(a.uid), Some(a.gid));
    drop(out);
    let fin = root.join(rel);
    mount_ok(root, guard)?;
    if let Some(parent) = fin.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    fs::rename(tmp, &fin).map_err(io_err)?;
    Ok(())
}

fn entry_from_meta(name: String, m: &fs::Metadata) -> Option<Entry> {
    let kind = if m.is_dir() {
        Kind::Dir
    } else if m.is_file() {
        Kind::File
    } else {
        return None;
    };
    let mtime_ms = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    Some(Entry { name, kind, size: if kind == Kind::File { m.len() } else { 0 }, mtime_ms, mode: m.mode(), uid: m.uid(), gid: m.gid() })
}

fn io_err(e: io::Error) -> NodeError {
    if e.kind() == io::ErrorKind::NotFound {
        NodeError::NotFound
    } else {
        NodeError::Io(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// HTTP data API (standalone node)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct NodeHttp {
    svc: Arc<NodeService>,
    token: Arc<String>,
}

#[derive(Deserialize)]
struct PathQ {
    path: String,
}

#[derive(Deserialize)]
struct ReadQ {
    path: String,
    offset: u64,
    len: u32,
}

#[derive(Deserialize)]
struct PutQ {
    path: String,
    mtime_ms: i64,
    mode: u32,
    uid: u32,
    gid: u32,
    size: Option<u64>,
}

fn node_err(e: NodeError) -> Response {
    let status = match e {
        NodeError::UnknownDest => StatusCode::CONFLICT,
        NodeError::NotFound => StatusCode::NOT_FOUND,
        NodeError::BadPath => StatusCode::BAD_REQUEST,
        NodeError::Timeout => StatusCode::GATEWAY_TIMEOUT,
        NodeError::Busy => StatusCode::SERVICE_UNAVAILABLE,
        NodeError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(serde_json::json!({ "error": e.code(), "message": e.to_string() }))).into_response()
}

fn authorized(h: &HeaderMap, token: &str) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).is_some_and(|t| t == token)
}

macro_rules! auth {
    ($st:expr, $h:expr) => {
        if !authorized(&$h, &$st.token) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };
}

async fn h_stat(State(st): State<NodeHttp>, h: HeaderMap, UrlPath(id): UrlPath<String>, Query(q): Query<PathQ>) -> Response {
    auth!(st, h);
    match st.svc.stat(&id, &q.path).await {
        Ok(e) => Json(e).into_response(),
        Err(e) => node_err(e),
    }
}

async fn h_list(State(st): State<NodeHttp>, h: HeaderMap, UrlPath(id): UrlPath<String>, Query(q): Query<PathQ>) -> Response {
    auth!(st, h);
    match st.svc.list(&id, &q.path).await {
        Ok(e) => Json(e).into_response(),
        Err(e) => node_err(e),
    }
}

async fn h_read(State(st): State<NodeHttp>, h: HeaderMap, UrlPath(id): UrlPath<String>, Query(q): Query<ReadQ>) -> Response {
    auth!(st, h);
    match st.svc.read(&id, &q.path, q.offset, q.len).await {
        Ok(b) => b.into_response(),
        Err(e) => node_err(e),
    }
}

async fn h_put(State(st): State<NodeHttp>, h: HeaderMap, UrlPath(id): UrlPath<String>, Query(q): Query<PutQ>, body: Body) -> Response {
    auth!(st, h);
    let attrs = PutAttrs { mtime_ms: q.mtime_ms, mode: q.mode, uid: q.uid, gid: q.gid };
    let mut body = body.into_data_stream();
    match st.svc.put_stream(&id, &q.path, &mut body, attrs, q.size).await {
        Ok(n) => Json(serde_json::json!({ "size": n })).into_response(),
        Err(e) => {
            // Refused before the upload was read (unknown destination, bad
            // path, mount missing): read the rest so the client gets this
            // answer instead of a connection reset that looks like a network
            // failure. Bounded, in case the client stalls.
            let _ = tokio::time::timeout(Duration::from_secs(30), async { while body.next().await.is_some() {} }).await;
            node_err(e)
        }
    }
}

async fn h_delete(State(st): State<NodeHttp>, h: HeaderMap, UrlPath(id): UrlPath<String>, Query(q): Query<PathQ>) -> Response {
    auth!(st, h);
    match st.svc.delete(&id, &q.path).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => node_err(e),
    }
}

async fn h_health(State(st): State<NodeHttp>) -> Response {
    Json(serde_json::json!({ "node": st.svc.id, "version": crate::proto::VERSION, "dests": st.svc.assigned() })).into_response()
}

pub fn router(svc: Arc<NodeService>, token: String) -> Router {
    Router::new()
        .route("/v1/health", get(h_health))
        .route("/v1/dest/{id}/stat", get(h_stat))
        .route("/v1/dest/{id}/list", get(h_list))
        .route("/v1/dest/{id}/read", get(h_read))
        .route("/v1/dest/{id}/file", put(h_put).delete(h_delete))
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state(NodeHttp { svc, token: Arc::new(token) })
}

// ---------------------------------------------------------------------------
// Standalone node process
// ---------------------------------------------------------------------------

pub struct NodeArgs {
    pub id: String,
    pub master: String,
    pub token: String,
    pub listen: String,
    pub advertise: String,
}

pub async fn run(args: NodeArgs) -> anyhow::Result<()> {
    let svc = NodeService::new(args.id.clone());
    tokio::spawn(svc.clone().probe_loop());

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(node = %args.id, listen = %args.listen, master = %args.master, "node started");
    let app = router(svc.clone(), args.token.clone());
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "node http server stopped");
        }
    });

    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build()?;
    let hostname = hostname();
    let started_at_ms = now_ms();
    let url = format!("{}/cluster/heartbeat", args.master.trim_end_matches('/'));
    let mut failures = 0u32;
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("node shutting down");
                return Ok(());
            }
        }
        let hb = crate::proto::Heartbeat {
            node_id: args.id.clone(),
            url: args.advertise.clone(),
            hostname: hostname.clone(),
            version: crate::proto::VERSION.into(),
            started_at_ms,
            reports: svc.reports(),
            sent_at_ms: now_ms(),
        };
        let res = client.post(&url).bearer_auth(&args.token).json(&hb).send().await;
        match res {
            Ok(r) if r.status().is_success() => match r.json::<crate::proto::HeartbeatResp>().await {
                Ok(resp) => {
                    if failures > 0 {
                        tracing::info!("reconnected to master");
                    }
                    failures = 0;
                    let changed = {
                        let mut a = svc.assigned();
                        let mut b = resp.dests.clone();
                        a.sort_by(|x, y| x.id.cmp(&y.id));
                        b.sort_by(|x, y| x.id.cmp(&y.id));
                        a != b
                    };
                    svc.set_dests(resp.dests);
                    if changed {
                        svc.probe_all_now().await;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "bad heartbeat response"),
            },
            Ok(r) => {
                failures += 1;
                if failures == 1 || failures % 30 == 0 {
                    tracing::warn!(status = %r.status(), "master rejected heartbeat (check --token)");
                }
            }
            Err(e) => {
                failures += 1;
                if failures == 1 || failures % 30 == 0 {
                    tracing::warn!(error = %e, "master unreachable");
                }
            }
        }
    }
}

pub fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assign(id: &str, p: &Path) -> DestAssign {
        DestAssign { id: id.into(), path: p.to_string_lossy().into(), expect_mount: false, gen: 1 }
    }

    #[tokio::test]
    async fn put_stat_list_read_delete() {
        let dir = tempfile::tempdir().unwrap();
        let svc = NodeService::new("n".into());
        svc.set_dests(vec![assign("d", dir.path())]);
        let src = dir.path().join("src.bin");
        fs::write(&src, b"hello spillway").unwrap();
        let attrs = PutAttrs { mtime_ms: 1_700_000_000_000, mode: 0o640, uid: 0, gid: 0 };
        let n = svc.put_from_path("d", "cam/2026-01-01/a.mp4", src, attrs, Duration::from_secs(5)).await.unwrap();
        assert_eq!(n, 14);
        let e = svc.stat("d", "cam/2026-01-01/a.mp4").await.unwrap().unwrap();
        assert_eq!((e.kind, e.size, e.mtime_ms, e.mode & 0o777), (Kind::File, 14, 1_700_000_000_000, 0o640));
        assert_eq!(svc.stat("d", "cam/nope").await.unwrap(), None);
        assert_eq!(svc.stat("d", "cam/2026-01-01/a.mp4/x").await.unwrap(), None);
        let root = svc.list("d", "").await.unwrap().unwrap();
        let names: Vec<_> = root.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"cam") && !names.contains(&META_DIR), "{names:?}");
        assert_eq!(svc.list("d", "missing").await.unwrap(), None);
        let b = svc.read("d", "cam/2026-01-01/a.mp4", 6, 100).await.unwrap();
        assert_eq!(&b[..], b"spillway");
        // no temp files left behind
        assert_eq!(fs::read_dir(dir.path().join(META_DIR).join("tmp")).unwrap().count(), 0);
        svc.delete("d", "cam/2026-01-01/a.mp4").await.unwrap();
        assert_eq!(svc.delete("d", "cam/2026-01-01/a.mp4").await, Err(NodeError::NotFound));
        assert_eq!(svc.stat("x", "a").await, Err(NodeError::UnknownDest));
        assert_eq!(svc.stat("d", "../etc").await, Err(NodeError::BadPath));
    }

    #[tokio::test]
    async fn refused_put_answers_instead_of_resetting() {
        let dir = tempfile::tempdir().unwrap();
        let svc = NodeService::new("n".into());
        svc.set_dests(vec![assign("d", dir.path())]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router(svc, "tok".into())).await });
        let body = vec![7u8; 16 << 20]; // far beyond the socket buffers
        let r = reqwest::Client::new()
            .put(format!("http://{addr}/v1/dest/gone/file?path=a.mp4&mtime_ms=0&mode=420&uid=0&gid=0&size={}", body.len()))
            .bearer_auth("tok")
            .body(body)
            .send()
            .await
            .expect("an answer, not a connection error");
        assert_eq!(r.status(), StatusCode::CONFLICT);
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"], "unknown_dest");
    }

    #[tokio::test]
    async fn put_stream_rejects_short_body() {
        let dir = tempfile::tempdir().unwrap();
        let svc = NodeService::new("n".into());
        svc.set_dests(vec![assign("d", dir.path())]);
        let attrs = PutAttrs { mtime_ms: 0, mode: 0o644, uid: 0, gid: 0 };
        let chunks = vec![Ok::<_, String>(Bytes::from_static(b"abc"))];
        let r = svc.put_stream("d", "x.mp4", futures::stream::iter(chunks), attrs, Some(10)).await;
        assert!(matches!(r, Err(NodeError::Io(_))));
        assert!(!dir.path().join("x.mp4").exists());
        let chunks = vec![Ok::<_, String>(Bytes::from_static(b"abc")), Ok(Bytes::from_static(b"def"))];
        let n = svc.put_stream("d", "x.mp4", futures::stream::iter(chunks), attrs, Some(6)).await.unwrap();
        assert_eq!(n, 6);
        assert_eq!(fs::read(dir.path().join("x.mp4")).unwrap(), b"abcdef");
    }

    #[tokio::test]
    async fn probe_reports_health() {
        let dir = tempfile::tempdir().unwrap();
        let svc = NodeService::new("n".into());
        let missing = dir.path().join("missing");
        svc.set_dests(vec![assign("ok", dir.path()), assign("bad", &missing)]);
        svc.probe_all_now().await;
        let reps = svc.reports();
        let ok = reps.iter().find(|r| r.dest_id == "ok").unwrap();
        assert!(ok.ok && ok.total_bytes > 0, "{ok:?}");
        let bad = reps.iter().find(|r| r.dest_id == "bad").unwrap();
        assert!(!bad.ok && bad.error.as_deref().unwrap().starts_with("stat"));
    }

    #[tokio::test]
    async fn expect_mount_rejects_plain_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("nfs");
        fs::create_dir(&sub).unwrap();
        let svc = NodeService::new("n".into());
        svc.set_dests(vec![DestAssign { id: "m".into(), path: sub.to_string_lossy().into(), expect_mount: true, gen: 1 }]);
        svc.probe_all_now().await;
        let r = &svc.reports()[0];
        assert!(!r.ok && r.error.as_deref().unwrap().contains("ponto de montagem"));
        assert_eq!(r.is_mount, Some(false));
    }
}
