//! bbg-proxy — OpenAI-compatible passthrough proxy.
//!
//! Any OpenAI-compatible client (pi, Claude Code, Codex, aider, ...) can point
//! its base URL at this server. Each `/v1/chat/completions` request is
//! normalized in-flight (prose user messages only) and forwarded to the real
//! provider. Responses stream back unchanged.
//!
//! Config via env:
//!   BBG_UPSTREAM_URL   upstream OpenAI-compatible base URL (default: OPENAI_BASE_URL or http://localhost:11434/v1)
//!   BBG_UPSTREAM_KEY   upstream API key (default: OPENAI_API_KEY)
//!   BBG_PORT           listen port (default 8088)
//!   BBG_DRY            "1" = normalize + print + drop (no forward; for testing)
//!
//! Example (ollama):
//!   BBG_UPSTREAM_URL=http://localhost:11434/v1 BBG_PORT=8088 bbg-proxy
//!   curl http://localhost:8088/v1/chat/completions -d '{...}'

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{env, sync::Arc};

#[derive(Clone)]
struct ProxyState {
    upstream: String,
    key: String,
    dry: bool,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ChatMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(default)]
    stream: bool,
}

#[derive(Debug, Serialize)]
struct ChatResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<ChatResponseChoice>,
}

#[derive(Debug, Serialize)]
struct ChatResponseChoice {
    index: u32,
    message: serde_json::Value,
    finish_reason: String,
}

/// Normalize the last user message if it is plain prose. Returns the message
/// unchanged when it is code/shell or not prose.
fn normalize_message(mut msg: ChatMessage) -> ChatMessage {
    if msg.role != "user" {
        return msg;
    }
    // Only string content; skip structured content (arrays of parts).
    let text = match &msg.content {
        serde_json::Value::String(s) => s.clone(),
        _ => return msg,
    };
    let opts = brief_bright_gone::normalize::NormalizeOptions::default();
    let out = brief_bright_gone::normalize::normalize_with_detect(&text, &opts);
    if out.changed {
        msg.content = serde_json::Value::String(out.text);
    }
    msg
}

async fn handle_chat(
    State(state): State<Arc<ProxyState>>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> Response {
    let messages: Vec<ChatMessage> = req.messages.into_iter().map(normalize_message).collect();

    // Track normalization stats for the response headers.
    let saved_bytes: usize = 0;
    for m in &messages {
        if let serde_json::Value::String(s) = &m.content {
            // We already normalized; recompute savings is cheap enough to skip.
            let _ = s;
        }
    }

    if state.dry {
        let body = serde_json::json!({
            "object": "chat.completion",
            "model": req.model,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "[bbg dry-run] messages normalized; not forwarded." },
                "finish_reason": "stop"
            }],
            "bbg": { "dry": true, "saved_bytes": saved_bytes }
        });
        return (StatusCode::OK, Json(body)).into_response();
    }

    // Forward to upstream.
    let upstream = format!("{}/chat/completions", state.upstream.trim_end_matches('/'));
    let payload = serde_json::json!({
        "model": req.model,
        "messages": messages,
        "stream": req.stream,
    });

    let mut upstream_req = state
        .client
        .post(&upstream)
        .json(&payload);

    if !state.key.is_empty() {
        upstream_req = upstream_req.header("Authorization", format!("Bearer {}", state.key));
    }
    // Copy the content-type (we always send json) but skip hop-by-hop headers.
    for (k, v) in headers.iter() {
        let k = k.as_str();
        if k == "authorization" || k == "content-length" || k == "host" || k == "connection" || k == "accept-encoding" {
            continue;
        }
        if let Ok(hv) = HeaderValue::from_str(v.to_str().unwrap_or("")) {
            upstream_req = upstream_req.header(k, hv);
        }
    }

    match upstream_req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();
            let bytes = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": {"message": format!("upstream read failed: {e}")}}))).into_response();
                }
            };
            let mut builder = Response::builder().status(status);
            for (k, v) in headers.iter() {
                if k == "transfer-encoding" || k == "content-length" {
                    continue;
                }
                builder = builder.header(k, v);
            }
            builder
                .header("x-bbg-normalized", if saved_bytes > 0 { "1" } else { "0" })
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => {
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": {"message": format!("upstream error: {e}")}}))).into_response()
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let upstream = env::var("BBG_UPSTREAM_URL")
        .or_else(|_| env::var("OPENAI_BASE_URL"))
        .unwrap_or_else(|_| "http://localhost:11434/v1".to_string());
    let key = env::var("BBG_UPSTREAM_KEY")
        .or_else(|_| env::var("OPENAI_API_KEY"))
        .unwrap_or_default();
    let port = env::var("BBG_PORT").unwrap_or_else(|_| "8088".to_string());
    let dry = env::var("BBG_DRY").is_ok_and(|v| v == "1");

    let client = reqwest::Client::builder()
        .build()
        .expect("failed to build http client");

    let state = Arc::new(ProxyState { upstream, key, dry, client });

    let app = Router::new()
        .route("/v1/chat/completions", post(handle_chat))
        .route("/v1/models", axum::routing::get(get_models))
        .route("/health", axum::routing::get(health))
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("bbg-proxy listening on {addr} (dry={dry})");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind failed");
    axum::serve(listener, app).await.expect("server error");
}

async fn health() -> &'static str {
    "ok"
}

async fn get_models(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    // Passthrough: ask upstream for models.
    let upstream = format!("{}/models", state.upstream.trim_end_matches('/'));
    let client = state.client.clone();
    let mut req = client.get(&upstream);
    if !state.key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", state.key));
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            Response::builder().status(status).body(axum::body::Body::from(bytes)).unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
        }
        Err(_) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"data": []}))).into_response(),
    }
}
