//! The FUSE filesystem: a union of the RAM spool (read-write, always wins) and
//! every readable destination (searched in configured order, first match wins —
//! like mergerfs `ff`).
//!
//! * New files are created in RAM. Writes stay there until the file is sealed.
//! * Files already on a destination are served from it (local or remote node,
//!   with 1 MiB read-ahead). Opening one for writing copies it back up to RAM
//!   first, so the file stays fully writable through the mount.
//! * Inodes are path-based and never reused; a file keeps its inode when it
//!   moves from RAM to disk, so open readers never notice the move.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use fuser::{
    Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo, InitFlags, KernelConfig, LockOwner, MountOption, OpenFlags,
    RenameFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
    WriteFlags,
};
use parking_lot::Mutex;
use tokio::runtime::Handle as Rt;

use crate::config::MountCfg;
use crate::loc::Loc;
use crate::master::Core;
use crate::proto::{Entry, Kind, NodeError};
use crate::ram::{DirInfo, Lookup, RamFile};
use crate::stats::Counters;

const TTL: Duration = Duration::from_secs(1);
const READAHEAD: u32 = 1024 * 1024;
const STAT_TIMEOUT: Duration = Duration::from_millis(2000);
/// `open` re-checks a cached destination location older than this.
const REVALIDATE: Duration = Duration::from_secs(2);
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

struct Inodes {
    by_ino: HashMap<u64, (String, u64)>,
    by_path: HashMap<String, u64>,
    next: u64,
    /// Recently forgotten inodes, to explain a kernel request for an unknown inode.
    forgotten: std::collections::VecDeque<(u64, String)>,
}

impl Inodes {
    fn new() -> Self {
        let mut by_ino = HashMap::new();
        let mut by_path = HashMap::new();
        by_ino.insert(1, (String::new(), u64::MAX / 2));
        by_path.insert(String::new(), 1);
        Inodes { by_ino, by_path, next: 2, forgotten: std::collections::VecDeque::new() }
    }
    fn path(&self, ino: u64) -> Option<String> {
        self.by_ino.get(&ino).map(|(p, _)| p.clone())
    }
    fn lookup(&mut self, path: &str) -> u64 {
        if let Some(&ino) = self.by_path.get(path) {
            self.by_ino.get_mut(&ino).unwrap().1 += 1;
            return ino;
        }
        let ino = self.next;
        self.next += 1;
        self.by_ino.insert(ino, (path.to_string(), 1));
        self.by_path.insert(path.to_string(), ino);
        ino
    }
    fn peek(&self, path: &str) -> Option<u64> {
        self.by_path.get(path).copied()
    }
    fn forget(&mut self, ino: u64, n: u64) {
        if ino == 1 {
            return;
        }
        let Some(e) = self.by_ino.get_mut(&ino) else { return };
        e.1 = e.1.saturating_sub(n);
        if e.1 == 0 {
            let (p, _) = self.by_ino.remove(&ino).unwrap();
            if self.by_path.get(&p) == Some(&ino) {
                self.by_path.remove(&p);
            }
            self.forgotten.push_back((ino, p));
            if self.forgotten.len() > 4096 {
                self.forgotten.pop_front();
            }
        }
    }
    fn rename(&mut self, from: &str, to: &str) {
        if let Some(old) = self.by_path.remove(to) {
            if let Some(e) = self.by_ino.get_mut(&old) {
                e.0 = format!("\0gone/{old}");
            }
        }
        if let Some(ino) = self.by_path.remove(from) {
            self.by_ino.get_mut(&ino).unwrap().0 = to.to_string();
            self.by_path.insert(to.to_string(), ino);
        }
    }
}

enum Handle {
    Ram { rel: Mutex<String>, file: Arc<File>, write: bool },
    Dest { dest: String, rel: String, size: u64, cache: Mutex<Option<(u64, Bytes)>> },
    Dir { entries: Vec<(u64, FileType, String)> },
}

#[derive(Debug)]
enum Res {
    RamFile(RamFile),
    RamDir(DirInfo),
    Dest(Loc),
}

pub struct SpillFs {
    core: Arc<Core>,
    rt: Rt,
    inodes: Mutex<Inodes>,
    handles: Mutex<HashMap<u64, Arc<Handle>>>,
    next_fh: AtomicU64,
    copy_up_lock: Mutex<()>,
    writeback: bool,
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn parent_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(p, _)| p)
}

fn e(n: i32) -> Errno {
    Errno::from_i32(n)
}

fn ms_to_time(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}

