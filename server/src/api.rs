//! HTTP/WebSocket surface.
//!
//! REST handles commands (zones, volume, links). The WebSocket pushes a full
//! `GraphState` snapshot on connect and on every change (snapshot-only protocol).
//! A single bearer token gates BOTH REST and the WS upgrade (header or `?token=`),
//! so an unauthenticated subscriber can't even read the graph.

use std::sync::{Arc, Mutex};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Request, State,
    },
    http::{header, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::backend::PwBackend;
use crate::model::{self, Action, VolumeTarget};
use crate::wire::{GraphState, LinkView, ZoneView};
use crate::zones::{LinkSpec, ZoneDef, ZoneError, ZoneStore};

pub struct AppState {
    pub backend: Arc<dyn PwBackend>,
    pub zones: Mutex<ZoneStore>,
    pub token: String,
    /// Composed snapshots (backend graph + zone overlay) fan out to WS clients here.
    pub tx: broadcast::Sender<GraphState>,
}

/// The node key owning a port, from its stable key (`<node_key>#<port>`).
fn node_of_port(port_key: &str) -> &str {
    port_key.split('#').next().unwrap_or(port_key)
}

/// A link whose two ports live on the same node — a sink's `monitor_* -> playback_*`
/// self-loop, which feeds the card's output back into itself (a feedback loop / loud
/// squeal). Always rejected at link creation and skipped by reconcile.
fn is_self_loop(output_port: &str, input_port: &str) -> bool {
    node_of_port(output_port) == node_of_port(input_port)
}

/// Reject a zone's link set if any link is a self-loop (feedback). Surfaced as 400.
fn reject_self_loops(links: &[LinkBody]) -> Result<(), ApiError> {
    if let Some(l) = links.iter().find(|l| is_self_loop(&l.output_port, &l.input_port)) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "link '{}' -> '{}' loops a device into itself (feedback); route a capture \
                 source into the destination instead",
                l.output_port, l.input_port
            ),
        ));
    }
    Ok(())
}

/// Tear down links a zone no longer owns. After an edit/delete, destroy each `removed` link
/// that is currently live AND not claimed by any *remaining* zone — so a zone's dropped
/// links (e.g. a monitor->playback self-loop) don't linger and feed back. Conservative:
/// never touches a link another zone still lists, nor one that isn't actually live.
fn teardown_orphaned_links(s: &AppState, removed: &[LinkSpec]) {
    use std::collections::HashSet;
    let live: HashSet<(String, String)> = s
        .backend
        .snapshot()
        .links
        .into_iter()
        .map(|l| (l.output_port, l.input_port))
        .collect();
    let claimed: HashSet<(String, String)> = {
        let store = s.zones.lock().unwrap();
        store
            .zones
            .iter()
            .flat_map(|z| z.links.iter())
            .map(|l| (l.output_port.clone(), l.input_port.clone()))
            .collect()
    };
    for l in removed {
        let key = (l.output_port.clone(), l.input_port.clone());
        if live.contains(&key) && !claimed.contains(&key) {
            if let Err(e) = s.backend.apply(Action::DestroyLink {
                output_port: l.output_port.clone(),
                input_port: l.input_port.clone(),
            }) {
                tracing::warn!("teardown_orphaned_links: {e}");
            }
        }
    }
}

