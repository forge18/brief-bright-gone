//! bbg CLI — pipe-friendly, agent-agnostic interface.
//!
//! Subcommands:
//!   bbg detect [path|-]     print detected content type
//!   bbg normalize [path|-]  normalize prose from stdin/file to stdout
//!   bbg stats [path|-]      bytes/tokens before->after
//!   bbg run -- <cmd> …      run an agent with its base URL pointed at bbg
//!   bbg version             print version

use std::io::{self, Read, Write};

/// The base-URL environment variables bbg sets for a wrapped agent. These are
/// the same variables `bbg doctor` treats as the endpoint-override signal.
const BASE_URL_ENV_VARS: [&str; 3] = ["ANTHROPIC_BASE_URL", "OPENAI_BASE_URL", "BASE_URL"];

/// Build the proxy base URL a wrapped agent should target, from the same
/// `BBG_BIND`/`BBG_PORT` the proxy binds to. The proxy routes live under
/// `/v1`, so that suffix is included.
fn proxy_base_url() -> String {
    proxy_base_url_from(std::env::var("BBG_BIND").ok(), std::env::var("BBG_PORT").ok())
}

/// Pure core of [`proxy_base_url`], with the environment lookups lifted out so
/// the formatting has one implementation and can be tested directly.
fn proxy_base_url_from(bind: Option<String>, port: Option<String>) -> String {
    let bind = bind.unwrap_or_else(|| "127.0.0.1".into());
    let port = port.unwrap_or_else(|| "8088".into());
    format!("http://{bind}:{port}/v1")
}

/// Resolve which base-URL variables to inject for the child. A variable the
/// user has already exported is left untouched so an explicit value always
/// wins; only unset variables are filled with `base_url`. `lookup` reads the
/// current environment (injected in tests).
fn base_url_overrides<'a>(
    base_url: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<(&'a str, String)> {
    BASE_URL_ENV_VARS
        .iter()
        .filter(|name| lookup(name).is_none())
        .map(|name| (*name, base_url.to_owned()))
        .collect()
}

fn read_input(path: &Option<String>) -> Result<String, String> {
    let mut buf = String::new();
    if let Some(p) = path
        && p != "-"
    {
        return std::fs::read_to_string(p).map_err(|e| format!("read {}: {}", p, e));
    }
    io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("read stdin: {e}"))?;
    Ok(buf)
}

fn write_then_mark_served<W: Write>(
    store: &brief_bright_gone::store::Store,
    reference: &str,
    bytes: &[u8],
    writer: &mut W,
    served_at_secs: u64,
) -> io::Result<()> {
    writer.write_all(bytes)?;
    writer.flush()?;
    store.mark_served(reference, served_at_secs)
}

fn rate(numerator: u64, denominator: u64) -> Option<f64> {
    (denominator > 0).then_some(numerator as f64 / denominator as f64)
}