fn attr(ino: u64, kind: FileType, size: u64, mtime: SystemTime, ctime: SystemTime, mode: u32, uid: u32, gid: u32) -> FileAttr {
    FileAttr {
        ino: INodeNo(ino),
        size,
        blocks: size.div_ceil(512),
        atime: mtime,
        mtime,
        ctime,
        crtime: ctime,
        kind,
        perm: (mode & 0o7777) as u16,
        nlink: if kind == FileType::Directory { 2 } else { 1 },
        uid,
        gid,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

impl Res {
    fn attr(&self, ino: u64) -> FileAttr {
        match self {
            Res::RamFile(f) => attr(ino, FileType::RegularFile, f.size, f.mtime, f.ctime, f.mode, f.uid, f.gid),
            Res::RamDir(d) => attr(ino, FileType::Directory, 0, d.mtime, d.mtime, d.mode, d.uid, d.gid),
            Res::Dest(l) => {
                let t = ms_to_time(l.entry.mtime_ms);
                let kind = if l.entry.kind == Kind::Dir { FileType::Directory } else { FileType::RegularFile };
                attr(ino, kind, l.entry.size, t, t, l.entry.mode, l.entry.uid, l.entry.gid)
            }
        }
    }
    fn is_dir(&self) -> bool {
        matches!(self, Res::RamDir(_)) || matches!(self, Res::Dest(l) if l.entry.kind == Kind::Dir)
    }
}

impl SpillFs {
    pub fn new(core: Arc<Core>, rt: Rt, writeback: bool) -> Self {
        SpillFs {
            core,
            rt,
            inodes: Mutex::new(Inodes::new()),
            handles: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
            copy_up_lock: Mutex::new(()),
            writeback,
        }
    }

    fn path(&self, ino: INodeNo) -> Result<String, Errno> {
        let p = {
            let g = self.inodes.lock();
            match g.path(ino.0) {
                Some(p) => p,
                None => {
                    let was = g.forgotten.iter().rev().find(|(i, _)| *i == ino.0).map(|(_, p)| p.clone());
                    tracing::warn!(ino = ino.0, forgotten = ?was, next = g.next, "kernel used an unknown inode");
                    return Err(Errno::ENOENT);
                }
            }
        };
        if p.starts_with('\0') {
            return Err(Errno::ENOENT);
        }
        Ok(p)
    }

    fn child(&self, parent: INodeNo, name: &OsStr) -> Result<String, Errno> {
        let name = name.to_str().ok_or(Errno::EINVAL)?;
        if name.contains('/') || name == "." || name == ".." {
            return Err(Errno::EINVAL);
        }
        let p = self.path(parent)?;
        if p.is_empty() && name == crate::proto::META_DIR {
            return Err(Errno::ENOENT);
        }
        Ok(join(&p, name))
    }

    fn add_handle(&self, h: Handle) -> FileHandle {
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        self.handles.lock().insert(fh, Arc::new(h));
        FileHandle(fh)
    }

    fn handle(&self, fh: FileHandle) -> Result<Arc<Handle>, Errno> {
        self.handles.lock().get(&fh.0).cloned().ok_or(Errno::EBADF)
    }

    /// Finds `rel` in RAM, then on the destinations (cached).
    fn resolve(&self, rel: &str) -> Result<Res, Errno> {
        match self.core.ram.lookup(rel) {
            Some(Lookup::File(f)) => return Ok(Res::RamFile(f)),
            Some(Lookup::Dir(d)) => return Ok(Res::RamDir(d)),
            None => {}
        }
        let cluster = &self.core.cluster;
        if let Some(loc) = self.core.locs.get(rel) {
            if cluster.is_readable(&loc.dest) {
                return Ok(Res::Dest(loc));
            }
        }
        if self.core.locs.is_negative(rel) {
            return Err(Errno::ENOENT);
        }
        let dests = cluster.readable();
        if dests.is_empty() {
            return Err(Errno::ENOENT);
        }
        let results = self.rt.block_on(futures::future::join_all(dests.iter().map(|d| {
            let rel = rel.to_string();
            async move { tokio::time::timeout(STAT_TIMEOUT, cluster.stat(d, &rel)).await }
        })));
        let mut incomplete = false;
        for (d, r) in dests.iter().zip(results) {
            match r {
                Ok(Ok(Some(entry))) => {
                    let id = d.id();
                    self.core.locs.put(rel, &id, entry.clone());
                    return Ok(Res::Dest(Loc { dest: id, entry }));
                }
                Ok(Ok(None)) => {}
                _ => incomplete = true,
            }
        }
        if !incomplete {
            self.core.locs.put_negative(rel);
        }
        Err(Errno::ENOENT)
    }

    /// `resolve` for `open`: a cached destination location older than
    /// REVALIDATE is confirmed with one stat on that destination, so a file
    /// deleted or moved behind spillway's back is not opened from the cache.
    fn resolve_fresh(&self, rel: &str) -> Result<Res, Errno> {
        if self.core.ram.lookup(rel).is_none() {
            if let Some((loc, age)) = self.core.locs.get_aged(rel) {
                if age > REVALIDATE {
                    if let Some(d) = self.core.cluster.dest(&loc.dest) {
                        // the timer must be created inside the runtime: wrap it in an async block
                        match self.rt.block_on(async { tokio::time::timeout(STAT_TIMEOUT, self.core.cluster.stat(&d, rel)).await }) {
                            Ok(Ok(Some(entry))) => self.core.locs.put(rel, &loc.dest, entry),
                            Ok(Ok(None)) => self.core.locs.invalidate(rel),
                            _ => {} // unreachable now: keep the cached answer, reads will tell
                        }
                    }
                }
            }
        }
        self.resolve(rel)
    }

    /// Merged directory listing: RAM entries win, then destinations in order.
    fn list_union(&self, rel: &str) -> Vec<(String, FileType)> {
        let mut out: BTreeMap<String, FileType> = BTreeMap::new();
        for (name, is_dir) in self.core.ram.children(rel) {
            out.insert(name, if is_dir { FileType::Directory } else { FileType::RegularFile });
        }
        let listing = match self.core.locs.listing(rel) {
            Some(l) => l,
            None => {
                let cluster = &self.core.cluster;
                let dests = cluster.readable();
                let results = self.rt.block_on(futures::future::join_all(dests.iter().map(|d| {
                    let rel = rel.to_string();
                    async move { tokio::time::timeout(LIST_TIMEOUT, cluster.list(d, &rel)).await }
                })));
                let mut merged: BTreeMap<String, (String, Entry)> = BTreeMap::new();
                let mut complete = true;
                for (d, r) in dests.iter().zip(results) {
                    match r {
                        Ok(Ok(Some(entries))) => {
                            let id = d.id();
                            for en in entries {
                                merged.entry(en.name.clone()).or_insert_with(|| (id.clone(), en));
                            }
                        }
                        Ok(Ok(None)) => {}
                        _ => complete = false,
                    }
                }
                let l: Vec<(String, Entry)> = merged.into_values().collect();
                for (dest, en) in &l {
                    self.core.locs.put(&join(rel, &en.name), dest, en.clone());
                }
                if complete {
                    self.core.locs.put_listing(rel, l.clone());
                }
                l
            }
        };
        for (_, en) in listing {
            out.entry(en.name.clone()).or_insert(if en.kind == Kind::Dir { FileType::Directory } else { FileType::RegularFile });
        }
        if rel.is_empty() {
            out.remove(crate::proto::META_DIR);
        }
        out.into_iter().collect()
    }

    /// Ensures `dir` exists in RAM (it may exist only on destinations).
    fn ensure_ram_dir(&self, dir: &str, req: &Request) -> Result<(), Errno> {
        if dir.is_empty() || self.core.ram.is_dir(dir) {
            return Ok(());
        }
        match self.resolve(dir)? {
            r if r.is_dir() => {
                let (mode, uid, gid) = match &r {
                    Res::Dest(l) => (l.entry.mode, l.entry.uid, l.entry.gid),
                    _ => (0o755, req.uid(), req.gid()),
                };
                // parents too
                self.ensure_ram_dir(parent_of(dir), req)?;
                self.core.ram.ensure_dir_all(dir, mode, uid, gid).map_err(e)
            }
            _ => Err(Errno::ENOTDIR),
        }
    }

    fn hard_limit(&self) -> u64 {
        self.core.cfg.read().ram.hard_limit_bytes()
    }

    /// Brings a destination file back into RAM so it can be modified.
    /// With `empty` (O_TRUNC) nothing is copied.
    fn copy_up(&self, rel: &str, loc: &Loc, empty: bool, req: &Request) -> Result<(), Errno> {
        let _g = self.copy_up_lock.lock();
        if self.core.ram.exists_file(rel) {
            return Ok(());
        }
        let size = if empty { 0 } else { loc.entry.size };
        if self.core.ram.used() + size > self.hard_limit() {
            return Err(Errno::ENOSPC);
        }
        self.ensure_ram_dir(parent_of(rel), req)?;
        let dest = self.core.cluster.dest(&loc.dest).ok_or(Errno::EIO)?;
        let path = self.core.ram.spool_path(rel);
        let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).map_err(|er| e(er.raw_os_error().unwrap_or(libc::EIO)))?;
        let mut off = 0u64;
        while off < size {
            let chunk = self
                .rt
                .block_on(self.core.cluster.read(&dest, rel, off, 8 * 1024 * 1024))
                .map_err(|er| e(er.errno()));
            let chunk = match chunk {
                Ok(c) if !c.is_empty() => c,
                Ok(_) => break,
                Err(er) => {
                    let _ = std::fs::remove_file(&path);
                    return Err(er);
                }
            };
            if let Err(er) = file.write_all_at(&chunk, off) {
                let _ = std::fs::remove_file(&path);
                return Err(e(er.raw_os_error().unwrap_or(libc::EIO)));
            }
            off += chunk.len() as u64;
        }
        let mtime = ms_to_time(loc.entry.mtime_ms);
        let _ = file.set_modified(mtime);
        self.core.ram.adopt(rel, file, off, mtime, loc.entry.mode, loc.entry.uid, loc.entry.gid, loc.dest.clone());
        self.core.locs.invalidate(rel);
        tracing::info!(file = rel, from = %loc.dest, bytes = off, "copied up for modification");
        Ok(())
    }

    fn ext_allowed(&self, name: &str) -> bool {
        let cfg = self.core.cfg.read();
        let allowed = &cfg.mount.allowed_extensions;
        if allowed.is_empty() {
            return true;
        }
        match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => allowed.iter().any(|a| a.eq_ignore_ascii_case(ext)),
            _ => false,
        }
    }

    fn update_handles_rel(&self, from: &str, to: &str) {
        for h in self.handles.lock().values() {
            if let Handle::Ram { rel, .. } = &**h {
                let mut r = rel.lock();
                if *r == from {
                    *r = to.to_string();
                }
            }
        }
    }

    fn open_ram(&self, rel: &str, write: bool, trunc: bool) -> Result<FileHandle, Errno> {
        let (file, _) = self.core.ram.open(rel, write, trunc).map_err(e)?;
        Ok(self.add_handle(Handle::Ram { rel: Mutex::new(rel.to_string()), file, write }))
    }
}

