//! AudioZones control-plane server.
//!
//! Wiring: load config + zones -> build a PwBackend (mock for now) -> reassert any
//! active zones on boot -> relay backend changes to WS clients -> serve axum.

use std::sync::{Arc, Mutex};

use tokio::net::TcpListener;
use tokio::sync::broadcast;

use audiozones_server::{api, backend, config, wire, zones};

use api::AppState;
use backend::PwBackend;
#[cfg(not(feature = "pipewire-backend"))]
use backend::MockBackend;
use config::Config;
use zones::ZoneStore;

const CONFIG_PATH: &str = "audiozones.toml";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = match Config::load(CONFIG_PATH)? {
        Some(c) => c,
        None => {
            tracing::warn!(
                "no {CONFIG_PATH} found — using dev defaults (bind 127.0.0.1:4040, token 'dev-token'). \
                 Create {CONFIG_PATH} (mode 0600) for real use."
            );
            Config::default()
        }
    };

    let zone_store = ZoneStore::load(&cfg.zones_file)?;
    tracing::info!(
        "loaded {} zone(s), {} active, from {}",
        zone_store.zones.len(),
        zone_store.active.len(),
        cfg.zones_file
    );

    #[cfg(feature = "pipewire-backend")]
    let backend: Arc<dyn PwBackend> = {
        tracing::info!("using real PipeWire backend");
        Arc::new(audiozones_server::backend_pipewire::PipewireBackend::new()?)
    };
    #[cfg(not(feature = "pipewire-backend"))]
    let backend: Arc<dyn PwBackend> = {
        tracing::info!("using mock backend (build with --features pipewire-backend for real PipeWire)");
        Arc::new(MockBackend::new())
    };

    let (tx, _) = broadcast::channel::<wire::GraphState>(16);

    let state = Arc::new(AppState {
        backend: backend.clone(),
        zones: Mutex::new(zone_store),
        token: cfg.token.clone(),
        tx: tx.clone(),
    });

    // Reassert active zones on boot (survives power blips, eng-review decision).
    api::reconcile_and_apply(&state);
    api::publish(&state);

    // Relay: any backend graph change -> recompose (with zone overlay) -> WS clients.
    // In the real backend this is how a CLI change shows up in the app with no refresh.
    {
        let state = state.clone();
        let mut brx = backend.subscribe();
        tokio::spawn(async move {
            loop {
                match brx.recv().await {
                    Ok(_) => api::publish(&state),
                    Err(broadcast::error::RecvError::Lagged(_)) => api::publish(&state),
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    let app = api::router(state);
    let listener = TcpListener::bind(&cfg.bind).await?;
    tracing::info!("AudioZones server listening on {}", cfg.bind);
    axum::serve(listener, app).await?;
    Ok(())
}
