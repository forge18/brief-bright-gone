//! Provider-specific normalization into the canonical wire contracts.

use crate::types::Usage;
use serde_json::Value;

/// Normalize the usage object from an OpenAI Chat Completions response.
///
/// OpenAI's `prompt_tokens` includes cache reads, so subtract them here to
/// yield the canonical uncached (full-price) input. Normalizing in the adapter
/// keeps `billing` provider-agnostic — see [`crate::types::Usage::input_tokens`].
pub fn openai_usage(response: &Value) -> Option<Usage> {
    let usage = response.get("usage")?;
    let cache_read = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64);
    let uncached_input = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .map(|prompt| prompt.saturating_sub(cache_read.unwrap_or(0)));
    usage_from_parts(
        uncached_input,
        usage.get("completion_tokens").and_then(Value::as_u64),
        usage.get("total_tokens").and_then(Value::as_u64),
        cache_read,
        None,
    )
}

/// Normalize the usage object from an Anthropic Messages response or SSE event.
///
/// Anthropic's `input_tokens` already excludes cache reads and writes, so it is
/// the canonical uncached input directly — no subtraction, unlike OpenAI.
pub fn anthropic_usage(response: &Value) -> Option<Usage> {
    let usage = response.get("usage")?;
    let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
    let output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
    usage_from_parts(
        input_tokens,
        output_tokens,
        input_tokens.zip(output_tokens).map(|(a, b)| a + b),
        usage.get("cache_read_input_tokens").and_then(Value::as_u64),
        usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64),
    )
}

fn usage_from_parts(
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
) -> Option<Usage> {
    (input_tokens.is_some() || output_tokens.is_some() || total_tokens.is_some()).then_some(Usage {
        input_tokens,
        output_tokens,
        total_tokens,
        cache_read_tokens,
        cache_creation_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_openai_usage_to_uncached_input() {
        // prompt_tokens (3) includes the 2 cache reads, so canonical uncached
        // input is 1.
        let value = serde_json::json!({"usage":{"prompt_tokens":3,"completion_tokens":5,"total_tokens":8,"prompt_tokens_details":{"cached_tokens":2}}});
        assert_eq!(
            openai_usage(&value),
            Some(Usage {
                input_tokens: Some(1),
                output_tokens: Some(5),
                total_tokens: Some(8),
                cache_read_tokens: Some(2),
                cache_creation_tokens: None
            })
        );
    }

    #[test]
    fn openai_uncached_input_never_underflows_when_cache_exceeds_prompt() {
        // Defensive: a malformed response reporting more cached than prompt
        // must saturate at zero, not wrap.
        let value = serde_json::json!({"usage":{"prompt_tokens":2,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":5}}});
        assert_eq!(openai_usage(&value).unwrap().input_tokens, Some(0));
    }

    #[test]
    fn normalizes_anthropic_usage_and_derives_total() {
        let value = serde_json::json!({"usage":{"input_tokens":3,"output_tokens":5,"cache_read_input_tokens":2,"cache_creation_input_tokens":1}});
        assert_eq!(
            anthropic_usage(&value),
            Some(Usage {
                input_tokens: Some(3),
                output_tokens: Some(5),
                total_tokens: Some(8),
                cache_read_tokens: Some(2),
                cache_creation_tokens: Some(1)
            })
        );
    }
}
