//! Testable local OpenAI Chat Completions and Anthropic Messages proxy runtime.

use crate::{
    adapters,
    compress::{self, Metadata},
    operations::{self, Gate, LocalConfig, compression_gate},
    session::Registry,
    sigil,
    store::Store,
    types::Provider,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use futures_util::{StreamExt, stream};
use reqwest::Url;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const FORWARDED_HEADERS: [&str; 3] = ["accept", "content-type", "user-agent"];
const TOOL_RESULT_MAX_AGE_SECS: u64 = 300;
pub const DEFAULT_BIND: &str = "127.0.0.1";

/// A locally-owned location for a tool result. This is never read from wire
/// data: it only identifies where an independently attested digest may apply.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolResultLocator {
    OpenAi {
        message_index: usize,
    },
    Anthropic {
        message_index: usize,
        block_index: usize,
    },
}

/// A local attestation is accepted only when both its locator and its content
/// digest match the outbound request. It deliberately has no deserialization
/// implementation, so request/provider JSON cannot manufacture one.
#[derive(Debug, Clone)]
pub struct LocalToolResultAttestation {
    pub digest: String,
    pub locator: ToolResultLocator,
    pub metadata: Metadata,
}

/// Immutable owner-provided metadata snapshot. An empty snapshot is the safe
/// default and makes tool-result compression a no-op.
#[derive(Debug, Clone, Default)]
pub struct LocalToolResultAttestations {
    by_digest: HashMap<String, LocalToolResultAttestation>,
}

impl LocalToolResultAttestations {
    pub fn new(attestations: impl IntoIterator<Item = LocalToolResultAttestation>) -> Self {
        let mut by_digest = HashMap::new();
        let mut conflicts = HashSet::new();
        for attestation in attestations {
            // Conflicting local attestations fail closed rather than letting a
            // construction-order detail select metadata.
            if !conflicts.insert(attestation.digest.clone()) {
                by_digest.remove(&attestation.digest);
            } else {
                by_digest.insert(attestation.digest.clone(), attestation);
            }
        }
        Self { by_digest }
    }

    fn is_empty(&self) -> bool {
        self.by_digest.is_empty()
    }

    fn metadata_for(&self, locator: &ToolResultLocator, bytes: &[u8]) -> Option<Metadata> {
        let digest = content_digest(bytes);
        let attestation = self.by_digest.get(&digest)?;
        (attestation.digest == digest && &attestation.locator == locator)
            .then_some(attestation.metadata)
    }
}

fn content_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct ProxyState {
    upstream: Url,
    key: String,
    proxy_token: Option<String>,
    dry: bool,
    client: reqwest::Client,
    store: Store,
    config: LocalConfig,
    tool_result_attestations: LocalToolResultAttestations,
    sessions: Mutex<Registry>,
    cost_ledger: PathBuf,
    transcript_ledger: PathBuf,
}

/// Injected runtime settings for production startup and hermetic integration
/// tests. Construction validates the upstream and disables redirects.
#[derive(Clone)]
pub struct ProxySettings {
    upstream: Url,
    key: String,
    proxy_token: Option<String>,
    dry: bool,
    client: reqwest::Client,
    cost_ledger: PathBuf,
    transcript_ledger: PathBuf,
}

impl ProxySettings {
    pub fn new(
        upstream_url: &str,
        key: String,
        proxy_token: Option<String>,
        dry: bool,
        timeout: Duration,
        cost_ledger: PathBuf,
        transcript_ledger: PathBuf,
    ) -> Result<Self, String> {
        Ok(Self {
            upstream: validate_upstream_url(upstream_url)?,
            key,
            proxy_token,
            dry,
            client: reqwest::Client::builder()
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "could not build upstream client".to_owned())?,
            cost_ledger,
            transcript_ledger,
        })
    }
}

