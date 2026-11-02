//! HTTP server of the master: panel (static, embedded), JSON API, live SSE
//! stream and the cluster heartbeat endpoint nodes talk to.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{DestCfg, MasterConfig};
use crate::master::{Core, ThiefState};
use crate::proto::{now_ms, Heartbeat, Kind};
use crate::ram::RamSnapshot;
use crate::stats::{Counters, Mp4Stats, Sample};

/// The React panel (web/, built with `pnpm build`) is compiled into the binary.
static PANEL: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/web/dist");

type St = State<Arc<Core>>;

fn err(status: StatusCode, msg: impl ToString) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

/// Refuses configuration changes when the file is managed by the deploy.
fn locked(core: &Core) -> Option<Response> {
    (!core.cfg.read().web.edit_config)
        .then(|| err(StatusCode::FORBIDDEN, "a configuração é gerenciada pelo arquivo (web.edit_config = false): edite o TOML e reinicie o master"))
}

#[derive(Serialize)]
struct RamView {
    #[serde(flatten)]
    snap: RamSnapshot,
    thief_max_bytes: u64,
    thief_resume_bytes: u64,
    hard_limit_bytes: u64,
    tmpfs_bytes: u64,
}

#[derive(Serialize)]
struct Totals {
    ingest_bytes: u64,
    files_created: u64,
    files_sealed: u64,
    zero_deleted: u64,
    invalid_dropped: u64,
    moved_files: u64,
    moved_bytes: u64,
    move_failures: u64,
    thief_files: u64,
    thief_bytes: u64,
    read_ram_bytes: u64,
    read_dest_bytes: u64,
    write_rejected: u64,
    hook_dropped: u64,
    hook_dropped_bytes: u64,
    outbox_errors: u64,
}

#[derive(Serialize)]
struct Overview {
    version: &'static str,
    hostname: String,
    started_ms: i64,
    now_ms: i64,
    mount_path: String,
    spool_dir: String,
    /// false when the panel may only show the configuration (web.edit_config).
    config_editable: bool,
    /// Physical memory of the master host (to sanity-check the RAM ceiling).
    host_mem_bytes: u64,
    ram: RamView,
    thief: ThiefState,
    totals: Totals,
    rate: Option<Sample>,
    dests: Vec<crate::cluster::DestView>,
    nodes: Vec<crate::cluster::NodeInfo>,
    mp4: Mp4Stats,
    /// Distinct label values seen, per label key.
    groups: std::collections::BTreeMap<String, usize>,
    hooks: HooksView,
    last_event_seq: u64,
    cache: (usize, usize),
}

fn overview(core: &Core) -> Overview {
    let cfg = core.cfg.read().clone();
    let c = &core.stats.c;
    Overview {
        version: crate::proto::VERSION,
        hostname: crate::node::hostname(),
        started_ms: core.stats.started_ms,
        now_ms: now_ms(),
        mount_path: cfg.mount.path.display().to_string(),
        spool_dir: cfg.ram.spool_dir.display().to_string(),
        config_editable: cfg.web.edit_config,
        host_mem_bytes: host_mem_bytes(),
        ram: RamView {
            snap: core.ram.snapshot(),
            thief_max_bytes: cfg.ram.thief_max_bytes(),
            thief_resume_bytes: cfg.ram.thief_resume_bytes(),
            hard_limit_bytes: cfg.ram.hard_limit_bytes(),
            tmpfs_bytes: cfg.ram.tmpfs_bytes(),
        },
        thief: core.thief.lock().clone(),
        totals: Totals {
            ingest_bytes: Counters::get(&c.ingest_bytes),
            files_created: Counters::get(&c.files_created),
            files_sealed: Counters::get(&c.files_sealed),
            zero_deleted: Counters::get(&c.zero_deleted),
            invalid_dropped: Counters::get(&c.invalid_dropped),
            moved_files: Counters::get(&c.moved_files),
            moved_bytes: Counters::get(&c.moved_bytes),
            move_failures: Counters::get(&c.move_failures),
            thief_files: Counters::get(&c.thief_files),
            thief_bytes: Counters::get(&c.thief_bytes),
            read_ram_bytes: Counters::get(&c.read_ram_bytes),
            read_dest_bytes: Counters::get(&c.read_dest_bytes),
            write_rejected: Counters::get(&c.write_rejected),
            hook_dropped: Counters::get(&c.hook_dropped),
            hook_dropped_bytes: Counters::get(&c.hook_dropped_bytes),
            outbox_errors: Counters::get(&c.outbox_errors),
        },
        rate: core.stats.last_sample(),
        dests: core.cluster.views(),
        nodes: core.cluster.nodes(),
        mp4: core.stats.mp4.lock().clone(),
        groups: core.stats.group_keys(),
        hooks: hooks_view(core, &cfg),
        last_event_seq: core.stats.events(0).last().map_or(0, |e| e.seq),
        cache: core.locs.len(),
    }
}

