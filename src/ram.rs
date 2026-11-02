//! RAM tier: files live in a spool directory (tmpfs) that mirrors the mount's
//! tree, plus an in-memory table tracking every file's lifecycle:
//!
//! ```text
//!  create/open(w) ──► Writing ──(no writers && quiet for settle)──► Sealing ──parse──► [Deciding] ──► Queued ──► InFlight ──► (gone from RAM)
//!                       ▲   ▲                                          │                 │            │
//!                       │   └──────── reopened for write ──────────────┴─────────────────┴────────────┘ (transfer invalidated)
//!                       └── copy-up from a destination when a moved file is reopened for write
//! ```
//!
//! Everything here is O(1)/O(log n) per operation: the sealer only looks at the
//! `settling` set (closed files), the queue is kept in two ordered sets (oldest
//! first / largest first) and the thief pops from the oldest end.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;

use crate::stats::{FileMeta, Labels};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FState {
    Writing,
    Sealing,
    /// Waiting for the validate hook. `asking`: part of a batch currently in flight.
    Deciding { asking: bool },
    Queued,
    InFlight { dest: String },
}

impl FState {
    pub fn name(&self) -> &'static str {
        match self {
            FState::Writing => "writing",
            FState::Sealing => "sealing",
            FState::Deciding { .. } => "deciding",
            FState::Queued => "queued",
            FState::InFlight { .. } => "inflight",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RamFile {
    pub size: u64,
    pub mtime: SystemTime,
    pub ctime: SystemTime,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub writers: u32,
    pub last_write: Instant,
    pub state: FState,
    /// Bumped on every write/truncate/reopen: a transfer is only committed if the version is unchanged.
    pub version: u64,
    /// Queue order (assigned when sealed).
    pub seq: u64,
    pub created: Instant,
    pub meta: Option<Arc<FileMeta>>,
    pub attempts: u32,
    pub not_before: Option<Instant>,
    pub avoid: Option<(String, Instant)>,
    /// Destination holding an older copy (copy-up); removed once the new version lands elsewhere.
    pub origin: Option<String>,
    /// Stable identity of this file in RAM (hooks see "fid:version").
    pub fid: u64,
    /// Tags given by the validate hook.
    pub labels: Option<Arc<Labels>>,
    /// When the file became ready for (another) hook decision.
    pub decide_since: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct DirInfo {
    pub mtime: SystemTime,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub created: Instant,
}

#[derive(Default)]
struct Inner {
    files: HashMap<String, RamFile>,
    dirs: HashMap<String, DirInfo>,
    children: HashMap<String, BTreeSet<String>>,
    /// Closed files waiting for the settle period.
    settling: HashSet<String>,
    oldest: BTreeSet<(u64, String)>,
    largest: BTreeSet<(Reverse<u64>, u64, String)>,
    /// Files waiting for the validate hook, in seal order.
    deciding: BTreeSet<(u64, String)>,
    pick_largest: bool,
    next_seq: u64,
}

#[derive(Debug, Clone)]
pub enum Lookup {
    File(RamFile),
    Dir(DirInfo),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub rel: String,
    pub size: u64,
    pub version: u64,
    pub mtime: SystemTime,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub meta: Option<Arc<FileMeta>>,
    pub origin: Option<String>,
    pub attempts: u32,
    pub fid: u64,
    pub labels: Option<Arc<Labels>>,
}

/// A file handed to the validate hook.
#[derive(Debug, Clone)]
pub struct DecisionItem {
    pub rel: String,
    pub fid: u64,
    pub version: u64,
    pub size: u64,
    pub mtime: SystemTime,
    pub meta: Option<Arc<FileMeta>>,
}

impl DecisionItem {
    /// What the hook sees as the file id; a modified file gets a new one.
    pub fn id(&self) -> String {
        format!("{}:{}", self.fid, self.version)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Move(Option<Labels>),
    Drop,
    Hold(Duration),
}

#[derive(Debug)]
pub enum Applied {
    Moved,
    Dropped(RamFile),
    Held,
    /// The file changed, moved on or disappeared since it was sent: decision ignored.
    Stale,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Commit {
    /// RAM copy removed, file now lives on the destination.
    Done,
    /// The file was deleted while in flight: the destination copy must be removed.
    Deleted,
    /// The file changed (or was renamed) while in flight: the destination copy is stale.
    Stale,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct RamSnapshot {
    pub used_bytes: u64,
    pub files: u64,
    pub dirs: u64,
    pub writing: u64,
    pub open_writers: u64,
    pub sealing: u64,
    pub deciding: u64,
    pub deciding_bytes: u64,
    pub queued: u64,
    pub queued_bytes: u64,
    pub inflight: u64,
    pub inflight_bytes: u64,
    pub oldest_queued_age_s: f64,
    pub oldest_open_age_s: f64,
}

pub struct Ram {
    pub root: PathBuf,
    inner: Mutex<Inner>,
    used: AtomicU64,
    next_fid: AtomicU64,
}

/// Suffix of a file being copied into the spool by `adopt` (skipped by `recover`).
const ADOPT_SUFFIX: &str = ".spillway-adopt";

fn parent_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(p, _)| p)
}

fn name_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, n)| n)
}

fn errno(e: io::Error) -> i32 {
    e.raw_os_error().unwrap_or(libc::EIO)
}

impl Inner {
    fn link(&mut self, rel: &str) {
        self.children.entry(parent_of(rel).to_string()).or_default().insert(name_of(rel).to_string());
    }
    fn unlink_child(&mut self, rel: &str) {
        let p = parent_of(rel);
        if let Some(set) = self.children.get_mut(p) {
            set.remove(name_of(rel));
        }
    }
    fn dequeue(&mut self, rel: &str) {
        if let Some(f) = self.files.get(rel) {
            if f.state == FState::Queued {
                self.oldest.remove(&(f.seq, rel.to_string()));
                self.largest.remove(&(Reverse(f.size), f.seq, rel.to_string()));
            }
        }
    }
    fn enqueue(&mut self, rel: &str) {
        let seq = self.next_seq;
        self.next_seq += 1;
        let f = self.files.get_mut(rel).unwrap();
        f.state = FState::Queued;
        f.seq = seq;
        let size = f.size;
        self.oldest.insert((seq, rel.to_string()));
        self.largest.insert((Reverse(size), seq, rel.to_string()));
    }
    fn undecide(&mut self, rel: &str) {
        if let Some(f) = self.files.get(rel) {
            if matches!(f.state, FState::Deciding { .. }) {
                self.deciding.remove(&(f.seq, rel.to_string()));
            }
        }
    }
    /// Any state → Writing (reopen / modification). The hook decides again later.
    fn back_to_writing(&mut self, rel: &str) {
        self.dequeue(rel);
        self.undecide(rel);
        if let Some(f) = self.files.get_mut(rel) {
            f.state = FState::Writing;
            f.version += 1;
            f.attempts = 0;
            f.not_before = None;
            f.meta = None;
            f.labels = None;
            f.decide_since = None;
        }
    }
}

impl Ram {
    pub fn new(root: PathBuf) -> Self {
        let mut inner = Inner::default();
        inner.dirs.insert(
            String::new(),
            DirInfo { mtime: SystemTime::now(), mode: 0o755, uid: 0, gid: 0, created: Instant::now() },
        );
        Ram { root, inner: Mutex::new(inner), used: AtomicU64::new(0), next_fid: AtomicU64::new(1) }
    }

