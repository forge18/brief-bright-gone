use brief_bright_gone::{
    operations::{CostRecord, HealthRecord},
    store::Store,
    transcript::TranscriptRecord,
    types::{Provider, Usage},
};
use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bbg"));
    command
        .env_clear()
        .env("BBG_STORE_DIR", root.join("store"))
        .env("HOME", root.join("home"));
    command
}

#[test]
fn cli_skill_lifecycle_and_doctor_are_stable() {
    let root = temporary_root("cli-lifecycle");
    let skill_dir = root.join("skills");
    let install = command(&root)
        .args(["install", "--path", skill_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&install.stderr)
    );
    assert!(skill_dir.join("BBG_SKILL.md").is_file());
    assert!(
        String::from_utf8(install.stdout)
            .unwrap()
            .contains("BBG_SKILL.md")
    );

    let upgrade = command(&root).arg("upgrade").output().unwrap();
    assert!(
        upgrade.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&upgrade.stderr)
    );

    let doctor = command(&root).arg("doctor").output().unwrap();
    assert!(doctor.status.success());
    let doctor_stdout = String::from_utf8(doctor.stdout).unwrap();
    assert!(doctor_stdout.contains("skill_current: pass"));
    assert!(doctor_stdout.contains("store_writable: pass"));
    assert!(!root.join("store").join(".doctor").exists());

    let uninstall = command(&root).arg("uninstall").output().unwrap();
    assert!(uninstall.status.success());
    assert!(!skill_dir.join("BBG_SKILL.md").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cli_stats_lint_benchmark_and_recovery_have_stable_output_and_exit_codes() {
    let root = temporary_root("cli-commands");
    let store_root = root.join("store");
    let store = Store::open(&store_root).unwrap();
    let original = b"recover exactly\n\0tail";
    let digest = store.put(original).unwrap();

    let stats = command(&root).arg("stats").output().unwrap();
    assert!(stats.status.success());
    assert!(
        String::from_utf8(stats.stdout)
            .unwrap()
            .contains("observed_usage_records: 0")
    );

    let lint_file = root.join("clean.md");
    fs::create_dir_all(&root).unwrap();
    fs::write(&lint_file, ". complete\n").unwrap();
    let lint = command(&root)
        .args(["lint", lint_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(lint.status.success());
    assert!(lint.stdout.is_empty());

    let transcript = root.join("transcript.jsonl");
    let record = TranscriptRecord::new(
        "t".into(),
        "s".into(),
        "user".into(),
        "Fix src/lib.rs. Done when tests pass.".into(),
        None,
    )
    .with_receipts(
        brief_bright_gone::signals::readiness_receipts_for_conversation(
            "Fix src/lib.rs. Done when tests pass.",
            true,
        ),
    )
    .with_session_turn(1);
    fs::write(
        &transcript,
        format!("{}\n", serde_json::to_string(&record).unwrap()),
    )
    .unwrap();

    // A cost record under the store's ledger, matching the transcript's
    // session id, so `benchmark report` can join dollars onto turns (§10).
    let cost_ledger = store_root.join("ledger").join("costs.jsonl");
    fs::create_dir_all(cost_ledger.parent().unwrap()).unwrap();
    let cost = CostRecord::from_usage(
        Provider::OpenAi,
        "m".into(),
        Some("s".into()),
        Usage {
            input_tokens: Some(50),
            output_tokens: Some(50),
            cache_read_tokens: Some(50),
            ..Default::default()
        },
        Some(&brief_bright_gone::operations::ProviderPricing {
            input_per_million_usd: 1.0,
            output_per_million_usd: 2.0,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        }),
    )
    .with_session_turn(2);
    fs::write(
        &cost_ledger,
        format!("{}\n", serde_json::to_string(&cost).unwrap()),
    )
    .unwrap();
    // An unmatched-session cost record must not leak into the sum.
    let unrelated_cost = CostRecord::from_usage(
        Provider::OpenAi,
        "m".into(),
        Some("other-session".into()),
        Usage {
            input_tokens: Some(1_000_000),
            cache_read_tokens: Some(0),
            ..Default::default()
        },
        Some(&brief_bright_gone::operations::ProviderPricing {
            input_per_million_usd: 5.0,
            output_per_million_usd: 5.0,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        }),
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&cost_ledger)
        .and_then(|mut file| {
            use std::io::Write;
            writeln!(file, "{}", serde_json::to_string(&unrelated_cost).unwrap())
        })
        .unwrap();

    let benchmark = command(&root)
        .args(["benchmark", "report", transcript.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(benchmark.status.success());
    let benchmark_stdout = String::from_utf8(benchmark.stdout).unwrap();
    assert!(benchmark_stdout.contains("\"turns\": 1"));
    // 100 * 1.0/1e6 + 50 * 2.0/1e6 = 0.0002; the unrelated session's cost is
    // excluded from the sum but counted as excluded.
    assert!(
        benchmark_stdout.contains("\"cost_usd\": 0.0002"),
        "stdout: {benchmark_stdout}"
    );
    assert!(benchmark_stdout.contains("\"cost_records_excluded\": 1"));

    let health_ledger = store_root.join("ledger").join("health.jsonl");
    fs::write(
        &health_ledger,
        [
            HealthRecord {
                schema_version: 2,
                provider: Provider::OpenAi,
                model: "gpt-health".into(),
                skill_version: Some("1.0.1".into()),
                session_id: Some("s".into()),
                session_turn: Some(3),
                substitution_attempts: 2,
                substitution_misses: 1,
                text_responses: 1,
                zero_sigil_responses: 1,
                table_runs: 1,
                malformed_table_runs: 0,
            },
            HealthRecord {
                schema_version: 2,
                provider: Provider::OpenAi,
                model: "gpt-health".into(),
                skill_version: Some("1.1.0".into()),
                session_id: Some("s".into()),
                session_turn: Some(4),
                substitution_attempts: 1,
                substitution_misses: 0,
                text_responses: 1,
                zero_sigil_responses: 0,
                table_runs: 1,
                malformed_table_runs: 1,
            },
            HealthRecord {
                schema_version: 2,
                provider: Provider::Anthropic,
                model: "claude-health".into(),
                skill_version: Some("1.1.0".into()),
                session_id: Some("s".into()),
                session_turn: Some(5),
                substitution_attempts: 0,
                substitution_misses: 0,
                text_responses: 0,
                zero_sigil_responses: 0,
                table_runs: 0,
                malformed_table_runs: 0,
            },
        ]
        .into_iter()
        .map(|record| serde_json::to_string(&record).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let stats = command(&root).arg("stats").output().unwrap();
    assert!(stats.status.success());
    let stats_stdout = String::from_utf8(stats.stdout).unwrap();
    assert!(stats_stdout.contains("cache_observed_records: 2"));
    assert!(stats_stdout.contains("cache_read_records: 1"));
    assert!(stats_stdout.contains("cache_miss_records: 1"));
    assert!(stats_stdout.contains("cache_read_rate_trend: -0.500000"));
    assert!(stats_stdout.contains("cache_miss_observed_billing_usd: 5.000000"));
    assert!(stats_stdout.contains("next_turn_estimated_billing_usd: 5.000200"));
    assert!(stats_stdout.contains(
        "model_health: provider=anthropic model=claude-health substitution_attempts=0 substitution_misses=0 substitution_miss_rate=unavailable text_responses=0 zero_sigil_responses=0 zero_sigil_rate=unavailable"
    ));
    assert!(stats_stdout.contains(
        "model_health: provider=openai model=gpt-health substitution_attempts=3 substitution_misses=1 substitution_miss_rate=0.333333 text_responses=2 zero_sigil_responses=1 zero_sigil_rate=0.500000"
    ));
    assert!(stats_stdout.contains(
        "format_health: provider=openai model=gpt-health skill_version=1.0.1 text_responses=1 zero_sigil_responses=1 zero_sigil_rate=1.000000 table_runs=1 malformed_table_runs=0 malformed_table_rate=0.000000 baseline_skill_version=unavailable"
    ));
    assert!(stats_stdout.contains(
        "format_health: provider=openai model=gpt-health skill_version=1.1.0 text_responses=1 zero_sigil_responses=0 zero_sigil_rate=0.000000 table_runs=1 malformed_table_runs=1 malformed_table_rate=1.000000 baseline_skill_version=1.0.1 baseline_text_responses=1 baseline_zero_sigil_rate=1.000000 zero_sigil_rate_delta=-1.000000 baseline_table_runs=1 baseline_malformed_table_rate=0.000000 malformed_table_rate_delta=+1.000000 assessment=insufficient_samples"
    ));
    assert!(!stats_stdout.contains("correct: "));

    let outcomes = root.join("outcomes.json");
    fs::write(
        &outcomes,
        r#"[{"session_id":"s","completed":true,"correct":true}]"#,
    )
    .unwrap();
    let readiness = command(&root)
        .args([
            "benchmark",
            "readiness-report",
            transcript.to_str().unwrap(),
            outcomes.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(readiness.status.success());
    let readiness_stdout = String::from_utf8(readiness.stdout).unwrap();
    assert!(readiness_stdout.contains("\"raw_composite\""));
    assert!(readiness_stdout.contains("\"unlabelled_sessions\": 0"));

    let readiness_analysis = command(&root)
        .args([
            "benchmark",
            "readiness-analysis",
            transcript.to_str().unwrap(),
            outcomes.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(readiness_analysis.status.success());
    assert!(
        String::from_utf8(readiness_analysis.stdout)
            .unwrap()
            .contains("\"stop_insufficient_evidence\"")
    );

    let thrash = TranscriptRecord::new(
        "t".into(),
        "s".into(),
        "assistant".into(),
        ". done".into(),
        None,
    )
    .with_receipts(vec![brief_bright_gone::signals::thrash_receipt(
        brief_bright_gone::session::ThrashObservation {
            exact_repeated_tool_results: 1,
            expensive_exact_repeated_tool_results: 1,
            near_repeated_tool_calls: 1,
            ..Default::default()
        },
    )]);
    fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .and_then(|mut file| writeln!(file, "{}", serde_json::to_string(&thrash).unwrap()))
        .unwrap();
    let thrash_report = command(&root)
        .args(["benchmark", "thrash-report", transcript.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(thrash_report.status.success());
    let thrash_stdout = String::from_utf8(thrash_report.stdout).unwrap();
    assert!(thrash_stdout.contains("\"score\": 2"));
    assert!(thrash_stdout.contains("\"expensive_exact_repeated_tool_results\": 1"));

    let decision = TranscriptRecord::new(
        "t".into(),
        "s".into(),
        "assistant".into(),
        "? Which target is missing?".into(),
        None,
    )
    .with_session_turn(1)
    .with_receipts(vec![brief_bright_gone::signals::terminal_receipt(
        "? Which target is missing?",
    )]);
    let reply = TranscriptRecord::new(
        "t".into(),
        "s".into(),
        "user".into(),
        "Fix src/lib.rs. Preserve compatibility. Done when tests pass.".into(),
        None,
    )
    .with_session_turn(2)
    .with_receipts(
        brief_bright_gone::signals::readiness_receipts_for_conversation(
            "Fix src/lib.rs. Preserve compatibility. Done when tests pass.",
            false,
        ),
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .and_then(|mut file| {
            writeln!(file, "{}", serde_json::to_string(&decision).unwrap())?;
            writeln!(file, "{}", serde_json::to_string(&reply).unwrap())
        })
        .unwrap();
    let clarification = command(&root)
        .args([
            "benchmark",
            "clarification-report",
            transcript.to_str().unwrap(),
            outcomes.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(clarification.status.success());
    let clarification_stdout = String::from_utf8(clarification.stdout).unwrap();
    assert!(clarification_stdout.contains("\"question_turn\": 1"));
    assert!(clarification_stdout.contains("\"reply_turn\": 2"));
    assert!(clarification_stdout.contains("\"reply_turn_observed_billing_usd\": 0.0002"));

    // An explicit but unreadable path errors instead of printing a zero report.
    let missing_report = command(&root)
        .args(["benchmark", "report", "/no/such/transcript.jsonl"])
        .output()
        .unwrap();
    assert_eq!(missing_report.status.code(), Some(1));
    assert!(
        String::from_utf8(missing_report.stderr)
            .unwrap()
            .contains("error: read")
    );

    let get = command(&root).args(["get", &digest]).output().unwrap();
    assert!(get.status.success());
    assert_eq!(get.stdout, original);

    let missing = command(&root)
        .args(["get", "not-a-digest"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        String::from_utf8(missing.stderr)
            .unwrap()
            .contains("cannot be recovered safely")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn bbg_proxy_binary_starts_and_serves_health_on_an_ephemeral_port() {
    let root = temporary_root("proxy-binary");
    fs::create_dir_all(&root).unwrap();

    // Reserve a free loopback port, then release it for the child to bind.
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let mut child: Child = Command::new(env!("CARGO_BIN_EXE_bbg-proxy"))
        .env_clear()
        .env("BBG_STORE_DIR", root.join("store"))
        .env("BBG_BIND", "127.0.0.1")
        .env("BBG_PORT", port.to_string())
        // Unused by /health; must still be a valid upstream for startup.
        .env("BBG_UPSTREAM_URL", "http://127.0.0.1:9/v1")
        .env("BBG_TRANSCRIPT", "0")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut healthy = false;
    for _ in 0..100 {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let _ = stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
            let mut response = String::new();
            if stream.read_to_string(&mut response).is_ok() && response.starts_with("HTTP/1.1 200")
            {
                healthy = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        healthy,
        "bbg-proxy did not serve /health on the ephemeral port"
    );
    let _ = fs::remove_dir_all(root);
}