pub fn validate_upstream_url(raw: &str) -> Result<Url, String> {
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

pub fn resolve_bind(
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
    for (name, value) in headers {
        if name.as_str().starts_with("anthropic-") {
            forwarded.insert(name.clone(), value.clone());
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

#[derive(Clone, Copy)]
enum StreamingProvider {
    OpenAi,
    Anthropic,
}

struct SseEvent {
    raw: String,
    ending: String,
    data: Option<String>,
    data_range: Option<std::ops::Range<usize>>,
}

impl SseEvent {
    fn replace_data(&mut self, data: &str) {
        let Some(range) = self.data_range.clone() else {
            return;
        };
        let start = range.start;
        self.raw.replace_range(range, data);
        self.data_range = Some(start..start + data.len());
        self.data = Some(data.to_owned());
    }

    fn render(self) -> String {
        format!("{}{}", self.raw, self.ending)
    }
}

/// Locate the earliest complete SSE record boundary in a byte buffer, returning
/// `(record_len, ending_len)`. Boundaries are ASCII (`\n\n` or `\r\n\r\n`) so
/// they are safe to find in bytes even when a chunk splits a multibyte char.
fn next_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let crlf = find_subslice(buffer, b"\r\n\r\n");
    let lf = find_subslice(buffer, b"\n\n");
    match (crlf, lf) {
        (Some(crlf), Some(lf)) if crlf <= lf => Some((crlf, 4)),
        (_, Some(lf)) => Some((lf, 2)),
        (Some(crlf), None) => Some((crlf, 4)),
        (None, None) => None,
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parse a single complete SSE record. A malformed record returns `None` and is
/// forwarded verbatim by the streaming decoder rather than aborting the stream.
fn parse_sse_record(record: &str, ending: &str) -> Option<SseEvent> {
    let mut data = None;
    let mut data_range = None;
    let mut offset = 0;
    for line in record.split_inclusive('\n') {
        let line_end = offset + line.len();
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        let bare = without_newline
            .strip_suffix('\r')
            .unwrap_or(without_newline);
        if let Some(value) = bare.strip_prefix("data:") {
            if data.is_some() {
                return None;
            }
            let value = value.strip_prefix(' ').unwrap_or(value);
            let start = offset + bare.len() - value.len();
            data = Some(value.to_owned());
            data_range = Some(start..start + value.len());
        } else if !bare.is_empty()
            && !bare.starts_with(':')
            && !bare.starts_with("event:")
            && !bare.starts_with("id:")
            && !bare.starts_with("retry:")
        {
            return None;
        }
        offset = line_end;
    }
    Some(SseEvent {
        raw: record.to_owned(),
        ending: ending.to_owned(),
        data,
        data_range,
    })
}

fn replace_openai_event_text(event: &mut SseEvent, replacement: &str) -> Option<()> {
    let mut value: Value = serde_json::from_str(event.data.as_deref()?).ok()?;
    let choices = value.get_mut("choices")?.as_array_mut()?;
    let mut text_slots = choices
        .iter_mut()
        .filter_map(|choice| choice.pointer_mut("/delta/content"));
    let slot = text_slots.next()?;
    if text_slots.next().is_some() || !slot.is_string() {
        return None;
    }
    *slot = Value::String(replacement.to_owned());
    event.replace_data(&serde_json::to_string(&value).ok()?);
    Some(())
}

fn replace_anthropic_event_text(event: &mut SseEvent, replacement: &str) -> Option<()> {
    let mut value: Value = serde_json::from_str(event.data.as_deref()?).ok()?;
    let slot = value.pointer_mut("/delta/text")?;
    if !slot.is_string() {
        return None;
    }
    *slot = Value::String(replacement.to_owned());
    event.replace_data(&serde_json::to_string(&value).ok()?);
    Some(())
}

/// Per-stream (OpenAI) or per-block (Anthropic) incremental sigil decode state.
/// `decoder` withholds only pending lines/table runs; `original`/`decoded`
/// accumulate the full mapping so the reverse substitution can be stored once
/// the stream (or block) completes.
#[derive(Default)]
struct TextBlock {
    decoder: sigil::Decoder,
    original: String,
    decoded: String,
    flushed: bool,
}

/// Streaming SSE transformer. Decodes provider deltas as complete events arrive
/// and forwards them immediately, so time-to-first-token tracks the upstream
/// rather than full generation. Non-text and unparseable events pass through
/// verbatim; the decoder's held-back tail is emitted as a synthetic content
/// event just before the stream's (or block's) terminal event.
struct SseStreamDecoder {
    provider: StreamingProvider,
    state: Arc<ProxyState>,
    model: String,
    session_id: String,
    buffer: Vec<u8>,
    openai: TextBlock,
    anthropic: HashMap<u64, TextBlock>,
    usage: Option<crate::types::Usage>,
    /// Top-level fields (id/object/model/created/…) of a real OpenAI chunk,
    /// echoed into synthetic tail events so strict SDKs see a well-formed chunk.
    openai_template: Option<serde_json::Map<String, Value>>,
    io: Vec<IoJob>,
}

/// A blocking side effect (each does a synchronous fsync) queued during
/// transform and executed off the async reactor by the stream driver.
enum IoJob {
    StoreOriginal {
        decoded: String,
        original: String,
    },
    Cost {
        protocol: Provider,
        model: String,
        session_id: String,
        usage: crate::types::Usage,
    },
    Transcript {
        session_id: String,
        content: String,
    },
}

impl SseStreamDecoder {
    fn new(
        provider: StreamingProvider,
        state: Arc<ProxyState>,
        model: String,
        session_id: String,
    ) -> Self {
        Self {
            provider,
            state,
            model,
            session_id,
            buffer: Vec::new(),
            openai: TextBlock::default(),
            anthropic: HashMap::new(),
            usage: None,
            openai_template: None,
            io: Vec::new(),
        }
    }

    /// Hand off queued blocking writes for the driver to run off the reactor.
    fn take_io(&mut self) -> Vec<IoJob> {
        std::mem::take(&mut self.io)
    }

    /// Execute queued blocking writes. Kept synchronous so it can run inside
    /// `spawn_blocking` (production) or directly (tests).
    fn run_io(state: &ProxyState, jobs: Vec<IoJob>) {
        for job in jobs {
            match job {
                IoJob::StoreOriginal { decoded, original } => {
                    let _ = state
                        .store
                        .put_sigil_original(&decoded, original.as_bytes());
                }
                IoJob::Cost {
                    protocol,
                    model,
                    session_id,
                    usage,
                } => record_cost(state, protocol, &model, &session_id, usage),
                IoJob::Transcript {
                    session_id,
                    content,
                } => record_transcript(state, &session_id, "assistant", &content),
            }
        }
    }

    /// Absorb an upstream byte chunk and return the transformed bytes to forward
    /// now. Incomplete trailing events stay buffered for the next chunk.
    fn push_chunk(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.buffer.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some((record_len, ending_len)) = next_sse_boundary(&self.buffer) {
            let record_bytes: Vec<u8> = self.buffer.drain(..record_len + ending_len).collect();
            let (record, ending) = record_bytes.split_at(record_len);
            match (std::str::from_utf8(record), std::str::from_utf8(ending)) {
                (Ok(record), Ok(ending)) => {
                    output.extend_from_slice(&self.transform_record(record, ending));
                }
                _ => output.extend_from_slice(&record_bytes),
            }
        }
        output
    }

    /// Complete the stream: forward any buffered remainder verbatim, flush the
    /// decoder tails as synthetic events (for streams that end without a
    /// terminal event), and record cost and transcript once.
    fn finish(&mut self) -> Vec<u8> {
        let mut output = std::mem::take(&mut self.buffer);
        let tail = match self.provider {
            StreamingProvider::OpenAi => self.flush_openai_event(),
            StreamingProvider::Anthropic => self.flush_all_blocks(),
        };
        output.extend_from_slice(tail.as_bytes());
        if let Some(usage) = self.usage.clone() {
            let protocol = match self.provider {
                StreamingProvider::OpenAi => Provider::OpenAi,
                StreamingProvider::Anthropic => Provider::Anthropic,
            };
            self.io.push(IoJob::Cost {
                protocol,
                model: self.model.clone(),
                session_id: self.session_id.clone(),
                usage,
            });
        }
        let content = self.assistant_text();
        if !content.is_empty() {
            self.io.push(IoJob::Transcript {
                session_id: self.session_id.clone(),
                content,
            });
        }
        output
    }

    fn transform_record(&mut self, record: &str, ending: &str) -> Vec<u8> {
        let Some(event) = parse_sse_record(record, ending) else {
            return format!("{record}{ending}").into_bytes();
        };
        match self.provider {
            StreamingProvider::OpenAi => self.transform_openai(event),
            StreamingProvider::Anthropic => self.transform_anthropic(event),
        }
    }

    fn transform_openai(&mut self, mut event: SseEvent) -> Vec<u8> {
        let Some(data) = event.data.clone() else {
            return event.render().into_bytes();
        };
        if data == "[DONE]" {
            let mut output = self.flush_openai_event();
            output.push_str(&event.render());
            return output.into_bytes();
        }
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            return event.render().into_bytes();
        };
        if let Some(usage) = adapters::openai_usage(&value) {
            self.merge_usage(usage);
        }
        // Capture the chunk envelope (id/object/model/created/…) once so a
        // synthetic tail event carries the same top-level fields.
        if self.openai_template.is_none()
            && let Some(object) = value.as_object()
        {
            let mut template = object.clone();
            template.remove("choices");
            template.remove("usage");
            self.openai_template = Some(template);
        }
        let finishing = value
            .get("choices")
            .and_then(Value::as_array)
            .is_some_and(|choices| {
                choices.iter().any(|choice| {
                    choice
                        .get("finish_reason")
                        .is_some_and(|reason| !reason.is_null())
                })
            });
        // Only feed the decoder when exactly one rewritable content slot exists;
        // multi-choice deltas pass through untouched to avoid a rewrite/decode
        // desync.
        let single_content = value
            .get("choices")
            .and_then(Value::as_array)
            .filter(|choices| choices.len() == 1)
            .and_then(|choices| choices[0].pointer("/delta/content"))
            .and_then(Value::as_str);
        if let Some(text) = single_content {
            let text = text.to_owned();
            self.openai.original.push_str(&text);
            let mut replacement = self.openai.decoder.push(&text);
            self.openai.decoded.push_str(&replacement);
            // A chunk that carries both content and finish_reason must push its
            // content before the decoder is drained, then fold the flushed tail
            // into this same event rather than a later synthetic one.
            if finishing {
                replacement.push_str(&self.flush_openai_tail());
            }
            if replacement != text {
                let _ = replace_openai_event_text(&mut event, &replacement);
            }
            return event.render().into_bytes();
        }
        // Content-free finishing chunk: emit the flushed tail as a synthetic
        // event before this event, since there is no content slot to fold into.
        let mut output = String::new();
        if finishing {
            output.push_str(&self.flush_openai_event());
        }
        output.push_str(&event.render());
        output.into_bytes()
    }

    /// Drain the decoder, accumulate the tail into the decoded mapping, store the
    /// reverse original, and return the raw tail text (no SSE framing).
    fn flush_openai_tail(&mut self) -> String {
        if self.openai.flushed {
            return String::new();
        }
        self.openai.flushed = true;
        let tail = self.openai.decoder.finish();
        self.openai.decoded.push_str(&tail);
        self.store_block_original(&self.openai.decoded.clone(), &self.openai.original.clone());
        tail
    }

    /// Flush the decoder tail as a standalone synthetic content event, carrying
    /// the captured chunk envelope so the event validates like a real one.
    fn flush_openai_event(&mut self) -> String {
        let tail = self.flush_openai_tail();
        if tail.is_empty() {
            return String::new();
        }
        let mut object = self.openai_template.clone().unwrap_or_default();
        object.insert(
            "choices".into(),
            serde_json::json!([{"index": 0, "delta": {"content": tail}}]),
        );
        format!("data: {}\n\n", Value::Object(object))
    }

    fn transform_anthropic(&mut self, event: SseEvent) -> Vec<u8> {
        let Some(data) = event.data.clone() else {
            return event.render().into_bytes();
        };
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            return event.render().into_bytes();
        };
        match value.get("type").and_then(Value::as_str) {
            // Streaming Anthropic reports input tokens only in `message_start`,
            // nested under `message`; output tokens arrive later in
            // `message_delta` at the top level. Merge so the cost record keeps
            // both instead of last-write-wins dropping the input count.
            Some("message_start") => {
                if let Some(usage) = value.get("message").and_then(adapters::anthropic_usage) {
                    self.merge_usage(usage);
                }
                event.render().into_bytes()
            }
            Some("message_delta") => {
                if let Some(usage) = adapters::anthropic_usage(&value) {
                    self.merge_usage(usage);
                }
                event.render().into_bytes()
            }
            Some("content_block_stop") => {
                let mut output = String::new();
                if let Some(index) = value.get("index").and_then(Value::as_u64) {
                    output.push_str(&self.flush_block(index));
                }
                output.push_str(&event.render());
                output.into_bytes()
            }
            Some("message_stop") => {
                let mut output = self.flush_all_blocks();
                output.push_str(&event.render());
                output.into_bytes()
            }
            Some("content_block_delta") => self.transform_anthropic_delta(&value, event),
            _ => event.render().into_bytes(),
        }
    }

    fn transform_anthropic_delta(&mut self, value: &Value, event: SseEvent) -> Vec<u8> {
        let Some(index) = value.get("index").and_then(Value::as_u64) else {
            return event.render().into_bytes();
        };
        // Skip tool-use (`input_json_delta`), thinking, and signature deltas
        // rather than aborting the whole-stream transform.
        if value.pointer("/delta/type").and_then(Value::as_str) != Some("text_delta") {
            return event.render().into_bytes();
        }
        let Some(text) = value.pointer("/delta/text").and_then(Value::as_str) else {
            return event.render().into_bytes();
        };
        let text = text.to_owned();
        let block = self.anthropic.entry(index).or_default();
        block.original.push_str(&text);
        let decoded = block.decoder.push(&text);
        block.decoded.push_str(&decoded);
        if decoded != text {
            let mut event = event;
            let _ = replace_anthropic_event_text(&mut event, &decoded);
            event.render().into_bytes()
        } else {
            event.render().into_bytes()
        }
    }

    fn flush_block(&mut self, index: u64) -> String {
        let Some(block) = self.anthropic.get_mut(&index) else {
            return String::new();
        };
        if block.flushed {
            return String::new();
        }
        block.flushed = true;
        let tail = block.decoder.finish();
        block.decoded.push_str(&tail);
        let (decoded, original) = (block.decoded.clone(), block.original.clone());
        self.store_block_original(&decoded, &original);
        if tail.is_empty() {
            String::new()
        } else {
            synth_anthropic_text_event(index, &tail)
        }
    }

    fn flush_all_blocks(&mut self) -> String {
        let mut indices: Vec<u64> = self.anthropic.keys().copied().collect();
        indices.sort_unstable();
        indices
            .into_iter()
            .map(|index| self.flush_block(index))
            .collect()
    }

    /// Merge usage across events, preferring the newest reported value per field
    /// so an Anthropic `message_start` (input tokens) and `message_delta`
    /// (output tokens) combine into one record instead of overwriting.
    fn merge_usage(&mut self, incoming: crate::types::Usage) {
        self.usage = Some(match self.usage.take() {
            None => incoming,
            Some(previous) => crate::types::Usage {
                input_tokens: incoming.input_tokens.or(previous.input_tokens),
                output_tokens: incoming.output_tokens.or(previous.output_tokens),
                total_tokens: incoming.total_tokens.or(previous.total_tokens),
                cache_read_tokens: incoming.cache_read_tokens.or(previous.cache_read_tokens),
                cache_creation_tokens: incoming
                    .cache_creation_tokens
                    .or(previous.cache_creation_tokens),
            },
        });
    }

    /// Queue the reverse mapping so a later request's assistant history restores
    /// the original sigil form. Only transforming blocks are stored; the write
    /// itself is deferred off the reactor.
    fn store_block_original(&mut self, decoded: &str, original: &str) {
        if decoded != original {
            self.io.push(IoJob::StoreOriginal {
                decoded: decoded.to_owned(),
                original: original.to_owned(),
            });
        }
    }

    /// The raw, pre-decode sigil text (not the decoded Markdown) for the
    /// transcript: design.md §5.8 lints the model's actual output, which stays
    /// stable if the decode mapping ever changes.
    fn assistant_text(&self) -> String {
        match self.provider {
            StreamingProvider::OpenAi => self.openai.original.clone(),
            StreamingProvider::Anthropic => {
                let mut indices: Vec<u64> = self.anthropic.keys().copied().collect();
                indices.sort_unstable();
                indices
                    .iter()
                    .map(|index| self.anthropic[index].original.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
    }
}

fn synth_anthropic_text_event(index: u64, text: &str) -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({
            "type":"content_block_delta",
            "index":index,
            "delta":{"type":"text_delta","text":text}
        })
    )
}

/// Run the decoder's queued blocking writes off the reactor. Awaited so the
/// writes are durable before the next stream item (no read-after-write race),
/// while `spawn_blocking` keeps the fsyncs from stalling other connections.
async fn drain_stream_io(decoder: &mut SseStreamDecoder) {
    let jobs = decoder.take_io();
    if jobs.is_empty() {
        return;
    }
    let state = decoder.state.clone();
    if tokio::task::spawn_blocking(move || SseStreamDecoder::run_io(&state, jobs))
        .await
        .is_err()
    {
        tracing::warn!("streaming side-effect task panicked");
    }
}

fn streaming_response(
    response: reqwest::Response,
    provider: StreamingProvider,
    state: Arc<ProxyState>,
    model: String,
    session_id: String,
) -> Response {
    let status = response.status();
    let headers = response.headers().clone();
    let decoder = SseStreamDecoder::new(provider, state, model, session_id);
    let upstream = Box::pin(response.bytes_stream());
    let stream = stream::unfold(
        (upstream, decoder, false),
        |(mut upstream, mut decoder, done)| async move {
            if done {
                return None;
            }
            match upstream.next().await {
                Some(Ok(chunk)) => {
                    let output = decoder.push_chunk(&chunk);
                    drain_stream_io(&mut decoder).await;
                    Some((
                        Ok::<_, std::io::Error>(axum::body::Bytes::from(output)),
                        (upstream, decoder, false),
                    ))
                }
                // Upstream aborted mid-body: end the stream without synthesizing
                // a terminal event, so the client sees a genuine truncation.
                // This intentionally drops both the decoder's held pending line
                // and any buffered partial SSE record. Unlike finish(), which
                // forwards the buffered remainder on a clean EOS, the error path
                // forwards nothing: a truncated, terminal-less stream is retried
                // or errored by every client, so the fragment has no recoverable
                // value, and emitting it would mean fabricating a delta on the
                // error path — the same lie as a synthetic terminal, just smaller.
                Some(Err(_)) => None,
                None => {
                    let output = decoder.finish();
                    drain_stream_io(&mut decoder).await;
                    Some((
                        Ok(axum::body::Bytes::from(output)),
                        (upstream, decoder, true),
                    ))
                }
            }
        },
    );
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

/// The last message's text only when it is a genuine user turn. Skips OpenAI
/// `tool` role messages so tool output is never captured as a "user" record.
fn last_openai_user_text(payload: &Value) -> Option<&str> {
    let last = payload
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())?;
    if last.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    last.get("content").and_then(string_content)
}

/// The last message's text only when it is a genuine user turn with real text.
/// Skips non-user roles and Anthropic `tool_result`-only user messages, whose
/// extracted text is empty.
fn last_anthropic_user_text(payload: &Value) -> Option<String> {
    let last = payload
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())?;
    if last.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let content = last.get("content").and_then(anthropic_content_text)?;
    (!content.trim().is_empty()).then_some(content)
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

fn substitute_anthropic_originals(payload: &mut Value, store: &Store) {
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("text") {
                continue;
            }
            let Some(text) = block.get("text").and_then(string_content) else {
                continue;
            };
            let Ok(Some(original)) = store.get_sigil_original(text) else {
                continue;
            };
            let Ok(original) = String::from_utf8(original) else {
                continue;
            };
            if let Some(slot) = block.get_mut("text") {
                *slot = Value::String(original);
            }
        }
    }
}

fn request_history(payload: &Value) -> Option<Vec<String>> {
    payload
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| serde_json::to_string(message).ok())
                .collect()
        })
}

