//! bbg-proxy — environment-backed production startup for the proxy runtime.

use brief_bright_gone::{
    operations::LocalConfig,
    proxy::{DEFAULT_BIND, ProxySettings, build_router, resolve_bind},
    store::Store,
};
use std::{env, path::PathBuf, time::Duration};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let upstream = env::var("BBG_UPSTREAM_URL")
        .or_else(|_| env::var("OPENAI_BASE_URL"))
        .unwrap_or_else(|_| "http://localhost:11434/v1".into());
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
    let store_root = env::var("BBG_STORE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| brief_bright_gone::store::default_store_dir());
    let store = Store::open(&store_root).unwrap_or_else(|error| {
        eprintln!("error: open CCR store: {error}");
        std::process::exit(2);
    });
    let config_path = env::var("BBG_CONFIG").ok().map(PathBuf::from);
    let config = LocalConfig::load(config_path.as_deref()).unwrap_or_else(|error| {
        eprintln!("error: local BBG_CONFIG: {error}");
        std::process::exit(2);
    });
    // Transcript capture is opt-out: BBG_TRANSCRIPT=0 (or "off") passes an empty
    // ledger path, which the proxy treats as a no-op.
    let transcript_disabled = env::var("BBG_TRANSCRIPT")
        .is_ok_and(|value| value == "0" || value.eq_ignore_ascii_case("off"));
    let transcript_ledger = if transcript_disabled {
        PathBuf::new()
    } else {
        store_root.join("ledger").join("transcripts.jsonl")
    };
    let settings = ProxySettings::new(
        &upstream,
        key,
        proxy_token,
        dry,
        Duration::from_secs(120),
        store_root.join("ledger").join("costs.jsonl"),
        transcript_ledger,
        store_root.join("ledger").join("health.jsonl"),
    )
    .unwrap_or_else(|error| {
        eprintln!("error: BBG_UPSTREAM_URL: {error}");
        std::process::exit(2);
    });
    let listener = tokio::net::TcpListener::bind((
        bind,
        port.parse::<u16>().unwrap_or_else(|_| {
            eprintln!("error: BBG_PORT must be a valid TCP port");
            std::process::exit(2);
        }),
    ))
    .await
    .expect("bind failed");
    axum::serve(listener, build_router(settings, store, config))
        .await
        .expect("server error");
}
