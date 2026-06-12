//! MetricsActor — collects LLM call latency, tool usage, and error rates.
//!
//! Exposes a Prometheus-compatible `/api/metrics` endpoint.
//!
//! ## Messages
//!
//! - `RecordLlmLatency` — record an LLM call duration in seconds.
//! - `RecordToolCall` — record a tool call (name, success/failure).
//! - `RecordError` — record an error (category label).
//! - `GetMetrics` — return Prometheus-formatted text.

use actix::prelude::*;
use plugin_interface::*;
use std::collections::HashMap;

// ── Messages ─────────────────────────────────────────────────────────────────

#[derive(Message)]
#[rtype(result = "()")]
pub struct RecordLlmLatency {
    pub seconds: f64,
    pub model: String,
    pub success: bool,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct RecordToolCall {
    pub tool_name: String,
    pub success: bool,
    pub duration_ms: u64,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct RecordError {
    pub category: String,  // e.g. "llm_api", "tool_exec", "rate_limit"
}

#[derive(Message)]
#[rtype(result = "String")]
pub struct GetMetrics;

/// Get metrics as a JSON object (for HTTP API).
#[derive(Message)]
#[rtype(result = "serde_json::Value")]
pub struct GetMetricsJson;

// ── Actor ────────────────────────────────────────────────────────────────────

pub struct MetricsActor {
    // LLM latencies (rolling window of last 1000)
    llm_latencies: Vec<f64>,
    llm_latency_count: u64,
    llm_latency_sum: f64,
    llm_latency_max: f64,

    // Tool call stats
    tool_calls: HashMap<String, ToolCallStats>,

    // Error counts by category
    errors: HashMap<String, u64>,

    // Per-model stats
    model_stats: HashMap<String, ModelStats>,

    // Start time
    start_time: std::time::Instant,
}

#[derive(Clone, Default, Debug)]
struct ToolCallStats {
    total: u64,
    successes: u64,
    failures: u64,
    total_duration_ms: u64,
}

#[derive(Clone, Default, Debug)]
struct ModelStats {
    call_count: u64,
    total_latency: f64,
}

impl MetricsActor {
    pub fn new() -> Self {
        Self {
            llm_latencies: Vec::with_capacity(1000),
            llm_latency_count: 0,
            llm_latency_sum: 0.0,
            llm_latency_max: 0.0,
            tool_calls: HashMap::new(),
            errors: HashMap::new(),
            model_stats: HashMap::new(),
            start_time: std::time::Instant::now(),
        }
    }
}

impl Actor for MetricsActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        log::info!("[MetricsActor] started");
    }
}

impl Handler<RecordLlmLatency> for MetricsActor {
    type Result = ();

    fn handle(&mut self, msg: RecordLlmLatency, _ctx: &mut Self::Context) {
        self.llm_latencies.push(msg.seconds);
        if self.llm_latencies.len() > 1000 {
            self.llm_latencies.remove(0);
        }
        self.llm_latency_count += 1;
        self.llm_latency_sum += msg.seconds;
        if msg.seconds > self.llm_latency_max {
            self.llm_latency_max = msg.seconds;
        }

        let stats = self.model_stats.entry(msg.model.clone()).or_default();
        stats.call_count += 1;
        stats.total_latency += msg.seconds;
    }
}

impl Handler<RecordToolCall> for MetricsActor {
    type Result = ();

    fn handle(&mut self, msg: RecordToolCall, _ctx: &mut Self::Context) {
        let stats = self.tool_calls.entry(msg.tool_name.clone()).or_default();
        stats.total += 1;
        if msg.success {
            stats.successes += 1;
        } else {
            stats.failures += 1;
        }
        stats.total_duration_ms += msg.duration_ms;
    }
}

impl Handler<RecordError> for MetricsActor {
    type Result = ();

    fn handle(&mut self, msg: RecordError, _ctx: &mut Self::Context) {
        *self.errors.entry(msg.category.clone()).or_insert(0) += 1;
    }
}

impl Handler<GetMetrics> for MetricsActor {
    type Result = MessageResult<GetMetrics>;