fn session_identifier(selection: &crate::session::Match) -> String {
    match selection {
        crate::session::Match::Existing(id) | crate::session::Match::New { id, .. } => id.clone(),
    }
}

/// Lock the session Registry, recovering from a poisoned mutex so one panicked
/// request does not permanently disable session tracking (and with it tool
/// compression) for the life of the process. The recovered `Registry` may be
/// mildly inconsistent, which is acceptable: it is best-effort dedup/GC state,
/// never correctness-critical.
fn lock_sessions(state: &ProxyState) -> std::sync::MutexGuard<'_, Registry> {
    state.sessions.lock().unwrap_or_else(|poisoned| {
        state.sessions.clear_poison();
        poisoned.into_inner()
    })
}

/// Select (and persist) the Registry session for this request, returning its id
/// for transcript grouping, and apply attested tool-result compression while the
/// lock is held. Runs for every request so transcript session ids are real;
/// compression and blob collection run only when attestations are configured.
fn open_openai_session(payload: &mut Value, state: &ProxyState) -> String {
    let history = request_history(payload).unwrap_or_default();
    let mut sessions = lock_sessions(state);
    let now_secs = compress::unix_now_secs();
    let selection = sessions.select(&history, now_secs);
    let session_id = session_identifier(&selection);
    let attested = !state.tool_result_attestations.is_empty();
    if attested {
        compress_openai_tool_results(payload, state, &mut sessions, &selection, now_secs);
    }
    record_session_history(&mut sessions, selection, history, now_secs);
    if attested {
        collect_unpinned_blobs(state, &sessions, now_secs);
    }
    session_id
}

