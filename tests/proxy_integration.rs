use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
};
use brief_bright_gone::{
    compress::{Metadata, Source, ToolKind, unix_now_secs},
    operations::{Calibration, LocalConfig},
    proxy::{
        LocalToolResultAttestation, LocalToolResultAttestations, ProxySettings, ToolResultLocator,
        build_router_with_tool_result_attestations,
    },
    store::Store,
};
use futures_util::{StreamExt, stream};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct RecordedRequest {
    path: String,
    headers: HeaderMap,
    body: Value,
}

type Records = Arc<Mutex<Vec<RecordedRequest>>>;

async fn sse_provider(
    State(records): State<Records>,
    uri: Uri,
    Json(body): Json<Value>,
) -> Response {
    records.lock().unwrap().push(RecordedRequest {
        path: uri.path().to_owned(),
        headers: HeaderMap::new(),
        body: body.clone(),
    });
    match body.get("scenario").and_then(Value::as_str) {
        Some("upstream-error") => (StatusCode::BAD_GATEWAY, "upstream failed").into_response(),
        Some("timeout") => {
            tokio::time::sleep(Duration::from_millis(100)).await;
            (
                StatusCode::OK,
                [("content-type", "text/event-stream")],
                "data: [DONE]\n\n",
            )
                .into_response()
        }
        Some("body-failure") => {
            let chunks = stream::iter(vec![
                Ok::<_, std::io::Error>(Bytes::from_static(
                    b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
                )),
                Err(std::io::Error::other("upstream body aborted")),
            ]);
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(chunks))
                .unwrap()
        }
        Some("truncated") => (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            "data: {\"choices\":[{\"delta\":{\"content\":\"§ Status\"}}]}\n\n",
        )
            .into_response(),
        Some("cancellable") => {
            let chunks = stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from_static(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
            ))])
            .chain(stream::once(async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok::<_, std::io::Error>(Bytes::from_static(b"data: [DONE]\n\n"))
            }));
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(chunks))
                .unwrap()
        }
        _ => {
            let chunks = stream::iter(vec![
                Ok::<_, std::io::Error>(Bytes::from(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"§ Status\\n\"}}]}\n\n",
                )),
                Ok(Bytes::from_static(
                    b"data: {\"choices\":[{\"delta\":{\"content\":\". done\"}}]}\n\n",
                )),
                Ok(Bytes::from_static(b"data: [DONE]\n\n")),
            ]);
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(chunks))
                .unwrap()
        }
    }
}

async fn anthropic_sse_provider(
    State(records): State<Records>,
    uri: Uri,
    Json(body): Json<Value>,
) -> Response {
    records.lock().unwrap().push(RecordedRequest {
        path: uri.path().to_owned(),
        headers: HeaderMap::new(),
        body,
    });
    // A realistic mixed stream: a thinking delta, a sigil text delta split
    // across two chunks, then the terminal events.
    let stream = stream::iter(vec![
        Ok::<_, std::io::Error>(Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"pondering\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"\xc2\xa7 Status\\n\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\". done\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":9}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )),
    ]);
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn sigil_response_provider(
    State(records): State<Records>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    records.lock().unwrap().push(RecordedRequest {
        path: uri.path().to_owned(),
        headers,
        body,
    });
    Json(serde_json::json!({
        "object": "chat.completion",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "§ Status\n. done"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 11, "completion_tokens": 7}
    }))
}

async fn mock_provider(
    State(records): State<Records>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    records.lock().unwrap().push(RecordedRequest {
        path: uri.path().to_owned(),
        headers,
        body,
    });
    if uri.path().ends_with("/messages") {
        Json(serde_json::json!({
            "type": "message",
            "content": [{"type": "text", "text": "plain response"}],
            "usage": {"input_tokens": 11, "output_tokens": 7}
        }))
    } else {
        Json(serde_json::json!({
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "plain response"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 11, "completion_tokens": 7}
        }))
    }
}

