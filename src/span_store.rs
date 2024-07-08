use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct SpanStore {
    spans: HashMap<(String, String), SpanAttributes>,
}

impl SpanStore {
    pub fn new() -> Self {
        SpanStore {
            spans: HashMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        trace_id: String,
        span_id: String,
        parent_id: Option<String>,
        query_attributes: QueryAttributes,
    ) {
        let value = SpanAttributes {
            timestamp: Utc::now(),
            duration_ms: 0,
            name: query_attributes.name,
            service_name: query_attributes.service_name,
            status_code: 0,
            trace_span_id: span_id.clone(),
            trace_trace_id: trace_id.clone(),
            trace_parent_id: parent_id,
            extra: query_attributes.extra,
            ttl: query_attributes.ttl.unwrap_or(10000), // 10 seconds
        };
        self.spans.insert((trace_id, span_id), value);
    }

    pub fn remove(&mut self, trace_id: String, span_id: String) -> Option<SpanAttributes> {
        // Remove the span from the store and return it
        if let Some(mut span_attributes) = self.spans.remove(&(trace_id, span_id)) {
            span_attributes.duration_ms =
                Utc::now().timestamp_millis() - span_attributes.timestamp.timestamp_millis();
            Some(span_attributes)
        } else {
            None
        }
    }

    pub fn get_expired_keys(&self) -> Vec<(String, String)> {
        self.spans
            .iter()
            .filter_map(|((trace_id, span_id), span_attributes)| {
                if span_attributes.has_expired() {
                    Some((trace_id.clone(), span_id.clone()))
                } else {
                    None
                }
            })
            .collect()
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct SpanAttributes {
    #[serde(skip_serializing)]
    timestamp: DateTime<Utc>,
    duration_ms: i64,
    name: String,
    status_code: i64,
    #[serde(rename = "service.name")]
    service_name: String,
    #[serde(rename = "trace.span_id")]
    trace_span_id: String,
    #[serde(rename = "trace.trace_id")]
    trace_trace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "trace.parent_id")]
    trace_parent_id: Option<String>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
    #[serde(skip_serializing)]
    ttl: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct HoneycombEvent {
    time: DateTime<Utc>,
    data: SpanAttributes,
}

impl From<SpanAttributes> for HoneycombEvent {
    fn from(span: SpanAttributes) -> Self {
        HoneycombEvent {
            time: span.timestamp,
            data: span,
        }
    }
}

impl HoneycombEvent {
    pub fn dataset_slug(&self) -> &str {
        self.data.service_name.as_str()
    }
}

impl SpanAttributes {
    pub fn has_expired(&self) -> bool {
        Utc::now().timestamp_millis() - self.timestamp.timestamp_millis() > self.ttl
    }
    pub fn error_timeout(&mut self) {
        self.status_code = 2;
        self.extra.insert("error".to_owned(), true.into());
        self.extra.insert(
            "status_message".to_owned(),
            format!("testevents timeout: TTL was {}", self.ttl).into(),
        );
    }
}

#[derive(Deserialize, Debug)]
pub struct QueryAttributes {
    service_name: String,
    name: String,
    ttl: Option<i64>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}