fn host_mem_bytes() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|kb| kb.parse::<u64>().ok()))
        .map_or(0, |kb| kb * 1024)
}

async fn h_overview(State(core): St) -> Response {
    Json(overview(&core)).into_response()
}

#[derive(Deserialize)]
struct SeriesQ {
    range: Option<String>,
}

async fn h_series(State(core): St, Query(q): Query<SeriesQ>) -> Response {
    let coarse = q.range.as_deref() == Some("24h");
    Json(core.stats.series(coarse)).into_response()
}

#[derive(Deserialize)]
struct EventsQ {
    after: Option<u64>,
}

async fn h_events(State(core): St, Query(q): Query<EventsQ>) -> Response {
    Json(core.stats.events(q.after.unwrap_or(0))).into_response()
}

async fn h_recent(State(core): St) -> Response {
    Json(core.stats.recent()).into_response()
}

#[derive(Serialize)]
struct HookView<T: Serialize> {
    enabled: bool,
    url: String,
    #[serde(flatten)]
    stats: T,
}

#[derive(Serialize)]
struct HooksView {
    validate: HookView<crate::hooks::ValidateStats>,
    moved: HookView<MovedView>,
}

#[derive(Serialize)]
struct MovedView {
    #[serde(flatten)]
    health: crate::hooks::HookHealth,
    pending: usize,
    outbox_ok: bool,
    outbox_dir: String,
    outbox_errors: u64,
}

fn hooks_view(core: &Core, cfg: &MasterConfig) -> HooksView {
    HooksView {
        validate: HookView { enabled: !cfg.hooks.validate.url.is_empty(), url: cfg.hooks.validate.url.clone(), stats: core.hooks.validate.lock().clone() },
        moved: HookView {
            enabled: !cfg.hooks.moved.url.is_empty(),
            url: cfg.hooks.moved.url.clone(),
            stats: MovedView {
                health: core.hooks.moved.lock().clone(),
                pending: core.outbox.as_ref().map_or(0, |o| o.len()),
                outbox_ok: core.outbox.is_some(),
                outbox_dir: cfg.hooks.moved.outbox_dir.display().to_string(),
                outbox_errors: Counters::get(&core.stats.c.outbox_errors),
            },
        },
    }
}

#[derive(Deserialize)]
struct GroupsQ {
    key: Option<String>,
}

async fn h_groups(State(core): St, Query(q): Query<GroupsQ>) -> Response {
    Json(json!({ "keys": core.stats.group_keys(), "groups": core.stats.groups(q.key.as_deref()) })).into_response()
}