/// Resolve a zone's volume target: the destination node it feeds, plus the channel indices
/// its links land on. The destination is the node owning the zone's first link's input port;
/// the channels are those links' input-port channel indices (`PortView.channel` = `port.id`).
/// This is "controlled against the destination" — each zone drives only the channels its
/// links route into, so zones sharing one multi-channel card stay independent.
///
/// Returns `(node_key, sorted-unique channels)`. Channels is empty when the ports carry no
/// index (e.g. a node with only a master volume) — the caller then drives the whole node
/// uniformly. Falls back to an explicit `VolumeSpec` (TOML per-channel #6, or a link-less
/// zone). `None` if the zone has neither a present destination node nor a volume spec.
fn zone_volume_target(zone: &ZoneDef, g: &GraphState) -> Option<(String, Vec<usize>)> {
    if let Some(first) = zone.links.first() {
        let node_key = node_of_port(&first.input_port).to_string();
        if let Some(node) = g.nodes.iter().find(|n| n.key == node_key) {
            let mut channels: Vec<usize> = zone
                .links
                .iter()
                .filter(|l| node_of_port(&l.input_port) == node_key)
                .filter_map(|l| node.ports.iter().find(|p| p.key == l.input_port))
                .filter_map(|p| p.channel)
                .collect();
            channels.sort_unstable();
            channels.dedup();
            return Some((node_key, channels));
        }
    }
    // Link-less / destination-absent: honor an explicit volume spec if present.
    let v = zone.volumes.first()?;
    let channels = v.channels.iter().map(|c| c.channel).collect();
    Some((v.node_key.clone(), channels))
}

/// The level to show on a zone tile: the loudest of its destination channels (matches the
/// one-knob UI), or the node's representative volume when the zone drives the whole node.
fn representative_volume(node: &crate::wire::NodeView, channels: &[usize]) -> Option<f32> {
    if channels.is_empty() {
        return node.volume;
    }
    let vals: Vec<f32> = channels.iter().filter_map(|&c| node.channel_volumes.get(c).copied()).collect();
    vals.into_iter().reduce(f32::max).or(node.volume)
}

/// Backend graph + zone overlay (active/degraded/missing) = what clients see.
pub fn compose(state: &AppState) -> GraphState {
    let mut g = state.backend.snapshot();
    let store = state.zones.lock().unwrap();
    g.zones = store
        .zones
        .iter()
        .map(|z| {
            let active = store.is_active(&z.name);
            let missing = if active {
                model::missing_for_zone(&store, &z.name, &g)
            } else {
                Vec::new()
            };
            // Surface the live volume/mute for the tile. The zone drives the channels its
            // links feed on the destination node (so co-resident zones stay independent);
            // the shown level is the loudest of those channels. Mute is node-wide in
            // PipeWire, so it reflects the destination node's mute (see PUT /zones/:name/volume).
            let (volume_node, volume, muted) = match zone_volume_target(z, &g) {
                Some((node_key, channels)) => {
                    let node = g.nodes.iter().find(|n| n.key == node_key);
                    let volume = node.and_then(|n| representative_volume(n, &channels));
                    let muted = node.map(|n| n.muted).unwrap_or(false);
                    (Some(node_key), volume, muted)
                }
                None => (None, None, false),
            };
            ZoneView {
                name: z.name.clone(),
                links: z
                    .links
                    .iter()
                    .map(|l| LinkView {
                        output_port: l.output_port.clone(),
                        input_port: l.input_port.clone(),
                    })
                    .collect(),
                active,
                degraded: !missing.is_empty(),
                missing,
                volume_node,
                volume,
                muted,
            }
        })
        .collect();
    g
}

/// Push the current composed snapshot to all connected clients.
pub fn publish(state: &AppState) {
    let _ = state.tx.send(compose(state));
}

