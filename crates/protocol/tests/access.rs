use sm_protocol::{access::AccessRequest, application::KnownParams};
#[test]
fn s07_access_shared_closed_requests_and_bounds() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/protocol/access-cases.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let ok = serde_json::from_value::<KnownParams>(case["value"].clone()).is_ok();
        assert_eq!(ok, case["valid"].as_bool().unwrap(), "{}", case["id"]);
    }
}
#[test]
fn s07_access_native_constructor_is_revalidated() {
    let value = serde_json::json!({"action":"update_profile","operation_id":"10000000-0000-4000-8000-000000000001","profile_id":"10000000-0000-4000-8000-000000000002","expected_revision":"9223372036854775807","name":"Agent","policy":{"scopes":["read_metadata"],"roots":[],"capture_modes":[],"max_active_mutations":1},"state":"disabled"});
    assert!(serde_json::from_value::<AccessRequest>(value.clone()).is_ok());
    let mut invalid = value;
    invalid["expected_revision"] = serde_json::json!("9223372036854775808");
    assert!(serde_json::from_value::<AccessRequest>(invalid).is_err());
}