/// Filesystem operations that panicked (and answered EIO) since start.
pub static FS_PANICS: AtomicU64 = AtomicU64::new(0);

/// Test-only fault injection: a lookup of this exact name panics. Set through
/// SPILLWAY_FAULT_PANIC_NAME (the e2e suite proves the mount survives it).
fn fault_name() -> Option<&'static str> {
    static NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| std::env::var("SPILLWAY_FAULT_PANIC_NAME").ok().filter(|s| !s.is_empty())).as_deref()
}

/// Runs one filesystem operation. A panic in a FUSE thread would end the
/// whole session (the mount vanishes and every writer fails), so it is caught
/// here: the operation answers EIO (fuser replies EIO for an unsent reply) and
/// the mount keeps serving.
fn guard(op: &'static str, f: impl FnOnce()) {
    if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        FS_PANICS.fetch_add(1, Ordering::Relaxed);
        let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
        tracing::error!(op, panic = %msg, "filesystem operation panicked; answered EIO");
    }
}

impl Filesystem for SpillFs {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> std::io::Result<()> {
        let _ = config.set_max_write(1024 * 1024);
        let _ = config.set_max_readahead(1024 * 1024);
        let _ = config.set_max_background(64);
        let mut caps = InitFlags::FUSE_PARALLEL_DIROPS | InitFlags::FUSE_ASYNC_READ | InitFlags::FUSE_BIG_WRITES;
        if self.writeback {
            caps |= InitFlags::FUSE_WRITEBACK_CACHE;
        }
        if let Err(missing) = config.add_capabilities(caps) {
            tracing::warn!(?missing, "kernel lacks some FUSE capabilities");
            let _ = config.add_capabilities(caps - missing);
        }
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        guard("lookup", || {
            if fault_name().is_some_and(|f| name == f) {
                panic!("injected fault (SPILLWAY_FAULT_PANIC_NAME)");
            }
            let rel = match self.child(parent, name) {
                Ok(r) => r,
                Err(er) => return reply.error(er),
            };
            match self.resolve(&rel) {
                Ok(res) => {
                    let ino = self.inodes.lock().lookup(&rel);
                    reply.entry(&TTL, &res.attr(ino), Generation(0));
                }
                Err(er) => reply.error(er),
            }
        })
    }

