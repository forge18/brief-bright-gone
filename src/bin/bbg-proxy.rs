//! bbg-proxy — local OpenAI Chat Completions and Anthropic Messages proxy.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use brief_bright_gone::{
    adapters,
    operations::{self, Gate, LocalConfig, compression_gate},
    sigil,
    store::Store,
    types::Provider,
};
use futures_util::StreamExt;
use reqwest::Url;
use serde_json::Value;
use std::{env, net::IpAddr, path::PathBuf, sync::Arc, time::Duration};

const FORWARDED_HEADERS: [&str; 3] = ["accept", "content-type", "user-agent"];
const DEFAULT_BIND: &str = "127.0.0.1";

#[derive(Clone)]
struct ProxyState {
    upstream: Url,
    key: String,
    proxy_token: Option<String>,
    dry: bool,
    client: reqwest::Client,
    store: Store,
    config: LocalConfig,
    cost_ledger: PathBuf,
}

fn validate_upstream_url(raw: &str) -> Result<Url, String> {
    let mut url = Url::parse(raw).map_err(|_| "upstream URL is invalid".to_owned())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("upstream URL must not contain userinfo credentials".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("upstream URL must not contain a query or fragment".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "upstream URL must have a host".to_owned())?;
    let local = host.eq_ignore_ascii_case("localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        "http" => return Err("plain HTTP upstreams are allowed only on loopback".into()),
        _ => return Err("upstream URL scheme must be http or https".into()),
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn upstream_endpoint(base: &Url, relative: &str) -> Url {
    base.join(relative).expect("validated upstream base URL")
}

fn resolve_bind(
    raw: &str,
    allow_non_loopback: bool,
    proxy_token: Option<&str>,
) -> Result<IpAddr, String> {
    let address = raw
        .parse::<IpAddr>()
        .map_err(|_| "BBG_BIND must be an IP address".to_owned())?;
    if !address.is_loopback() && (!allow_non_loopback || proxy_token.is_none_or(str::is_empty)) {
        return Err(
            "non-loopback BBG_BIND requires BBG_ALLOW_NON_LOOPBACK=1 and BBG_PROXY_TOKEN".into(),
        );
    }
    Ok(address)
}

fn client_authorized(state: &ProxyState, headers: &HeaderMap) -> bool {
    let Some(token) = &state.proxy_token else {
        return true;
    };
    let Some(presented) = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    constant_time_eq(presented.as_bytes(), format!("Bearer {token}").as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error":{"code":"proxy_auth_required","message":"proxy authentication required"}})),
    )
        .into_response()
}

fn forwarded_headers(headers: &HeaderMap) -> HeaderMap {
    let mut forwarded = HeaderMap::new();
    for name in FORWARDED_HEADERS {
        if let Some(value) = headers.get(name) {
            forwarded.insert(HeaderName::from_static(name), value.clone());
        }
    }
    forwarded
}

fn redacted_error(error: &reqwest::Error) -> Response {
    let (status, code, message) = if error.is_timeout() {
        (
            StatusCode::GATEWAY_TIMEOUT,
            "upstream_timeout",
            "upstream request timed out",
        )
    } else {
        (
            StatusCode::BAD_GATEWAY,
            "upstream_unavailable",
            "upstream request failed",
        )
    };
    (
        status,
        Json(serde_json::json!({"error":{"code":code,"message":message}})),
    )
        .into_response()
}

fn response_from_upstream(status: StatusCode, headers: &HeaderMap, bytes: Vec<u8>) -> Response {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        if name != "transfer-encoding" && name != "content-length" && name != "connection" {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn streaming_response(response: reqwest::Response) -> Response {
    let status = response.status();
    let headers = response.headers().clone();
    let stream = response
        .bytes_stream()
        .map(|item| item.map_err(std::io::Error::other));
    let mut builder = Response::builder().status(status);
    for (name, value) in &headers {
        if name != "transfer-encoding" && name != "content-length" && name != "connection" {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(axum::body::Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn string_content(value: &Value) -> Option<&str> {
    value.as_str()
}

fn substitute_openai_originals(payload: &mut Value, store: &Store) {
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = message.get("content").and_then(string_content) else {
            continue;
        };
        let Ok(Some(original)) = store.get_sigil_original(content) else {
            continue;
        };
        let Ok(original) = String::from_utf8(original) else {
            continue;
        };
        if let Some(slot) = message.get_mut("content") {
            *slot = Value::String(original);
        }
    }
}

fn predicted_openai_normalization_savings(payload: &Value, config: &LocalConfig) -> f64 {
    let Some(price) = config.price_for(&Provider::OpenAi) else {
        return 0.0;
    };
    let Some(messages) = payload.get("messages").and_then(Value::as_array) else {
        return 0.0;
    };
    let saved_bytes = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content").and_then(string_content))
        .map(|text| {
            let output =
                brief_bright_gone::normalize::normalize_with_detect(text, &Default::default());
            text.len().saturating_sub(output.text.len())
        })
        .sum::<usize>();
    (saved_bytes as f64 / 4.0) * price.input_per_million_usd / 1_000_000.0
}

fn normalize_openai_messages(payload: &mut Value, gate: Gate) {
    if gate != Gate::Allowed {
        return;
    }
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(text) = message.get("content").and_then(string_content) else {
            continue;
        };
        let output = brief_bright_gone::normalize::normalize_with_detect(text, &Default::default());
        if output.changed
            && let Some(slot) = message.get_mut("content")
        {
            *slot = Value::String(output.text);
        }
    }
}

fn record_cost(
    state: &ProxyState,
    provider: Provider,
    model: &str,
    usage: brief_bright_gone::types::Usage,
) {
    let record = operations::CostRecord::from_usage(
        provider.clone(),
        model.to_owned(),
        usage,
        state.config.price_for(&provider),
    );
    if let Err(error) = operations::append_cost_record(&state.cost_ledger, &record) {
        tracing::warn!("could not append local cost record: {error}");
    }
}

fn decode_openai_response(bytes: &[u8], store: &Store) -> Vec<u8> {
    let Ok(mut response) = serde_json::from_slice::<Value>(bytes) else {
        return bytes.to_vec();
    };
    let Some(content) = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
    else {
        return bytes.to_vec();
    };
    let decoded = sigil::decode(content);
    if decoded == content
        || store
            .put_sigil_original(&decoded, content.as_bytes())
            .is_err()
    {
        return bytes.to_vec();
    }
    if let Some(slot) = response.pointer_mut("/choices/0/message/content") {
        *slot = Value::String(decoded);
    }
    serde_json::to_vec(&response).unwrap_or_else(|_| bytes.to_vec())
}

fn decode_anthropic_response(bytes: &[u8], store: &Store) -> Vec<u8> {
    let Ok(mut response) = serde_json::from_slice::<Value>(bytes) else {
        return bytes.to_vec();
    };
    let Some(blocks) = response.get_mut("content").and_then(Value::as_array_mut) else {
        return bytes.to_vec();
    };
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = block.get("text").and_then(string_content) else {
            continue;
        };
        let decoded = sigil::decode(text);
        if decoded != text
            && store.put_sigil_original(&decoded, text.as_bytes()).is_ok()
            && let Some(slot) = block.get_mut("text")
        {
            *slot = Value::String(decoded);
        }
    }
    serde_json::to_vec(&response).unwrap_or_else(|_| bytes.to_vec())
}

async fn handle_chat(
    State(state): State<Arc<ProxyState>>,
    headers: HeaderMap,
    Json(mut payload): Json<Value>,
) -> Response {
    if !client_authorized(&state, &headers) {
        return unauthorized();
    }
    substitute_openai_originals(&mut payload, &state.store);
    let predicted_savings = predicted_openai_normalization_savings(&payload, &state.config);
    let gate = compression_gate(&state.config.calibration, predicted_savings, 0);
    normalize_openai_messages(&mut payload, gate);
    if !operations::inject_and_verify_constraints(
        &mut payload,
        Provider::OpenAi,
        &state.config.protected_constraints,
    ) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":{"code":"constraint_verification_failed","message":"local protected constraints could not be verified"}}))).into_response();
    }
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let stream = payload
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if state.dry {
        return (StatusCode::OK, Json(serde_json::json!({"object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"[bbg dry-run]"},"finish_reason":"stop"}],"bbg":{"dry":true}}))).into_response();
    }
    let upstream = upstream_endpoint(&state.upstream, "chat/completions");
    let mut request = state
        .client
        .post(upstream)
        .headers(forwarded_headers(&headers))
        .json(&payload);
    if !state.key.is_empty() {
        request = request.bearer_auth(&state.key);
    }
    match request.send().await {
        Ok(response) if stream => streaming_response(response),
        Ok(response) => {
            let status = response.status();
            let upstream_headers = response.headers().clone();
            match response.bytes().await {
                Ok(bytes) => {
                    let decoded = decode_openai_response(&bytes, &state.store);
                    let mut output = response_from_upstream(status, &upstream_headers, decoded);
                    if let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                        && let Some(usage) = adapters::openai_usage(&value)
                    {
                        record_cost(&state, Provider::OpenAi, &model, usage.clone());
                        let headers = output.headers_mut();
                        if let Some(value) = usage.input_tokens { headers.insert("x-bbg-input-tokens", HeaderValue::from(value)); }
                        if let Some(value) = usage.output_tokens { headers.insert("x-bbg-output-tokens", HeaderValue::from(value)); }
                    }
                    output
                }
                Err(_) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error":{"code":"upstream_read","message":"upstream response could not be read"}}))).into_response(),
            }
        }
        Err(error) => redacted_error(&error),
    }
}

async fn handle_anthropic(
    State(state): State<Arc<ProxyState>>,
    headers: HeaderMap,
    Json(mut payload): Json<Value>,
) -> Response {
    if !client_authorized(&state, &headers) {
        return unauthorized();
    }
    // Constraint injection occurs before the final, stable cache placement.
    if !operations::inject_and_verify_constraints(
        &mut payload,
        Provider::Anthropic,
        &state.config.protected_constraints,
    ) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":{"code":"constraint_verification_failed","message":"local protected constraints could not be verified"}}))).into_response();
    }
    let gate = operations::cache_breakpoint_gate(&state.config.calibration);
    operations::place_anthropic_cache_breakpoint(&mut payload, gate);
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let stream = payload
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if state.dry {
        return (StatusCode::OK, Json(serde_json::json!({"type":"message","content":[{"type":"text","text":"[bbg dry-run]"}]}))).into_response();
    }
    let upstream = upstream_endpoint(&state.upstream, "messages");
    let mut request = state
        .client
        .post(upstream)
        .headers(forwarded_headers(&headers))
        .json(&payload)
        .header("anthropic-version", "2023-06-01");
    if !state.key.is_empty() {
        request = request.header("x-api-key", &state.key);
    }
    match request.send().await {
        Ok(response) if stream => streaming_response(response),
        Ok(response) => {
            let status = response.status();
            let upstream_headers = response.headers().clone();
            match response.bytes().await {
                Ok(bytes) => {
                    let decoded = decode_anthropic_response(&bytes, &state.store);
                    let mut output = response_from_upstream(status, &upstream_headers, decoded);
                    if let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                        && let Some(usage) = adapters::anthropic_usage(&value)
                    {
                        record_cost(&state, Provider::Anthropic, &model, usage.clone());
                        let headers = output.headers_mut();
                        if let Some(value) = usage.input_tokens { headers.insert("x-bbg-input-tokens", HeaderValue::from(value)); }
                        if let Some(value) = usage.output_tokens { headers.insert("x-bbg-output-tokens", HeaderValue::from(value)); }
                    }
                    output
                }
                Err(_) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error":{"code":"upstream_read","message":"upstream response could not be read"}}))).into_response(),
            }
        }
        Err(error) => redacted_error(&error),
    }
}

async fn get_models(State(state): State<Arc<ProxyState>>, headers: HeaderMap) -> Response {
    if !client_authorized(&state, &headers) {
        return unauthorized();
    }
    let upstream = upstream_endpoint(&state.upstream, "models");
    let mut request = state.client.get(upstream);
    if !state.key.is_empty() {
        request = request.bearer_auth(&state.key);
    }
    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let headers = response.headers().clone();
            match response.bytes().await {
                Ok(bytes) => response_from_upstream(status, &headers, bytes.to_vec()),
                Err(_) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error":{"code":"upstream_read","message":"upstream response could not be read"}}))).into_response(),
            }
        }
        Err(error) => redacted_error(&error),
    }
}

async fn health(State(state): State<Arc<ProxyState>>, headers: HeaderMap) -> Response {
    if !client_authorized(&state, &headers) {
        return unauthorized();
    }
    "ok".into_response()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let upstream_raw = env::var("BBG_UPSTREAM_URL")
        .or_else(|_| env::var("OPENAI_BASE_URL"))
        .unwrap_or_else(|_| "http://localhost:11434/v1".into());
    let upstream = validate_upstream_url(&upstream_raw).unwrap_or_else(|error| {
        eprintln!("error: BBG_UPSTREAM_URL: {error}");
        std::process::exit(2);
    });
    let key = env::var("BBG_UPSTREAM_KEY")
        .or_else(|_| env::var("OPENAI_API_KEY"))
        .unwrap_or_default();
    let port = env::var("BBG_PORT").unwrap_or_else(|_| "8088".into());
    let bind_raw = env::var("BBG_BIND").unwrap_or_else(|_| DEFAULT_BIND.into());
    let allow_non_loopback = env::var("BBG_ALLOW_NON_LOOPBACK").is_ok_and(|value| value == "1");
    let proxy_token = env::var("BBG_PROXY_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());
    let bind = resolve_bind(&bind_raw, allow_non_loopback, proxy_token.as_deref()).unwrap_or_else(
        |error| {
            eprintln!("error: {error}");
            std::process::exit(2);
        },
    );
    let dry = env::var("BBG_DRY").is_ok_and(|value| value == "1");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build http client");
    let store_root = env::var("BBG_STORE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".bbg-store"));
    let store = Store::open(&store_root).expect("open CCR store");
    let config_path = env::var("BBG_CONFIG").ok().map(PathBuf::from);
    let config = LocalConfig::load(config_path.as_deref()).unwrap_or_else(|error| {
        eprintln!("error: local BBG_CONFIG: {error}");
        std::process::exit(2);
    });
    let state = Arc::new(ProxyState {
        upstream,
        key,
        proxy_token,
        dry,
        client,
        store,
        config,
        cost_ledger: store_root.join("ledger").join("costs.jsonl"),
    });
    let app = Router::new()
        .route("/v1/chat/completions", post(handle_chat))
        .route("/v1/messages", post(handle_anthropic))
        .route("/v1/models", axum::routing::get(get_models))
        .route("/health", axum::routing::get(health))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind((
        bind,
        port.parse::<u16>().unwrap_or_else(|_| {
            eprintln!("error: BBG_PORT must be a valid TCP port");
            std::process::exit(2);
        }),
    ))
    .await
    .expect("bind failed");
    axum::serve(listener, app).await.expect("server error");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn store() -> Store {
        Store::open(std::env::temp_dir().join(format!(
                "bbg-proxy-test-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
        .unwrap()
    }

    #[test]
    fn upstream_validation_allows_https_and_explicit_local_ollama_only() {
        let local = validate_upstream_url("http://127.0.0.1:11434/v1").unwrap();
        assert_eq!(
            upstream_endpoint(&local, "chat/completions").as_str(),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert!(validate_upstream_url("http://localhost:11434/v1").is_ok());
        assert!(validate_upstream_url("http://[::1]:11434/v1").is_ok());
        assert!(validate_upstream_url("https://api.example.test/v1").is_ok());
        assert!(validate_upstream_url("http://api.example.test/v1").is_err());
        assert!(validate_upstream_url("https://user:secret@api.example.test/v1").is_err());
        assert!(validate_upstream_url("https://api.example.test/v1?token=secret").is_err());
        assert!(validate_upstream_url("file:///tmp/socket").is_err());
    }

    #[test]
    fn non_loopback_bind_requires_explicit_opt_in_and_authentication() {
        assert_eq!(DEFAULT_BIND, "127.0.0.1");
        assert_eq!(
            resolve_bind(DEFAULT_BIND, false, None).unwrap(),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
        assert!(resolve_bind("0.0.0.0", false, Some("token")).is_err());
        assert!(resolve_bind("0.0.0.0", true, None).is_err());
        assert_eq!(
            resolve_bind("0.0.0.0", true, Some("token")).unwrap(),
            "0.0.0.0".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn calibrated_gates_can_enable_real_transforms() {
        let payload = serde_json::json!({
            "messages": [{"role": "user", "content": "please   fix this, thank you"}]
        });
        let config = LocalConfig {
            pricing: [(
                "openai".into(),
                brief_bright_gone::operations::ProviderPricing {
                    input_per_million_usd: 1.0,
                    output_per_million_usd: 0.0,
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            calibration: brief_bright_gone::operations::Calibration {
                samples: 20,
                cache_hit_rate: 0.5,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(predicted_openai_normalization_savings(&payload, &config) > 0.0);
        assert_eq!(
            compression_gate(
                &config.calibration,
                predicted_openai_normalization_savings(&payload, &config),
                0,
            ),
            Gate::Allowed
        );
        assert_eq!(
            operations::cache_breakpoint_gate(&config.calibration),
            Gate::Allowed
        );
    }

    #[test]
    fn forwards_only_safe_end_to_end_headers() {
        let mut inbound = HeaderMap::new();
        inbound.insert(
            "authorization",
            HeaderValue::from_static("Bearer client-secret"),
        );
        inbound.insert("cookie", HeaderValue::from_static("session=secret"));
        inbound.insert("accept", HeaderValue::from_static("text/event-stream"));
        let outbound = forwarded_headers(&inbound);
        assert_eq!(outbound.get("accept").unwrap(), "text/event-stream");
        assert!(outbound.get("authorization").is_none());
        assert!(outbound.get("cookie").is_none());
    }

    #[test]
    fn openai_response_decode_and_substitution_restore_original_bytes() {
        let store = store();
        let source = serde_json::to_vec(
            &serde_json::json!({"choices":[{"message":{"content":"§ Status\n. done"}}]}),
        )
        .unwrap();
        let decoded = decode_openai_response(&source, &store);
        let response: Value = serde_json::from_slice(&decoded).unwrap();
        let markdown = response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap();
        let mut request = serde_json::json!({"messages":[{"role":"assistant","content":markdown}]});
        substitute_openai_originals(&mut request, &store);
        assert_eq!(request["messages"][0]["content"], "§ Status\n. done");
        assert_ne!(decoded, source);
    }

    #[test]
    fn anthropic_response_decode_stores_reversible_original() {
        let store = store();
        let source = serde_json::to_vec(
            &serde_json::json!({"content":[{"type":"text","text":"§ Status\n. done"}]}),
        )
        .unwrap();
        let decoded = decode_anthropic_response(&source, &store);
        let response: Value = serde_json::from_slice(&decoded).unwrap();
        let markdown = response["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            store.get_sigil_original(markdown).unwrap(),
            Some("§ Status\n. done".into())
        );
    }
}
