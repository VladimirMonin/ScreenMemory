use serde_json::{json, Value};
use sm_protocol::application::KnownParams;
use sm_protocol::knowledge::*;
#[test]
fn manual_knowledge_shared_rust_ajv_python_positive_negative_parity() {
    let cases: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/protocol/knowledge-cases.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        assert_eq!(
            serde_json::from_value::<KnownParams>(case["value"].clone()).is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}",
            case["id"]
        );
    }
}
#[test]
fn manual_unicode_white_space_contract_is_not_platform_regex_s() {
    // The explicit schema set must agree with Rust trim for every White_Space member.
    let whitespace = "\u{0009}\u{000a}\u{000b}\u{000c}\u{000d}\u{0020}\u{0085}\u{00a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}";
    for c in whitespace.chars() {
        let text = c.to_string();
        assert!(text.trim().is_empty(), "U+{:04X}", c as u32);
        assert!(serde_json::from_value::<CardTitle>(json!(text)).is_err());
        assert!(serde_json::from_value::<CategoryName>(json!(text)).is_err());
        assert!(serde_json::from_value::<CardTitle>(json!(format!("{text}a{text}"))).is_ok());
    }
    for c in [
        '\u{001c}', '\u{001d}', '\u{001e}', '\u{001f}', '\u{180e}', '\u{200b}', '\u{feff}',
    ] {
        let text = c.to_string();
        assert!(!text.trim().is_empty(), "U+{:04X}", c as u32);
        assert!(serde_json::from_value::<CardTitle>(json!(text)).is_ok());
        assert!(serde_json::from_value::<CategoryName>(json!(text)).is_ok());
    }
}
#[test]
fn manual_text_scalar_bounds_and_signed_sqlite_decimal_revisions() {
    for (len, valid) in [(0, false), (1, true), (160, true), (161, false)] {
        assert_eq!(
            serde_json::from_value::<CardTitle>(json!("😀".repeat(len))).is_ok(),
            valid
        );
    }
    for (value, valid) in [
        ("0", true),
        ("01", false),
        ("-1", false),
        ("9223372036854775807", true),
        ("9223372036854775808", false),
        ("18446744073709551615", false),
    ] {
        assert_eq!(
            serde_json::from_value::<SqlRevision>(json!(value)).is_ok(),
            valid
        );
    }
    assert!(serde_json::from_value::<CardDescription>(json!("\u{0}")).is_err());
    assert!(serde_json::from_value::<CardDescription>(json!("😀".repeat(8000))).is_ok());
    assert!(serde_json::from_value::<CardDescription>(json!("😀".repeat(8001))).is_err());
    let empty: CardPatch = serde_json::from_value(json!({})).unwrap();
    assert_eq!(empty, CardPatch::default());
    assert_ne!(
        serde_json::from_value::<CardPatch>(json!({"category_id":{"op":"set","value":null}}))
            .unwrap(),
        serde_json::from_value::<CardPatch>(json!({"category_id":{"op":"inherit"}})).unwrap()
    );
}
