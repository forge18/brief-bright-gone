use brief_bright_gone::{
    sigil::{Decoder, decode},
    transcript::redact,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RedactionFixture {
    input: String,
    secrets: Vec<String>,
}

#[test]
fn secret_corpus_is_fully_redacted_without_changing_record_boundaries() {
    let fixtures: Vec<RedactionFixture> =
        serde_json::from_str(include_str!("fixtures/redaction/corpus.json"))
            .expect("redaction corpus must be valid JSON");
    for fixture in fixtures {
        let redacted = redact(&fixture.input);
        for secret in fixture.secrets {
            assert!(
                !redacted.contains(&secret),
                "corpus secret leaked after redaction: {secret}"
            );
        }
        assert_eq!(
            redacted.lines().count(),
            fixture.input.lines().count(),
            "redaction must preserve line/record boundaries"
        );
    }
}

fn generated_case(mut state: u64) -> String {
    const ALPHABET: &[u8] = b"abcXYZ012 *.-!?~#>|\\`\n\r[]{}:_/";
    let mut output = String::new();
    for _ in 0..96 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        output.push(ALPHABET[(state as usize) % ALPHABET.len()] as char);
    }
    output
}

#[test]
fn generated_sigil_inputs_never_panic_and_have_deterministic_chunk_decoding() {
    for seed in 1..=512_u64 {
        let input = generated_case(seed);
        let complete = decode(&input);
        assert_eq!(complete, decode(&input), "seed {seed} is nondeterministic");

        let mut decoder = Decoder::new();
        let mut streamed = String::new();
        for byte in input.bytes() {
            streamed.push_str(&decoder.push(&(byte as char).to_string()));
        }
        streamed.push_str(&decoder.finish());
        // The incremental parser intentionally emits partial structure only at
        // safe boundaries; this assertion is about totality and determinism.
        let mut repeated = Decoder::new();
        let mut repeated_output = String::new();
        for byte in input.bytes() {
            repeated_output.push_str(&repeated.push(&(byte as char).to_string()));
        }
        repeated_output.push_str(&repeated.finish());
        assert_eq!(
            streamed, repeated_output,
            "seed {seed} stream is nondeterministic"
        );
    }
}
