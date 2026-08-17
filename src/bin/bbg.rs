//! bbg CLI — pipe-friendly, agent-agnostic interface.
//!
//! Subcommands:
//!   bbg detect [path|-]     print detected content type
//!   bbg normalize [path|-]  normalize prose from stdin/file to stdout
//!   bbg stats [path|-]      bytes/tokens before->after
//!   bbg version             print version

use std::io::{self, Read, Write};

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
            let store = std::env::var("BBG_STORE_DIR").unwrap_or_else(|_| ".bbg-store".into());
            let writable = std::fs::create_dir_all(&store).is_ok()
                && std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(std::path::Path::new(&store).join(".doctor"))
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
                let rows = input.lines().map(str::to_owned).collect::<Vec<_>>();
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
                let text = read_input(&p).unwrap_or_default();
                let rows = brief_bright_gone::transcript::read(std::path::Path::new(
                    p.as_deref().unwrap_or("-"),
                ))
                .unwrap_or_else(|_| {
                    text.lines()
                        .filter_map(|l| serde_json::from_str(l).ok())
                        .collect()
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&brief_bright_gone::benchmark::report(&rows))
                        .unwrap()
                );
            } else {
                eprintln!("usage: bbg benchmark report <transcript.jsonl>");
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
            let root = std::env::var("BBG_STORE_DIR").unwrap_or_else(|_| ".bbg-store".into());
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
            println!("observed_usage_records: {observed_usage_records}");
            println!("observed_billing_usd: {observed_billing:.6}");
            println!("observed_billing_priced_records: {priced_records}");
            println!("estimated_savings_usd: {estimated_savings:.6}");
            println!("estimated_savings_records: {estimate_records}");
            println!(
                "note: observed billing is derived from provider-reported usage and local pricing; estimates are not provider-billed facts"
            );
        }
        "get" => {
            let Some(reference) = rest.first() else {
                eprintln!("error: bbg get requires a content reference");
                std::process::exit(2);
            };
            let root = std::env::var("BBG_STORE_DIR").unwrap_or_else(|_| ".bbg-store".into());
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
            eprintln!("  lint [--transcript <file>] [file|-]");
            eprintln!("  benchmark report <transcript.jsonl>");
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
