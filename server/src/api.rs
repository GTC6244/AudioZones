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
use crate::wire::{GraphState, ZoneView};
use crate::zones::ZoneStore;

pub struct AppState {
    pub backend: Arc<dyn PwBackend>,
    pub zones: Mutex<ZoneStore>,
    pub token: String,
    /// Composed snapshots (backend graph + zone overlay) fan out to WS clients here.
    pub tx: broadcast::Sender<GraphState>,
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
            // Surface the live volume/mute of the zone's representative node so the tile
            // can show a slider; `volume_node` is where the client PUTs changes.
            let volume_node = z.primary_node();
            let (volume, muted) = volume_node
                .as_ref()
                .and_then(|k| g.nodes.iter().find(|n| &n.key == k))
                .map(|n| (n.volume, n.muted))
                .unwrap_or((None, false));
            ZoneView {
                name: z.name.clone(),
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
        .route("/zones", get(get_zones))
        .route("/zones/:name/activate", post(activate))
        .route("/zones/:name/deactivate", post(deactivate))
        .route("/nodes/:key/volume", put(set_volume))
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
        for pair in q.split('&') {
            if let Some(t) = pair.strip_prefix("token=") {
                if t == expected {
                    return true;
                }
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

async fn set_volume(
    State(s): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(body): Json<VolumeBody>,
) -> Result<StatusCode, ApiError> {
    s.backend
        .apply(Action::SetVolume { node_key: key.clone(), target: VolumeTarget::Uniform(body.volume) })
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    if let Some(muted) = body.muted {
        let _ = s.backend.apply(Action::SetMute { node_key: key, muted });
    }
    publish(&s);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct LinkBody {
    output_port: String,
    input_port: String,
}

async fn create_link(State(s): State<Arc<AppState>>, Json(b): Json<LinkBody>) -> Result<StatusCode, ApiError> {
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