fn format_rate(rate: Option<f64>) -> String {
    rate.map(|value| format!("{value:.6}"))
        .unwrap_or_else(|| "unavailable".into())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let rest = &args[1..];
    let path = rest.first().cloned();

    match cmd {
        "skill" => {
            print!("{}", brief_bright_gone::skill::SKILL);
        }
        "install" | "upgrade" => {
            let mut paths = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                if rest[i] == "--path" {
                    if let Some(p) = rest.get(i + 1) {
                        paths.push(std::path::PathBuf::from(p));
                        i += 1;
                    } else {
                        eprintln!("error: --path requires a directory");
                        std::process::exit(2);
                    }
                }
                i += 1;
            }
            let result = if cmd == "upgrade" {
                brief_bright_gone::skill::upgrade()
            } else {
                brief_bright_gone::skill::install(&paths)
            };
            match result {
                Ok(ps) => {
                    for p in ps {
                        println!("{}", p.display());
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        "uninstall" => match brief_bright_gone::skill::uninstall() {
            Ok(ps) => {
                for p in ps {
                    println!("{}", p.display())
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1)
            }
        },
        "doctor" => {
            let m = brief_bright_gone::skill::manifest().unwrap_or_default();
            let current = m
                .entries
                .iter()
                .filter(|e| e.skill_version == brief_bright_gone::skill::SKILL_VERSION)
                .count();
            let store_root = std::env::var("BBG_STORE_DIR").unwrap_or_else(|_| {
                brief_bright_gone::store::default_store_dir()
                    .to_string_lossy()
                    .into_owned()
            });
            // Open through the store's own discipline (owner-only 0700) and probe
            // writability without leaking a probe file behind.
            let writable = (|| -> Result<(), String> {
                let store = brief_bright_gone::store::Store::open(&store_root)
                    .map_err(|error| format!("open store: {error}"))?;
                let probe = std::path::PathBuf::from(&store_root).join(".doctor");
                let mut file = brief_bright_gone::private_fs::open_private_append(&probe)
                    .map_err(|error| format!("probe write: {error}"))?;
                use std::io::Write;
                file.write_all(b"probe")
                    .map_err(|error| format!("probe write: {error}"))?;
                drop(file);
                std::fs::remove_file(&probe).map_err(|error| format!("probe cleanup: {error}"))?;
                let _ = store;
                Ok(())
            })()
            .is_ok();
            let endpoint = std::env::var("BASE_URL")
                .or_else(|_| std::env::var("ANTHROPIC_BASE_URL"))
                .is_ok();
            println!(
                "skill_current: {}",
                if current > 0 { "pass" } else { "fail" }
            );
            println!(
                "proxy_reachable: {}",
                if std::env::var("BBG_PROXY_URL").is_ok() {
                    "pass"
                } else {
                    "fail (set BBG_PROXY_URL)"
                }
            );
            println!(
                "endpoint_override: {}",
                if endpoint {
                    "pass"
                } else {
                    "fail (set BASE_URL or ANTHROPIC_BASE_URL)"
                }
            );
            println!("store_writable: {}", if writable { "pass" } else { "fail" });
            if current == 0 || !writable {
                std::process::exit(1)
            }
        }
        "lint" => {
            let transcript = rest.iter().position(|x| x == "--transcript");
            let input_path = transcript
                .and_then(|i| rest.get(i + 1))
                .or_else(|| rest.first());
            let input = match read_input(&input_path.cloned()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1)
                }
            };
            let findings = if transcript.is_some() {
                let rows = input
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect::<Vec<brief_bright_gone::transcript::TranscriptRecord>>();
                brief_bright_gone::lint::lint_transcript(&rows)
            } else {
                brief_bright_gone::lint::lint_document(&input)
            };
            for f in &findings {
                println!(
                    "{}{}: {}",
                    if f.heuristic { "heuristic " } else { "" },
                    f.rule,
                    f.message
                );
            }
            if !findings.is_empty() {
                std::process::exit(1)
            }
        }
        "benchmark" => {
            if rest.first().map(String::as_str) == Some("report") {
                let p = rest.get(1).cloned();
                let rows = match p.as_deref() {
                    // Empty stdin is a valid empty report; an explicit path that
                    // cannot be read is an error, not a zero report.
                    None | Some("-") => read_input(&p)
                        .unwrap_or_default()
                        .lines()
                        .filter_map(|line| serde_json::from_str(line).ok())
                        .collect(),
                    Some(path) => {
                        match brief_bright_gone::transcript::read_external(std::path::Path::new(
                            path,
                        )) {
                            Ok(rows) => rows,
                            Err(error) => {
                                eprintln!("error: read {path}: {error}");
                                std::process::exit(1);
                            }
                        }
                    }
                };
                // Cost is joined from the store's cost ledger by session id
                // (§10: "billed dollars per task, from provider usage fields").
                // A missing or unreadable ledger is not an error here — an
                // empty cost join, not a report failure — since a caller
                // benchmarking lint/turns alone may have no cost ledger at all.
                let store_root = std::env::var("BBG_STORE_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| brief_bright_gone::store::default_store_dir());
                let costs = brief_bright_gone::operations::read_cost_records(
                    &store_root.join("ledger").join("costs.jsonl"),
                )
                .unwrap_or_default();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&brief_bright_gone::benchmark::report(
                        &rows, &costs
                    ))
                    .unwrap()
                );
            } else if rest.first().map(String::as_str) == Some("clarification-report") {
                let Some(transcript_path) = rest.get(1) else {
                    eprintln!(
                        "usage: bbg benchmark clarification-report <transcript.jsonl> [outcomes.json]"
                    );
                    std::process::exit(2);
                };
                let rows = match brief_bright_gone::transcript::read_external(std::path::Path::new(
                    transcript_path,
                )) {
                    Ok(rows) => rows,
                    Err(error) => {
                        eprintln!("error: read {transcript_path}: {error}");
                        std::process::exit(1);
                    }
                };
                let labels = match rest.get(2) {
                    None => Vec::new(),
                    Some(path) => match std::fs::read_to_string(path)
                        .map_err(|error| format!("read {path}: {error}"))
                        .and_then(|input| {
                            serde_json::from_str::<
                                Vec<brief_bright_gone::benchmark::TaskOutcomeLabel>,
                            >(&input)
                            .map_err(|error| format!("parse {path}: {error}"))
                        }) {
                        Ok(labels) => labels,
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    },
                };
                let store_root = std::env::var("BBG_STORE_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| brief_bright_gone::store::default_store_dir());
                let costs = brief_bright_gone::operations::read_cost_records(
                    &store_root.join("ledger").join("costs.jsonl"),
                )
                .unwrap_or_default();
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &brief_bright_gone::benchmark::clarification_report(&rows, &costs, &labels)
                    )
                    .unwrap()
                );
            } else if rest.first().map(String::as_str) == Some("thrash-report") {
                let Some(transcript_path) = rest.get(1) else {
                    eprintln!("usage: bbg benchmark thrash-report <transcript.jsonl>");
                    std::process::exit(2);
                };
                let rows = match brief_bright_gone::transcript::read_external(std::path::Path::new(
                    transcript_path,
                )) {
                    Ok(rows) => rows,
                    Err(error) => {
                        eprintln!("error: read {transcript_path}: {error}");
                        std::process::exit(1);
                    }
                };
                let store_root = std::env::var("BBG_STORE_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| brief_bright_gone::store::default_store_dir());
                let costs = brief_bright_gone::operations::read_cost_records(
                    &store_root.join("ledger").join("costs.jsonl"),
                )
                .unwrap_or_default();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&brief_bright_gone::benchmark::thrash_report(
                        &rows, &costs
                    ))
                    .unwrap()
                );
            } else if rest.first().map(String::as_str) == Some("terminal-report") {
                let Some(transcript_path) = rest.get(1) else {
                    eprintln!(
                        "usage: bbg benchmark terminal-report <transcript.jsonl> [outcomes.json]"
                    );
                    std::process::exit(2);
                };
                let rows = match brief_bright_gone::transcript::read_external(std::path::Path::new(
                    transcript_path,
                )) {
                    Ok(rows) => rows,
                    Err(error) => {
                        eprintln!("error: read {transcript_path}: {error}");
                        std::process::exit(1);
                    }
                };
                let labels = match rest.get(2) {
                    None => Vec::new(),
                    Some(path) => match std::fs::read_to_string(path)
                        .map_err(|error| format!("read {path}: {error}"))
                        .and_then(|input| {
                            serde_json::from_str(&input)
                                .map_err(|error| format!("parse {path}: {error}"))
                        }) {
                        Ok(labels) => labels,
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    },
                };
                let store_root = std::env::var("BBG_STORE_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| brief_bright_gone::store::default_store_dir());
                let costs = brief_bright_gone::operations::read_cost_records(
                    &store_root.join("ledger").join("costs.jsonl"),
                )
                .unwrap_or_default();
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &brief_bright_gone::benchmark::terminal_trajectory_report(
                            &rows, &costs, &labels
                        )
                    )
                    .unwrap()
                );
            } else if rest.first().map(String::as_str) == Some("experiment-report") {
                let Some(transcript_path) = rest.get(1) else {
                    eprintln!(
                        "usage: bbg benchmark experiment-report <transcript.jsonl> [outcomes.json]"
                    );
                    std::process::exit(2);
                };
                let rows = match brief_bright_gone::transcript::read_external(std::path::Path::new(
                    transcript_path,
                )) {
                    Ok(rows) => rows,
                    Err(error) => {
                        eprintln!("error: read {transcript_path}: {error}");
                        std::process::exit(1);
                    }
                };
                let labels = match rest.get(2) {
                    None => Vec::new(),
                    Some(path) => match std::fs::read_to_string(path)
                        .map_err(|error| format!("read {path}: {error}"))
                        .and_then(|input| {
                            serde_json::from_str::<
                                Vec<brief_bright_gone::benchmark::TaskOutcomeLabel>,
                            >(&input)
                            .map_err(|error| format!("parse {path}: {error}"))
                        }) {
                        Ok(labels) => labels,
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    },
                };
                let store_root = std::env::var("BBG_STORE_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| brief_bright_gone::store::default_store_dir());
                let costs = brief_bright_gone::operations::read_cost_records(
                    &store_root.join("ledger").join("costs.jsonl"),
                )
                .unwrap_or_default();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&brief_bright_gone::benchmark::experiment_report(
                        &rows, &costs, &labels
                    ))
                    .unwrap()
                );
            } else if rest.first().map(String::as_str) == Some("readiness-analysis") {
                let Some(transcript_path) = rest.get(1) else {
                    eprintln!(
                        "usage: bbg benchmark readiness-analysis <transcript.jsonl> [outcomes.json]"
                    );
                    std::process::exit(2);
                };
                let rows = match brief_bright_gone::transcript::read_external(std::path::Path::new(
                    transcript_path,
                )) {
                    Ok(rows) => rows,
                    Err(error) => {
                        eprintln!("error: read {transcript_path}: {error}");
                        std::process::exit(1);
                    }
                };
                let labels = match rest.get(2) {
                    None => Vec::new(),
                    Some(path) => match std::fs::read_to_string(path)
                        .map_err(|error| format!("read {path}: {error}"))
                        .and_then(|input| {
                            serde_json::from_str::<
                                Vec<brief_bright_gone::benchmark::TaskOutcomeLabel>,
                            >(&input)
                            .map_err(|error| format!("parse {path}: {error}"))
                        }) {
                        Ok(labels) => labels,
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    },
                };
                let store_root = std::env::var("BBG_STORE_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| brief_bright_gone::store::default_store_dir());
                let costs = brief_bright_gone::operations::read_cost_records(
                    &store_root.join("ledger").join("costs.jsonl"),
                )
                .unwrap_or_default();
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &brief_bright_gone::benchmark::readiness_held_out_analysis(
                            &rows, &costs, &labels
                        )
                    )
                    .unwrap()
                );
            } else if rest.first().map(String::as_str) == Some("readiness-report") {
                let Some(transcript_path) = rest.get(1) else {
                    eprintln!(
                        "usage: bbg benchmark readiness-report <transcript.jsonl> [outcomes.json]"
                    );
                    std::process::exit(2);
                };
                let rows = match brief_bright_gone::transcript::read_external(std::path::Path::new(
                    transcript_path,
                )) {
                    Ok(rows) => rows,
                    Err(error) => {
                        eprintln!("error: read {transcript_path}: {error}");
                        std::process::exit(1);
                    }
                };
                let labels = match rest.get(2) {
                    None => Vec::new(),
                    Some(path) => match std::fs::read_to_string(path)
                        .map_err(|error| format!("read {path}: {error}"))
                        .and_then(|input| {
                            serde_json::from_str::<
                                Vec<brief_bright_gone::benchmark::TaskOutcomeLabel>,
                            >(&input)
                            .map_err(|error| format!("parse {path}: {error}"))
                        }) {
                        Ok(labels) => labels,
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    },
                };
                let store_root = std::env::var("BBG_STORE_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| brief_bright_gone::store::default_store_dir());
                let costs = brief_bright_gone::operations::read_cost_records(
                    &store_root.join("ledger").join("costs.jsonl"),
                )
                .unwrap_or_default();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&brief_bright_gone::benchmark::readiness_report(
                        &rows, &costs, &labels
                    ),)
                    .unwrap()
                );
            } else {
                eprintln!(
                    "usage: bbg benchmark report <transcript.jsonl>\n       bbg benchmark readiness-report <transcript.jsonl> [outcomes.json]\n       bbg benchmark readiness-analysis <transcript.jsonl> [outcomes.json]\n       bbg benchmark experiment-report <transcript.jsonl> [outcomes.json]\n       bbg benchmark terminal-report <transcript.jsonl> [outcomes.json]\n       bbg benchmark thrash-report <transcript.jsonl>\n       bbg benchmark clarification-report <transcript.jsonl> [outcomes.json]"
                );
                std::process::exit(2)
            }
        }
        "detect" => {
            let input = match read_input(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            };
            println!("{}", brief_bright_gone::detect::detect(&input).name());
        }
        "normalize" => {
            let input = match read_input(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            };
            let opts = brief_bright_gone::normalize::NormalizeOptions::default();
            let out = brief_bright_gone::normalize::normalize_with_detect(&input, &opts);
            let mut stdout = io::stdout();
            if stdout.write_all(out.text.as_bytes()).is_err() {
                std::process::exit(1);
            }
            if !out.text.ends_with('\n') {
                let _ = stdout.write_all(b"\n");
            }
        }
        "stats" => {
            let root = std::env::var("BBG_STORE_DIR").unwrap_or_else(|_| {
                brief_bright_gone::store::default_store_dir()
                    .to_string_lossy()
                    .into_owned()
            });
            let ledger = std::path::Path::new(&root)
                .join("ledger")
                .join("costs.jsonl");
            let records = match brief_bright_gone::operations::read_cost_records(&ledger) {
                Ok(records) => records,
                Err(error) => {
                    eprintln!("error: read cost ledger: {error}");
                    std::process::exit(1);
                }
            };
            let health_records = brief_bright_gone::operations::read_health_records(
                &std::path::Path::new(&root)
                    .join("ledger")
                    .join("health.jsonl"),
            )
            .unwrap_or_default();
            let model_health = brief_bright_gone::operations::model_health(&health_records);
            let format_health = brief_bright_gone::operations::format_health(&health_records);
            let observed_usage_records = records.len();
            let observed_billing: f64 = records
                .iter()
                .filter_map(|record| record.observed_billing_usd)
                .sum::<f64>()
                .max(0.0);
            let priced_records = records
                .iter()
                .filter(|record| record.observed_billing_usd.is_some())
                .count();
            let estimated_savings: f64 = records
                .iter()
                .filter_map(|record| record.estimated_savings_usd)
                .sum::<f64>()
                .max(0.0);
            let estimate_records = records
                .iter()
                .filter(|record| record.estimated_savings_usd.is_some())
                .count();
            let cache_health = brief_bright_gone::operations::cache_health(&records);
            let cost_burn = brief_bright_gone::operations::cost_burn_projection(&records);
            println!("observed_usage_records: {observed_usage_records}");
            println!("observed_billing_usd: {observed_billing:.6}");
            println!("observed_billing_priced_records: {priced_records}");
            println!("estimated_savings_usd: {estimated_savings:.6}");
            println!("estimated_savings_records: {estimate_records}");
            println!("cache_observed_records: {}", cache_health.observed_records);
            println!("cache_read_records: {}", cache_health.cache_read_records);
            println!("cache_miss_records: {}", cache_health.cache_miss_records);
            println!("cache_read_tokens: {}", cache_health.cache_read_tokens);
            match cache_health.cache_read_rate {
                Some(rate) => println!("cache_read_rate: {rate:.6}"),
                None => println!("cache_read_rate: unavailable"),
            }
            match cache_health.cache_read_rate_trend {
                Some(trend) => println!("cache_read_rate_trend: {trend:+.6}"),
                None => println!("cache_read_rate_trend: unavailable"),
            }
            println!(
                "cache_read_to_miss_sessions: {}",
                cache_health.cache_read_to_miss_sessions
            );
            println!(
                "cache_miss_observed_billing_usd: {:.6}",
                cache_health.cache_miss_observed_billing_usd
            );
            println!("active_cost_sessions: {}", cost_burn.active_cost_sessions);
            match cost_burn.session_median_observed_billing_usd {
                Some(median) => println!("session_median_observed_billing_usd: {median:.6}"),
                None => println!("session_median_observed_billing_usd: unavailable"),
            }
            println!(
                "sessions_above_three_x_median: {}",
                cost_burn.sessions_above_three_x_median
            );
            match cost_burn.next_turn_estimated_billing_usd {
                Some(estimate) => println!("next_turn_estimated_billing_usd: {estimate:.6}"),
                None => println!("next_turn_estimated_billing_usd: unavailable"),
            }
            match cost_burn.projected_billing_after_next_turn_usd {
                Some(projection) => {
                    println!("projected_billing_after_next_turn_usd: {projection:.6}")
                }
                None => println!("projected_billing_after_next_turn_usd: unavailable"),
            }
            for health in model_health {
                let miss_rate = (health.substitution_attempts > 0).then_some(
                    health.substitution_misses as f64 / health.substitution_attempts as f64,
                );
                let zero_rate = (health.text_responses > 0)
                    .then_some(health.zero_sigil_responses as f64 / health.text_responses as f64);
                let malformed_table_rate = rate(health.malformed_table_runs, health.table_runs);
                println!(
                    "model_health: provider={} model={} substitution_attempts={} substitution_misses={} substitution_miss_rate={} text_responses={} zero_sigil_responses={} zero_sigil_rate={} table_runs={} malformed_table_runs={} malformed_table_rate={}",
                    health.provider,
                    health.model,
                    health.substitution_attempts,
                    health.substitution_misses,
                    format_rate(miss_rate),
                    health.text_responses,
                    health.zero_sigil_responses,
                    format_rate(zero_rate),
                    health.table_runs,
                    health.malformed_table_runs,
                    format_rate(malformed_table_rate),
                );
            }
            for health in format_health {
                let zero_rate = rate(health.zero_sigil_responses, health.text_responses);
                let malformed_table_rate = rate(health.malformed_table_runs, health.table_runs);
                let baseline_zero_rate = health
                    .baseline_text_responses
                    .zip(health.baseline_zero_sigil_responses)
                    .and_then(|(denominator, numerator)| rate(numerator, denominator));
                let baseline_malformed_table_rate = health
                    .baseline_table_runs
                    .zip(health.baseline_malformed_table_runs)
                    .and_then(|(denominator, numerator)| rate(numerator, denominator));
                let zero_delta = zero_rate
                    .zip(baseline_zero_rate)
                    .map(|(current, baseline)| current - baseline);
                let malformed_table_delta = malformed_table_rate
                    .zip(baseline_malformed_table_rate)
                    .map(|(current, baseline)| current - baseline);
                let assessment = match brief_bright_gone::operations::format_health_assessment(
                    &health,
                ) {
                    brief_bright_gone::operations::FormatHealthAssessment::NoBaseline => {
                        "no_baseline"
                    }
                    brief_bright_gone::operations::FormatHealthAssessment::InsufficientSamples => {
                        "insufficient_samples"
                    }
                    brief_bright_gone::operations::FormatHealthAssessment::Monitoring => {
                        "monitoring"
                    }
                    brief_bright_gone::operations::FormatHealthAssessment::RollbackRecommended => {
                        "rollback_recommended"
                    }
                };
                println!(
                    "format_health: provider={} model={} skill_version={} text_responses={} zero_sigil_responses={} zero_sigil_rate={} table_runs={} malformed_table_runs={} malformed_table_rate={} baseline_skill_version={} baseline_text_responses={} baseline_zero_sigil_rate={} zero_sigil_rate_delta={} baseline_table_runs={} baseline_malformed_table_rate={} malformed_table_rate_delta={} assessment={}",
                    health.provider,
                    health.model,
                    health.skill_version,
                    health.text_responses,
                    health.zero_sigil_responses,
                    format_rate(zero_rate),
                    health.table_runs,
                    health.malformed_table_runs,
                    format_rate(malformed_table_rate),
                    health
                        .baseline_skill_version
                        .as_deref()
                        .unwrap_or("unavailable"),
                    health
                        .baseline_text_responses
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unavailable".into()),
                    format_rate(baseline_zero_rate),
                    zero_delta
                        .map(|value| format!("{value:+.6}"))
                        .unwrap_or_else(|| "unavailable".into()),
                    health
                        .baseline_table_runs
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unavailable".into()),
                    format_rate(baseline_malformed_table_rate),
                    malformed_table_delta
                        .map(|value| format!("{value:+.6}"))
                        .unwrap_or_else(|| "unavailable".into()),
                    assessment,
                );
            }
            println!(
                "note: observed billing is derived from provider-reported usage and local pricing; cache churn is an observation, not proof of prefix instability; model health is protocol/lookup health, not correctness; format-health comparisons are observational per provider/model/skill version, require at least 40 text responses in both versions, and recommend—not automatically perform—rollback; projections are deterministic estimates, not provider-billed facts"
            );
        }
        "get" => {
            let Some(reference) = rest.first() else {
                eprintln!("error: bbg get requires a content reference");
                std::process::exit(2);
            };
            let root = std::env::var("BBG_STORE_DIR").unwrap_or_else(|_| {
                brief_bright_gone::store::default_store_dir()
                    .to_string_lossy()
                    .into_owned()
            });
            let store = brief_bright_gone::store::Store::open(root).unwrap_or_else(|error| {
                eprintln!("error: open CCR store: {error}");
                std::process::exit(1);
            });
            match store.get(reference) {
                Ok(Some(bytes)) => {
                    let mut stdout = io::stdout().lock();
                    let now = brief_bright_gone::compress::unix_now_secs();
                    // Only a completed write and flush counts as served. A
                    // broken pipe must not create a false recovery exemption.
                    if let Err(error) =
                        write_then_mark_served(&store, reference, &bytes, &mut stdout, now)
                    {
                        eprintln!("error: cannot serve reference {reference}: {error}");
                        std::process::exit(1);
                    }
                }
                Ok(None) => {
                    eprintln!(
                        "error: reference {reference} is not held; re-read the source to recover its bytes"
                    );
                    std::process::exit(1);
                }
                Err(error) => {
                    eprintln!("error: reference {reference} cannot be recovered safely: {error}");
                    std::process::exit(1);
                }
            }
        }
        "run" => {
            // Everything after `--` is the agent command and its arguments.
            let Some(split) = rest.iter().position(|arg| arg == "--") else {
                eprintln!("usage: bbg run -- <agent-command> [args…]");
                std::process::exit(2);
            };
            let command = &rest[split + 1..];
            let Some((program, program_args)) = command.split_first() else {
                eprintln!("usage: bbg run -- <agent-command> [args…]");
                std::process::exit(2);
            };
            let base_url = proxy_base_url();
            let overrides =
                base_url_overrides(&base_url, |name| std::env::var(name).ok());
            let status = std::process::Command::new(program)
                .args(program_args)
                .envs(overrides)
                .status();
            match status {
                Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                Err(error) => {
                    eprintln!("error: could not run {program}: {error}");
                    std::process::exit(1);
                }
            }
        }
        "version" | "--version" | "-V" => {
            println!("bbg {}", env!("CARGO_PKG_VERSION"));
        }
        _ => {
            eprintln!("bbg {} — brief, bright, gone", env!("CARGO_PKG_VERSION"));
            eprintln!();
            eprintln!("USAGE: bbg <command> [path|-]   (default input: stdin)");
            eprintln!();
            eprintln!("commands:");
            eprintln!(
                "  detect      print detected content type (json|code|log|diff|search-result|text|tabular|terminal)"
            );
            eprintln!("  normalize   normalize prose from stdin/file to stdout (skips code/shell)");
            eprintln!(
                "  stats       provider usage billing and transform estimates from the local ledger"
            );
            eprintln!("  get <ref>   recover original bytes from the local CCR store");
            eprintln!("  skill       print the versioned skill");
            eprintln!("  install --path <dir> | upgrade | uninstall");
            eprintln!("  doctor      check installation and endpoint configuration");
            eprintln!(
                "  run -- <cmd> run an agent with its base URL pointed at bbg (needs a running bbg-proxy; works with any agent that honors standard base-URL environment variables)"
            );
            eprintln!("  lint [--transcript <file>] [file|-]");
            eprintln!(
                "  benchmark <report|readiness-report|readiness-analysis|experiment-report|terminal-report|thrash-report|clarification-report> <transcript.jsonl> [outcomes.json]"
            );
            eprintln!("  version     print version");
            eprintln!();
            eprintln!("example:");
            eprintln!("  echo 'please   fix the  bug, thank you' | bbg normalize");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FlushFailure(Vec<u8>);

    impl Write for FlushFailure {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "test flush failure",
            ))
        }
    }

    fn store() -> brief_bright_gone::store::Store {
        brief_bright_gone::store::Store::open(std::env::temp_dir().join(format!(
            "bbg-cli-test-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        )))
        .unwrap()
    }

    #[test]
    fn base_url_overrides_fill_only_unset_variables() {
        // Nothing set: all three base-URL vars are injected.
        let none = base_url_overrides("http://127.0.0.1:8088/v1", |_| None);
        assert_eq!(none.len(), 3);
        assert!(
            none.iter()
                .all(|(_, value)| value == "http://127.0.0.1:8088/v1")
        );

        // A user-set value wins: that variable is left untouched, the rest fill.
        let some = base_url_overrides("http://127.0.0.1:8088/v1", |name| {
            (name == "ANTHROPIC_BASE_URL").then(|| "http://user-set:9999".to_owned())
        });
        assert_eq!(some.len(), 2);
        assert!(some.iter().all(|(name, _)| *name != "ANTHROPIC_BASE_URL"));

        // All set: nothing is injected.
        let all = base_url_overrides("http://127.0.0.1:8088/v1", |_| Some("x".to_owned()));
        assert!(all.is_empty());
    }

    #[test]
    fn proxy_base_url_uses_bind_and_port_with_v1_suffix() {
        // Defaults when unset.
        assert_eq!(
            base_url_overrides(&proxy_base_url_from(None, None), |_| None)[0].1,
            "http://127.0.0.1:8088/v1"
        );
        // Honors BBG_BIND / BBG_PORT.
        assert_eq!(
            proxy_base_url_from(Some("0.0.0.0".into()), Some("9000".into())),
            "http://0.0.0.0:9000/v1"
        );
    }

    #[test]
    fn marks_reference_only_after_writer_flushes_successfully() {
        let store = store();
        let digest = store.put(b"recover").unwrap();
        let mut failing = FlushFailure(Vec::new());
        assert!(write_then_mark_served(&store, &digest, b"recover", &mut failing, 100).is_err());
        assert!(!store.was_served_recent(&digest, 100, 1).unwrap());

        let mut output = Vec::new();
        write_then_mark_served(&store, &digest, b"recover", &mut output, 100).unwrap();
        assert_eq!(output, b"recover");
        assert!(store.was_served_recent(&digest, 100, 1).unwrap());
    }
}
