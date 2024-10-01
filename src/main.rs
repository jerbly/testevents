mod span_store;

use std::{
    collections::HashMap,
    env,
    sync::{Arc, RwLock},
};

use anyhow::Context;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, patch, post},
    Json, Router,
};
use serde_json::{json, Value};

use span_store::{QueryAttributes, SpanAttributes, SpanStore};
use tokio::sync::mpsc;

use opentelemetry::{
    global::{self},
    trace::{Span, SpanContext, SpanKind, TraceContextExt, TraceFlags, TraceState},
};
use opentelemetry::{trace::Tracer, KeyValue};
use opentelemetry_otlp::TonicExporterBuilder;
use opentelemetry_sdk::trace::{Config, IdGenerator, RandomIdGenerator};
use opentelemetry_sdk::{trace as sdktrace, Resource};

type SharedState = Arc<RwLock<AppState>>;

const HONEYCOMB_API_KEY: &str = "HONEYCOMB_API_KEY";

fn init_hc_exporter() -> anyhow::Result<TonicExporterBuilder> {
    use tonic::{metadata::MetadataMap, transport::ClientTlsConfig};
    const HONEYCOMB_ENDPOINT_HOST: &str = "api.honeycomb.io";

    let api_key = env::var(HONEYCOMB_API_KEY).context(format!(
        "Environment variable {} not found",
        HONEYCOMB_API_KEY
    ))?;

    let mut metadata = MetadataMap::with_capacity(1);
    metadata.insert("x-honeycomb-team", api_key.parse()?);

    let tls_config = ClientTlsConfig::new().domain_name(HONEYCOMB_ENDPOINT_HOST);
    Ok(opentelemetry_otlp::new_exporter()
        .tonic()
        .with_metadata(metadata)
        .with_tls_config(tls_config))
}

fn init_tracer_provider() -> anyhow::Result<sdktrace::Tracer> {
    let service_name = env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "testevents".to_string());

    let config = Config::default().with_resource(Resource::new(vec![KeyValue::new(
        opentelemetry_semantic_conventions::resource::SERVICE_NAME,
        service_name,
    )]));

    // Is "honeycomb" in the OTEL_EXPORTER_OTLP_ENDPOINT env var?
    let otel_exporter = if env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "".to_string())
        .contains("honeycomb")
    {
        init_hc_exporter()?
    } else {
        opentelemetry_otlp::new_exporter().tonic()
    };

    Ok(opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(otel_exporter)
        .with_trace_config(config)
        .install_batch(opentelemetry_sdk::runtime::Tokio)?)
}

struct AppState {
    otel_tx: mpsc::UnboundedSender<SpanAttributes>,
    spans: SpanStore,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // load configuration
    dotenv::dotenv().ok();

    let _tracer = init_tracer_provider()?;

    let (otel_tx, otel_rx) = mpsc::unbounded_channel();

    let state = AppState {
        otel_tx,
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
    let task2 = handle_otel(otel_rx);

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
    let id_generator = RandomIdGenerator::default();
    let trace_id = id_generator.new_trace_id();
    let span_id = id_generator.new_span_id();
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
    let id_generator = RandomIdGenerator::default();
    let span_id = id_generator.new_span_id();
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
            state.otel_tx.send(span).expect("Must send span");
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
                    state.otel_tx.send(span).expect("Must send span");
                }
            }
        }
    })
}

fn handle_otel(
    mut action_rx: mpsc::UnboundedReceiver<SpanAttributes>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let span_attributes = action_rx.recv().await;
            if let Some(span_attributes) = span_attributes {
                // Get the otel trace, span and parent ids
                let Some(trace_id) = span_attributes.otel_trace_id() else {
                    eprintln!("Invalid trace_id {}", span_attributes.trace_trace_id);
                    continue;
                };
                let Some(span_id) = span_attributes.otel_span_id() else {
                    eprintln!("Invalid span_id {}", span_attributes.trace_span_id);
                    continue;
                };
                let parent_id = span_attributes.otel_parent_id();

                let tracer = global::tracer("testevents");

                let span_context: Option<SpanContext> = parent_id.map(|parent| {
                    SpanContext::new(
                        trace_id,
                        parent,
                        TraceFlags::SAMPLED,
                        false,
                        TraceState::NONE,
                    )
                });

                // convert the extra hashmap to a vec of KeyValue
                fn serde_to_otel(v: Value) -> opentelemetry::Value {
                    match v {
                        Value::String(s) => opentelemetry::Value::String(s.into()),
                        Value::Number(n) => {
                            if n.is_i64() {
                                // Unwrap is safe because we know it's an i64
                                opentelemetry::Value::I64(n.as_i64().unwrap())
                            } else if n.is_f64() {
                                // Unwrap is safe because we know it's an f64
                                opentelemetry::Value::F64(n.as_f64().unwrap())
                            } else {
                                opentelemetry::Value::String("unsupported".into())
                            }
                        }
                        Value::Bool(b) => opentelemetry::Value::Bool(b),
                        _ => opentelemetry::Value::String("unsupported".into()),
                    }
                }

                let attrs = span_attributes
                    .extra
                    .into_iter()
                    .map(|(k, v)| KeyValue::new(k, serde_to_otel(v)));

                let span_builder = tracer
                    .span_builder(span_attributes.name)
                    .with_kind(SpanKind::Client)
                    .with_trace_id(trace_id)
                    .with_span_id(span_id)
                    .with_start_time(span_attributes.timestamp)
                    .with_attributes(attrs);

                let mut span = if let Some(sc) = span_context {
                    span_builder.start_with_context(
                        &tracer,
                        &opentelemetry::Context::new().with_remote_span_context(sc),
                    )
                } else {
                    span_builder.start(&tracer)
                };

                if span_attributes.status_code == 2 {
                    span.set_status(opentelemetry::trace::Status::Error {
                        description: span_attributes.status_message.into(),
                    });
                }

                drop(span);
            }
        }
    })
}
