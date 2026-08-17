//! Versioned, byte-stable skill and bounded installation support.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

pub const SKILL_VERSION: &str = "1.0.0";
pub const SKILL_FILENAME: &str = "BBG_SKILL.md";
pub const SKILL: &str = r#"# bbg communication skill v1.0.0
Emit compact sigil lines; never delete words from a retained sentence.

Blocks: `§ heading`, `- bullet`, `> consequence`, `! blocking`, `~ note`.
Nesting repeats `-`; `-#` is an ordered item. A sigil is recognized only when followed by whitespace. Use backticks for paths, flags, identifiers, commands and exact errors; fenced code is byte-exact. `*word` emphasizes one non-space span. Tables are `|cell|cell` runs with a header and data rows.

End every response with exactly one top-level terminal: `. result` (done), `? decision; options: ...` (decision needed), or `x cause; options: ...` (blocked). Put the answer first, label actionable severity, state options and uncertainty once, and do not add preambles, recaps, acknowledgments, hedges, or closers. Keep retained sentences grammatical and preserve identifiers, paths, versions, commands, errors, and line numbers.

For exact bytes recovered from the local store, run `bbg get <ref>`.
"#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    pub path: PathBuf,
    pub digest: String,
    pub skill_version: String,
    pub installer_version: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub schema_version: u32,
    pub entries: Vec<ManifestEntry>,
}

fn manifest_path() -> PathBuf {
    env::var_os("BBG_STORE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".bbg-store"))
        .join("skill-manifest.json")
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn load_manifest() -> io::Result<Manifest> {
    match fs::read(manifest_path()) {
        Ok(b) => serde_json::from_slice(&b).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Manifest {
            schema_version: 1,
            entries: vec![],
        }),
        Err(e) => Err(e),
    }
}
fn save_manifest(m: &Manifest) -> io::Result<()> {
    let p = manifest_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = p.with_extension("tmp");
    fs::write(
        &tmp,
        serde_json::to_vec_pretty(m).map_err(io::Error::other)?,
    )?;
    fs::rename(tmp, p)
}
fn target(dir: &Path) -> PathBuf {
    dir.join(SKILL_FILENAME)
}
fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    fs::rename(tmp, path)
}

/// Bounded, non-recursive probe. Missing locations are intentionally harmless.
pub fn probe_dirs() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut out = Vec::new();
    if let Some(h) = home {
        for rel in [
            ".config/bbg",
            ".config/claude",
            ".config/codex",
            ".config/opencode",
        ] {
            let p = h.join(rel);
            if p.is_dir() {
                out.push(p)
            }
        }
    }
    out
}
pub fn install(paths: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut m = load_manifest()?;
    let mut written = Vec::new();
    let dirs = if paths.is_empty() {
        probe_dirs()
    } else {
        paths.to_vec()
    };
    for dir in dirs {
        let p = target(&dir);
        if let Some(old) = m.entries.iter().find(|e| e.path == p)
            && p.exists()
            && digest(&fs::read(&p)?) != old.digest
            && !paths.iter().any(|x| target(x) == p)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("refusing modified unowned skill {}", p.display()),
            ));
        }
        atomic_write(&p, SKILL.as_bytes())?;
        let e = ManifestEntry {
            path: p.clone(),
            digest: digest(SKILL.as_bytes()),
            skill_version: SKILL_VERSION.into(),
            installer_version: env!("CARGO_PKG_VERSION").into(),
        };
        m.entries.retain(|x| x.path != p);
        m.entries.push(e);
        written.push(p);
    }
    save_manifest(&m)?;
    Ok(written)
}
pub fn uninstall() -> io::Result<Vec<PathBuf>> {
    let mut m = load_manifest()?;
    let mut removed = Vec::new();
    m.entries.retain(|e| match fs::read(&e.path) {
        Ok(b) if digest(&b) == e.digest => {
            let _ = fs::remove_file(&e.path);
            removed.push(e.path.clone());
            false
        }
        _ => true,
    });
    save_manifest(&m)?;
    Ok(removed)
}
pub fn upgrade() -> io::Result<Vec<PathBuf>> {
    let entries = load_manifest()?.entries;
    for entry in &entries {
        if let Ok(bytes) = fs::read(&entry.path)
            && digest(&bytes) != entry.digest
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("refusing modified managed skill {}", entry.path.display()),
            ));
        }
    }
    let paths: Vec<_> = entries
        .into_iter()
        .map(|e| e.path.parent().unwrap_or(Path::new(".")).to_path_buf())
        .collect();
    install(&paths)
}
pub fn manifest() -> io::Result<Manifest> {
    load_manifest()
}
