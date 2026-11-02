//! Master configuration (TOML). Editable at runtime from the web panel; every
//! change is persisted atomically back to the same file.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MasterConfig {
    pub mount: MountCfg,
    pub ram: RamCfg,
    pub mover: MoverCfg,
    pub mp4: Mp4Cfg,
    pub web: WebCfg,
    pub cluster: ClusterCfg,
    pub hooks: HooksCfg,
    #[serde(rename = "destination")]
    pub destinations: Vec<DestCfg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MountCfg {
    /// Where the unified filesystem is mounted (what MediaMTX writes to / players read from).
    pub path: PathBuf,
    pub allow_other: bool,
    /// FUSE worker threads.
    pub threads: usize,
    /// Only these extensions can be created (case-insensitive). Empty = anything.
    pub allowed_extensions: Vec<String>,
    /// Allow deleting files that already live on a destination through the mount.
    pub allow_delete_on_dest: bool,
    /// Kernel write-back cache: small writes are merged into pages before reaching us.
    pub writeback_cache: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RamCfg {
    /// Backing directory for files still in RAM. Mounted as tmpfs when `manage_tmpfs`.
    pub spool_dir: PathBuf,
    pub manage_tmpfs: bool,
    /// RAM usage (GiB) at which the thief starts discarding the oldest pending files.
    pub thief_max_gb: f64,
    /// Thief keeps discarding until usage drops to this % of `thief_max_gb`.
    pub thief_resume_pct: f64,
    /// Writes fail with ENOSPC above this % of `thief_max_gb`.
    pub hard_limit_pct: f64,
    /// A closed file must stay untouched this long before it is sealed and queued.
    pub settle_secs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MoverCfg {
    /// Consecutive transfer failures before a destination is suspended until its next good probe.
    pub fail_streak: u32,
    /// Minimum expected throughput, used to derive per-file transfer timeouts.
    pub min_rate_mbps: f64,
    pub fsync: bool,
    /// How long a file avoids a destination after failing on it.
    pub avoid_secs: f64,
    /// Seconds to keep flushing RAM to destinations on shutdown.
    pub shutdown_flush_secs: f64,
    /// Above this % of the thief ceiling the balancer stops waiting for each
    /// destination's weighted turn and uses every free transfer slot.
    pub pressure_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Mp4Cfg {
    /// Discard files whose MP4 structure is invalid instead of moving them.
    pub drop_invalid: bool,
}

/// External hooks: how an application plugs its own rules into spillway
/// without spillway knowing anything about it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HooksCfg {
    pub validate: ValidateHookCfg,
    pub moved: MovedHookCfg,
}

/// Called (in batches) before a sealed file is moved: answers move / drop / hold.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ValidateHookCfg {
    /// Empty = disabled (every sealed file is moved).
    pub url: String,
    pub token: String,
    /// A batch is sent when it reaches this many files...
    pub batch_max: usize,
    /// ...or when its oldest file has waited this long.
    pub batch_wait_ms: u64,
    /// Batches in flight at the same time.
    pub inflight: usize,
    pub timeout_ms: u64,
    /// When the hook fails or omits a file: "move" (fail-open) or "hold" (keep in RAM, ask again).
    pub on_error: String,
    /// Retry delay for "hold" on error.
    pub hold_retry_ms: u64,
}

/// Called after files land on a destination. Delivery is at-least-once through
/// an on-disk outbox, so the receiver must be idempotent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MovedHookCfg {
    /// Empty = disabled (no events recorded).
    pub url: String,
    pub token: String,
    pub batch_max: usize,
    pub batch_wait_ms: u64,
    pub timeout_ms: u64,
    /// Durable queue of undelivered events (must not be on tmpfs). File-only setting.
    pub outbox_dir: PathBuf,
}

impl Default for ValidateHookCfg {
    fn default() -> Self {
        ValidateHookCfg {
            url: String::new(),
            token: String::new(),
            batch_max: 256,
            batch_wait_ms: 50,
            inflight: 4,
            timeout_ms: 2000,
            on_error: "move".into(),
            hold_retry_ms: 5000,
        }
    }
}