    fn forget(&self, _req: &Request, ino: INodeNo, nlookup: u64) {
        guard("forget", || {
            self.inodes.lock().forget(ino.0, nlookup);
        })
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        guard("getattr", || {
            let rel = match self.path(ino) {
                Ok(r) => r,
                Err(er) => return reply.error(er),
            };
            match self.resolve(&rel) {
                Ok(res) => reply.attr(&TTL, &res.attr(ino.0)),
                Err(er) => reply.error(er),
            }
        })
    }

    fn setattr(
        &self,
        req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        guard("setattr", || {
            let rel = match self.path(ino) {
                Ok(r) => r,
                Err(er) => return reply.error(er),
            };
            let mtime = mtime.map(|t| match t {
                TimeOrNow::SpecificTime(t) => t,
                TimeOrNow::Now => SystemTime::now(),
            });
            let mut res = match self.resolve(&rel) {
                Ok(r) => r,
                Err(er) => return reply.error(er),
            };
            if let Res::Dest(loc) = &res {
                if loc.entry.kind == Kind::File && (size.is_some() || mode.is_some() || uid.is_some() || gid.is_some()) {
                    // modifying a file that already moved: bring it back to RAM
                    if let Err(er) = self.copy_up(&rel, &loc.clone(), size == Some(0), req) {
                        return reply.error(er);
                    }
                    res = match self.resolve(&rel) {
                        Ok(r) => r,
                        Err(er) => return reply.error(er),
                    };
                }
            }
            match res {
                Res::RamFile(_) => {
                    if let Some(sz) = size {
                        if let Err(er) = self.core.ram.truncate(&rel, sz, self.hard_limit()) {
                            return reply.error(e(er));
                        }
                    }
                    match self.core.ram.set_attrs(&rel, mode, uid, gid, mtime) {
                        Ok(f) => reply.attr(&TTL, &Res::RamFile(f).attr(ino.0)),
                        Err(er) => reply.error(e(er)),
                    }
                }
                Res::RamDir(_) => match self.core.ram.set_dir_attrs(&rel, mode, uid, gid) {
                    Ok(d) => reply.attr(&TTL, &Res::RamDir(d).attr(ino.0)),
                    Err(er) => reply.error(e(er)),
                },
                // timestamps on files/dirs living on destinations are accepted as no-ops
                r @ Res::Dest(_) => {
                    if mode.is_some() || uid.is_some() || gid.is_some() {
                        reply.error(Errno::EROFS)
                    } else {
                        reply.attr(&TTL, &r.attr(ino.0))
                    }
                }
            }
        })
    }