/// Live stream: an `overview` event every second plus `event` messages as they happen.
async fn h_stream(State(core): St) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut last_seq = core.stats.events(0).last().map_or(0, |e| e.seq);
    let stream = async_stream(move |tx| async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let ov = overview(&core);
            let data = serde_json::to_string(&ov).unwrap_or_default();
            if tx.send(Ok(SseEvent::default().event("overview").data(data))).await.is_err() {
                return;
            }
            let evs = core.stats.events(last_seq);
            if let Some(l) = evs.last() {
                last_seq = l.seq;
                let data = serde_json::to_string(&evs).unwrap_or_default();
                if tx.send(Ok(SseEvent::default().event("events").data(data))).await.is_err() {
                    return;
                }
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Minimal channel-backed stream builder (avoids pulling async-stream).
fn async_stream<T, F, Fut>(f: F) -> impl Stream<Item = T>
where
    T: Send + 'static,
    F: FnOnce(tokio::sync::mpsc::Sender<T>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(f(tx));
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

// ---------------- settings ----------------

#[derive(Serialize)]
struct SettingsView {
    config: MasterConfig,
    config_path_hint: &'static str,
    /// false: shown read-only (web.edit_config = false).
    editable: bool,
    /// Settings that come from environment variables (read-only in the panel).
    env: Vec<EnvView>,
    /// Hook tokens that are set (they are never sent to the browser).
    secrets_set: Vec<&'static str>,
}

#[derive(Serialize)]
struct EnvView {
    path: &'static str,
    var: &'static str,
}

async fn h_settings_get(State(core): St) -> Response {
    let mut cfg = core.cfg.read().clone();
    cfg.web.token = if cfg.web.token.is_empty() { String::new() } else { "********".into() };
    let mut secrets_set = Vec::new();
    for path in ["hooks.validate.token", "hooks.moved.token"] {
        let t = cfg.secret_mut(path);
        if !t.is_empty() {
            t.clear();
            secrets_set.push(path);
        }
    }
    let env = core.env_secrets.lock().iter().map(|(path, _)| EnvView { path, var: crate::config::env_var_for(path).unwrap_or("?") }).collect();
    let editable = cfg.web.edit_config;
    Json(SettingsView { config: cfg, config_path_hint: "edited live; persisted to the master's TOML file", editable, env, secrets_set }).into_response()
}

/// RFC 7386 JSON merge patch.
fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        (t, p) => *t = p.clone(),
    }
}

async fn edit_config(core: &Core, f: impl FnOnce(&mut MasterConfig) -> Result<(), String>) -> Response {
    let _guard = core.cfg_edit.lock().await;
    let mut next = core.cfg.read().clone();
    if let Err(e) = f(&mut next) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    // writes the TOML (and may resize the tmpfs): keep the disk wait off the async workers
    match tokio::task::block_in_place(|| core.replace_config(next)) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

async fn h_settings_put(State(core): St, Json(patch): Json<Value>) -> Response {
    if let Some(r) = locked(&core) {
        return r;
    }
    for (path, _) in core.env_secrets.lock().iter() {
        if patch.pointer(&format!("/{}", path.replace('.', "/"))).is_some() {
            let var = crate::config::env_var_for(path).unwrap_or("?");
            return err(StatusCode::BAD_REQUEST, format!("{path} vem da variável de ambiente {var} e não pode ser alterado pelo painel"));
        }
    }
    // destinations and the panel token have their own endpoints / the file
    if patch.get("destination").is_some() || patch.pointer("/web").is_some() {
        return err(StatusCode::BAD_REQUEST, "use /api/destinations for destinations; web settings are file-only");
    }
    if patch.pointer("/hooks/moved/outbox_dir").is_some() {
        return err(StatusCode::BAD_REQUEST, "hooks.moved.outbox_dir is file-only (needs a restart)");
    }
    let before = core.cfg.read().ram.clone();
    let resp = edit_config(&core, |cfg| {
        let mut v = serde_json::to_value(&*cfg).map_err(|e| e.to_string())?;
        merge(&mut v, &patch);
        *cfg = serde_json::from_value(v).map_err(|e| format!("invalid settings: {e}"))?;
        Ok(())
    })
    .await;
    if resp.status().is_success() {
        let after = core.cfg.read().ram.clone();
        if after.thief_max_gb != before.thief_max_gb || after.thief_resume_pct != before.thief_resume_pct {
            core.stats.event(
                "info",
                "settings",
                format!("Teto de RAM ajustado para {:.2} GiB (thief para em {:.0}%)", after.thief_max_gb, after.thief_resume_pct),
            );
        } else {
            core.stats.event("info", "settings", "Configurações atualizadas");
        }
    }
    resp
}

// ---------------- destinations ----------------

async fn h_dest_add(State(core): St, Json(d): Json<DestCfg>) -> Response {
    if let Some(r) = locked(&core) {
        return r;
    }
    let id = d.id.clone();
    let resp = edit_config(&core, |cfg| {
        if cfg.destinations.iter().any(|x| x.id == d.id) {
            return Err(format!("destination '{}' already exists", d.id));
        }
        cfg.destinations.push(d);
        Ok(())
    })
    .await;
    if resp.status().is_success() {
        core.stats.event("info", "dest_added", format!("Destino '{id}' adicionado"));
        let local = core.cluster.local.clone();
        tokio::spawn(async move { local.probe_all_now().await });
    }
    resp
}

async fn h_dest_patch(State(core): St, Path(id): Path<String>, Json(patch): Json<Value>) -> Response {
    if let Some(r) = locked(&core) {
        return r;
    }
    if patch.get("id").is_some_and(|v| v.as_str() != Some(id.as_str())) {
        return err(StatusCode::BAD_REQUEST, "id cannot be changed");
    }
    let resp = edit_config(&core, |cfg| {
        let Some(d) = cfg.destinations.iter_mut().find(|x| x.id == id) else { return Err(format!("no destination '{id}'")) };
        let mut v = serde_json::to_value(&*d).map_err(|e| e.to_string())?;
        merge(&mut v, &patch);
        *d = serde_json::from_value(v).map_err(|e| format!("invalid destination: {e}"))?;
        Ok(())
    })
    .await;
    if resp.status().is_success() {
        core.stats.event("info", "dest_changed", format!("Destino '{id}' atualizado"));
        let local = core.cluster.local.clone();
        tokio::spawn(async move { local.probe_all_now().await });
    }
    resp
}

#[derive(Deserialize)]
struct MoveQ {
    to: usize,
}

async fn h_dest_move(State(core): St, Path(id): Path<String>, Query(q): Query<MoveQ>) -> Response {
    if let Some(r) = locked(&core) {
        return r;
    }
    edit_config(&core, |cfg| {
        let Some(i) = cfg.destinations.iter().position(|x| x.id == id) else { return Err(format!("no destination '{id}'")) };
        let d = cfg.destinations.remove(i);
        let to = q.to.min(cfg.destinations.len());
        cfg.destinations.insert(to, d);
        Ok(())
    })
    .await
}

async fn h_dest_delete(State(core): St, Path(id): Path<String>) -> Response {
    if let Some(r) = locked(&core) {
        return r;
    }
    let resp = edit_config(&core, |cfg| {
        let n = cfg.destinations.len();
        cfg.destinations.retain(|x| x.id != id);
        if cfg.destinations.len() == n {
            return Err(format!("no destination '{id}'"));
        }
        Ok(())
    })
    .await;
    if resp.status().is_success() {
        core.stats.event("warn", "dest_removed", format!("Destino '{id}' removido (os arquivos dele não aparecem mais no mount)"));
    }
    resp
}

async fn h_dest_probe(State(core): St, Path(id): Path<String>) -> Response {
    let Some(d) = core.cluster.dest(&id) else { return err(StatusCode::NOT_FOUND, "no such destination") };
    if d.cfg.read().node == *core.cluster.local_id.read() {
        core.cluster.local.probe_all_now().await;
    } else {
        // remote nodes probe every 2 s; wait for a fresh heartbeat
        tokio::time::sleep(Duration::from_millis(2500)).await;
    }
    core.cluster.maintain();
    match core.cluster.views().into_iter().find(|v| v.id == id) {
        Some(v) => Json(v).into_response(),
        None => err(StatusCode::NOT_FOUND, "no such destination"),
    }
}

async fn h_node_forget(State(core): St, Path(id): Path<String>) -> Response {
    if core.cluster.nodes().iter().any(|n| n.id == id && n.online) {
        return err(StatusCode::BAD_REQUEST, "node is online");
    }
    if core.cluster.forget_node(&id) {
        Json(json!({ "ok": true })).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "no such node")
    }
}

// ---------------- browse ----------------

#[derive(Deserialize)]
struct BrowseQ {
    path: Option<String>,
}

#[derive(Serialize)]
struct BrowseEntry {
    name: String,
    dir: bool,
    size: u64,
    mtime_ms: i64,
    /// "ram" or destination ids holding this name.
    locations: Vec<String>,
    state: Option<&'static str>,
}

async fn h_browse(State(core): St, Query(q): Query<BrowseQ>) -> Response {
    let rel = q.path.unwrap_or_default().trim_matches('/').to_string();
    if !crate::proto::valid_rel(&rel) {
        return err(StatusCode::BAD_REQUEST, "invalid path");
    }
    let mut map: std::collections::BTreeMap<String, BrowseEntry> = Default::default();
    for (name, dir) in core.ram.children(&rel) {
        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let (size, mtime_ms, state) = match core.ram.lookup(&child) {
            Some(crate::ram::Lookup::File(f)) => (
                f.size,
                f.mtime.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64),
                Some(if f.writers > 0 { "open" } else { f.state.name() }),
            ),
            Some(crate::ram::Lookup::Dir(d)) => (0, d.mtime.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64), None),
            _ => (0, 0, None),
        };
        map.insert(name.clone(), BrowseEntry { name, dir, size, mtime_ms, locations: vec!["ram".into()], state });
    }
    let dests = core.cluster.readable();
    let lists = futures::future::join_all(dests.iter().map(|d| {
        let rel = rel.clone();
        let cluster = core.cluster.clone();
        async move { tokio::time::timeout(Duration::from_secs(5), cluster.list(d, &rel)).await }
    }))
    .await;
    let mut truncated = false;
    for (d, r) in dests.iter().zip(lists) {
        if let Ok(Ok(Some(entries))) = r {
            for e in entries {
                let id = d.id();
                let be = map.entry(e.name.clone()).or_insert_with(|| BrowseEntry {
                    name: e.name.clone(),
                    dir: e.kind == Kind::Dir,
                    size: e.size,
                    mtime_ms: e.mtime_ms,
                    locations: vec![],
                    state: None,
                });
                be.locations.push(id);
                if be.mtime_ms == 0 {
                    be.mtime_ms = e.mtime_ms;
                }
            }
        }
    }
    let mut items: Vec<BrowseEntry> = map.into_values().collect();
    items.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    if items.len() > 5000 {
        items.truncate(5000);
        truncated = true;
    }
    Json(json!({ "path": rel, "entries": items, "truncated": truncated })).into_response()
}

