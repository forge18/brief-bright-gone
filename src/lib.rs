//! brief-bright-gone (bbg) — a communication-efficiency system for coding agents.
//!
//! Core engine: content detection → safety classification → lossy-safe
//! compression/normalization. Written in Rust as our own design, informed by
//! the general architecture of agent communication-compression tools:
//!
//! - `detect`: classify a payload (json, code, log, diff, prose, ...) so we
//!   know *what kind* of content we're handling.
//! - `safety`: every transform declares a safety class. Lossy-class transforms
//!   may only run if they are reversible (the original is stored) and the
//!   class itself decides which transforms may run at all.
//! - `normalize`: the safe-first text cleanups for user-typed prose
//!   (whitespace, punctuation, polite filler, profanity placeholder).

pub mod adapters;
pub mod benchmark;
pub mod compress;
pub mod detect;
pub mod lint;
pub mod normalize;
pub mod operations;
pub mod private_fs;
pub mod proxy;
pub mod safety;
pub mod session;
pub mod sigil;
pub mod signals;
pub mod skill;
pub mod store;
pub mod transcript;
pub mod types;