    fn new_fid(&self) -> u64 {
        self.next_fid.fetch_add(1, Ordering::Relaxed)
    }

    pub fn spool_path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    /// Rebuilds the table from the spool after a restart. Every file is treated
    /// as closed and idle, so it gets sealed and moved right away.
    pub fn recover(&self, settle: Duration) -> io::Result<(u64, u64)> {
        let mut files = 0u64;
        let mut bytes = 0u64;
        let mut stack = vec![String::new()];
        let past = Instant::now().checked_sub(settle).unwrap_or_else(Instant::now);
        let mut g = self.inner.lock();
        while let Some(dir) = stack.pop() {
            let rd = match fs::read_dir(self.spool_path(&dir)) {
                Ok(rd) => rd,
                Err(e) if dir.is_empty() => return Err(e),
                Err(_) => continue,
            };
            for ent in rd.flatten() {
                let Ok(name) = ent.file_name().into_string() else { continue };
                if name.ends_with(ADOPT_SUFFIX) {
                    // half-copied by `adopt` when we crashed: the source is still under the mount
                    let _ = fs::remove_file(ent.path());
                    continue;
                }
                let rel = if dir.is_empty() { name } else { format!("{dir}/{name}") };
                let Ok(m) = ent.metadata() else { continue };
                if m.is_dir() {
                    g.dirs.insert(
                        rel.clone(),
                        DirInfo { mtime: m.modified().unwrap_or(SystemTime::now()), mode: m.mode(), uid: m.uid(), gid: m.gid(), created: Instant::now() },
                    );
                    g.link(&rel);
                    stack.push(rel);
                } else if m.is_file() {
                    files += 1;
                    bytes += m.len();
                    g.files.insert(
                        rel.clone(),
                        RamFile {
                            size: m.len(),
                            mtime: m.modified().unwrap_or(SystemTime::now()),
                            ctime: SystemTime::now(),
                            mode: m.mode(),
                            uid: m.uid(),
                            gid: m.gid(),
                            writers: 0,
                            last_write: past,
                            state: FState::Writing,
                            version: 1,
                            seq: 0,
                            created: past,
                            meta: None,
                            attempts: 0,
                            not_before: None,
                            avoid: None,
                            origin: None,
                            fid: self.new_fid(),
                            labels: None,
                            decide_since: None,
                        },
                    );
                    g.settling.insert(rel.clone());
                    g.link(&rel);
                }
            }
        }
        self.used.store(bytes, Ordering::Relaxed);
        Ok((files, bytes))
    }

    pub fn lookup(&self, rel: &str) -> Option<Lookup> {
        let g = self.inner.lock();
        if let Some(f) = g.files.get(rel) {
            return Some(Lookup::File(f.clone()));
        }
        g.dirs.get(rel).map(|d| Lookup::Dir(d.clone()))
    }

    pub fn is_dir(&self, rel: &str) -> bool {
        self.inner.lock().dirs.contains_key(rel)
    }

    /// Children names with `true` for directories.
    pub fn children(&self, dir: &str) -> Vec<(String, bool)> {
        let g = self.inner.lock();
        let Some(set) = g.children.get(dir) else { return Vec::new() };
        set.iter()
            .map(|n| {
                let rel = if dir.is_empty() { n.clone() } else { format!("{dir}/{n}") };
                (n.clone(), g.dirs.contains_key(&rel))
            })
            .collect()
    }

    /// mkdir -p inside the spool for `rel` (used when the directory only exists on destinations).
    pub fn ensure_dir_all(&self, rel: &str, mode: u32, uid: u32, gid: u32) -> Result<(), i32> {
        if rel.is_empty() || self.is_dir(rel) {
            return Ok(());
        }
        fs::create_dir_all(self.spool_path(rel)).map_err(errno)?;
        let mut g = self.inner.lock();
        let mut cur = String::new();
        for part in rel.split('/') {
            if !cur.is_empty() {
                cur.push('/');
            }
            cur.push_str(part);
            if g.files.contains_key(&cur) {
                return Err(libc::ENOTDIR);
            }
            if !g.dirs.contains_key(&cur) {
                let _ = fs::set_permissions(self.spool_path(&cur), fs::Permissions::from_mode(mode & 0o7777));
                let _ = std::os::unix::fs::chown(self.spool_path(&cur), Some(uid), Some(gid));
                g.dirs.insert(cur.clone(), DirInfo { mtime: SystemTime::now(), mode: libc::S_IFDIR | (mode & 0o7777), uid, gid, created: Instant::now() });
                g.link(&cur);
            }
        }
        Ok(())
    }

    pub fn mkdir(&self, rel: &str, mode: u32, uid: u32, gid: u32) -> Result<DirInfo, i32> {
        {
            let g = self.inner.lock();
            if g.files.contains_key(rel) || g.dirs.contains_key(rel) {
                return Err(libc::EEXIST);
            }
            if !g.dirs.contains_key(parent_of(rel)) {
                return Err(libc::ENOENT);
            }
        }
        match fs::create_dir(self.spool_path(rel)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(errno(e)),
        }
        let _ = fs::set_permissions(self.spool_path(rel), fs::Permissions::from_mode(mode & 0o7777));
        let _ = std::os::unix::fs::chown(self.spool_path(rel), Some(uid), Some(gid));
        let info = DirInfo { mtime: SystemTime::now(), mode: libc::S_IFDIR | (mode & 0o7777), uid, gid, created: Instant::now() };
        let mut g = self.inner.lock();
        g.dirs.insert(rel.to_string(), info.clone());
        g.link(rel);
        Ok(info)
    }

    /// Removes an empty RAM directory.
    pub fn rmdir(&self, rel: &str) -> Result<(), i32> {
        let mut g = self.inner.lock();
        if !g.dirs.contains_key(rel) || rel.is_empty() {
            return Err(libc::ENOENT);
        }
        if g.children.get(rel).is_some_and(|c| !c.is_empty()) {
            return Err(libc::ENOTEMPTY);
        }
        fs::remove_dir(self.spool_path(rel)).map_err(errno)?;
        g.dirs.remove(rel);
        g.children.remove(rel);
        g.unlink_child(rel);
        Ok(())
    }