fn open_anthropic_session(payload: &mut Value, state: &ProxyState) -> String {
    let history = request_history(payload).unwrap_or_default();
    let mut sessions = lock_sessions(state);
    let now_secs = compress::unix_now_secs();
    let selection = sessions.select(&history, now_secs);
    let session_id = session_identifier(&selection);
    let attested = !state.tool_result_attestations.is_empty();
    if attested {
        compress_anthropic_tool_results(payload, state, &mut sessions, &selection, now_secs);
    }
    record_session_history(&mut sessions, selection, history, now_secs);
    if attested {
        collect_unpinned_blobs(state, &sessions, now_secs);
    }
    session_id
}

fn compress_openai_tool_results(
    payload: &mut Value,
    state: &ProxyState,
    sessions: &mut Registry,
    selection: &crate::session::Match,
    now_secs: u64,
) {
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for (message_index, message) in messages.iter_mut().enumerate() {
        if message.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(content) = message.get("content").and_then(string_content) else {
            continue;
        };
        let bytes = content.as_bytes();
        let locator = ToolResultLocator::OpenAi { message_index };
        let Some(metadata) = state.tool_result_attestations.metadata_for(&locator, bytes) else {
            continue;
        };
        let output = compress::transform(
            &state.store,
            sessions,
            selection,
            bytes,
            metadata,
            now_secs,
            TOOL_RESULT_MAX_AGE_SECS,
        );
        if output.receipt.is_some()
            && let Ok(content) = String::from_utf8(output.bytes)
            && let Some(slot) = message.get_mut("content")
        {
            *slot = Value::String(content);
        }
    }
}