// ---------------- cluster ----------------

async fn h_heartbeat(State(core): St, headers: HeaderMap, Json(hb): Json<Heartbeat>) -> Response {
    let token = core.cluster.token();
    let ok = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).is_some_and(|t| t == token);
    if !ok {
        return err(StatusCode::UNAUTHORIZED, "bad cluster token");
    }
    match core.cluster.heartbeat(hb) {
        Ok(r) => Json(r).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

async fn h_health() -> &'static str {
    "ok"
}

// ---------------- static ----------------

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "png" => "image/png",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Static panel files; unknown paths fall back to index.html (client-side routing).
async fn h_static(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let (file, path) = match PANEL.get_file(path) {
        Some(f) if !path.is_empty() => (f, path),
        _ => match PANEL.get_file("index.html") {
            Some(f) => (f, "index.html"),
            None => return err(StatusCode::NOT_FOUND, "panel not built"),
        },
    };
    // hashed assets are immutable; index.html must always be revalidated
    let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    ([(header::CONTENT_TYPE, mime(path)), (header::CACHE_CONTROL, cache)], file.contents()).into_response()
}

/// Optional panel/API token: `Authorization: Bearer` or `?token=` (EventSource can't set headers).
async fn auth_mw(State(core): St, req: Request, next: Next) -> Response {
    let token = core.cfg.read().web.token.clone();
    if token.is_empty() {
        return next.run(req).await;
    }
    let header_ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == token);
    let query_ok = req.uri().query().is_some_and(|q| q.split('&').any(|kv| kv == format!("token={token}")));
    if header_ok || query_ok {
        next.run(req).await
    } else {
        err(StatusCode::UNAUTHORIZED, "token required")
    }
}