/// Drive the live graph toward the active zones' desired state.
pub fn reconcile_and_apply(state: &AppState) {
    let desired = {
        let store = state.zones.lock().unwrap();
        model::desired_from_active(&store)
    };
    let snap = state.backend.snapshot();
    for action in model::reconcile(&desired, &snap) {
        if let Err(e) = state.backend.apply(action) {
            tracing::warn!("reconcile apply failed: {e}");
        }
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/graph", get(get_graph))
        .route("/zones", get(get_zones).post(create_zone))
        .route("/zones/:name", put(update_zone).delete(delete_zone))
        .route("/zones/:name/activate", post(activate))
        .route("/zones/:name/deactivate", post(deactivate))
        .route("/zones/:name/volume", put(set_zone_volume))
        .route("/nodes/:key/volume", put(set_volume))
        .route("/refresh", post(refresh))
        .route("/links", post(create_link).delete(delete_link))
        .route("/ws", get(ws_handler))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}

// ---- auth ----------------------------------------------------------------

async fn auth(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Result<Response, StatusCode> {
    if token_ok(&state.token, &req) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn token_ok(expected: &str, req: &Request) -> bool {
    if let Some(val) = req.headers().get(header::AUTHORIZATION) {
        if let Ok(s) = val.to_str() {
            if let Some(t) = s.strip_prefix("Bearer ") {
                if t == expected {
                    return true;
                }
            }
        }
    }
    if let Some(q) = req.uri().query() {
        // Percent-decode the query so a token with reserved characters (the Dart
        // client sends it via `Uri.encodeComponent`) compares against the raw value.
        for (k, v) in form_urlencoded::parse(q.as_bytes()) {
            if k == "token" && v == expected {
                return true;
            }
        }
    }
    false
}

// ---- REST handlers -------------------------------------------------------

type ApiError = (StatusCode, String);

async fn get_graph(State(s): State<Arc<AppState>>) -> Json<GraphState> {
    Json(compose(&s))
}

async fn get_zones(State(s): State<Arc<AppState>>) -> Json<Vec<ZoneView>> {
    Json(compose(&s).zones)
}

/// Body for create (`POST /zones`) and edit (`PUT /zones/:name`): a name + the zone's
/// links. On edit, `name` is the (possibly new) name and the path carries the current one.
#[derive(Deserialize)]
struct ZoneBody {
    name: String,
    links: Vec<LinkBody>,
}

fn link_specs(links: Vec<LinkBody>) -> Vec<LinkSpec> {
    links
        .into_iter()
        .map(|l| LinkSpec { output_port: l.output_port, input_port: l.input_port })
        .collect()
}

/// Create a new (inactive) zone from a name + its links. The client toggles it on
/// afterward via `/activate`. Volumes aren't set here — a zone with links auto-derives
/// its representative volume node from the first link's sink (see `ZoneDef::primary_node`).
async fn create_zone(
    State(s): State<Arc<AppState>>,
    Json(body): Json<ZoneBody>,
) -> Result<Json<Vec<ZoneView>>, ApiError> {
    if body.links.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "a zone needs at least one link".into()));
    }
    reject_self_loops(&body.links)?;
    let zone = ZoneDef {
        name: body.name,
        links: link_specs(body.links),
        volumes: Vec::new(),
    };
    {
        let mut store = s.zones.lock().unwrap();
        store.add_zone(zone).map_err(|e| match e {
            ZoneError::ZoneExists(_) => (StatusCode::CONFLICT, e.to_string()),
            ZoneError::InvalidZone(_) => (StatusCode::BAD_REQUEST, e.to_string()),
            other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
        })?;
        store
            .save()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    // New zone is inactive, so nothing to reconcile — just push the updated list.
    publish(&s);
    Ok(Json(compose(&s).zones))
}

/// Edit a zone's name and/or links (volumes are preserved). The path is the current name;
/// `body.name` is what to rename to (same value = links-only edit). A rename can't collide
/// with another zone, and an active zone stays active under its new name.
async fn update_zone(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(body): Json<ZoneBody>,
) -> Result<Json<Vec<ZoneView>>, ApiError> {
    if body.links.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "a zone needs at least one link".into()));
    }
    reject_self_loops(&body.links)?;
    // Snapshot the zone's old links before we replace them, so we can tear down the ones it
    // no longer owns (otherwise a re-point leaves stale live links — e.g. a self-loop — that
    // keep playing / feed back).
    let old_links: Vec<LinkSpec> = {
        let store = s.zones.lock().unwrap();
        store.get(&name).map(|z| z.links.clone()).unwrap_or_default()
    };
    {
        let mut store = s.zones.lock().unwrap();
        store
            .update_zone(&name, &body.name, link_specs(body.links))
            .map_err(|e| match e {
                ZoneError::ZoneExists(_) => (StatusCode::CONFLICT, e.to_string()),
                ZoneError::InvalidZone(_) => (StatusCode::BAD_REQUEST, e.to_string()),
                ZoneError::NoSuchZone(_) => (StatusCode::NOT_FOUND, e.to_string()),
                other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
            })?;
        store
            .save()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    // Drop links this zone no longer owns (kept only if another zone still wants them),
    // then reconcile so a newly-added link on an active zone is created.
    teardown_orphaned_links(&s, &old_links);
    reconcile_and_apply(&s);
    publish(&s);
    Ok(Json(compose(&s).zones))
}