fn compress_anthropic_tool_results(
    payload: &mut Value,
    state: &ProxyState,
    sessions: &mut Registry,
    selection: &crate::session::Match,
    now_secs: u64,
) {
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for (message_index, message) in messages.iter_mut().enumerate() {
        let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        for (block_index, block) in blocks.iter_mut().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(content) = block.get("content").and_then(string_content) else {
                continue;
            };
            let bytes = content.as_bytes();
            let locator = ToolResultLocator::Anthropic {
                message_index,
                block_index,
            };
            let Some(metadata) = state.tool_result_attestations.metadata_for(&locator, bytes)
            else {
                continue;
            };
            let output = compress::transform(
                &state.store,
                sessions,
                selection,
                bytes,
                metadata,
                now_secs,
                TOOL_RESULT_MAX_AGE_SECS,
            );
            if output.receipt.is_some()
                && let Ok(content) = String::from_utf8(output.bytes)
                && let Some(slot) = block.get_mut("content")
            {
                *slot = Value::String(content);
            }
        }
    }
}

fn collect_unpinned_blobs(state: &ProxyState, sessions: &Registry, now_secs: u64) {
    let mut pins = sessions.pinned_digests();
    match state
        .store
        .recently_served_digests(now_secs, TOOL_RESULT_MAX_AGE_SECS)
    {
        Ok(served) => pins.extend(served),
        Err(error) => {
            tracing::warn!("could not read served-reference pins for collection: {error}");
            return;
        }
    }
    if let Err(error) = state.store.collect_unpinned(&pins) {
        tracing::warn!("could not collect unpinned CCR blobs: {error}");
    }
}

fn record_session_history(
    sessions: &mut Registry,
    selection: crate::session::Match,
    history: Vec<String>,
    now_secs: u64,
) {
    let id = match selection {
        crate::session::Match::Existing(id) | crate::session::Match::New { id, .. } => id,
    };
    sessions.record(id, history, now_secs);
}

/// One user message's normalization, computed once and reused for the savings
/// estimate, the cache-invalidation estimate, and the apply step, instead of
/// re-running `normalize_with_detect` three times per request.
struct UserNormalization {
    index: usize,
    original: String,
    output: crate::normalize::Normalized,
}

fn precompute_openai_normalizations(payload: &Value) -> Vec<UserNormalization> {
    let Some(messages) = payload.get("messages").and_then(Value::as_array) else {
        return Vec::new();
    };
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            if message.get("role").and_then(Value::as_str) != Some("user") {
                return None;
            }
            let text = message.get("content").and_then(string_content)?;
            let output = crate::normalize::normalize_with_detect(text, &Default::default());
            Some(UserNormalization {
                index,
                original: text.to_owned(),
                output,
            })
        })
        .collect()
}

fn normalization_savings(normalizations: &[UserNormalization], config: &LocalConfig) -> f64 {
    let Some(price) = config.price_for(&Provider::OpenAi) else {
        return 0.0;
    };
    let saved_bytes: usize = normalizations
        .iter()
        .map(|norm| {
            norm.output
                .bytes_before
                .saturating_sub(norm.output.bytes_after)
        })
        .sum();
    (saved_bytes as f64 / 4.0) * price.input_per_million_usd / 1_000_000.0
}

/// Estimate the input tokens that normalization would invalidate in the
/// provider's cache. Changing any user message forces a re-send of that
/// message and everything after it, so the estimate is the token count of the
/// suffix beginning at the first user message that normalization changes.
fn normalization_cache_invalidation(payload: &Value, normalizations: &[UserNormalization]) -> u64 {
    let Some(first_changed) = normalizations
        .iter()
        .find(|norm| norm.output.changed)
        .map(|norm| norm.index)
    else {
        return 0;
    };
    let Some(messages) = payload.get("messages").and_then(Value::as_array) else {
        return 0;
    };
    let suffix_bytes = messages
        .iter()
        .skip(first_changed)
        .filter_map(|message| serde_json::to_string(message).ok())
        .map(|json| json.len())
        .sum::<usize>();
    (suffix_bytes / 4) as u64
}

fn apply_openai_normalizations(
    payload: &mut Value,
    normalizations: &[UserNormalization],
    gate: Gate,
    store: &Store,
) {
    if gate != Gate::Allowed {
        return;
    }
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for norm in normalizations {
        if !norm.output.changed
            || store
                .put_normalization_original(&norm.output.text, norm.original.as_bytes())
                .is_err()
        {
            continue;
        }
        if let Some(message) = messages.get_mut(norm.index)
            && let Some(slot) = message.get_mut("content")
        {
            *slot = Value::String(norm.output.text.clone());
        }
    }
}

fn record_cost(
    state: &ProxyState,
    provider: Provider,
    model: &str,
    session_id: &str,
    usage: crate::types::Usage,
) {
    let record = operations::CostRecord::from_usage(
        provider.clone(),
        model.to_owned(),
        Some(session_id.to_owned()),
        usage,
        state.config.price_for(&provider),
    );
    if let Err(error) = operations::append_cost_record(&state.cost_ledger, &record) {
        tracing::warn!("could not append local cost record: {error}");
    }
}