    fn mkdir(&self, req: &Request, parent: INodeNo, name: &OsStr, mode: u32, umask: u32, reply: ReplyEntry) {
        guard("mkdir", || {
            let rel = match self.child(parent, name) {
                Ok(r) => r,
                Err(er) => return reply.error(er),
            };
            if self.resolve(&rel).is_ok() {
                return reply.error(Errno::EEXIST);
            }
            if let Err(er) = self.ensure_ram_dir(parent_of(&rel), req) {
                return reply.error(er);
            }
            match self.core.ram.mkdir(&rel, mode & !umask, req.uid(), req.gid()) {
                Ok(d) => {
                    self.core.locs.invalidate(&rel);
                    let ino = self.inodes.lock().lookup(&rel);
                    reply.entry(&TTL, &Res::RamDir(d).attr(ino), Generation(0));
                }
                Err(er) => reply.error(e(er)),
            }
        })
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        guard("unlink", || {
            let rel = match self.child(parent, name) {
                Ok(r) => r,
                Err(er) => return reply.error(er),
            };
            match self.resolve(&rel) {
                Ok(Res::RamFile(_)) => match self.core.ram.unlink(&rel) {
                    Ok(f) => {
                        self.core.locs.invalidate(&rel);
                        if let Some(origin) = f.origin.and_then(|o| self.core.cluster.dest(&o)) {
                            // the copied-up file also has an older copy on disk
                            let _ = self.rt.block_on(self.core.cluster.delete(&origin, &rel));
                        }
                        reply.ok()
                    }
                    Err(er) => reply.error(e(er)),
                },
                Ok(Res::Dest(loc)) if loc.entry.kind == Kind::File => {
                    if !self.core.cfg.read().mount.allow_delete_on_dest {
                        return reply.error(Errno::EROFS);
                    }
                    let Some(d) = self.core.cluster.dest(&loc.dest) else { return reply.error(Errno::EIO) };
                    match self.rt.block_on(self.core.cluster.delete(&d, &rel)) {
                        Ok(()) => {
                            self.core.locs.invalidate(&rel);
                            reply.ok()
                        }
                        Err(er) => reply.error(e(er.errno())),
                    }
                }
                Ok(_) => reply.error(Errno::EISDIR),
                Err(er) => reply.error(er),
            }
        })
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        guard("rmdir", || {
            let rel = match self.child(parent, name) {
                Ok(r) => r,
                Err(er) => return reply.error(er),
            };
            match self.resolve(&rel) {
                Ok(r) if !r.is_dir() => return reply.error(Errno::ENOTDIR),
                Err(er) => return reply.error(er),
                _ => {}
            }
            // the directory must be empty everywhere, and must not exist on a destination
            if !self.list_union(&rel).is_empty() {
                return reply.error(Errno::ENOTEMPTY);
            }
            self.core.locs.invalidate(&rel);
            let on_dest = {
                let cluster = &self.core.cluster;
                let dests = cluster.readable();
                self.rt
                    .block_on(futures::future::join_all(dests.iter().map(|d| cluster.stat(d, &rel))))
                    .into_iter()
                    .any(|r| matches!(r, Ok(Some(_))))
            };
            if on_dest {
                return reply.error(Errno::EROFS);
            }
            match self.core.ram.rmdir(&rel) {
                Ok(()) => reply.ok(),
                Err(er) => reply.error(e(er)),
            }
        })
    }