async fn delete_zone(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<Vec<ZoneView>>, ApiError> {
    let old_links: Vec<LinkSpec> = {
        let store = s.zones.lock().unwrap();
        store.get(&name).map(|z| z.links.clone()).unwrap_or_default()
    };
    {
        let mut store = s.zones.lock().unwrap();
        store
            .remove_zone(&name)
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
        store
            .save()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    // Tear down the deleted zone's links (unless another zone still lists them) so they
    // don't keep playing / feed back after the zone is gone.
    teardown_orphaned_links(&s, &old_links);
    publish(&s);
    Ok(Json(compose(&s).zones))
}

async fn activate(State(s): State<Arc<AppState>>, Path(name): Path<String>) -> Result<Json<Vec<ZoneView>>, ApiError> {
    set_zone_active(&s, &name, true)
}

async fn deactivate(State(s): State<Arc<AppState>>, Path(name): Path<String>) -> Result<Json<Vec<ZoneView>>, ApiError> {
    set_zone_active(&s, &name, false)
}

fn set_zone_active(s: &AppState, name: &str, active: bool) -> Result<Json<Vec<ZoneView>>, ApiError> {
    {
        let mut store = s.zones.lock().unwrap();
        store
            .set_active(name, active)
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
        store
            .save()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    reconcile_and_apply(s);
    publish(s);
    Ok(Json(compose(s).zones))
}

#[derive(Deserialize)]
struct VolumeBody {
    volume: f32,
    #[serde(default)]
    muted: Option<bool>,
}

/// Set a zone's volume against the channels its links feed on the destination node (the
/// "controlled against the destination" model). Two zones sharing one multi-channel card
/// move independently because each touches only its own channels. Falls back to a uniform
/// node volume when the destination exposes no per-channel index. Mute is node-wide in
/// PipeWire (v1): muting a zone mutes its destination node, so co-resident zones share mute.
async fn set_zone_volume(
    State(s): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(body): Json<VolumeBody>,
) -> Result<StatusCode, ApiError> {
    let snap = s.backend.snapshot();
    let (node_key, channels) = {
        let store = s.zones.lock().unwrap();
        let zone = store
            .get(&name)
            .ok_or((StatusCode::NOT_FOUND, format!("no such zone: {name}")))?;
        zone_volume_target(zone, &snap)
            .ok_or((StatusCode::UNPROCESSABLE_ENTITY, "zone has no controllable destination".to_string()))?
    };
    let active = { s.zones.lock().unwrap().is_active(&name) };
    // Persist the level onto the zone so the reconcile loop re-asserts THIS value instead of
    // reverting it (and it survives a restart). See `set_volume` for the full rationale.
    {
        let mut store = s.zones.lock().unwrap();
        store.upsert_zone_volume(&name, &node_key, &channels, body.volume, body.muted);
        store
            .save()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    if active {
        // reconcile drives the backend to the newly-stored desired for this active zone.
        reconcile_and_apply(&s);
    } else {
        // Inactive zone: reconcile ignores it, so apply the one-off change directly.
        let target = if channels.is_empty() {
            VolumeTarget::Uniform(body.volume)
        } else {
            VolumeTarget::Channels(channels.into_iter().map(|c| (c, body.volume)).collect())
        };
        s.backend
            .apply(Action::SetVolume { node_key: node_key.clone(), target })
            .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
        if let Some(muted) = body.muted {
            s.backend
                .apply(Action::SetMute { node_key, muted })
                .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
        }
    }
    publish(&s);
    Ok(StatusCode::NO_CONTENT)
}

async fn set_volume(
    State(s): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(body): Json<VolumeBody>,
) -> Result<StatusCode, ApiError> {
    // Persist the level onto the active zones that own this node BEFORE touching the backend.
    // The relay loop reconciles active zones on every backend graph change, and a `SetVolume`
    // is itself such a change — so if the stored volume still held the old value, reconcile
    // would instantly revert the slider. With the store updated first, reconcile re-asserts the
    // requested level (and it now survives a restart). Falls back to a direct apply when no
    // active zone governs the node.
    let persisted = {
        let mut store = s.zones.lock().unwrap();
        let changed = store.set_node_volume(&key, body.volume, body.muted);
        if changed {
            store
                .save()
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        changed
    };
    if persisted {
        reconcile_and_apply(&s);
    } else {
        s.backend
            .apply(Action::SetVolume { node_key: key.clone(), target: VolumeTarget::Uniform(body.volume) })
            .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
        if let Some(muted) = body.muted {
            // The client always sends `muted`, so a failure here is a real partial
            // apply — surface it instead of returning 204 for a half-applied command.
            s.backend
                .apply(Action::SetMute { node_key: key, muted })
                .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
        }
    }
    publish(&s);
    Ok(StatusCode::NO_CONTENT)
}

/// Force a re-read of live volume/mute from PipeWire (node Props + device routes) so
/// external changes (`wpctl`/GNOME) surface in the app. The backend pulls asynchronously
/// and broadcasts a fresh snapshot when the values arrive (snapshot-only) — so this just
/// kicks the scan and returns; the WS delivers the updated state. See NEXT_STEPS #3.
async fn refresh(State(s): State<Arc<AppState>>) -> Result<StatusCode, ApiError> {
    s.backend
        .rescan()
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct LinkBody {
    output_port: String,
    input_port: String,
}

async fn create_link(State(s): State<Arc<AppState>>, Json(b): Json<LinkBody>) -> Result<StatusCode, ApiError> {
    if is_self_loop(&b.output_port, &b.input_port) {
        return Err((
            StatusCode::BAD_REQUEST,
            "self-loop link (a device into itself) rejected — it feeds back".into(),
        ));
    }
    s.backend
        .apply(Action::CreateLink { output_port: b.output_port, input_port: b.input_port })
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    publish(&s);
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_link(State(s): State<Arc<AppState>>, Json(b): Json<LinkBody>) -> Result<StatusCode, ApiError> {
    s.backend
        .apply(Action::DestroyLink { output_port: b.output_port, input_port: b.input_port })
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    publish(&s);
    Ok(StatusCode::NO_CONTENT)
}

// ---- WebSocket -----------------------------------------------------------

async fn ws_handler(State(s): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| ws_loop(socket, s))
}

async fn ws_loop(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.tx.subscribe();
    // Initial full snapshot on connect.
    if send_snapshot(&mut socket, &compose(&state)).await.is_err() {
        return;
    }
    loop {
        match rx.recv().await {
            Ok(snap) => {
                if send_snapshot(&mut socket, &snap).await.is_err() {
                    break;
                }
            }
            // Slow client: we dropped messages. Snapshot-only means we just resend
            // the latest full state — the client self-heals, no gap recovery needed.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                if send_snapshot(&mut socket, &compose(&state)).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn send_snapshot(socket: &mut WebSocket, snap: &GraphState) -> Result<(), axum::Error> {
    let text = serde_json::to_string(snap).unwrap_or_else(|_| "{}".into());
    socket.send(Message::Text(text)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use crate::wire::{Direction, NodeView, PortView};
    use crate::zones::{LinkSpec, VolumeSpec};

    fn in_port(node: &str, name: &str, ch: Option<usize>) -> PortView {
        PortView {
            key: format!("{node}#in:{name}"),
            name: name.into(),
            direction: Direction::In,
            channel: ch,
        }
    }

    /// An 8-ch surround sink whose ports carry channel indices in audio.position order
    /// (FL=0,FR=1,RL=2,RR=3,...), matching the real box.
    fn surround_sink() -> NodeView {
        NodeView {
            key: "SINK".into(),
            name: "8ch".into(),
            media_class: "Audio/Sink".into(),
            ports: vec![
                in_port("SINK", "playback_FL", Some(0)),
                in_port("SINK", "playback_FR", Some(1)),
                in_port("SINK", "playback_RL", Some(2)),
                in_port("SINK", "playback_RR", Some(3)),
            ],
            volume: Some(0.9),
            channel_volumes: vec![0.1, 0.2, 0.3, 0.4],
            muted: false,
            present: true,
        }
    }

    fn zone_into(name: &str, ports: &[&str]) -> ZoneDef {
        ZoneDef {
            name: name.into(),
            links: ports
                .iter()
                .map(|p| LinkSpec { output_port: "SRC#out:x".into(), input_port: format!("SINK#in:{p}") })
                .collect(),
            volumes: vec![],
        }
    }

    #[test]
    fn self_loop_detection() {
        // Same node on both ends (monitor -> own playback) = feedback loop.
        assert!(is_self_loop("Audio/Sink|Amp#out:monitor_FL", "Audio/Sink|Amp#in:playback_FL"));
        // Capture source -> a different sink is the normal, allowed shape.
        assert!(!is_self_loop("Audio/Source|Cap#out:capture_FL", "Audio/Sink|Amp#in:playback_FL"));
    }

    #[test]
    fn zone_target_resolves_destination_channels_from_links() {
        let g = GraphState { nodes: vec![surround_sink()], ..Default::default() };
        // Patio feeds RL+RR -> channels 2,3 on SINK.
        let (node, ch) = zone_volume_target(&zone_into("patio", &["playback_RL", "playback_RR"]), &g).unwrap();
        assert_eq!(node, "SINK");
        assert_eq!(ch, vec![2, 3]);
        // A different zone feeds FL+FR -> channels 0,1 (independent of patio).
        let (_, ch2) = zone_volume_target(&zone_into("kitchen", &["playback_FL", "playback_FR"]), &g).unwrap();
        assert_eq!(ch2, vec![0, 1]);
    }

    #[test]
    fn representative_volume_is_loudest_of_the_zones_channels() {
        let sink = surround_sink(); // channel_volumes = [0.1, 0.2, 0.3, 0.4]
        // Patio (channels 2,3) -> max(0.3, 0.4) = 0.4, not the node-wide 0.9.
        assert_eq!(representative_volume(&sink, &[2, 3]), Some(0.4));
        // No channels -> falls back to the node's representative volume.
        assert_eq!(representative_volume(&sink, &[]), Some(0.9));
    }

    #[test]
    fn zone_target_falls_back_to_volume_spec_when_link_less() {
        let g = GraphState { nodes: vec![surround_sink()], ..Default::default() };
        let zone = ZoneDef {
            name: "tomlzone".into(),
            links: vec![],
            volumes: vec![VolumeSpec {
                node_key: "SINK".into(),
                volume: None,
                channels: vec![
                    crate::zones::ChannelVolume { channel: 6, volume: 0.6 },
                    crate::zones::ChannelVolume { channel: 7, volume: 0.6 },
                ],
                muted: false,
            }],
        };
        let (node, ch) = zone_volume_target(&zone, &g).unwrap();
        assert_eq!(node, "SINK");
        assert_eq!(ch, vec![6, 7]);
    }

    fn req_with_query(query: &str) -> Request {
        Request::builder()
            .uri(format!("/ws?{query}"))
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn query_token_is_percent_decoded() {
        // A token with reserved chars, sent as the Dart client encodes it.
        let token = "a b+c/d=e";
        let encoded = "token=a%20b%2Bc%2Fd%3De";
        assert!(token_ok(token, &req_with_query(encoded)));
    }

    #[test]
    fn query_token_mismatch_is_rejected() {
        assert!(!token_ok("secret", &req_with_query("token=wrong")));
    }

    #[test]
    fn header_bearer_token_still_works() {
        let req = Request::builder()
            .uri("/graph")
            .header(header::AUTHORIZATION, "Bearer secret")
            .body(Body::empty())
            .unwrap();
        assert!(token_ok("secret", &req));
    }
}
