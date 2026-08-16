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
    if let Some(p) = path {
        if p != "-" {
            return std::fs::read_to_string(p).map_err(|e| format!("read {}: {}", p, e));
        }
    }
    io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("read stdin: {e}"))?;
    Ok(buf)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let rest = &args[1..];
    let path = rest.first().cloned();

    match cmd {
        "detect" => {
            let input = match read_input(&path) {
                Ok(s) => s,
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1); }
            };
            println!("{}", brief_bright_gone::detect::detect(&input).name());
        }
        "normalize" => {
            let input = match read_input(&path) {
                Ok(s) => s,
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1); }
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
            let input = match read_input(&path) {
                Ok(s) => s,
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1); }
            };
            let opts = brief_bright_gone::normalize::NormalizeOptions::default();
            let out = brief_bright_gone::normalize::normalize_with_detect(&input, &opts);
            let ct = brief_bright_gone::detect::detect(&input);
            println!("type: {}", ct.name());
            println!("bytes_before: {}", out.bytes_before);
            println!("bytes_after: {}", out.bytes_after);
            println!("saved_bytes: {}", out.bytes_before.saturating_sub(out.bytes_after));
            println!("changed: {}", out.changed);
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
            eprintln!("  detect      print detected content type (json|code|log|diff|search-result|text|tabular|terminal)");
            eprintln!("  normalize   normalize prose from stdin/file to stdout (skips code/shell)");
            eprintln!("  stats       bytes before/after and change flag");
            eprintln!("  version     print version");
            eprintln!();
            eprintln!("example:");
            eprintln!("  echo 'please   fix the  bug, thank you' | bbg normalize");
            std::process::exit(1);
        }
    }
}