impl Default for MovedHookCfg {
    fn default() -> Self {
        MovedHookCfg {
            url: String::new(),
            token: String::new(),
            batch_max: 500,
            batch_wait_ms: 1000,
            timeout_ms: 5000,
            outbox_dir: "/var/lib/spillway/outbox".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebCfg {
    pub listen: String,
    /// Optional bearer token for the panel/API. Empty = open.
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClusterCfg {
    /// Shared secret nodes use to join this master.
    pub token: String,
    /// Id of the node embedded in the master process (local destinations).
    pub local_node_id: String,
    /// A node is offline after this many seconds without a heartbeat.
    pub node_timeout_secs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DestCfg {
    pub id: String,
    pub node: String,
    pub path: String,
    /// Multiplier on top of the free-space weight.
    pub weight: f64,
    /// Above this usage the destination stops receiving files.
    pub max_use_pct: f64,
    /// Parallel transfers into this destination.
    pub max_inflight: u32,
    /// Disabled destinations neither receive nor serve files.
    pub enabled: bool,
    /// Draining destinations keep serving reads but receive nothing new.
    pub draining: bool,
    /// Refuse to write unless `path` is a mountpoint (protects the root disk when an NFS mount is missing).
    pub expect_mount: bool,
}

impl Default for MasterConfig {
    fn default() -> Self {
        MasterConfig {
            mount: MountCfg::default(),
            ram: RamCfg::default(),
            mover: MoverCfg::default(),
            mp4: Mp4Cfg::default(),
            web: WebCfg::default(),
            cluster: ClusterCfg::default(),
            hooks: HooksCfg::default(),
            destinations: Vec::new(),
        }
    }
}

impl Default for MountCfg {
    fn default() -> Self {
        MountCfg {
            path: "/videos".into(),
            allow_other: true,
            threads: 16,
            allowed_extensions: vec!["mp4".into(), "m4v".into(), "mov".into()],
            allow_delete_on_dest: false,
            writeback_cache: true,
        }
    }
}

impl Default for RamCfg {
    fn default() -> Self {
        RamCfg {
            spool_dir: "/run/spillway/ram".into(),
            manage_tmpfs: true,
            thief_max_gb: 100.0,
            thief_resume_pct: 80.0,
            hard_limit_pct: 110.0,
            settle_secs: 3.0,
        }
    }
}

impl Default for MoverCfg {
    fn default() -> Self {
        MoverCfg { fail_streak: 3, min_rate_mbps: 2.0, fsync: false, avoid_secs: 30.0, shutdown_flush_secs: 120.0, pressure_pct: 50.0 }
    }
}

impl Default for Mp4Cfg {
    fn default() -> Self {
        Mp4Cfg { drop_invalid: false }
    }
}

impl Default for WebCfg {
    fn default() -> Self {
        WebCfg { listen: "0.0.0.0:7070".into(), token: String::new() }
    }
}

impl Default for ClusterCfg {
    fn default() -> Self {
        ClusterCfg { token: "change-me".into(), local_node_id: "local".into(), node_timeout_secs: 6.0 }
    }
}

impl Default for DestCfg {
    fn default() -> Self {
        DestCfg {
            id: String::new(),
            node: "local".into(),
            path: String::new(),
            weight: 1.0,
            max_use_pct: 95.0,
            max_inflight: 4,
            enabled: true,
            draining: false,
            expect_mount: false,
        }
    }
}

impl RamCfg {
    pub fn thief_max_bytes(&self) -> u64 {
        (self.thief_max_gb * GIB) as u64
    }
    pub fn thief_resume_bytes(&self) -> u64 {
        (self.thief_max_gb * GIB * self.thief_resume_pct / 100.0) as u64
    }
    pub fn hard_limit_bytes(&self) -> u64 {
        (self.thief_max_gb * GIB * self.hard_limit_pct / 100.0) as u64
    }
    /// tmpfs size: hard limit plus headroom for metadata.
    pub fn tmpfs_bytes(&self) -> u64 {
        self.hard_limit_bytes() + 256 * 1024 * 1024
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

impl MasterConfig {
    pub fn validate(&self) -> Result<()> {
        let r = &self.ram;
        if !(r.thief_max_gb > 0.0) {
            bail!("ram.thief_max_gb must be > 0");
        }
        if !(10.0..=99.0).contains(&r.thief_resume_pct) {
            bail!("ram.thief_resume_pct must be between 10 and 99");
        }
        if !(100.0..=200.0).contains(&r.hard_limit_pct) {
            bail!("ram.hard_limit_pct must be between 100 and 200");
        }
        if !(0.2..=600.0).contains(&r.settle_secs) {
            bail!("ram.settle_secs must be between 0.2 and 600");
        }
        if !(0.0..=100.0).contains(&self.mover.pressure_pct) {
            bail!("mover.pressure_pct must be between 0 and 100");
        }
        let url_ok = |u: &str| u.is_empty() || u.starts_with("http://") || u.starts_with("https://");
        let v = &self.hooks.validate;
        if !url_ok(&v.url) || !url_ok(&self.hooks.moved.url) {
            bail!("hook urls must start with http:// or https://");
        }
        if !(1..=10_000).contains(&v.batch_max) || !(1..=10_000).contains(&self.hooks.moved.batch_max) {
            bail!("hook batch_max must be between 1 and 10000");
        }
        if !(1..=64).contains(&v.inflight) {
            bail!("hooks.validate.inflight must be between 1 and 64");
        }
        if v.on_error != "move" && v.on_error != "hold" {
            bail!("hooks.validate.on_error must be \"move\" or \"hold\"");
        }
        if v.timeout_ms < 50 || self.hooks.moved.timeout_ms < 50 {
            bail!("hook timeouts must be at least 50 ms");
        }
        if self.mover.fail_streak == 0 {
            bail!("mover.fail_streak must be >= 1");
        }
        if !valid_id(&self.cluster.local_node_id) {
            bail!("cluster.local_node_id is not a valid id");
        }
        let mut seen = std::collections::HashSet::new();
        for d in &self.destinations {
            d.validate()?;
            if !seen.insert(d.id.as_str()) {
                bail!("duplicate destination id '{}'", d.id);
            }
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            let cfg = MasterConfig::default();
            save_atomic(path, &cfg)?;
            tracing::info!(path = %path.display(), "wrote default config");
            return Ok(cfg);
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: MasterConfig = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }
}

impl DestCfg {
    pub fn validate(&self) -> Result<()> {
        if !valid_id(&self.id) {
            bail!("destination id '{}' must be 1-64 chars of [A-Za-z0-9._-]", self.id);
        }
        if !valid_id(&self.node) {
            bail!("destination '{}': invalid node id '{}'", self.id, self.node);
        }
        if !self.path.starts_with('/') || self.path.contains("/../") || self.path.ends_with("/..") {
            bail!("destination '{}': path must be absolute", self.id);
        }
        if !(self.weight > 0.0 && self.weight <= 1000.0) {
            bail!("destination '{}': weight must be in (0, 1000]", self.id);
        }
        if !(1.0..=100.0).contains(&self.max_use_pct) {
            bail!("destination '{}': max_use_pct must be in [1, 100]", self.id);
        }
        if !(1..=64).contains(&self.max_inflight) {
            bail!("destination '{}': max_inflight must be in [1, 64]", self.id);
        }
        Ok(())
    }
}

pub fn save_atomic(path: &Path, cfg: &MasterConfig) -> Result<()> {
    let text = toml::to_string_pretty(cfg)?;
    let header = "# spillway master configuration — edited by the web panel as well; comments are not preserved.\n\n";
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, format!("{header}{text}"))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_validate() {
        let mut c = MasterConfig::default();
        c.destinations.push(DestCfg { id: "storage".into(), path: "/storage".into(), ..Default::default() });
        c.validate().unwrap();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: MasterConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.destinations, c.destinations);
        assert!(text.contains("[[destination]]"));
    }

    #[test]
    fn partial_file_uses_defaults() {
        let c: MasterConfig = toml::from_str("[ram]\nthief_max_gb = 2.5\n[[destination]]\nid='a'\npath='/a'\n").unwrap();
        assert_eq!(c.ram.thief_max_gb, 2.5);
        assert_eq!(c.ram.settle_secs, 3.0);
        assert_eq!(c.destinations[0].node, "local");
        assert_eq!(c.destinations[0].max_inflight, 4);
        c.validate().unwrap();
    }

    #[test]
    fn rejects_bad_values() {
        let mut c = MasterConfig::default();
        c.destinations.push(DestCfg { id: "x y".into(), path: "/a".into(), ..Default::default() });
        assert!(c.validate().is_err());
        let mut c = MasterConfig::default();
        c.destinations.push(DestCfg { id: "a".into(), path: "rel".into(), ..Default::default() });
        assert!(c.validate().is_err());
        let mut c = MasterConfig::default();
        c.ram.thief_resume_pct = 100.0;
        assert!(c.validate().is_err());
        let mut c = MasterConfig::default();
        c.hooks.validate.on_error = "maybe".into();
        assert!(c.validate().is_err());
        let mut c = MasterConfig::default();
        c.hooks.moved.url = "ftp://x".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn example_config_parses() {
        let c: MasterConfig = toml::from_str(include_str!("../deploy/spillway.example.toml")).unwrap();
        c.validate().unwrap();
        assert_eq!(c.destinations.len(), 2);
        assert!(c.hooks.validate.url.is_empty());
    }

    #[test]
    fn hooks_parse_from_toml() {
        let c: MasterConfig = toml::from_str("[hooks.validate]
url='http://h/v'
batch_max=10
[hooks.moved]
url='http://h/m'
").unwrap();
        assert_eq!(c.hooks.validate.batch_max, 10);
        assert_eq!(c.hooks.validate.on_error, "move");
        assert_eq!(c.hooks.moved.batch_wait_ms, 1000);
        c.validate().unwrap();
    }

    #[test]
    fn byte_thresholds() {
        let r = RamCfg { thief_max_gb: 1.0, thief_resume_pct: 50.0, hard_limit_pct: 150.0, ..Default::default() };
        assert_eq!(r.thief_max_bytes(), 1 << 30);
        assert_eq!(r.thief_resume_bytes(), 1 << 29);
        assert_eq!(r.hard_limit_bytes(), (1u64 << 30) * 3 / 2);
    }
}
