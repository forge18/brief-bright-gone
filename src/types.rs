//! Canonical data contracts shared by adapters, storage, and cost accounting.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    OpenAi,
    Anthropic,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Usage {
    /// Canonical uncached (full-price) input tokens: provider input with cache
    /// reads and writes excluded. The `adapters` normalize each provider into
    /// this shape (OpenAI's `prompt_tokens` has cache reads subtracted;
    /// Anthropic's `input_tokens` already excludes them) so `billing` needs no
    /// per-provider formula.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// Provider-reported cached input tokens, normalized without guessing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Provider-reported cache creation/write tokens, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
}