fn temporary_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "bbg-{label}-{}-{nonce}-{sequence}",
        std::process::id()
    ))
}

async fn bind(app: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, task)
}

async fn start_proxy(
    upstream: SocketAddr,
    config: LocalConfig,
) -> (SocketAddr, tokio::task::JoinHandle<()>, PathBuf, Store) {
    start_proxy_with_attestations(upstream, config, LocalToolResultAttestations::default()).await
}

async fn start_proxy_with_attestations(
    upstream: SocketAddr,
    config: LocalConfig,
    attestations: LocalToolResultAttestations,
) -> (SocketAddr, tokio::task::JoinHandle<()>, PathBuf, Store) {
    start_proxy_with_timeout(upstream, config, attestations, Duration::from_secs(2)).await
}

async fn start_proxy_with_timeout(
    upstream: SocketAddr,
    config: LocalConfig,
    attestations: LocalToolResultAttestations,
    timeout: Duration,
) -> (SocketAddr, tokio::task::JoinHandle<()>, PathBuf, Store) {
    start_proxy_with_args(upstream, config, attestations, timeout, None).await
}

async fn start_proxy_with_args(
    upstream: SocketAddr,
    config: LocalConfig,
    attestations: LocalToolResultAttestations,
    timeout: Duration,
    proxy_token: Option<&str>,
) -> (SocketAddr, tokio::task::JoinHandle<()>, PathBuf, Store) {
    let root = temporary_root("proxy-integration");
    let store = Store::open(&root).unwrap();
    let settings = ProxySettings::new(
        &format!("http://{upstream}/v1"),
        "local-upstream-secret".into(),
        proxy_token.map(str::to_owned),
        false,
        timeout,
        root.join("ledger").join("costs.jsonl"),
        root.join("ledger").join("transcripts.jsonl"),
    )
    .unwrap();
    let (address, task) = bind(build_router_with_tool_result_attestations(
        settings,
        store.clone(),
        config,
        attestations,
    ))
    .await;
    (address, task, root, store)
}

fn attestation(content: &str, locator: ToolResultLocator) -> LocalToolResultAttestation {
    LocalToolResultAttestation {
        digest: format!("{:x}", Sha256::digest(content.as_bytes())),
        locator,
        metadata: Metadata {
            source: Source::ToolResult,
            kind: ToolKind::Json,
            captured_at_secs: Some(unix_now_secs()),
            protected: false,
            in_recent_window: false,
        },
    }
}

