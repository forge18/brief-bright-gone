use brief_bright_gone::{sigil, store::Store};
use std::time::{SystemTime, UNIX_EPOCH};

fn store() -> Store {
    Store::open(std::env::temp_dir().join(format!(
        "bbg-round-trip-{}",
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    )))
    .unwrap()
}

#[test]
fn decode_store_substitute_is_identity_for_supported_sigil_forms() {
    let cases = [
        "§ Heading\n. done",
        "- *keyword\n--# item\n. done",
        "> consequence\n! blocker\n? choose A or B",
        "|Name|Count\n|one|2\n. done",
        "\\§ literal\n. done",
        "`*literal` *live\n```rust\n*unchanged\n```\n. done",
    ];

    let store = store();
    for original in cases {
        let decoded = sigil::decode(original);
        store
            .put_sigil_original(&decoded, original.as_bytes())
            .unwrap();
        assert_eq!(
            store.get_sigil_original(&decoded).unwrap(),
            Some(original.as_bytes().to_vec())
        );
    }
}

#[test]
fn normalized_markdown_variants_restore_the_same_original() {
    let store = store();
    let original = "§ Heading\n. done";
    let decoded = sigil::decode(original);
    store
        .put_sigil_original(&(decoded.clone() + "  \n"), original.as_bytes())
        .unwrap();
    assert_eq!(
        store.get_sigil_original(&decoded).unwrap(),
        Some(original.as_bytes().to_vec())
    );
}
