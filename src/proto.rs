//! Types shared between master and nodes (wire format is JSON).

use serde::{Deserialize, Serialize};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Directory used inside each destination for probes and in-progress transfers. Hidden from listings.
pub const META_DIR: &str = ".spillway";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DestAssign {
    pub id: String,
    pub path: String,
    pub expect_mount: bool,
    /// Changes every time the master (re)assigns the destination. The node
    /// echoes it in its reports so the master can tell a report about the
    /// current assignment from one sent before a disable / re-enable.
    #[serde(default)]
    pub gen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeReport {
    pub dest_id: String,
    pub ok: bool,
    pub error: Option<String>,
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub is_mount: Option<bool>,
    pub probe_ms: f64,
    /// Unix ms when the probe finished.
    pub at_ms: i64,
    /// The probe has not returned for a long time (mount probably hung).
    #[serde(default)]
    pub hung: bool,
    /// `DestAssign::gen` the probe ran under (0 from nodes older than this field).
    #[serde(default)]
    pub gen: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    File,
    Dir,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
    pub mtime_ms: i64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub node_id: String,
    /// Base URL where the master can reach this node's data API.
    pub url: String,
    pub hostname: String,
    pub version: String,
    pub started_at_ms: i64,
    pub reports: Vec<ProbeReport>,
    /// Node clock when sent; the master uses it to translate report times to its own clock.
    #[serde(default)]
    pub sent_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResp {
    pub dests: Vec<DestAssign>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeError {
    UnknownDest,
    NotFound,
    BadPath,
    Timeout,
    Busy,
    Io(String),
}

impl NodeError {
    pub fn errno(&self) -> i32 {
        match self {
            NodeError::NotFound => libc::ENOENT,
            NodeError::BadPath => libc::EINVAL,
            NodeError::Busy => libc::EAGAIN,
            _ => libc::EIO,
        }
    }
    pub fn code(&self) -> &'static str {
        match self {
            NodeError::UnknownDest => "unknown_dest",
            NodeError::NotFound => "not_found",
            NodeError::BadPath => "bad_path",
            NodeError::Timeout => "timeout",
            NodeError::Busy => "busy",
            NodeError::Io(_) => "io",
        }
    }
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeError::Io(m) => write!(f, "io: {m}"),
            other => f.write_str(other.code()),
        }
    }
}

impl std::error::Error for NodeError {}

/// Relative paths are `a/b/c` — no leading slash, no `.`/`..`, no empty components, not inside META_DIR.
pub fn valid_rel(rel: &str) -> bool {
    if rel.is_empty() {
        return true;
    }
    if rel.starts_with('/') || rel.ends_with('/') || rel.contains('\0') {
        return false;
    }
    let mut first = true;
    for c in rel.split('/') {
        if c.is_empty() || c == "." || c == ".." || (first && c == META_DIR) {
            return false;
        }
        first = false;
    }
    true
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_paths() {
        assert!(valid_rel(""));
        assert!(valid_rel("a/b.mp4"));
        assert!(valid_rel("a/.spillway"));
        for bad in ["/a", "a/", "a//b", "../a", "a/../b", "./a", ".spillway", ".spillway/tmp/x"] {
            assert!(!valid_rel(bad), "{bad}");
        }
    }
}