fn stop(task: tokio::task::JoinHandle<()>, root: PathBuf) {
    task.abort();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn openai_proxy_uses_local_credential_and_injects_local_constraints() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(mock_provider))
            .with_state(records.clone()),
    )
    .await;
    let config = LocalConfig {
        protected_constraints: "do not disclose local rules".into(),
        ..Default::default()
    };
    let (proxy, proxy_task, root, _) = start_proxy(upstream, config).await;

    let response = reqwest::Client::new()
        .post(format!("http://{proxy}/v1/chat/completions"))
        .header("authorization", "Bearer client-secret")
        .header("cookie", "session=client-secret")
        .header("accept", "application/json")
        .json(&serde_json::json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    assert_eq!(response.headers()["x-bbg-input-tokens"], "11");
    assert_eq!(response.headers()["x-bbg-output-tokens"], "7");
    let request = records.lock().unwrap().pop().unwrap();
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(
        request.headers["authorization"],
        "Bearer local-upstream-secret"
    );
    assert!(request.headers.get("cookie").is_none());
    assert_eq!(
        request.body["messages"][0],
        serde_json::json!({"role":"system","content":"[bbg:local-protected-constraints]\ndo not disclose local rules"})
    );
    assert_eq!(request.body["messages"][1]["content"], "hello");

    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn anthropic_proxy_preserves_system_and_uses_local_credential() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/messages", post(mock_provider))
            .with_state(records.clone()),
    )
    .await;
    let config = LocalConfig {
        protected_constraints: "do not disclose local rules".into(),
        calibration: Calibration {
            samples: 20,
            cache_hit_rate: 0.5,
            ..Default::default()
        },
        ..Default::default()
    };
    let (proxy, proxy_task, root, store) = start_proxy(upstream, config).await;

    store
        .put_sigil_original("agent-visible markdown", "§ Status\n. done".as_bytes())
        .unwrap();
    let response = reqwest::Client::new()
        .post(format!("http://{proxy}/v1/messages"))
        .header("x-api-key", "client-secret")
        .json(&serde_json::json!({
            "model": "test-model",
            "system": "retain this exactly",
            "messages": [
                {"role": "assistant", "content": [{"type":"text","text":"agent-visible markdown"}]},
                {"role": "user", "content": "hello"}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    assert_eq!(response.headers()["x-bbg-input-tokens"], "11");
    let request = records.lock().unwrap().pop().unwrap();
    assert_eq!(request.path, "/v1/messages");
    assert_eq!(request.headers["x-api-key"], "local-upstream-secret");
    assert_eq!(request.headers["anthropic-version"], "2023-06-01");
    assert_eq!(
        request.body["system"][0]["text"],
        "[bbg:local-protected-constraints]\ndo not disclose local rules"
    );
    assert_eq!(
        request.body["system"][0]["cache_control"]["type"],
        "ephemeral"
    );
    assert_eq!(
        request.body["system"][1],
        serde_json::json!({"type":"text","text":"retain this exactly"})
    );
    assert_eq!(
        request.body["messages"][0]["content"][0]["text"],
        "§ Status\n. done"
    );

    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn sse_proxy_preserves_order_and_fails_open_without_retry() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(sse_provider))
            .with_state(records.clone()),
    )
    .await;
    let (proxy, proxy_task, root, _) = start_proxy(upstream, LocalConfig::default()).await;
    let client = reqwest::Client::new();

    let success = client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .json(&serde_json::json!({"stream":true,"messages":[{"role":"user","content":"hello"}]}))
        .send()
        .await
        .unwrap();
    assert!(success.status().is_success());
    let success_body = success.text().await.unwrap();
    let heading = success_body.find("## Status").unwrap();
    let terminal = success_body.find("**Done.** done").unwrap();
    assert!(
        heading < terminal,
        "decoded chunks must retain provider order"
    );
    assert_eq!(success_body.matches("data: [DONE]").count(), 1);

    for scenario in ["upstream-error", "truncated", "body-failure"] {
        let response = client
            .post(format!("http://{proxy}/v1/chat/completions"))
            .json(&serde_json::json!({"stream":true,"scenario":scenario,"messages":[{"role":"user","content":"hello"}]}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        if scenario == "upstream-error" {
            assert_eq!(status, StatusCode::BAD_GATEWAY);
            assert_eq!(body, "upstream failed");
        } else {
            assert!(
                !body.contains("data: [DONE]"),
                "{scenario} must not synthesize success"
            );
        }
    }
    assert_eq!(
        records.lock().unwrap().len(),
        4,
        "no scenario may retry upstream"
    );
    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn anthropic_sse_decodes_text_and_passes_non_text_deltas_through() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/messages", post(anthropic_sse_provider))
            .with_state(records.clone()),
    )
    .await;
    let (proxy, proxy_task, root, store) = start_proxy(upstream, LocalConfig::default()).await;

    let response = reqwest::Client::new()
        .post(format!("http://{proxy}/v1/messages"))
        .json(&serde_json::json!({"stream":true,"messages":[{"role":"user","content":"hello"}]}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body = response.text().await.unwrap();

    // Text block decoded; thinking delta forwarded verbatim (task 2).
    assert!(body.contains("## Status"), "text decoded: {body}");
    assert!(body.contains("**Done.** done"));
    assert!(body.contains("\"thinking_delta\""));
    assert!(!body.contains("\u{a7} Status"));
    assert!(body.ends_with("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));

    // Reverse mapping is stored so a later request restores the sigil original.
    let decoded = "## Status\n**Done.** done";
    assert_eq!(
        store.get_sigil_original(decoded).unwrap(),
        Some("\u{a7} Status\n. done".into())
    );

    // The streamed cost record keeps the input tokens from message_start, not
    // just the output tokens from message_delta.
    let ledger = std::fs::read_to_string(root.join("ledger").join("costs.jsonl")).unwrap();
    let record: Value = serde_json::from_str(ledger.lines().next().unwrap()).unwrap();
    assert_eq!(record["usage"]["input_tokens"], 5);
    assert_eq!(record["usage"]["output_tokens"], 9);

    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn sse_upstream_timeout_returns_gateway_timeout_once() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(sse_provider))
            .with_state(records.clone()),
    )
    .await;
    let (proxy, proxy_task, root, _) = start_proxy_with_timeout(
        upstream,
        LocalConfig::default(),
        LocalToolResultAttestations::default(),
        Duration::from_millis(20),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://{proxy}/v1/chat/completions"))
        .json(&serde_json::json!({"stream":true,"scenario":"timeout","messages":[{"role":"user","content":"hello"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(records.lock().unwrap().len(), 1, "timeout must not retry");
    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn cancelling_after_sse_headers_does_not_retry_upstream() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(sse_provider))
            .with_state(records.clone()),
    )
    .await;
    let (proxy, proxy_task, root, _) = start_proxy(upstream, LocalConfig::default()).await;
    let response = reqwest::Client::new()
        .post(format!("http://{proxy}/v1/chat/completions"))
        .json(&serde_json::json!({"stream":true,"scenario":"cancellable","messages":[{"role":"user","content":"hello"}]}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    drop(response);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        records.lock().unwrap().len(),
        1,
        "cancellation must not retry upstream"
    );
    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn transcript_records_real_session_ids_and_skips_tool_turns() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(mock_provider))
            .with_state(records.clone()),
    )
    .await;
    let (proxy, proxy_task, root, _) = start_proxy(upstream, LocalConfig::default()).await;
    let client = reqwest::Client::new();

    // A genuine user turn.
    client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .json(
            &serde_json::json!({"model":"m","messages":[{"role":"user","content":"hello there"}]}),
        )
        .send()
        .await
        .unwrap();
    // A request whose last message is a tool result must not be captured as user.
    client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .json(&serde_json::json!({"model":"m","messages":[
            {"role":"user","content":"earlier"},
            {"role":"tool","content":"tool output that must not become a user record"}
        ]}))
        .send()
        .await
        .unwrap();

    let ledger = std::fs::read_to_string(root.join("ledger").join("transcripts.jsonl")).unwrap();
    let transcripts: Vec<Value> = ledger
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let user_records: Vec<&Value> = transcripts
        .iter()
        .filter(|record| record["role"] == "user")
        .collect();

    // Only the genuine user turn was captured.
    assert_eq!(user_records.len(), 1);
    assert_eq!(user_records[0]["content"], "hello there");
    // The session id is a real Registry hash, not the old hardcoded "proxy".
    let session_id = user_records[0]["session_id"].as_str().unwrap();
    assert_ne!(session_id, "proxy");
    assert_eq!(session_id.len(), 64);
    // Tool output never leaks into a user record.
    assert!(
        !transcripts.iter().any(|record| record["role"] == "user"
            && record["content"]
                .as_str()
                .unwrap_or("")
                .contains("tool output")),
        "tool output captured as user turn"
    );

    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn transcript_stores_raw_sigil_and_lints_only_the_assistant_turn() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(sigil_response_provider))
            .with_state(records.clone()),
    )
    .await;
    let (proxy, proxy_task, root, _) = start_proxy(upstream, LocalConfig::default()).await;
    let client = reqwest::Client::new();

    // A user turn that would trip G1 (no terminal) and R3 (actionable, no
    // severity marker) if it were ever linted with the response rules.
    let response = client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .json(&serde_json::json!({"model":"m","messages":[
            {"role":"user","content":"please fix this, it must run correctly"}
        ]}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    // The client-visible response is decoded Markdown.
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "## Status\n**Done.** done"
    );

    let ledger = std::fs::read_to_string(root.join("ledger").join("transcripts.jsonl")).unwrap();
    let transcripts: Vec<Value> = ledger
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(transcripts.len(), 2);

    let user = transcripts.iter().find(|r| r["role"] == "user").unwrap();
    let assistant = transcripts
        .iter()
        .find(|r| r["role"] == "assistant")
        .unwrap();

    // Assistant content in the transcript is the model's raw sigil output, not
    // the decoded Markdown the client received (design.md §5.8).
    assert_eq!(assistant["content"], "§ Status\n. done");
    // The raw form has a valid terminal, so it must not trip G1.
    assert!(
        assistant["lint"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["rule"] != "G1"),
        "assistant lint: {:?}",
        assistant["lint"]
    );

    // The user turn is never linted at all, even though its raw text would
    // trip G1/R3 under the response rules.
    assert_eq!(
        user["lint"].as_array().unwrap().len(),
        0,
        "user turn was linted: {:?}",
        user["lint"]
    );

    // The cost record for this turn carries the same session id as the
    // transcript, so `bbg benchmark report` can join dollars onto turns.
    let cost_ledger = std::fs::read_to_string(root.join("ledger").join("costs.jsonl")).unwrap();
    let cost: Value = serde_json::from_str(cost_ledger.lines().next().unwrap()).unwrap();
    assert_eq!(cost["session_id"], user["session_id"]);

    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn proxy_token_denies_missing_and_wrong_bearer_and_accepts_exact() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(mock_provider))
            .with_state(records.clone()),
    )
    .await;
    let (proxy, proxy_task, root, _) = start_proxy_with_args(
        upstream,
        LocalConfig::default(),
        LocalToolResultAttestations::default(),
        Duration::from_secs(2),
        Some("s3cr3t"),
    )
    .await;
    let client = reqwest::Client::new();
    let url = format!("http://{proxy}/v1/chat/completions");
    let payload = serde_json::json!({ "model": "m", "messages": [
        {"role": "user", "content": "hello"}
    ] });

    // No token -> 401, and the upstream must never be reached.
    let missing = client.post(&url).json(&payload).send().await.unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    // Wrong token -> 401.
    let wrong = client
        .post(&url)
        .bearer_auth("wrong")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    // Exact token -> forwarded upstream.
    let ok = client
        .post(&url)
        .bearer_auth("s3cr3t")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert!(ok.status().is_success());

    assert_eq!(
        records.lock().unwrap().len(),
        1,
        "only the authorized request may reach upstream"
    );
    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn tool_results_pass_through_without_local_attestation() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(mock_provider))
            .with_state(records.clone()),
    )
    .await;
    let original = "{\n  \"untrusted\": true\n}";
    let (proxy, proxy_task, root, store) = start_proxy(upstream, LocalConfig::default()).await;

    let response = reqwest::Client::new()
        .post(format!("http://{proxy}/v1/chat/completions"))
        .header("x-bbg-tool-metadata", "not trusted")
        .json(&serde_json::json!({
            "model": "test-model",
            "bbg_tool_metadata": {"kind": "json"},
            "messages": [{"role": "tool", "content": original}]
        }))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    let request = records.lock().unwrap().pop().unwrap();
    assert_eq!(request.body["messages"][0]["content"], original);
    assert!(store.receipts().unwrap().is_empty());
    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn locator_mismatched_local_attestation_passes_through() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(mock_provider))
            .with_state(records.clone()),
    )
    .await;
    let original = "{\n  \"result\": 7\n}";
    let attestations = LocalToolResultAttestations::new([attestation(
        original,
        ToolResultLocator::OpenAi { message_index: 1 },
    )]);
    let (proxy, proxy_task, root, store) =
        start_proxy_with_attestations(upstream, LocalConfig::default(), attestations).await;

    let response = reqwest::Client::new()
        .post(format!("http://{proxy}/v1/chat/completions"))
        .json(&serde_json::json!({
            "model": "test-model",
            "messages": [{"role": "tool", "content": original}]
        }))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    let request = records.lock().unwrap().pop().unwrap();
    assert_eq!(request.body["messages"][0]["content"], original);
    assert!(store.receipts().unwrap().is_empty());
    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn digest_mismatched_local_attestation_passes_through() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(mock_provider))
            .with_state(records.clone()),
    )
    .await;
    let original = "{\n  \"result\": 8\n}";
    let mut mismatched = attestation(original, ToolResultLocator::OpenAi { message_index: 0 });
    mismatched.digest = format!("{:x}", Sha256::digest(b"different local bytes"));
    let (proxy, proxy_task, root, store) = start_proxy_with_attestations(
        upstream,
        LocalConfig::default(),
        LocalToolResultAttestations::new([mismatched]),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("http://{proxy}/v1/chat/completions"))
        .json(&serde_json::json!({
            "model": "test-model",
            "messages": [{"role": "tool", "content": original}]
        }))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    let request = records.lock().unwrap().pop().unwrap();
    assert_eq!(request.body["messages"][0]["content"], original);
    assert!(store.receipts().unwrap().is_empty());
    stop(proxy_task, root);
    upstream_task.abort();
}

