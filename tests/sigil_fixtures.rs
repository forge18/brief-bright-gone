use brief_bright_gone::sigil::{Decoder, decode};
use serde::Deserialize;

#[derive(Deserialize)]
struct DecodeFixture {
    source: String,
    expected: String,
}

#[derive(Deserialize)]
struct StreamFixture {
    chunks: Vec<String>,
    emitted: Vec<String>,
    final_output: String,
}

fn decode_fixture(path: &str) -> DecodeFixture {
    serde_json::from_str(path).expect("fixture must be valid JSON")
}

#[test]
fn parser_fixtures_cover_valid_malformed_ambiguous_escaped_and_fenced_input() {
    for fixture in [
        include_str!("fixtures/parser/valid.json"),
        include_str!("fixtures/parser/malformed-table.json"),
        include_str!("fixtures/parser/ambiguous-prefix.json"),
        include_str!("fixtures/parser/escaped.json"),
        include_str!("fixtures/parser/fenced.json"),
    ] {
        let fixture = decode_fixture(fixture);
        assert_eq!(decode(&fixture.source), fixture.expected);
    }
}

#[test]
fn streaming_fixture_preserves_chunk_boundaries() {
    let fixture: StreamFixture =
        serde_json::from_str(include_str!("fixtures/streaming/chunk-boundary.json"))
            .expect("fixture must be valid JSON");
    let mut decoder = Decoder::new();
    let emitted = fixture
        .chunks
        .iter()
        .map(|chunk| decoder.push(chunk))
        .collect::<Vec<_>>();

    assert_eq!(emitted, fixture.emitted);
    assert_eq!(decoder.finish(), fixture.final_output);
}

#[test]
fn decoder_is_deterministic_for_every_parser_fixture() {
    for fixture in [
        include_str!("fixtures/parser/valid.json"),
        include_str!("fixtures/parser/malformed-table.json"),
        include_str!("fixtures/parser/ambiguous-prefix.json"),
        include_str!("fixtures/parser/escaped.json"),
        include_str!("fixtures/parser/fenced.json"),
    ] {
        let fixture = decode_fixture(fixture);
        assert_eq!(decode(&fixture.source), decode(&fixture.source));
    }
}
