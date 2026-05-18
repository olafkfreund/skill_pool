use std::time::Instant;

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

use crate::state::AppState;

#[derive(Serialize)]
struct DepStatus {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'static str>,
}

impl DepStatus {
    fn up(latency_ms: u64) -> Self {
        Self {
            status: "up",
            latency_ms: Some(latency_ms),
            error: None,
            note: None,
        }
    }

    fn down(error: String) -> Self {
        Self {
            status: "down",
            latency_ms: None,
            error: Some(error),
            note: None,
        }
    }

    fn off() -> Self {
        Self {
            status: "off",
            latency_ms: None,
            error: None,
            note: None,
        }
    }

    fn off_note(note: &'static str) -> Self {
        Self {
            status: "off",
            latency_ms: None,
            error: None,
            note: Some(note),
        }
    }
}

async fn probe_db(state: &AppState) -> DepStatus {
    let t = Instant::now();
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(state.db())
        .await
    {
        Ok(_) => DepStatus::up(t.elapsed().as_millis() as u64),
        Err(e) => DepStatus::down(e.to_string()),
    }
}

async fn probe_storage(state: &AppState) -> DepStatus {
    // `stat("")` on opendal resolves to the root of the operator's configured
    // path. For `fs://` this is a directory stat — cheap, no I/O beyond
    // lstat(2). For `s3://` it issues a HeadObject on the root prefix which
    // works on every bucket regardless of contents. If the backend does not
    // support Stat (e.g. write-only memory stores) opendal returns
    // ErrorKind::Unsupported, which we fold into `off`.
    match state.storage().probe().await {
        Ok(latency_ms) => DepStatus::up(latency_ms),
        Err(e) if e.kind() == crate::storage::Storage::unsupported_kind() => {
            DepStatus::off_note("storage backend does not support stat")
        }
        Err(e) => DepStatus::down(e.to_string()),
    }
}

fn probe_embedder(state: &AppState) -> DepStatus {
    // dimension() returns Some only when a real embedder is wired in.
    // NullEmbedder returns None → "off".
    match state.embedder().dimension() {
        Some(_) => DepStatus::up(0),
        None => DepStatus::off(),
    }
}

pub async fn healthz(State(state): State<AppState>) -> Json<Value> {
    // Deliberate: HTTP 200 always. LB should not pull the node on a transient
    // DB blip. Monitoring systems read `deps.db.status` instead.
    let db = probe_db(&state).await;
    let storage = probe_storage(&state).await;
    let embedder = probe_embedder(&state);

    let top_status = if db.status == "down" {
        "degraded"
    } else {
        "ok"
    };

    Json(json!({
        "status": top_status,
        "version": env!("CARGO_PKG_VERSION"),
        "deps": {
            "db": db,
            "storage": storage,
            "embedder": embedder,
        }
    }))
}