#[tokio::test]
async fn locally_attested_openai_and_anthropic_tool_results_transform_and_receipt() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let (upstream, upstream_task) = bind(
        Router::new()
            .route("/v1/chat/completions", post(mock_provider))
            .route("/v1/messages", post(mock_provider))
            .with_state(records.clone()),
    )
    .await;
    let openai_original = "{\n  \"openai\": [1, 2]\n}";
    let anthropic_original = "{\n  \"anthropic\": [3, 4]\n}";
    let attestations = LocalToolResultAttestations::new([
        attestation(
            openai_original,
            ToolResultLocator::OpenAi { message_index: 0 },
        ),
        attestation(
            anthropic_original,
            ToolResultLocator::Anthropic {
                message_index: 0,
                block_index: 0,
            },
        ),
    ]);
    let (proxy, proxy_task, root, store) =
        start_proxy_with_attestations(upstream, LocalConfig::default(), attestations).await;
    let client = reqwest::Client::new();

    let openai = client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .json(&serde_json::json!({
            "model": "test-model",
            "messages": [{"role": "tool", "content": openai_original}]
        }))
        .send()
        .await
        .unwrap();
    assert!(openai.status().is_success());
    let anthropic = client
        .post(format!("http://{proxy}/v1/messages"))
        .json(&serde_json::json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": [{"type":"tool_result","content": anthropic_original}]}]
        }))
        .send()
        .await
        .unwrap();
    assert!(anthropic.status().is_success());

    let requests = records.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let openai_request = requests
        .iter()
        .find(|request| request.path.ends_with("chat/completions"))
        .unwrap();
    assert!(
        openai_request.body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("[bbg:toon:")
    );
    let anthropic_request = requests
        .iter()
        .find(|request| request.path.ends_with("messages"))
        .unwrap();
    assert!(
        anthropic_request.body["messages"][0]["content"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("[bbg:toon:")
    );
    drop(requests);
    assert_eq!(
        store
            .get(&format!("{:x}", Sha256::digest(openai_original)))
            .unwrap(),
        Some(openai_original.into())
    );
    assert_eq!(
        store
            .get(&format!("{:x}", Sha256::digest(anthropic_original)))
            .unwrap(),
        Some(anthropic_original.into())
    );
    assert_eq!(store.receipts().unwrap().len(), 2);
    stop(proxy_task, root);
    upstream_task.abort();
}