    fn handle(&mut self, _: GetMetrics, _ctx: &mut Self::Context) -> Self::Result {
        let uptime = self.start_time.elapsed().as_secs_f64();
        let mut lines: Vec<String> = Vec::new();

        // Uptime.
        lines.push("# HELP bn_agent_uptime_seconds Agent uptime".to_string());
        lines.push("# TYPE bn_agent_uptime_seconds gauge".to_string());
        lines.push(format!("bn_agent_uptime_seconds {}", uptime));

        // LLM calls.
        lines.push("# HELP bn_agent_llm_calls_total Total LLM API calls".to_string());
        lines.push("# TYPE bn_agent_llm_calls_total counter".to_string());
        for (model, stats) in &self.model_stats {
            lines.push(format!("bn_agent_llm_calls_total{{model=\"{}\"}} {}", model, stats.call_count));
        }

        // LLM latency.
        let avg_latency = if self.llm_latency_count > 0 {
            self.llm_latency_sum / self.llm_latency_count as f64
        } else {
            0.0
        };
        lines.push("# HELP bn_agent_llm_latency_seconds LLM API call latency".to_string());
        lines.push("# TYPE bn_agent_llm_latency_seconds gauge".to_string());
        lines.push(format!("bn_agent_llm_latency_avg_seconds {}", avg_latency));
        lines.push(format!("bn_agent_llm_latency_max_seconds {}", self.llm_latency_max));
        lines.push(format!("bn_agent_llm_latency_count {}", self.llm_latency_count));

        // Tool calls.
        lines.push("# HELP bn_agent_tool_calls_total Tool calls by name".to_string());
        lines.push("# TYPE bn_agent_tool_calls_total counter".to_string());
        for (name, stats) in &self.tool_calls {
            lines.push(format!("bn_agent_tool_calls_total{{tool=\"{}\",status=\"success\"}} {}", name, stats.successes));
            lines.push(format!("bn_agent_tool_calls_total{{tool=\"{}\",status=\"failure\"}} {}", name, stats.failures));
            if stats.total > 0 {
                let avg_duration = stats.total_duration_ms as f64 / stats.total as f64;
                lines.push(format!("bn_agent_tool_duration_ms{{tool=\"{}\"}} {}", name, avg_duration));
            }
        }

        // Errors.
        lines.push("# HELP bn_agent_errors_total Errors by category".to_string());
        lines.push("# TYPE bn_agent_errors_total counter".to_string());
        for (cat, count) in &self.errors {
            lines.push(format!("bn_agent_errors_total{{category=\"{}\"}} {}", cat, count));
        }

        MessageResult(lines.join("\n") + "\n")
    }
}

impl Handler<GetMetricsJson> for MetricsActor {
    type Result = MessageResult<GetMetricsJson>;

    fn handle(&mut self, _: GetMetricsJson, _ctx: &mut Self::Context) -> Self::Result {
        let uptime = self.start_time.elapsed().as_secs_f64();
        let avg_latency = if self.llm_latency_count > 0 {
            self.llm_latency_sum / self.llm_latency_count as f64
        } else {
            0.0
        };

        let tool_summary: serde_json::Value = self.tool_calls.iter().map(|(name, stats)| {
            (name.clone(), serde_json::json!({
                "total": stats.total,
                "successes": stats.successes,
                "failures": stats.failures,
                "avg_duration_ms": if stats.total > 0 { stats.total_duration_ms as f64 / stats.total as f64 } else { 0.0 },
            }))
        }).collect();

        MessageResult(serde_json::json!({
            "uptime_seconds": uptime,
            "llm": {
                "total_calls": self.llm_latency_count,
                "avg_latency_seconds": avg_latency,
                "max_latency_seconds": self.llm_latency_max,
                "by_model": self.model_stats.iter().map(|(m, s)| {
                    (m.clone(), serde_json::json!({
                        "call_count": s.call_count,
                        "avg_latency_seconds": if s.call_count > 0 { s.total_latency / s.call_count as f64 } else { 0.0 },
                    }))
                }).collect::<serde_json::Value>(),
            },
            "tool_calls": tool_summary,
            "errors": self.errors,
        }))
    }
}
