mod span_store;

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, patch, post},
    Json, Router,
};
use honeycomb_client::honeycomb::HoneyComb;
use serde_json::{json, Value};

use opentelemetry_sdk::trace::TracerProvider;

use span_store::{HoneycombEvent, QueryAttributes, SpanAttributes, SpanStore};
use tokio::sync::mpsc;

type SharedState = Arc<RwLock<AppState>>;

struct AppState {
    honeycomb_tx: mpsc::UnboundedSender<SpanAttributes>,
    provider: TracerProvider,
    spans: SpanStore,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // load configuration
    dotenv::dotenv().ok();

    let provider = TracerProvider::builder()
        .with_simple_exporter(opentelemetry_stdout::SpanExporter::default())
        .build();
    let hc = honeycomb_client::get_honeycomb(&["createDatasets"])
        .await
        .expect("Honeycomb connection must be established")
        .expect("Honeycomb API key must be valid");

    let (honeycomb_tx, honeycomb_rx) = mpsc::unbounded_channel();

    let state = AppState {
        honeycomb_tx,
        provider,
        spans: SpanStore::new(),
    };
    let shared_state = Arc::new(RwLock::new(state));

    // build our application with a route
    let app = Router::new()
        .route("/", post(root_handler)) // POST == create
        .route("/:trace_id/:span_id/", post(child_handler))
        .route("/:trace_id/:span_id/", delete(close_handler))
        .route("/:trace_id/:span_id/", patch(update_handler)) // PATCH == merge-update
        .with_state(Arc::clone(&shared_state));

    // Start the span ttl handler
    let task1 = handle_span_ttl(&shared_state);
    let task2 = handle_honeycomb(hc, honeycomb_rx);

    // run it
    let bind_port = std::env::var("TESTEVENTS_PORT").unwrap_or("3003".to_string());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{bind_port}")).await?;
    println!("testevents version: {}", env!("CARGO_PKG_VERSION"));
    println!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    // Abort the task
    task1.abort();
    task2.abort();

    Ok(())
}

async fn root_handler(
    State(state): State<SharedState>,
    Json(attributes): Json<QueryAttributes>,
) -> impl IntoResponse {
    let state = &mut state.write().expect("RwLock should not be poisoned");
    let provider = &state.provider;
    let trace_id = provider.config().id_generator.new_trace_id();
    let span_id = provider.config().id_generator.new_span_id();
    let traceparent = format!("00-{}-{}-01", trace_id, span_id);
    // Store the span
    state
        .spans
        .insert(trace_id.to_string(), span_id.to_string(), None, attributes);
    Json(
        json!({ "trace_id": trace_id.to_string(), "span_id": span_id.to_string(), "traceparent": traceparent }),
    )
}

async fn child_handler(
    State(state): State<SharedState>,
    Path((trace_id, span_id)): Path<(String, String)>,
    Json(attributes): Json<QueryAttributes>,
) -> impl IntoResponse {
    let state = &mut state.write().expect("RwLock should not be poisoned");
    // Set the parent_id to the span_id
    let parent_id = span_id;
    let span_id = &state.provider.config().id_generator.new_span_id();
    let traceparent = format!("00-{}-{}-01", trace_id, span_id);
    // Store the span
    state.spans.insert(
        trace_id.to_string(),
        span_id.to_string(),
        Some(parent_id),
        attributes,
    );
    Json(
        json!({ "trace_id": trace_id.to_string(), "span_id": span_id.to_string(), "traceparent": traceparent }),
    )
}

async fn close_handler(
    State(state): State<SharedState>,
    Path((trace_id, span_id)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    // Find the span (remove it from the store)
    let mut state = state.write().expect("RwLock should not be poisoned");
    let span = state.spans.remove(trace_id, span_id);
    match span {
        Some(span) => {
            state.honeycomb_tx.send(span).expect("Must send span");
            Ok(Json(json!({ "message": "OK" })))
        }
        None => Err(AppError::SpanNotFound),
    }
}

async fn update_handler(
    State(state): State<SharedState>,
    Path((trace_id, span_id)): Path<(String, String)>,
    Json(attributes): Json<HashMap<String, Value>>,
) -> Result<Json<Value>, AppError> {
    // Find the span and update it
    let mut state = state.write().expect("RwLock should not be poisoned");
    if state.spans.update(trace_id, span_id, attributes) {
        Ok(Json(json!({ "message": "OK" })))
    } else {
        Err(AppError::SpanNotFound)
    }
}

enum AppError {
    SpanNotFound,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::SpanNotFound => (StatusCode::NOT_FOUND, "Span not found".to_owned()),
        };

        (status, Json(json!({ "err": message }))).into_response()
    }
}

fn handle_span_ttl(shared_state: &SharedState) -> tokio::task::JoinHandle<()> {
    let shared_state = Arc::clone(shared_state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

            let mut state = shared_state.write().expect("RwLock should not be poisoned");
            // Get to_remove spans
            let to_remove = state.spans.get_expired_keys();

            for (trace_id, span_id) in to_remove {
                println!("Expired: {}/{}", trace_id, span_id);
                let s = state.spans.remove(trace_id, span_id);
                if let Some(mut span) = s {
                    span.error_timeout();
                    state.honeycomb_tx.send(span).expect("Must send span");
                }
            }
        }
    })
}

fn handle_honeycomb(
    hc: HoneyComb,
    mut action_rx: mpsc::UnboundedReceiver<SpanAttributes>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let span = action_rx.recv().await;
            if let Some(span) = span {
                let he: HoneycombEvent = span.into();
                let he_list = vec![&he];
                match hc
                    .create_events(
                        he.dataset_slug(),
                        serde_json::to_value(he_list).expect("Must serialize"),
                    )
                    .await
                {
                    Err(e) => eprintln!("Error sending to Honeycomb: {:?}", e),
                    Ok(statuses) => {
                        for status in statuses {
                            if status.status != 202 {
                                eprintln!("Error sending to Honeycomb: {:?}", status);
                            }
                        }
                    }
                }
            }
        }
    })
}
