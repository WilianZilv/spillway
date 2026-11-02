//! The directory under the mount. While spillway is down the mountpoint is a
//! plain local directory, and a recorder that keeps going (MediaMTX does)
//! writes its segments there. Once spillway mounts on top they would be
//! hidden for good. A descriptor opened *before* mounting keeps pointing at
//! that directory (through /proc/self/fd), so those files are found, pulled
//! into RAM and moved like everything else, then deleted from the local disk.

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::master::{fmt_bytes, Core};

/// How often the underlay is swept while mounted (something holding an old
/// handle to the directory may still be writing there).
const SWEEP_EVERY: Duration = Duration::from_secs(10);
/// A file must be this long without changes to be adopted (still being written otherwise).
const QUIET: Duration = Duration::from_secs(10);

/// Opens the mountpoint directory itself. Call before mounting.
pub fn open(mountpoint: &Path) -> std::io::Result<File> {
    File::open(mountpoint)
}

#[derive(Default, Debug, PartialEq)]
pub struct Sweep {
    pub adopted: u64,
    pub bytes: u64,
    /// Same path already in RAM: left on the local disk.
    pub conflicts: u64,
    /// Not adopted now (RAM full, extension not allowed, I/O error); retried later.
    pub skipped: u64,
}

/// One pass over the directory under the mount. Blocking.
pub fn sweep(core: &Core, root: &Path, quiet: Duration) -> Sweep {
    let (hard_limit, exts) = {
        let c = core.cfg.read();
        (c.ram.hard_limit_bytes(), c.mount.allowed_extensions.clone())
    };
    let ext_ok = |name: &str| exts.is_empty() || name.rsplit_once('.').is_some_and(|(_, e)| exts.iter().any(|x| x.eq_ignore_ascii_case(e)));
    let mut out = Sweep::default();
    let mut dirs = Vec::new();
    let mut stack = vec![String::new()];
    let now = SystemTime::now();
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(root.join(&dir)) else { continue };
        for ent in rd.flatten() {
            let Ok(name) = ent.file_name().into_string() else { continue };
            let rel = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
            let Ok(m) = ent.metadata() else { continue };
            if m.is_dir() {
                dirs.push(rel.clone());
                stack.push(rel);
                continue;
            }
            if !m.is_file() {
                continue;
            }
            if m.modified().ok().and_then(|t| now.duration_since(t).ok()).is_none_or(|age| age < quiet) {
                continue; // still being written
            }
            if !ext_ok(&name) || m.len() == 0 {
                out.skipped += 1;
                continue;
            }
            match core.ram.adopt_external(&rel, &ent.path(), &m, hard_limit) {
                Ok(true) => {
                    let _ = fs::remove_file(ent.path());
                    out.adopted += 1;
                    out.bytes += m.len();
                }
                Ok(false) => out.conflicts += 1,
                Err(e) => {
                    out.skipped += 1;
                    tracing::debug!(file = %rel, errno = e, "could not adopt a file from under the mount");
                }
            }
        }
    }
    // emptied directories go too (deepest first); the mountpoint itself stays
    dirs.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
    for d in dirs {
        let _ = fs::remove_dir(root.join(d));
    }
    out
}

/// Sweeps once right away (files left from a downtime), then every SWEEP_EVERY.
pub async fn underlay_loop(core: Arc<Core>, dir: File) {
    let root = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
    let dir = Arc::new(dir); // keeps the descriptor (and the path above) alive
    let mut warned_conflicts = 0;
    loop {
        let (c, r, d) = (core.clone(), root.clone(), dir.clone());
        let res = tokio::task::spawn_blocking(move || {
            let _keep = d;
            sweep(&c, &r, QUIET)
        })
        .await;
        if let Ok(s) = res {
            if s.adopted > 0 {
                core.stats.event(
                    "warn",
                    "underlay_rescued",
                    format!(
                        "{} arquivo(s) ({}) gravados no disco local embaixo do mount (com o spillway fora do ar) foram recuperados para a RAM",
                        s.adopted,
                        fmt_bytes(s.bytes)
                    ),
                );
                core.wake.notify_one();
            }
            if s.conflicts > warned_conflicts {
                core.stats.event("warn", "underlay_conflict", format!("{} arquivo(s) embaixo do mount têm o mesmo caminho de um arquivo na RAM e ficaram no disco local", s.conflicts));
            }
            warned_conflicts = s.conflicts;
        }
        tokio::time::sleep(SWEEP_EVERY).await;
    }
}
