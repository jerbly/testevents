use std::collections::HashMap;

use chrono::{DateTime, Utc};
use opentelemetry::trace::{SpanId, TraceId};
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
            status_code: 0,
            status_message: "".to_string(),
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

    pub fn update(
        &mut self,
        trace_id: String,
        span_id: String,
        merge_attributes: HashMap<String, Value>,
    ) -> bool {
        // Update the span in the store and return it
        if let Some(span_attributes) = self.spans.get_mut(&(trace_id, span_id)) {
            span_attributes.merge(merge_attributes);
            true
        } else {
            false
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
    pub timestamp: DateTime<Utc>,
    duration_ms: i64,
    pub name: String,
    pub status_code: i64,
    pub status_message: String,
    #[serde(rename = "trace.span_id")]
    pub trace_span_id: String,
    #[serde(rename = "trace.trace_id")]
    pub trace_trace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "trace.parent_id")]
    pub trace_parent_id: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
    #[serde(skip_serializing)]
    ttl: i64,
}

impl SpanAttributes {
    pub fn merge(&mut self, merge_attributes: HashMap<String, Value>) {
        for (key, value) in merge_attributes {
            match key.as_str() {
                "name" => value
                    .as_str()
                    .unwrap_or_default()
                    .clone_into(&mut self.name),
                "status_code" => self.status_code = value.as_i64().unwrap_or_default(),
                "status_message" => value
                    .as_str()
                    .unwrap_or_default()
                    .clone_into(&mut self.status_message),
                "ttl" => self.set_ttl(value.as_i64().unwrap_or_default()),
                _ => {
                    self.extra.insert(key, value);
                }
            }
        }
    }

    fn set_ttl(&mut self, ttl: i64) {
        // set a new ttl which is millis from timestamp + incoming ttl
        let now = Utc::now().timestamp_millis();
        let current_used_millis = now - self.timestamp.timestamp_millis();
        self.ttl = current_used_millis + ttl;
    }

    pub fn otel_trace_id(&self) -> Option<TraceId> {
        TraceId::from_hex(&self.trace_trace_id).ok()
    }

    pub fn otel_span_id(&self) -> Option<SpanId> {
        SpanId::from_hex(&self.trace_span_id).ok()
    }

    pub fn otel_parent_id(&self) -> Option<SpanId> {
        self.trace_parent_id
            .as_ref()
            .and_then(|parent_id| SpanId::from_hex(parent_id).ok())
    }
}

impl SpanAttributes {
    pub fn has_expired(&self) -> bool {
        Utc::now().timestamp_millis() - self.timestamp.timestamp_millis() > self.ttl
    }
    pub fn error_timeout(&mut self) {
        self.status_code = 2;
        self.status_message = format!("testevents timeout: TTL was {}", self.ttl);
    }
}

#[derive(Deserialize, Debug)]
pub struct QueryAttributes {
    name: String,
    ttl: Option<i64>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}