/// Append a redacted transcript record for one turn. `content` for an
/// assistant turn is the model's raw sigil output, pre-decode (design.md
/// §5.8): lint findings must be computed against what the model actually
/// wrote, and that representation stays stable if the decode mapping ever
/// changes — the decoded Markdown does not. Lint only runs on assistant turns:
/// the single-document rules (typed terminal, severity labels, ...) are
/// contracts on the model's sigil output, not on user prose, so linting a user
/// turn with the same rules only produces noise. Failures log and are never
/// allowed to break request forwarding.
fn record_transcript(state: &ProxyState, session_id: &str, role: &str, content: &str) {
    use crate::transcript::TranscriptRecord;
    if state.transcript_ledger.as_os_str().is_empty() {
        return;
    }
    let timestamp = format!("{}", compress::unix_now_secs());
    let findings = if role == "assistant" {
        crate::lint::lint_document(content)
    } else {
        Vec::new()
    };
    let mut record = TranscriptRecord::new(
        timestamp,
        session_id.to_owned(),
        role.into(),
        content.into(),
        Some(crate::skill::SKILL_VERSION.into()),
    );
    record.lint = findings;
    if let Err(error) = crate::transcript::append_capped(
        &state.transcript_ledger,
        &record,
        crate::transcript::DEFAULT_MAX_BYTES,
    ) {
        tracing::warn!("could not append transcript record: {error}");
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

fn anthropic_content_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    value.as_array().map(|blocks| {
        blocks
            .iter()
            .filter_map(|block| {
                let text = block.get("text").and_then(Value::as_str)?;
                Some(text.to_owned())
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
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
    let session_id = open_openai_session(&mut payload, &state);
    let normalizations = precompute_openai_normalizations(&payload);
    let predicted_savings = normalization_savings(&normalizations, &state.config);
    let invalidated = normalization_cache_invalidation(&payload, &normalizations);
    let gate = compression_gate(&state.config.calibration, predicted_savings, invalidated);
    apply_openai_normalizations(&mut payload, &normalizations, gate, &state.store);
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
    if let Some(content) = last_openai_user_text(&payload) {
        record_transcript(&state, &session_id, "user", content);
    }
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
        Ok(response) if stream => streaming_response(
            response,
            StreamingProvider::OpenAi,
            state.clone(),
            model,
            session_id,
        ),
        Ok(response) => {
            let status = response.status();
            let upstream_headers = response.headers().clone();
            match response.bytes().await {
                Ok(bytes) => {
                    let decoded = decode_openai_response(&bytes, &state.store);
                    let mut output = response_from_upstream(status, &upstream_headers, decoded.clone());
                    // Parsed once from the pre-decode bytes: usage, and the raw
                    // sigil transcript content (design.md §5.8 lints the model's
                    // actual output, not our post-decode rewrite of it).
                    let raw_value = serde_json::from_slice::<Value>(&bytes).ok();
                    if let Some(value) = &raw_value
                        && let Some(usage) = adapters::openai_usage(value)
                    {
                        record_cost(&state, Provider::OpenAi, &model, &session_id, usage.clone());
                        let headers = output.headers_mut();
                        if let Some(value) = usage.input_tokens { headers.insert("x-bbg-input-tokens", HeaderValue::from(value)); }
                        if let Some(value) = usage.output_tokens { headers.insert("x-bbg-output-tokens", HeaderValue::from(value)); }
                    }
                    if let Some(content) = raw_value
                        .as_ref()
                        .and_then(|value| value.pointer("/choices/0/message/content"))
                        .and_then(Value::as_str)
                    {
                        record_transcript(&state, &session_id, "assistant", content);
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
    substitute_anthropic_originals(&mut payload, &state.store);
    let session_id = open_anthropic_session(&mut payload, &state);
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
    if let Some(content) = last_anthropic_user_text(&payload) {
        record_transcript(&state, &session_id, "user", &content);
    }
    if state.dry {
        return (StatusCode::OK, Json(serde_json::json!({"type":"message","content":[{"type":"text","text":"[bbg dry-run]"}]}))).into_response();
    }
    let upstream = upstream_endpoint(&state.upstream, "messages");
    let mut request = state
        .client
        .post(upstream)
        .headers(forwarded_headers(&headers))
        .json(&payload);
    if !headers.contains_key("anthropic-version") {
        request = request.header("anthropic-version", "2023-06-01");
    }
    if !state.key.is_empty() {
        request = request.header("x-api-key", &state.key);
    }
    match request.send().await {
        Ok(response) if stream => streaming_response(
            response,
            StreamingProvider::Anthropic,
            state.clone(),
            model,
            session_id,
        ),
        Ok(response) => {
            let status = response.status();
            let upstream_headers = response.headers().clone();
            match response.bytes().await {
                Ok(bytes) => {
                    let decoded = decode_anthropic_response(&bytes, &state.store);
                    let mut output = response_from_upstream(status, &upstream_headers, decoded.clone());
                    // Parsed once from the pre-decode bytes: usage, and the raw
                    // sigil transcript content (design.md §5.8 lints the model's
                    // actual output, not our post-decode rewrite of it).
                    let raw_value = serde_json::from_slice::<Value>(&bytes).ok();
                    if let Some(value) = &raw_value
                        && let Some(usage) = adapters::anthropic_usage(value)
                    {
                        record_cost(&state, Provider::Anthropic, &model, &session_id, usage.clone());
                        let headers = output.headers_mut();
                        if let Some(value) = usage.input_tokens { headers.insert("x-bbg-input-tokens", HeaderValue::from(value)); }
                        if let Some(value) = usage.output_tokens { headers.insert("x-bbg-output-tokens", HeaderValue::from(value)); }
                    }
                    if let Some(content) = raw_value
                        .as_ref()
                        .and_then(|value| value.get("content"))
                        .and_then(anthropic_content_text)
                    {
                        record_transcript(&state, &session_id, "assistant", &content);
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

/// Build a router with no locally-attested tool metadata. This is the
/// fail-closed production default.
pub fn build_router(settings: ProxySettings, store: Store, config: LocalConfig) -> Router {
    build_router_with_tool_result_attestations(
        settings,
        store,
        config,
        LocalToolResultAttestations::default(),
    )
}

/// Build a router from owner-provided immutable tool-result attestations.
/// Attestations are local runtime configuration, never request/provider data.
pub fn build_router_with_tool_result_attestations(
    settings: ProxySettings,
    store: Store,
    config: LocalConfig,
    tool_result_attestations: LocalToolResultAttestations,
) -> Router {
    let state = Arc::new(ProxyState {
        upstream: settings.upstream,
        key: settings.key,
        proxy_token: settings.proxy_token,
        dry: settings.dry,
        client: settings.client,
        store,
        config,
        tool_result_attestations,
        sessions: Mutex::new(Registry::default()),
        cost_ledger: settings.cost_ledger,
        transcript_ledger: settings.transcript_ledger,
    });
    Router::new()
        .route("/v1/chat/completions", post(handle_chat))
        .route("/v1/messages", post(handle_anthropic))
        .route("/v1/models", axum::routing::get(get_models))
        .route("/health", axum::routing::get(health))
        .with_state(state)
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
                crate::operations::ProviderPricing {
                    input_per_million_usd: 1.0,
                    output_per_million_usd: 0.0,
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            calibration: crate::operations::Calibration {
                samples: 20,
                cache_hit_rate: 0.5,
                ..Default::default()
            },
            ..Default::default()
        };
        let normalizations = precompute_openai_normalizations(&payload);
        assert!(normalization_savings(&normalizations, &config) > 0.0);
        assert_eq!(
            compression_gate(
                &config.calibration,
                normalization_savings(&normalizations, &config),
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
    fn normalization_stores_original_before_forwarding_and_fails_closed_on_collision() {
        let store = store();
        let mut first = serde_json::json!({
            "messages": [{"role": "user", "content": "please fix"}]
        });
        let first_norm = precompute_openai_normalizations(&first);
        apply_openai_normalizations(&mut first, &first_norm, Gate::Allowed, &store);
        assert_eq!(first["messages"][0]["content"], "fix");
        assert_eq!(
            store.get_normalization_original("fix").unwrap(),
            Some(b"please fix".to_vec())
        );

        let mut collision = serde_json::json!({
            "messages": [{"role": "user", "content": "fix thank you"}]
        });
        let collision_norm = precompute_openai_normalizations(&collision);
        apply_openai_normalizations(&mut collision, &collision_norm, Gate::Allowed, &store);
        assert_eq!(collision["messages"][0]["content"], "fix thank you");
    }

    #[test]
    fn bearer_authorization_allows_only_the_exact_token() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));

        let root = std::env::temp_dir().join(format!(
            "bbg-proxy-auth-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let state = ProxyState {
            upstream: validate_upstream_url("https://api.example.test/v1").unwrap(),
            key: String::new(),
            proxy_token: Some("secret".into()),
            dry: false,
            client: reqwest::Client::new(),
            store: store(),
            config: LocalConfig::default(),
            tool_result_attestations: LocalToolResultAttestations::default(),
            sessions: Mutex::new(Registry::default()),
            cost_ledger: root.join("costs.jsonl"),
            transcript_ledger: root.join("transcripts.jsonl"),
        };
        let mut ok = HeaderMap::new();
        ok.insert("authorization", HeaderValue::from_static("Bearer secret"));
        assert!(client_authorized(&state, &ok));
        let mut wrong = HeaderMap::new();
        wrong.insert("authorization", HeaderValue::from_static("Bearer wrong"));
        assert!(!client_authorized(&state, &wrong));
        let missing = HeaderMap::new();
        assert!(!client_authorized(&state, &missing));
        let mut nonzero = HeaderMap::new();
        nonzero.insert("authorization", HeaderValue::from_static("Bearer se"));
        assert!(!client_authorized(&state, &nonzero));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn transcript_recording_appends_redacted_user_and_assistant_roles_with_lint() {
        let root = std::env::temp_dir().join(format!(
            "bbg-proxy-transcript-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let transcript_ledger = root.join("transcripts.jsonl");
        let state = ProxyState {
            upstream: validate_upstream_url("https://api.example.test/v1").unwrap(),
            key: String::new(),
            proxy_token: None,
            dry: false,
            client: reqwest::Client::new(),
            store: store(),
            config: LocalConfig::default(),
            tool_result_attestations: LocalToolResultAttestations::default(),
            sessions: Mutex::new(Registry::default()),
            cost_ledger: root.join("costs.jsonl"),
            transcript_ledger: transcript_ledger.clone(),
        };
        record_transcript(&state, "sess-1", "user", "delete the config please");
        record_transcript(&state, "sess-1", "assistant", "Sure, done.");
        let records = crate::transcript::read(&transcript_ledger).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].role, "user");
        assert_eq!(records[0].session_id, "sess-1");
        assert_eq!(records[1].role, "assistant");
        assert_eq!(records[1].skill_version.as_deref(), Some("1.0.0"));
        let inthe_loop_findings: usize = records.iter().map(|r| r.lint.len()).sum();
        assert!(inthe_loop_findings > 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn normalization_cache_invalidation_covers_changed_suffix() {
        let unchanged = serde_json::json!({
            "messages": [{"role": "user", "content": "keep this"}]
        });
        assert_eq!(
            normalization_cache_invalidation(
                &unchanged,
                &precompute_openai_normalizations(&unchanged)
            ),
            0
        );

        let changed = serde_json::json!({
            "messages": [
                {"role": "user", "content": "please   fix   spacing"},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": "then keep this"}
            ]
        });
        let invalidated =
            normalization_cache_invalidation(&changed, &precompute_openai_normalizations(&changed));
        assert!(invalidated > 0);
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
        inbound.insert("anthropic-beta", HeaderValue::from_static("beta-feature"));
        inbound.insert("anthropic-version", HeaderValue::from_static("2024-01-01"));
        let outbound = forwarded_headers(&inbound);
        assert_eq!(outbound.get("accept").unwrap(), "text/event-stream");
        assert_eq!(outbound.get("anthropic-beta").unwrap(), "beta-feature");
        assert_eq!(outbound.get("anthropic-version").unwrap(), "2024-01-01");
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

    fn proxy_state(store: Store) -> Arc<ProxyState> {
        Arc::new(ProxyState {
            upstream: validate_upstream_url("https://api.example.test/v1").unwrap(),
            key: String::new(),
            proxy_token: None,
            dry: false,
            client: reqwest::Client::new(),
            store,
            config: LocalConfig::default(),
            tool_result_attestations: LocalToolResultAttestations::default(),
            sessions: Mutex::new(Registry::default()),
            cost_ledger: std::env::temp_dir().join("bbg-proxy-test-cost.jsonl"),
            transcript_ledger: PathBuf::new(),
        })
    }

    fn stream_decoder(provider: StreamingProvider, store: Store) -> SseStreamDecoder {
        SseStreamDecoder::new(
            provider,
            proxy_state(store),
            "test-model".into(),
            "test-session".into(),
        )
    }

    #[test]
    fn lock_sessions_recovers_from_a_poisoned_mutex() {
        let state = proxy_state(store());
        // Poison the mutex: panic while holding the guard.
        let poisoner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = state.sessions.lock().unwrap();
            panic!("poison the session registry");
        }));
        assert!(poisoner.is_err());
        assert!(state.sessions.is_poisoned());

        // Recovery yields a usable Registry and clears the poison so later
        // requests are not permanently degraded.
        let mut sessions = lock_sessions(&state);
        let _ = sessions.select(&[], 0);
        drop(sessions);
        assert!(!state.sessions.is_poisoned());
    }

    #[test]
    fn openai_synthetic_tail_event_carries_the_chunk_envelope() {
        let store = store();
        let mut decoder = stream_decoder(StreamingProvider::OpenAi, store);
        let output = run_stream(
            &mut decoder,
            &[
                "data: {\"id\":\"chatcmpl-x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"§ Status\\n. done\"}}]}\n\n",
                "data: [DONE]\n\n",
            ],
        );
        // The flushed tail rides a synthetic chunk that echoes id/object/model.
        let synth = output
            .split("\n\n")
            .find(|event| event.contains("**Done.** done"))
            .expect("synthetic tail event");
        assert!(
            synth.contains("\"id\":\"chatcmpl-x\""),
            "envelope id: {synth}"
        );
        assert!(synth.contains("\"model\":\"m\""));
        assert!(synth.contains("\"object\":\"chat.completion.chunk\""));
    }

    fn run_stream(decoder: &mut SseStreamDecoder, chunks: &[&str]) -> String {
        let mut output = Vec::new();
        for chunk in chunks {
            output.extend_from_slice(&decoder.push_chunk(chunk.as_bytes()));
        }
        output.extend_from_slice(&decoder.finish());
        drain_io(decoder);
        String::from_utf8(output).unwrap()
    }

    /// Execute the decoder's queued blocking writes synchronously (the async
    /// driver does this off the reactor via `spawn_blocking`).
    fn drain_io(decoder: &mut SseStreamDecoder) {
        let jobs = decoder.take_io();
        SseStreamDecoder::run_io(&decoder.state, jobs);
    }

    #[test]
    fn openai_stream_decodes_incrementally_and_stores_original() {
        let store = store();
        let mut decoder = stream_decoder(StreamingProvider::OpenAi, store.clone());

        // A partial event is buffered — nothing is emitted until it completes.
        let partial =
            decoder.push_chunk("data: {\"choices\":[{\"delta\":{\"content\":\"§ Sta".as_bytes());
        assert!(partial.is_empty());

        // The heading forwards as soon as its line completes, before the terminal.
        let mut assembled =
            String::from_utf8(decoder.push_chunk("tus\\n\"}}]}\n\n".as_bytes())).unwrap();
        assert!(
            assembled.contains("## Status"),
            "heading forwards incrementally: {assembled}"
        );

        assembled.push_str(
            &String::from_utf8(decoder.push_chunk(
                "data: {\"choices\":[{\"delta\":{\"content\":\". done\"}}]}\n\n".as_bytes(),
            ))
            .unwrap(),
        );
        assembled.push_str(
            &String::from_utf8(decoder.push_chunk("data: [DONE]\n\n".as_bytes())).unwrap(),
        );
        assembled.push_str(&String::from_utf8(decoder.finish()).unwrap());
        drain_io(&mut decoder);

        assert!(assembled.contains("**Done.** done"));
        assert!(assembled.ends_with("data: [DONE]\n\n"));
        assert!(!assembled.contains("§ Status"));
        let decoded = sigil::decode("§ Status\n. done");
        assert_eq!(
            store.get_sigil_original(&decoded).unwrap(),
            Some("§ Status\n. done".into())
        );
    }

    #[test]
    fn anthropic_stream_flushes_tail_before_block_stop() {
        let store = store();
        let mut decoder = stream_decoder(StreamingProvider::Anthropic, store.clone());
        let output = run_stream(
            &mut decoder,
            &[
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"§ Status\\n\"}}\n\n",
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\". done\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ],
        );
        assert!(output.ends_with("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
        assert!(output.contains("## Status"));
        assert!(output.contains("**Done.** done"));
        assert!(!output.contains("§ Status"));
        let decoded = sigil::decode("§ Status\n. done");
        assert_eq!(
            store.get_sigil_original(&decoded).unwrap(),
            Some("§ Status\n. done".into())
        );
    }

    #[test]
    fn anthropic_stream_passes_through_tool_and_thinking_deltas() {
        let store = store();
        let mut decoder = stream_decoder(StreamingProvider::Anthropic, store);
        let output = run_stream(
            &mut decoder,
            &[
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n",
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"a\\\":1}\"}}\n\n",
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"text_delta\",\"text\":\"§ Status\\n. done\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ],
        );
        // Non-text deltas survive verbatim instead of aborting the transform.
        assert!(output.contains("\"thinking_delta\""));
        assert!(output.contains("\"input_json_delta\""));
        // The interleaved text block still decodes.
        assert!(output.contains("## Status"));
        assert!(output.contains("**Done.** done"));
        assert!(!output.contains("§ Status"));
    }

    #[test]
    fn anthropic_stream_preserves_plain_block_without_trailing_newline() {
        let store = store();
        let mut decoder = stream_decoder(StreamingProvider::Anthropic, store);
        let output = run_stream(
            &mut decoder,
            &[
                // Block 0 transforms; block 1 is plain and its last line has no newline.
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"§ Status\\n. done\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"plain tail no newline\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ],
        );
        assert!(output.contains("## Status"));
        assert!(output.contains("**Done.** done"));
        // The plain block's withheld pending line must flush, not vanish.
        assert!(
            output.contains("plain tail no newline"),
            "plain block preserved: {output}"
        );
    }

    #[test]
    fn streaming_usage_merges_input_from_message_start_and_output_from_delta() {
        let store = store();
        let mut anthropic = stream_decoder(StreamingProvider::Anthropic, store.clone());
        // Real Anthropic streams: input tokens nested in message_start, output
        // tokens top-level in message_delta. Both must survive into one record.
        let _ = anthropic.push_chunk(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n".as_bytes(),
        );
        let _ = anthropic.push_chunk(
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":6}}\n\n".as_bytes(),
        );
        let usage = anthropic.usage.clone().unwrap();
        assert_eq!(usage.input_tokens, Some(4));
        assert_eq!(usage.output_tokens, Some(6));

        let mut openai = stream_decoder(StreamingProvider::OpenAi, store);
        let _ = openai.push_chunk(
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":5,\"total_tokens\":8}}\n\n".as_bytes(),
        );
        let usage = openai.usage.clone().unwrap();
        assert_eq!(usage.input_tokens, Some(3));
        assert_eq!(usage.output_tokens, Some(5));
    }

    #[test]
    fn openai_combined_content_and_finish_reason_chunk_preserves_text() {
        let store = store();
        let mut decoder = stream_decoder(StreamingProvider::OpenAi, store.clone());
        // Ollama/vLLM combine the last content and finish_reason in one chunk.
        let output = run_stream(
            &mut decoder,
            &[
                "data: {\"choices\":[{\"delta\":{\"content\":\"§ Status\\n. done\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n",
            ],
        );
        assert!(output.contains("## Status"), "heading kept: {output}");
        assert!(
            output.contains("**Done.** done"),
            "tail folded in, not dropped: {output}"
        );
        assert!(!output.contains("§ Status"));
        let decoded = sigil::decode("§ Status\n. done");
        assert_eq!(
            store.get_sigil_original(&decoded).unwrap(),
            Some("§ Status\n. done".into())
        );
    }

    #[test]
    fn openai_plain_finishing_chunk_keeps_trailing_text() {
        let store = store();
        let mut decoder = stream_decoder(StreamingProvider::OpenAi, store);
        // Plain (non-sigil) content in a combined finish chunk must survive.
        let output = run_stream(
            &mut decoder,
            &[
                "data: {\"choices\":[{\"delta\":{\"content\":\"final words\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            ],
        );
        assert!(
            output.contains("final words"),
            "plain tail preserved: {output}"
        );
    }

    #[test]
    fn truncated_openai_stream_does_not_synthesize_terminal() {
        let store = store();
        let mut decoder = stream_decoder(StreamingProvider::OpenAi, store);
        let output = run_stream(
            &mut decoder,
            &["data: {\"choices\":[{\"delta\":{\"content\":\"§ Status\"}}]}\n\n"],
        );
        // No `[DONE]` is fabricated for a stream the upstream never terminated.
        assert!(!output.contains("data: [DONE]"));
        // The held-back line still flushes at end; incremental decode has no
        // whole-response terminal gate.
        assert!(output.contains("## Status"));
    }

    #[test]
    fn user_text_helpers_skip_tool_turns_and_keep_real_user_text() {
        // OpenAI: a trailing `tool` message is not a user turn.
        let openai_tool = serde_json::json!({"messages":[
            {"role":"user","content":"hi"},
            {"role":"tool","content":"tool output"}
        ]});
        assert_eq!(last_openai_user_text(&openai_tool), None);
        let openai_user = serde_json::json!({"messages":[{"role":"user","content":"hi"}]});
        assert_eq!(last_openai_user_text(&openai_user), Some("hi"));

        // Anthropic: a tool_result-only user message extracts to empty text.
        let anthropic_tool = serde_json::json!({"messages":[
            {"role":"user","content":[{"type":"tool_result","content":"tool output"}]}
        ]});
        assert_eq!(last_anthropic_user_text(&anthropic_tool), None);
        let anthropic_user = serde_json::json!({"messages":[
            {"role":"user","content":[{"type":"text","text":"hi"}]}
        ]});
        assert_eq!(
            last_anthropic_user_text(&anthropic_user).as_deref(),
            Some("hi")
        );
    }

    #[test]
    fn error_event_passes_through_verbatim_without_aborting() {
        let store = store();
        let mut decoder = stream_decoder(StreamingProvider::OpenAi, store);
        let output = run_stream(
            &mut decoder,
            &["data: {\"error\":{\"message\":\"nope\"}}\n\n"],
        );
        assert!(output.contains("\"error\""));
    }
}