    fn rename(&self, req: &Request, parent: INodeNo, name: &OsStr, newparent: INodeNo, newname: &OsStr, flags: RenameFlags, reply: ReplyEmpty) {
        guard("rename", || {
            if !flags.is_empty() {
                return reply.error(Errno::EINVAL);
            }
            let (from, to) = match (self.child(parent, name), self.child(newparent, newname)) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(er), _) | (_, Err(er)) => return reply.error(er),
            };
            match self.resolve(&from) {
                Ok(Res::RamFile(_)) => {}
                Ok(Res::Dest(l)) if l.entry.kind == Kind::File => return reply.error(Errno::EROFS),
                Ok(_) => return reply.error(Errno::EXDEV),
                Err(er) => return reply.error(er),
            }
            match self.resolve(&to) {
                Ok(Res::Dest(_)) => return reply.error(Errno::EROFS),
                Ok(r) if r.is_dir() => return reply.error(Errno::EISDIR),
                _ => {}
            }
            if let Err(er) = self.ensure_ram_dir(parent_of(&to), req) {
                return reply.error(er);
            }
            match self.core.ram.rename(&from, &to) {
                Ok(()) => {
                    self.inodes.lock().rename(&from, &to);
                    self.update_handles_rel(&from, &to);
                    self.core.locs.invalidate(&from);
                    self.core.locs.invalidate(&to);
                    reply.ok()
                }
                Err(er) => reply.error(e(er)),
            }
        })
    }

    fn open(&self, req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        guard("open", || {
            let rel = match self.path(ino) {
                Ok(r) => r,
                Err(er) => return reply.error(er),
            };
            let acc = flags.0 & libc::O_ACCMODE;
            let write = acc != libc::O_RDONLY;
            let trunc = flags.0 & libc::O_TRUNC != 0;
            match self.resolve_fresh(&rel) {
                Ok(Res::RamFile(_)) => match self.open_ram(&rel, write, trunc) {
                    Ok(fh) => reply.opened(fh, FopenFlags::empty()),
                    Err(er) => reply.error(er),
                },
                Ok(Res::Dest(loc)) if loc.entry.kind == Kind::File => {
                    if write {
                        if let Err(er) = self.copy_up(&rel, &loc, trunc, req) {
                            return reply.error(er);
                        }
                        return match self.open_ram(&rel, true, trunc) {
                            Ok(fh) => reply.opened(fh, FopenFlags::empty()),
                            Err(er) => reply.error(er),
                        };
                    }
                    let fh = self.add_handle(Handle::Dest { dest: loc.dest, rel, size: loc.entry.size, cache: Mutex::new(None) });
                    // files on destinations are immutable: let the kernel keep their pages
                    reply.opened(fh, FopenFlags::FOPEN_KEEP_CACHE);
                }
                Ok(_) => reply.error(Errno::EISDIR),
                Err(er) => reply.error(er),
            }
        })
    }

    fn create(&self, req: &Request, parent: INodeNo, name: &OsStr, mode: u32, umask: u32, flags: i32, reply: ReplyCreate) {
        guard("create", || {
            let rel = match self.child(parent, name) {
                Ok(r) => r,
                Err(er) => return reply.error(er),
            };
            let fname = rel.rsplit('/').next().unwrap_or("");
            if !self.ext_allowed(fname) {
                Counters::add(&self.core.stats.c.write_rejected, 1);
                return reply.error(Errno::EACCES);
            }
            let trunc = flags & libc::O_TRUNC != 0;
            match self.resolve(&rel) {
                Ok(Res::RamFile(_)) if flags & libc::O_EXCL != 0 => return reply.error(Errno::EEXIST),
                Ok(Res::Dest(loc)) if loc.entry.kind == Kind::File => {
                    if flags & libc::O_EXCL != 0 {
                        return reply.error(Errno::EEXIST);
                    }
                    if let Err(er) = self.copy_up(&rel, &loc, trunc, req) {
                        return reply.error(er);
                    }
                }
                Ok(r) if r.is_dir() => return reply.error(Errno::EISDIR),
                _ => {}
            }
            if self.core.ram.used() >= self.hard_limit() {
                Counters::add(&self.core.stats.c.write_rejected, 1);
                return reply.error(Errno::ENOSPC);
            }
            if let Err(er) = self.ensure_ram_dir(parent_of(&rel), req) {
                tracing::warn!(file = %rel, ?er, "create: parent directory unavailable");
                return reply.error(er);
            }
            let existed = self.core.ram.exists_file(&rel);
            let res = if existed { self.core.ram.open(&rel, true, trunc) } else { self.core.ram.create(&rel, mode & !umask, req.uid(), req.gid()) };
            match res {
                Ok((file, f)) => {
                    if !existed {
                        Counters::add(&self.core.stats.c.files_created, 1);
                    }
                    self.core.locs.invalidate(&rel);
                    let ino = self.inodes.lock().lookup(&rel);
                    let fh = self.add_handle(Handle::Ram { rel: Mutex::new(rel), file, write: true });
                    reply.created(&TTL, &Res::RamFile(f).attr(ino), Generation(0), fh, FopenFlags::empty());
                }
                Err(er) => {
                    tracing::warn!(file = %rel, errno = er, existed, "create failed in RAM");
                    reply.error(e(er))
                }
            }
        })
    }

    fn read(&self, _req: &Request, _ino: INodeNo, fh: FileHandle, offset: u64, size: u32, _flags: OpenFlags, _lock: Option<LockOwner>, reply: ReplyData) {
        guard("read", || {
            let h = match self.handle(fh) {
                Ok(h) => h,
                Err(er) => return reply.error(er),
            };
            match &*h {
                Handle::Ram { file, .. } => {
                    let mut buf = vec![0u8; size as usize];
                    let mut got = 0usize;
                    while got < buf.len() {
                        match file.read_at(&mut buf[got..], offset + got as u64) {
                            Ok(0) => break,
                            Ok(n) => got += n,
                            Err(er) if er.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(er) => return reply.error(e(er.raw_os_error().unwrap_or(libc::EIO))),
                        }
                    }
                    Counters::add(&self.core.stats.c.read_ram_bytes, got as u64);
                    reply.data(&buf[..got]);
                }
                Handle::Dest { dest, rel, size: fsize, cache } => {
                    if offset >= *fsize {
                        return reply.data(&[]);
                    }
                    let want_end = (offset + size as u64).min(*fsize);
                    {
                        let c = cache.lock();
                        if let Some((at, data)) = &*c {
                            if offset >= *at && want_end <= *at + data.len() as u64 {
                                let s = (offset - at) as usize;
                                let n = (want_end - offset) as usize;
                                Counters::add(&self.core.stats.c.read_dest_bytes, n as u64);
                                return reply.data(&data[s..s + n]);
                            }
                        }
                    }
                    let Some(d) = self.core.cluster.dest(dest) else { return reply.error(Errno::EIO) };
                    let len = size.max(READAHEAD);
                    let mut res = self.rt.block_on(self.core.cluster.read(&d, rel, offset, len));
                    if matches!(res, Err(NodeError::NotFound)) {
                        // gone from that destination behind our back: maybe it lives elsewhere now
                        self.core.locs.invalidate(rel);
                        if let Ok(Res::Dest(loc)) = self.resolve(rel) {
                            if let Some(d2) = self.core.cluster.dest(&loc.dest).filter(|_| loc.dest != *dest) {
                                res = self.rt.block_on(self.core.cluster.read(&d2, rel, offset, len));
                            }
                        }
                    }
                    match res {
                        Ok(data) => {
                            let n = ((want_end - offset) as usize).min(data.len());
                            Counters::add(&self.core.stats.c.read_dest_bytes, n as u64);
                            reply.data(&data[..n]);
                            *cache.lock() = Some((offset, data));
                        }
                        Err(er) => reply.error(e(er.errno())),
                    }
                }
                Handle::Dir { .. } => reply.error(Errno::EISDIR),
            }
        })
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _wf: WriteFlags,
        _flags: OpenFlags,
        _lock: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        guard("write", || {
            let h = match self.handle(fh) {
                Ok(h) => h,
                Err(er) => return reply.error(er),
            };
            match &*h {
                Handle::Ram { rel, file, .. } => {
                    let rel = rel.lock().clone();
                    match self.core.ram.write(&rel, file, offset, data, self.hard_limit()) {
                        Ok(n) => {
                            Counters::add(&self.core.stats.c.ingest_bytes, n as u64);
                            reply.written(n as u32)
                        }
                        Err(er) => {
                            if er == libc::ENOSPC {
                                Counters::add(&self.core.stats.c.write_rejected, 1);
                            }
                            reply.error(e(er))
                        }
                    }
                }
                _ => reply.error(Errno::EBADF),
            }
        })
    }

    fn flush(&self, _req: &Request, _ino: INodeNo, _fh: FileHandle, _lock: LockOwner, reply: ReplyEmpty) {
        guard("flush", || {
            reply.ok();
        })
    }

    fn fsync(&self, _req: &Request, _ino: INodeNo, _fh: FileHandle, _datasync: bool, reply: ReplyEmpty) {
        guard("fsync", || {
            reply.ok();
        })
    }

    fn release(&self, _req: &Request, _ino: INodeNo, fh: FileHandle, _flags: OpenFlags, _lock: Option<LockOwner>, _flush: bool, reply: ReplyEmpty) {
        guard("release", || {
            let h = self.handles.lock().remove(&fh.0);
            if let Some(h) = h {
                if let Handle::Ram { rel, write: true, .. } = &*h {
                    self.core.ram.release_writer(&rel.lock());
                }
            }
            reply.ok();
        })
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        guard("opendir", || {
            let rel = match self.path(ino) {
                Ok(r) => r,
                Err(er) => return reply.error(er),
            };
            match self.resolve(&rel) {
                Ok(r) if r.is_dir() => {}
                Ok(_) => return reply.error(Errno::ENOTDIR),
                Err(er) => return reply.error(er),
            }
            let parent_ino = if rel.is_empty() { 1 } else { self.inodes.lock().peek(parent_of(&rel)).unwrap_or(1) };
            let mut entries = vec![(ino.0, FileType::Directory, ".".to_string()), (parent_ino, FileType::Directory, "..".to_string())];
            let inodes = self.inodes.lock();
            let listing = {
                drop(inodes);
                self.list_union(&rel)
            };
            let inodes = self.inodes.lock();
            for (name, kind) in listing {
                let child = join(&rel, &name);
                // readdir inode numbers are informational; avoid allocating table entries for them
                let i = inodes.peek(&child).unwrap_or_else(|| synthetic_ino(&child));
                entries.push((i, kind, name));
            }
            drop(inodes);
            let fh = self.add_handle(Handle::Dir { entries });
            reply.opened(fh, FopenFlags::empty());
        })
    }

    fn readdir(&self, _req: &Request, _ino: INodeNo, fh: FileHandle, offset: u64, mut reply: ReplyDirectory) {
        guard("readdir", || {
            let h = match self.handle(fh) {
                Ok(h) => h,
                Err(er) => return reply.error(er),
            };
            let Handle::Dir { entries } = &*h else { return reply.error(Errno::ENOTDIR) };
            for (i, (ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
                if reply.add(INodeNo(*ino), (i + 1) as u64, *kind, name) {
                    break;
                }
            }
            reply.ok();
        })
    }

    fn releasedir(&self, _req: &Request, _ino: INodeNo, fh: FileHandle, _flags: OpenFlags, reply: ReplyEmpty) {
        guard("releasedir", || {
            self.handles.lock().remove(&fh.0);
            reply.ok();
        })
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        guard("statfs", || {
            let mut total = 0u64;
            let mut free = 0u64;
            for v in self.core.cluster.views() {
                if v.status.readable() {
                    total += v.total_bytes;
                    if v.status.writable() {
                        free += v.free_bytes;
                    }
                }
            }
            let hard = self.hard_limit();
            total += hard;
            free = free.min(self.core.ram.room(hard).saturating_add(free));
            let bs = 4096u64;
            reply.statfs(total / bs, free / bs, free / bs, 0, 0, bs as u32, 255, bs as u32);
        })
    }

    fn access(&self, _req: &Request, _ino: INodeNo, _mask: fuser::AccessFlags, reply: ReplyEmpty) {
        guard("access", || {
            reply.ok();
        })
    }

    fn fallocate(&self, _req: &Request, _ino: INodeNo, fh: FileHandle, offset: u64, length: u64, mode: i32, reply: ReplyEmpty) {
        guard("fallocate", || {
            let h = match self.handle(fh) {
                Ok(h) => h,
                Err(er) => return reply.error(er),
            };
            let Handle::Ram { rel, file, .. } = &*h else { return reply.error(Errno::EBADF) };
            let rel = rel.lock().clone();
            match self.core.ram.fallocate(&rel, file, offset, length, mode, self.hard_limit()) {
                Ok(()) => reply.ok(),
                Err(er) => reply.error(e(er)),
            }
        })
    }

    // Recordings carry no extended attributes; answer quickly instead of ENOSYS noise.
    fn getxattr(&self, _req: &Request, _ino: INodeNo, _name: &OsStr, _size: u32, reply: fuser::ReplyXattr) {
        guard("getxattr", || {
            reply.error(e(libc::ENODATA));
        })
    }

    fn listxattr(&self, _req: &Request, _ino: INodeNo, size: u32, reply: fuser::ReplyXattr) {
        guard("listxattr", || {
            if size == 0 {
                reply.size(0)
            } else {
                reply.data(&[])
            }
        })
    }

    fn setxattr(&self, _req: &Request, _ino: INodeNo, _name: &OsStr, _value: &[u8], _flags: i32, _position: u32, reply: ReplyEmpty) {
        guard("setxattr", || {
            reply.error(e(libc::ENOTSUP));
        })
    }

    fn removexattr(&self, _req: &Request, _ino: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        guard("removexattr", || {
            reply.error(e(libc::ENODATA));
        })
    }

    fn ioctl(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _flags: fuser::IoctlFlags,
        _cmd: u32,
        _in_data: &[u8],
        _out_size: u32,
        reply: fuser::ReplyIoctl,
    ) {
        guard("ioctl", || {
            reply.error(e(libc::ENOTTY));
        })
    }
}

fn synthetic_ino(path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish() | (1 << 63)
}

pub fn mount(core: Arc<Core>, rt: Rt, cfg: &MountCfg) -> anyhow::Result<fuser::BackgroundSession> {
    let fs = SpillFs::new(core, rt, cfg.writeback_cache);
    let mut c = fuser::Config::default();
    c.mount_options = vec![
        MountOption::FSName("spillway".into()),
        MountOption::Subtype("spillway".into()),
        MountOption::DefaultPermissions,
        MountOption::NoAtime,
        MountOption::NoDev,
        MountOption::NoSuid,
    ];
    c.acl = if cfg.allow_other { fuser::SessionACL::All } else { fuser::SessionACL::Owner };
    c.n_threads = Some(cfg.threads.max(1));
    c.clone_fd = true;
    Ok(fuser::spawn_mount(fs, &cfg.path, &c)?)
}
