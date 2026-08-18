use brief_bright_gone::{
    operations::CostRecord,
    store::Store,
    transcript::TranscriptRecord,
    types::{Provider, Usage},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
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
    let record = TranscriptRecord::new("t".into(), "s".into(), "user".into(), "ok".into(), None);
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
            input_tokens: Some(100),
            output_tokens: Some(50),
            ..Default::default()
        },
        Some(&brief_bright_gone::operations::ProviderPricing {
            input_per_million_usd: 1.0,
            output_per_million_usd: 2.0,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        }),
    );
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