    fn open_spool(&self, rel: &str, create: bool, trunc: bool, mode: u32) -> Result<File, i32> {
        let mut o = OpenOptions::new();
        o.read(true).write(true).create(create).truncate(trunc).mode(mode & 0o7777);
        o.open(self.spool_path(rel)).map_err(errno)
    }

    /// Creates (or truncates) a file for writing. The parent must exist in RAM.
    pub fn create(&self, rel: &str, mode: u32, uid: u32, gid: u32) -> Result<(Arc<File>, RamFile), i32> {
        {
            let g = self.inner.lock();
            if g.dirs.contains_key(rel) {
                return Err(libc::EISDIR);
            }
            if !g.dirs.contains_key(parent_of(rel)) {
                return Err(libc::ENOENT);
            }
        }
        if self.lookup(rel).is_some() {
            let (f, rf) = self.open(rel, true, true)?;
            return Ok((f, rf));
        }
        let file = self.open_spool(rel, true, true, mode)?;
        let _ = std::os::unix::fs::fchown(&file, Some(uid), Some(gid));
        let now = SystemTime::now();
        let rf = RamFile {
            size: 0,
            mtime: now,
            ctime: now,
            mode: libc::S_IFREG | (mode & 0o7777),
            uid,
            gid,
            writers: 1,
            last_write: Instant::now(),
            state: FState::Writing,
            version: 1,
            seq: 0,
            created: Instant::now(),
            meta: None,
            attempts: 0,
            not_before: None,
            avoid: None,
            origin: None,
            fid: self.new_fid(),
            labels: None,
            decide_since: None,
        };
        let mut g = self.inner.lock();
        g.files.insert(rel.to_string(), rf.clone());
        g.link(rel);
        Ok((Arc::new(file), rf))
    }

    /// Opens an existing RAM file. Opening for write pulls it back to `Writing`,
    /// whatever stage it was in (an in-flight transfer becomes stale).
    pub fn open(&self, rel: &str, write: bool, trunc: bool) -> Result<(Arc<File>, RamFile), i32> {
        let file = self.open_spool(rel, false, false, 0)?;
        let mut g = self.inner.lock();
        if !g.files.contains_key(rel) {
            return Err(libc::ENOENT);
        }
        if write {
            g.back_to_writing(rel);
            g.settling.remove(rel);
            let f = g.files.get_mut(rel).unwrap();
            f.writers += 1;
            f.last_write = Instant::now();
            if trunc && f.size > 0 {
                file.set_len(0).map_err(errno)?;
                self.used.fetch_sub(f.size, Ordering::Relaxed);
                f.size = 0;
                f.mtime = SystemTime::now();
            }
        }
        Ok((Arc::new(file), g.files[rel].clone()))
    }

    /// Brings a file from outside the spool into RAM as a closed, settled file
    /// (it gets sealed and moved like any other). Used for files written to
    /// the directory under the mount while spillway was down. Returns
    /// `Ok(false)` when `rel` already exists in RAM (left alone).
    pub fn adopt_external(&self, rel: &str, src: &Path, m: &fs::Metadata, hard_limit: u64) -> Result<bool, i32> {
        if self.lookup(rel).is_some() {
            return Ok(false);
        }
        if self.used() + m.len() > hard_limit {
            return Err(libc::ENOSPC);
        }
        let parent = parent_of(rel);
        self.ensure_dir_all(parent, 0o755, m.uid(), m.gid())?;
        // copy under a temporary name, publish with a rename under the lock
        let tmp = self.spool_path(&format!("{rel}{ADOPT_SUFFIX}"));
        fs::copy(src, &tmp).map_err(errno)?;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(m.mode() & 0o7777));
        let _ = std::os::unix::fs::chown(&tmp, Some(m.uid()), Some(m.gid()));
        let mtime = m.modified().unwrap_or(SystemTime::now());
        let _ = File::options().write(true).open(&tmp).and_then(|f| f.set_modified(mtime));
        let mut g = self.inner.lock();
        if g.files.contains_key(rel) || g.dirs.contains_key(rel) {
            drop(g);
            let _ = fs::remove_file(&tmp);
            return Ok(false);
        }
        fs::rename(&tmp, self.spool_path(rel)).map_err(errno)?;
        let long_ago = Instant::now().checked_sub(Duration::from_secs(3600)).unwrap_or_else(Instant::now);
        g.files.insert(
            rel.to_string(),
            RamFile {
                size: m.len(),
                mtime,
                ctime: SystemTime::now(),
                mode: libc::S_IFREG | (m.mode() & 0o7777),
                uid: m.uid(),
                gid: m.gid(),
                writers: 0,
                last_write: long_ago,
                state: FState::Writing,
                version: 1,
                seq: 0,
                created: long_ago,
                meta: None,
                attempts: 0,
                not_before: None,
                avoid: None,
                origin: None,
                fid: self.new_fid(),
                labels: None,
                decide_since: None,
            },
        );
        g.settling.insert(rel.to_string());
        g.link(rel);
        self.used.fetch_add(m.len(), Ordering::Relaxed);
        Ok(true)
    }

    /// Bytes that may still be written before hitting `hard_limit`.
    pub fn room(&self, hard_limit: u64) -> u64 {
        hard_limit.saturating_sub(self.used())
    }

    /// Writes into a RAM file. Fails with ENOSPC when growing past `hard_limit`.
    pub fn write(&self, rel: &str, file: &File, offset: u64, data: &[u8], hard_limit: u64) -> Result<usize, i32> {
        let end = offset + data.len() as u64;
        {
            let g = self.inner.lock();
            let Some(f) = g.files.get(rel) else { return Err(libc::ENOENT) };
            if end > f.size && self.used() + (end - f.size) > hard_limit {
                return Err(libc::ENOSPC);
            }
        }
        file.write_all_at(data, offset).map_err(errno)?;
        let mut g = self.inner.lock();
        if let Some(f) = g.files.get_mut(rel) {
            if end > f.size {
                self.used.fetch_add(end - f.size, Ordering::Relaxed);
                f.size = end;
            }
            f.mtime = SystemTime::now();
            f.last_write = Instant::now();
            f.version += 1;
            if f.state != FState::Writing {
                // Written through a handle opened before sealing (e.g. a reader-writer): unseal.
                g.back_to_writing(rel);
            }
        }
        Ok(data.len())
    }