pub fn router(core: Arc<Core>) -> Router {
    let api = Router::new()
        .route("/api/overview", get(h_overview))
        .route("/api/series", get(h_series))
        .route("/api/events", get(h_events))
        .route("/api/recent", get(h_recent))
        .route("/api/groups", get(h_groups))
        .route("/api/stream", get(h_stream))
        .route("/api/settings", get(h_settings_get).put(h_settings_put))
        .route("/api/destinations", post(h_dest_add))
        .route("/api/destinations/{id}", axum::routing::patch(h_dest_patch).delete(h_dest_delete))
        .route("/api/destinations/{id}/probe", post(h_dest_probe))
        .route("/api/destinations/{id}/move", post(h_dest_move))
        .route("/api/nodes/{id}", axum::routing::delete(h_node_forget))
        .route("/api/browse", get(h_browse))
        .layer(middleware::from_fn_with_state(core.clone(), auth_mw));
    Router::new()
        .route("/healthz", get(h_health))
        .route("/cluster/heartbeat", post(h_heartbeat))
        .merge(api)
        .fallback(h_static)
        .with_state(core)
}

pub async fn serve(core: Arc<Core>, listen: &str) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(listen, "panel listening");
    let app = router(core);
    Ok(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "http server stopped");
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_patch() {
        let mut a = json!({"ram": {"thief_max_gb": 1.0, "settle_secs": 3.0}, "x": 1});
        merge(&mut a, &json!({"ram": {"thief_max_gb": 2.0}, "x": null}));
        assert_eq!(a, json!({"ram": {"thief_max_gb": 2.0, "settle_secs": 3.0}}));
    }
}