    /// fallocate on a RAM file (preallocation counts against the RAM budget).
    pub fn fallocate(&self, rel: &str, file: &File, offset: u64, length: u64, mode: i32, hard_limit: u64) -> Result<(), i32> {
        use std::os::fd::AsRawFd;
        let end = offset + length;
        let keep_size = mode & libc::FALLOC_FL_KEEP_SIZE != 0;
        {
            let g = self.inner.lock();
            let Some(f) = g.files.get(rel) else { return Err(libc::ENOENT) };
            if !keep_size && end > f.size && self.used() + (end - f.size) > hard_limit {
                return Err(libc::ENOSPC);
            }
        }
        let rc = unsafe { libc::fallocate(file.as_raw_fd(), mode, offset as i64, length as i64) };
        if rc != 0 {
            return Err(io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO));
        }
        let mut g = self.inner.lock();
        if let Some(f) = g.files.get_mut(rel) {
            if !keep_size && end > f.size {
                self.used.fetch_add(end - f.size, Ordering::Relaxed);
                f.size = end;
            }
            f.mtime = SystemTime::now();
            f.last_write = Instant::now();
            f.version += 1;
        }
        Ok(())
    }

    pub fn truncate(&self, rel: &str, size: u64, hard_limit: u64) -> Result<RamFile, i32> {
        let mut g = self.inner.lock();
        let Some(f) = g.files.get(rel) else { return Err(libc::ENOENT) };
        if size > f.size && self.used() + (size - f.size) > hard_limit {
            return Err(libc::ENOSPC);
        }
        let file = OpenOptions::new().write(true).open(self.spool_path(rel)).map_err(errno)?;
        file.set_len(size).map_err(errno)?;
        g.back_to_writing(rel);
        let was_settling = g.settling.contains(rel);
        let f = g.files.get_mut(rel).unwrap();
        if size >= f.size {
            self.used.fetch_add(size - f.size, Ordering::Relaxed);
        } else {
            self.used.fetch_sub(f.size - size, Ordering::Relaxed);
        }
        f.size = size;
        f.mtime = SystemTime::now();
        f.last_write = Instant::now();
        let out = f.clone();
        if out.writers == 0 && !was_settling {
            g.settling.insert(rel.to_string());
        }
        Ok(out)
    }

    pub fn set_attrs(&self, rel: &str, mode: Option<u32>, uid: Option<u32>, gid: Option<u32>, mtime: Option<SystemTime>) -> Result<RamFile, i32> {
        let p = self.spool_path(rel);
        if let Some(m) = mode {
            fs::set_permissions(&p, fs::Permissions::from_mode(m & 0o7777)).map_err(errno)?;
        }
        if uid.is_some() || gid.is_some() {
            std::os::unix::fs::chown(&p, uid, gid).map_err(errno)?;
        }
        let mut g = self.inner.lock();
        let Some(f) = g.files.get_mut(rel) else { return Err(libc::ENOENT) };
        if let Some(m) = mode {
            f.mode = libc::S_IFREG | (m & 0o7777);
        }
        if let Some(u) = uid {
            f.uid = u;
        }
        if let Some(gg) = gid {
            f.gid = gg;
        }
        if let Some(t) = mtime {
            f.mtime = t;
        }
        Ok(f.clone())
    }

    pub fn set_dir_attrs(&self, rel: &str, mode: Option<u32>, uid: Option<u32>, gid: Option<u32>) -> Result<DirInfo, i32> {
        let p = self.spool_path(rel);
        if let Some(m) = mode {
            fs::set_permissions(&p, fs::Permissions::from_mode(m & 0o7777)).map_err(errno)?;
        }
        if uid.is_some() || gid.is_some() {
            std::os::unix::fs::chown(&p, uid, gid).map_err(errno)?;
        }
        let mut g = self.inner.lock();
        let Some(d) = g.dirs.get_mut(rel) else { return Err(libc::ENOENT) };
        if let Some(m) = mode {
            d.mode = libc::S_IFDIR | (m & 0o7777);
        }
        if let Some(u) = uid {
            d.uid = u;
        }
        if let Some(gg) = gid {
            d.gid = gg;
        }
        Ok(d.clone())
    }

    /// A write handle was closed.
    pub fn release_writer(&self, rel: &str) {
        let mut g = self.inner.lock();
        let Some(f) = g.files.get_mut(rel) else { return };
        f.writers = f.writers.saturating_sub(1);
        f.last_write = Instant::now();
        if f.writers == 0 && f.state == FState::Writing {
            g.settling.insert(rel.to_string());
        }
    }

    pub fn unlink(&self, rel: &str) -> Result<RamFile, i32> {
        let mut g = self.inner.lock();
        if !g.files.contains_key(rel) {
            return Err(if g.dirs.contains_key(rel) { libc::EISDIR } else { libc::ENOENT });
        }
        match fs::remove_file(self.spool_path(rel)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(errno(e)),
        }
        g.dequeue(rel);
        g.undecide(rel);
        g.settling.remove(rel);
        let f = g.files.remove(rel).unwrap();
        g.unlink_child(rel);
        self.used.fetch_sub(f.size, Ordering::Relaxed);
        Ok(f)
    }

    /// Renames a RAM file (target parent must be a RAM dir). An existing RAM
    /// file at the target is replaced. In-flight transfers become stale.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), i32> {
        let mut g = self.inner.lock();
        if !g.files.contains_key(from) {
            return Err(libc::ENOENT);
        }
        if from == to {
            return Ok(()); // POSIX: renaming onto itself is a no-op
        }
        if g.dirs.contains_key(to) {
            return Err(libc::EISDIR);
        }
        if !g.dirs.contains_key(parent_of(to)) {
            return Err(libc::ENOENT);
        }
        fs::rename(self.spool_path(from), self.spool_path(to)).map_err(errno)?;
        if g.files.contains_key(to) {
            g.dequeue(to);
            g.undecide(to);
            g.settling.remove(to);
            let old = g.files.remove(to).unwrap();
            self.used.fetch_sub(old.size, Ordering::Relaxed);
        }
        g.back_to_writing(from);
        let was_settling = g.settling.remove(from);
        let mut f = g.files.remove(from).unwrap();
        f.last_write = Instant::now();
        let closed = f.writers == 0;
        g.files.insert(to.to_string(), f);
        g.unlink_child(from);
        g.link(to);
        if closed || was_settling {
            g.settling.insert(to.to_string());
        }
        Ok(())
    }

    /// Imports a file copied up from a destination so it can be modified.
    pub fn adopt(&self, rel: &str, file: File, size: u64, mtime: SystemTime, mode: u32, uid: u32, gid: u32, origin: String) -> Arc<File> {
        let mut g = self.inner.lock();
        let rf = RamFile {
            size,
            mtime,
            ctime: SystemTime::now(),
            mode: libc::S_IFREG | (mode & 0o7777),
            uid,
            gid,
            writers: 0,
            last_write: Instant::now(),
            state: FState::Writing,
            version: 1,
            seq: 0,
            created: Instant::now(),
            meta: None,
            attempts: 0,
            not_before: None,
            avoid: None,
            origin: Some(origin),
            fid: self.new_fid(),
            labels: None,
            decide_since: None,
        };
        g.files.insert(rel.to_string(), rf);
        g.link(rel);
        g.settling.insert(rel.to_string());
        self.used.fetch_add(size, Ordering::Relaxed);
        Arc::new(file)
    }

    /// Closed files quiet for `settle`: moved to `Sealing` and returned with their version.
    pub fn take_settled(&self, settle: Duration, max: usize) -> Vec<(String, u64, u64)> {
        let mut g = self.inner.lock();
        let now = Instant::now();
        let ready: Vec<String> = g
            .settling
            .iter()
            .filter(|rel| {
                g.files.get(*rel).is_some_and(|f| f.writers == 0 && f.state == FState::Writing && now.duration_since(f.last_write) >= settle)
            })
            .take(max)
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(ready.len());
        for rel in ready {
            g.settling.remove(&rel);
            let f = g.files.get_mut(&rel).unwrap();
            f.state = FState::Sealing;
            out.push((rel, f.version, f.size));
        }
        // drop entries that no longer qualify (deleted / reopened)
        let stale: Vec<String> = g.settling.iter().filter(|r| g.files.get(*r).map_or(true, |f| f.writers > 0 || f.state != FState::Writing)).cloned().collect();
        for r in stale {
            g.settling.remove(&r);
        }
        out
    }

    /// Finishes sealing if the file did not change meanwhile: it joins the
    /// transfer queue, or waits for the validate hook when `decide`.
    pub fn finish_seal(&self, rel: &str, version: u64, meta: Arc<FileMeta>, decide: bool) -> bool {
        let mut g = self.inner.lock();
        match g.files.get_mut(rel) {
            Some(f) if f.version == version && f.state == FState::Sealing => {
                f.meta = Some(meta);
                if decide {
                    let seq = g.next_seq;
                    g.next_seq += 1;
                    let f = g.files.get_mut(rel).unwrap();
                    f.state = FState::Deciding { asking: false };
                    f.seq = seq;
                    f.not_before = None;
                    f.decide_since = Some(Instant::now());
                    g.deciding.insert((seq, rel.to_string()));
                } else {
                    g.enqueue(rel);
                }
                true
            }
            _ => false,
        }
    }

    /// How many files are ready for a hook decision (up to `max`) and since
    /// when the oldest of them has been waiting.
    pub fn deciding_ready(&self, max: usize) -> (usize, Option<Instant>) {
        let g = self.inner.lock();
        let now = Instant::now();
        let mut n = 0;
        let mut oldest: Option<Instant> = None;
        for (_, rel) in g.deciding.iter().take(max.saturating_mul(4).max(1024)) {
            let Some(f) = g.files.get(rel) else { continue };
            if f.state != (FState::Deciding { asking: false }) || f.not_before.is_some_and(|t| t > now) {
                continue;
            }
            n += 1;
            let since = f.not_before.unwrap_or(f.decide_since.unwrap_or(now));
            oldest = Some(oldest.map_or(since, |o| o.min(since)));
            if n >= max {
                break;
            }
        }
        (n, oldest)
    }

    /// Hands up to `max` ready files to a hook batch (they become `asking`).
    pub fn take_for_decision(&self, max: usize) -> Vec<DecisionItem> {
        let mut g = self.inner.lock();
        let now = Instant::now();
        let picked: Vec<String> = g
            .deciding
            .iter()
            .take(max.saturating_mul(4).max(1024))
            .filter(|(_, rel)| {
                g.files.get(rel).is_some_and(|f| f.state == (FState::Deciding { asking: false }) && f.not_before.is_none_or(|t| t <= now))
            })
            .take(max)
            .map(|(_, rel)| rel.clone())
            .collect();
        let mut out = Vec::with_capacity(picked.len());
        for rel in picked {
            let f = g.files.get_mut(&rel).unwrap();
            f.state = FState::Deciding { asking: true };
            out.push(DecisionItem { rel, fid: f.fid, version: f.version, size: f.size, mtime: f.mtime, meta: f.meta.clone() });
        }
        out
    }

    /// Applies the hook's answer to a file, if it is still the same file in the same batch.
    pub fn apply_decision(&self, item: &DecisionItem, d: Decision) -> Applied {
        {
            let mut g = self.inner.lock();
            match g.files.get(&item.rel) {
                Some(f) if f.fid == item.fid && f.version == item.version && f.state == (FState::Deciding { asking: true }) => {}
                _ => return Applied::Stale,
            }
            match d {
                Decision::Move(labels) => {
                    g.undecide(&item.rel);
                    let f = g.files.get_mut(&item.rel).unwrap();
                    f.labels = labels.map(Arc::new);
                    f.not_before = None;
                    g.enqueue(&item.rel);
                    return Applied::Moved;
                }
                Decision::Hold(after) => {
                    let f = g.files.get_mut(&item.rel).unwrap();
                    f.state = FState::Deciding { asking: false };
                    f.not_before = Some(Instant::now() + after);
                    return Applied::Held;
                }
                Decision::Drop => {}
            }
        }
        match self.unlink(&item.rel) {
            Ok(f) => Applied::Dropped(f),
            Err(_) => Applied::Stale,
        }
    }

    /// Hook turned off: everything still waiting for a decision goes straight to the queue.
    pub fn release_deciding(&self) -> usize {
        let mut g = self.inner.lock();
        let all: Vec<String> = g.deciding.iter().map(|(_, r)| r.clone()).collect();
        let mut n = 0;
        for rel in all {
            g.undecide(&rel);
            if let Some(f) = g.files.get_mut(&rel) {
                f.not_before = None;
                g.enqueue(&rel);
                n += 1;
            }
        }
        n
    }

    /// Removes a file that will not be moved (empty / invalid MP4) if unchanged.
    pub fn discard_if(&self, rel: &str, version: u64) -> Option<RamFile> {
        {
            let g = self.inner.lock();
            match g.files.get(rel) {
                Some(f) if f.version == version && f.writers == 0 => {}
                _ => return None,
            }
        }
        self.unlink(rel).ok()
    }

    /// Next file to transfer, alternating oldest-first and largest-first (the
    /// largest pick frees RAM fastest; the oldest pick bounds latency). Files in
    /// retry backoff are skipped. Does not change state — see `claim`.
    pub fn peek_next(&self) -> Option<(String, Option<String>, u64)> {
        let mut g = self.inner.lock();
        let now = Instant::now();
        let largest = g.pick_largest;
        g.pick_largest = !largest;
        let due = |rel: &String, g: &Inner| g.files.get(rel).is_some_and(|f| f.not_before.map_or(true, |t| t <= now));
        let found = if largest {
            g.largest.iter().take(256).map(|(_, _, r)| r).find(|r| due(r, &g)).cloned()
        } else {
            None
        }
        .or_else(|| g.oldest.iter().take(256).map(|(_, r)| r).find(|r| due(r, &g)).cloned());
        let rel = found?;
        let f = &g.files[&rel];
        let avoid = f.avoid.as_ref().filter(|(_, until)| *until > now).map(|(d, _)| d.clone());
        let size = f.size;
        Some((rel, avoid, size))
    }

    pub fn claim(&self, rel: &str, dest: &str) -> Option<Job> {
        let mut g = self.inner.lock();
        if g.files.get(rel).map(|f| &f.state) != Some(&FState::Queued) {
            return None;
        }
        g.dequeue(rel);
        let f = g.files.get_mut(rel).unwrap();
        f.state = FState::InFlight { dest: dest.to_string() };
        f.attempts += 1;
        Some(Job {
            rel: rel.to_string(),
            size: f.size,
            version: f.version,
            mtime: f.mtime,
            mode: f.mode,
            uid: f.uid,
            gid: f.gid,
            meta: f.meta.clone(),
            origin: f.origin.clone(),
            attempts: f.attempts,
            fid: f.fid,
            labels: f.labels.clone(),
        })
    }

    /// Transfer finished successfully: drop the RAM copy if nothing changed meanwhile.
    pub fn commit(&self, job: &Job, dest: &str) -> Commit {
        let mut g = self.inner.lock();
        match g.files.get(&job.rel) {
            None => return Commit::Deleted,
            Some(f) if f.version != job.version || f.state != (FState::InFlight { dest: dest.to_string() }) => return Commit::Stale,
            _ => {}
        }
        let _ = fs::remove_file(self.spool_path(&job.rel));
        let f = g.files.remove(&job.rel).unwrap();
        g.unlink_child(&job.rel);
        self.used.fetch_sub(f.size, Ordering::Relaxed);
        Commit::Done
    }

    /// Transfer failed: back to the queue after `backoff`, avoiding `dest` for `avoid_for`.
    pub fn requeue(&self, job: &Job, dest: &str, backoff: Duration, avoid_for: Duration) {
        let mut g = self.inner.lock();
        let Some(f) = g.files.get_mut(&job.rel) else { return };
        if f.state != (FState::InFlight { dest: dest.to_string() }) {
            return;
        }
        let now = Instant::now();
        f.not_before = Some(now + backoff);
        f.avoid = Some((dest.to_string(), now + avoid_for));
        g.enqueue(&job.rel);
    }

    /// Thief: removes the oldest queued file (never one being written or in flight).
    pub fn steal_oldest(&self) -> Option<(String, RamFile)> {
        let mut g = self.inner.lock();
        let rel = match g.oldest.iter().next().or_else(|| g.deciding.iter().next()) {
            Some((_, r)) => r.clone(),
            // Nothing sealed: fall back to closed files still settling (a burst
            // faster than the settle window). Never touches open files.
            None => g
                .settling
                .iter()
                .filter(|r| g.files.get(*r).is_some_and(|f| f.writers == 0 && f.state == FState::Writing))
                .min_by_key(|r| g.files[*r].created)
                .cloned()?,
        };
        g.dequeue(&rel);
        g.undecide(&rel);
        g.settling.remove(&rel);
        let _ = fs::remove_file(self.spool_path(&rel));
        let f = g.files.remove(&rel)?;
        g.unlink_child(&rel);
        self.used.fetch_sub(f.size, Ordering::Relaxed);
        Some((rel, f))
    }

    /// Removes empty spool directories older than `min_age` (bottom-up).
    pub fn prune_empty_dirs(&self, min_age: Duration) -> usize {
        let mut g = self.inner.lock();
        let mut removed = 0;
        loop {
            let victims: Vec<String> = g
                .dirs
                .iter()
                .filter(|(rel, d)| !rel.is_empty() && d.created.elapsed() >= min_age && g.children.get(*rel).map_or(true, |c| c.is_empty()))
                .map(|(rel, _)| rel.clone())
                .collect();
            if victims.is_empty() {
                break;
            }
            for rel in victims {
                if fs::remove_dir(self.spool_path(&rel)).is_ok() || !self.spool_path(&rel).exists() {
                    g.dirs.remove(&rel);
                    g.children.remove(&rel);
                    g.unlink_child(&rel);
                    removed += 1;
                } else {
                    // Something else lives there; keep it and stop retrying this round.
                    return removed;
                }
            }
        }
        removed
    }

    pub fn snapshot(&self) -> RamSnapshot {
        let g = self.inner.lock();
        let now = Instant::now();
        let mut s = RamSnapshot { used_bytes: self.used(), files: g.files.len() as u64, dirs: g.dirs.len() as u64, ..Default::default() };
        for f in g.files.values() {
            match &f.state {
                FState::Writing => {
                    s.writing += 1;
                    if f.writers > 0 {
                        s.open_writers += 1;
                        s.oldest_open_age_s = s.oldest_open_age_s.max(now.duration_since(f.created).as_secs_f64());
                    }
                }
                FState::Sealing => s.sealing += 1,
                FState::Deciding { .. } => {
                    s.deciding += 1;
                    s.deciding_bytes += f.size;
                }
                FState::Queued => {
                    s.queued += 1;
                    s.queued_bytes += f.size;
                }
                FState::InFlight { .. } => {
                    s.inflight += 1;
                    s.inflight_bytes += f.size;
                }
            }
        }
        if let Some((_, rel)) = g.oldest.iter().next() {
            if let Some(f) = g.files.get(rel) {
                s.oldest_queued_age_s = now.duration_since(f.last_write).as_secs_f64();
            }
        }
        s
    }

    /// Sum of queued bytes + count of files not yet on a destination (for shutdown).
    pub fn pending(&self) -> (usize, usize) {
        let g = self.inner.lock();
        let open = g.files.values().filter(|f| f.writers > 0).count();
        (g.files.len(), open)
    }

    /// Path of the spool file, checking it is a RAM file.
    pub fn exists_file(&self, rel: &str) -> bool {
        self.inner.lock().files.contains_key(rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ram() -> (tempfile::TempDir, Ram) {
        let d = tempfile::tempdir().unwrap();
        let r = Ram::new(d.path().to_path_buf());
        (d, r)
    }

    const HL: u64 = u64::MAX / 2;

    fn write_file(r: &Ram, rel: &str, data: &[u8]) {
        let (f, _) = r.create(rel, 0o644, 0, 0).unwrap();
        r.write(rel, &f, 0, data, HL).unwrap();
        r.release_writer(rel);
    }

    fn meta() -> Arc<FileMeta> {
        Arc::new(FileMeta { mp4: Err("x".into()), mp4_error_code: Some("not_mp4"), parse_ms: 0.0 })
    }

    #[test]
    fn lifecycle_seal_queue_claim_commit() {
        let (_d, r) = ram();
        r.mkdir("cam", 0o755, 0, 0).unwrap();
        write_file(&r, "cam/a.mp4", b"hello");
        assert_eq!(r.used(), 5);
        // not settled yet
        assert!(r.take_settled(Duration::from_secs(60), 10).is_empty());
        let s = r.take_settled(Duration::ZERO, 10);
        assert_eq!(s.len(), 1);
        let (rel, ver, size) = s[0].clone();
        assert_eq!((rel.as_str(), size), ("cam/a.mp4", 5));
        assert!(r.finish_seal(&rel, ver, meta(), false));
        let (next, avoid, _) = r.peek_next().unwrap();
        assert_eq!((next.as_str(), avoid), ("cam/a.mp4", None));
        let job = r.claim(&next, "d1").unwrap();
        assert!(r.claim(&next, "d1").is_none());
        assert_eq!(r.commit(&job, "d1"), Commit::Done);
        assert_eq!(r.used(), 0);
        assert!(r.lookup("cam/a.mp4").is_none());
        assert!(!r.spool_path("cam/a.mp4").exists());
        assert!(r.children("cam").is_empty());
    }

    #[test]
    fn open_writer_blocks_sealing() {
        let (_d, r) = ram();
        let (f, _) = r.create("a.mp4", 0o644, 0, 0).unwrap();
        r.write("a.mp4", &f, 0, b"x", HL).unwrap();
        assert!(r.take_settled(Duration::ZERO, 10).is_empty(), "still open for write");
        r.release_writer("a.mp4");
        assert_eq!(r.take_settled(Duration::ZERO, 10).len(), 1);
    }

    #[test]
    fn reopen_for_write_invalidates_inflight_transfer() {
        let (_d, r) = ram();
        write_file(&r, "a.mp4", b"v1");
        let (rel, ver, _) = r.take_settled(Duration::ZERO, 1).remove(0);
        r.finish_seal(&rel, ver, meta(), false);
        let job = r.claim("a.mp4", "d").unwrap();
        // writer appends while the copy runs
        let (f, _) = r.open("a.mp4", true, false).unwrap();
        r.write("a.mp4", &f, 2, b"v2", HL).unwrap();
        r.release_writer("a.mp4");
        assert_eq!(r.commit(&job, "d"), Commit::Stale);
        assert_eq!(r.lookup("a.mp4").map(|l| matches!(l, Lookup::File(f) if f.size == 4 && f.state == FState::Writing)), Some(true));
        // it settles and queues again
        let (rel, ver, size) = r.take_settled(Duration::ZERO, 1).remove(0);
        assert_eq!(size, 4);
        assert!(r.finish_seal(&rel, ver, meta(), false));
    }

    #[test]
    fn write_during_sealing_rejects_seal() {
        let (_d, r) = ram();
        write_file(&r, "a.mp4", b"v1");
        let (rel, ver, _) = r.take_settled(Duration::ZERO, 1).remove(0);
        let (f, _) = r.open("a.mp4", true, false).unwrap();
        r.write("a.mp4", &f, 0, b"zz", HL).unwrap();
        assert!(!r.finish_seal(&rel, ver, meta(), false));
    }

    #[test]
    fn deleted_while_inflight() {
        let (_d, r) = ram();
        write_file(&r, "a.mp4", b"v1");
        let (rel, ver, _) = r.take_settled(Duration::ZERO, 1).remove(0);
        r.finish_seal(&rel, ver, meta(), false);
        let job = r.claim("a.mp4", "d").unwrap();
        r.unlink("a.mp4").unwrap();
        assert_eq!(r.commit(&job, "d"), Commit::Deleted);
        assert_eq!(r.used(), 0);
    }

    #[test]
    fn requeue_backoff_and_avoid() {
        let (_d, r) = ram();
        write_file(&r, "a.mp4", b"v1");
        let (rel, ver, _) = r.take_settled(Duration::ZERO, 1).remove(0);
        r.finish_seal(&rel, ver, meta(), false);
        let job = r.claim("a.mp4", "bad").unwrap();
        r.requeue(&job, "bad", Duration::from_millis(80), Duration::from_secs(30));
        assert!(r.peek_next().is_none(), "in backoff");
        std::thread::sleep(Duration::from_millis(100));
        let (_, avoid, _) = r.peek_next().unwrap();
        assert_eq!(avoid.as_deref(), Some("bad"));
        assert_eq!(r.claim("a.mp4", "good").unwrap().attempts, 2);
    }

    #[test]
    fn alternates_oldest_and_largest() {
        let (_d, r) = ram();
        for (name, n) in [("a", 10usize), ("b", 1000), ("c", 20), ("d", 500)] {
            write_file(&r, name, &vec![0u8; n]);
        }
        let mut s = r.take_settled(Duration::ZERO, 10);
        s.sort();
        for (rel, ver, _) in s {
            r.finish_seal(&rel, ver, meta(), false);
        }
        let mut order = Vec::new();
        while let Some((rel, _, _)) = r.peek_next() {
            r.claim(&rel, "d").unwrap();
            order.push(rel);
        }
        assert_eq!(order, vec!["a", "b", "c", "d"]); // oldest a, largest b, oldest c, largest d
    }

    #[test]
    fn thief_takes_oldest_queued_only() {
        let (_d, r) = ram();
        write_file(&r, "old", b"1111");
        write_file(&r, "new", b"22");
        let mut s = r.take_settled(Duration::ZERO, 10);
        s.sort_by_key(|x| x.0 != "old");
        for (rel, ver, _) in s {
            r.finish_seal(&rel, ver, meta(), false);
        }
        let (f, _) = r.create("open", 0o644, 0, 0).unwrap();
        r.write("open", &f, 0, b"333", HL).unwrap();
        assert_eq!(r.steal_oldest().unwrap().0, "old");
        assert_eq!(r.steal_oldest().unwrap().0, "new");
        assert!(r.steal_oldest().is_none(), "open file is never stolen");
        assert_eq!(r.used(), 3);
    }

    #[test]
    fn thief_falls_back_to_closed_settling_files() {
        let (_d, r) = ram();
        write_file(&r, "a", b"12345"); // closed, not sealed yet
        let (f, _) = r.create("b", 0o644, 0, 0).unwrap();
        r.write("b", &f, 0, b"x", HL).unwrap(); // still open
        assert_eq!(r.steal_oldest().unwrap().0, "a");
        assert!(r.steal_oldest().is_none());
        assert!(r.take_settled(Duration::ZERO, 10).is_empty(), "stolen file left the settling set");
    }

    /// Seals every closed file and parks it waiting for the hook, in name order.
    fn seal_all_for_decision(r: &Ram) {
        let mut s = r.take_settled(Duration::ZERO, 100);
        s.sort();
        for (rel, ver, _) in s {
            assert!(r.finish_seal(&rel, ver, meta(), true));
        }
    }

    #[test]
    fn decision_move_drop_hold() {
        let (_d, r) = ram();
        for n in ["a", "b", "c"] {
            write_file(&r, n, b"data");
        }
        seal_all_for_decision(&r);
        assert!(r.peek_next().is_none(), "nothing reaches the queue before a decision");
        assert_eq!(r.deciding_ready(10).0, 3);
        let batch = r.take_for_decision(10);
        assert_eq!(batch.len(), 3);
        assert_eq!(r.deciding_ready(10).0, 0, "asking files are not handed out twice");
        assert!(r.take_for_decision(10).is_empty());
        let by = |n: &str| batch.iter().find(|i| i.rel == n).unwrap().clone();
        let mut labels = Labels::new();
        labels.insert("camera".into(), "x".into());
        assert!(matches!(r.apply_decision(&by("a"), Decision::Move(Some(labels.clone()))), Applied::Moved));
        assert!(matches!(r.apply_decision(&by("b"), Decision::Drop), Applied::Dropped(_)));
        assert!(matches!(r.apply_decision(&by("c"), Decision::Hold(Duration::from_millis(60))), Applied::Held));
        // the same answer twice is ignored
        assert!(matches!(r.apply_decision(&by("a"), Decision::Drop), Applied::Stale));
        assert!(r.lookup("b").is_none() && !r.spool_path("b").exists());
        let (next, _, _) = r.peek_next().unwrap();
        let job = r.claim(&next, "d").unwrap();
        assert_eq!((job.rel.as_str(), job.labels.as_deref()), ("a", Some(&labels)));
        // held: not ready until the delay passes, then asked again
        assert_eq!(r.deciding_ready(10).0, 0);
        std::thread::sleep(Duration::from_millis(80));
        let again = r.take_for_decision(10);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].rel, "c");
        assert_eq!(snapshot_deciding(&r), 1);
    }

    fn snapshot_deciding(r: &Ram) -> u64 {
        r.snapshot().deciding
    }

    #[test]
    fn modified_while_deciding_gets_a_new_id() {
        let (_d, r) = ram();
        write_file(&r, "a", b"v1");
        seal_all_for_decision(&r);
        let item = r.take_for_decision(1).remove(0);
        let (f, _) = r.open("a", true, false).unwrap();
        r.write("a", &f, 2, b"v2", HL).unwrap();
        r.release_writer("a");
        assert!(matches!(r.apply_decision(&item, Decision::Move(None)), Applied::Stale));
        seal_all_for_decision(&r);
        let item2 = r.take_for_decision(1).remove(0);
        assert_eq!(item2.fid, item.fid);
        assert_ne!(item2.id(), item.id());
    }

    #[test]
    fn thief_and_release_cover_deciding_files() {
        let (_d, r) = ram();
        write_file(&r, "a", b"1234");
        write_file(&r, "b", b"56");
        seal_all_for_decision(&r);
        assert_eq!(r.steal_oldest().unwrap().0, "a", "a file waiting for the hook can still be stolen");
        assert_eq!(r.release_deciding(), 1);
        assert_eq!(r.peek_next().unwrap().0, "b");
    }

    #[test]
    fn hard_limit_enospc() {
        let (_d, r) = ram();
        let (f, _) = r.create("a", 0o644, 0, 0).unwrap();
        r.write("a", &f, 0, &[0u8; 10], 12).unwrap();
        assert_eq!(r.write("a", &f, 10, &[0u8; 5], 12), Err(libc::ENOSPC));
        // overwriting inside the file does not grow it
        r.write("a", &f, 0, &[1u8; 10], 12).unwrap();
    }

    #[test]
    fn rename_and_recover() {
        let (d, r) = ram();
        r.mkdir("x", 0o755, 0, 0).unwrap();
        write_file(&r, "x/tmp.mp4", b"abc");
        r.rename("x/tmp.mp4", "x/final.mp4").unwrap();
        assert!(r.lookup("x/tmp.mp4").is_none());
        assert_eq!(r.children("x"), vec![("final.mp4".to_string(), false)]);
        let r2 = Ram::new(d.path().to_path_buf());
        assert_eq!(r2.recover(Duration::from_secs(1)).unwrap(), (1, 3));
        assert!(r2.is_dir("x"));
        assert_eq!(r2.take_settled(Duration::from_millis(1), 10).len(), 1);
    }

    #[test]
    fn adopt_external_files() {
        let (_d, r) = ram();
        let outside = tempfile::tempdir().unwrap();
        let src = outside.path().join("outside.mp4");
        fs::write(&src, b"segment").unwrap();
        let m = fs::metadata(&src).unwrap();
        assert_eq!(r.adopt_external("cam/day/a.mp4", &src, &m, 1 << 30), Ok(true));
        assert_eq!(r.used(), 7);
        assert_eq!(fs::read(r.spool_path("cam/day/a.mp4")).unwrap(), b"segment");
        // settled right away: sealed on the next pass
        assert_eq!(r.take_settled(Duration::from_secs(2), 10).len(), 1);
        // same path already in RAM: left alone
        assert_eq!(r.adopt_external("cam/day/a.mp4", &src, &m, 1 << 30), Ok(false));
        // RAM full: refused, retried later
        assert_eq!(r.adopt_external("cam/day/b.mp4", &src, &m, 8), Err(libc::ENOSPC));
        // a copy cut short by a crash is not recovered as a file
        fs::write(r.spool_path(&format!("cam/day/c.mp4{ADOPT_SUFFIX}")), b"half").unwrap();
        let r2 = Ram::new(r.spool_path(""));
        r2.recover(Duration::from_millis(1)).unwrap();
        assert!(r2.lookup("cam/day/a.mp4").is_some());
        assert!(r2.lookup(&format!("cam/day/c.mp4{ADOPT_SUFFIX}")).is_none());
        assert!(!r.spool_path(&format!("cam/day/c.mp4{ADOPT_SUFFIX}")).exists());
    }

    #[test]
    fn prune_empty_dirs() {
        let (_d, r) = ram();
        r.ensure_dir_all("a/b/c", 0o755, 0, 0).unwrap();
        r.ensure_dir_all("keep", 0o755, 0, 0).unwrap();
        write_file(&r, "keep/f", b"1");
        assert_eq!(r.prune_empty_dirs(Duration::ZERO), 3);
        assert!(!r.is_dir("a") && r.is_dir("keep"));
    }
}
