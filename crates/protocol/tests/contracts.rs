use serde_json::{json, Value};
use sm_domain::{LibraryId, Revision};
use sm_protocol::{
    application::{
        dispatch_parsed, ApplicationResult, AuthorizeProbeRequest, KnownParams, RequestEnvelope,
        Response, APPLICATION_VERSION,
    },
    bridge::{BridgeProbeRequest, BRIDGE_VERSION},
    errors::ErrorCode,
    values::{BuildId, ConsentLifetime, MethodName, ProfileId, SafeMessage, UtcTime},
};
use std::cell::Cell;

const STATUS: &str = include_str!("../../../tests/fixtures/protocol/valid-status.json");
const CONTROL: &str = include_str!("../../../tests/fixtures/protocol/valid-control.json");
const SAFE_MESSAGES: &str = include_str!("../../../tests/fixtures/protocol/safe-messages.json");
fn fixture() -> Value {
    serde_json::from_str(STATUS).unwrap()
}
fn request(value: Value) -> RequestEnvelope {
    serde_json::from_value(value).unwrap()
}
fn library() -> LibraryId {
    request(fixture()).library_id
}
fn route(value: Value, called: &Cell<u32>) -> Response {
    dispatch_parsed(request(value), library(), |_| {
        called.set(called.get() + 1);
        ApplicationResult::AiPolicy(sm_protocol::application::AiPolicyView {
            revision: Revision::new(9_007_199_254_740_993),
            enabled: false,
        })
    })
}
fn error_code(response: Response) -> ErrorCode {
    match response {
        Response::Error { error, .. } => error.code,
        _ => panic!("expected error"),
    }
}

#[test]
fn s01_ipc_03_valid_requests_route_once_and_precision_survives() {
    let called = Cell::new(0);
    let result = route(fixture(), &called);
    assert!(matches!(result, Response::Result { .. }));
    assert_eq!(called.get(), 1);
    let revision = serde_json::from_value::<Revision>(json!("9007199254740993")).unwrap();
    assert_eq!(revision.value(), 9_007_199_254_740_993);
    assert_eq!(
        serde_json::to_value(revision).unwrap(),
        json!("9007199254740993")
    );
    let control = request(serde_json::from_str(CONTROL).unwrap());
    assert!(matches!(
        serde_json::from_value::<KnownParams>(
            json!({"method": control.method, "params": control.params})
        ),
        Ok(KnownParams::SetAiPolicy(_))
    ));
}
#[test]
fn s01_ipc_03_unknown_method_and_versions_never_call_handler() {
    let called = Cell::new(0);
    let mut raw = fixture();
    raw["method"] = json!("search");
    assert_eq!(
        error_code(route(raw, &called)),
        ErrorCode::MethodNotSupported
    );
    let mut raw = fixture();
    raw["protocol"]["major"] = json!(2);
    assert_eq!(error_code(route(raw, &called)), ErrorCode::ProtocolMismatch);
    let mut raw = fixture();
    raw["protocol"]["minor"] = json!(1);
    assert_eq!(error_code(route(raw, &called)), ErrorCode::ProtocolMismatch);
    let mut raw = fixture();
    raw["library_id"] = json!("01900000-0000-7000-8000-000000000002");
    assert_eq!(error_code(route(raw, &called)), ErrorCode::LibraryMismatch);
    assert_eq!(called.get(), 0);
    assert_eq!(APPLICATION_VERSION.major, BRIDGE_VERSION.major);
}
#[test]
fn s01_dto_01_unknown_fields_rejected_before_handler() {
    let called = Cell::new(0);
    let mut raw = fixture();
    raw["params"] = json!({"client_kind": "gui"});
    assert_eq!(error_code(route(raw, &called)), ErrorCode::InvalidRequest);
    assert_eq!(called.get(), 0);
    let mut raw = fixture();
    raw["client_kind"] = json!("gui");
    assert!(serde_json::from_value::<RequestEnvelope>(raw).is_err());
    let mut raw = fixture();
    raw["protocol"]["extra"] = json!(true);
    assert!(serde_json::from_value::<RequestEnvelope>(raw).is_err());
    assert!(serde_json::from_value::<BridgeProbeRequest>(json!({"protocol":{"major":1,"minor":0},"request_id":"01900000-0000-7000-8000-000000000101","fixture":"synthetic_menu_v1","profile_id":"fixture","allow_model_load":false,"credential":"forbidden"})).is_err());
}
#[test]
fn s01_dto_02_revision_identifier_string_and_time_bounds() {
    for bad in [
        json!(-1),
        json!(9007199254740993u64),
        json!("-1"),
        json!("01"),
        json!("18446744073709551616"),
        json!("1.0"),
        json!(""),
    ] {
        assert!(serde_json::from_value::<Revision>(bad).is_err());
    }
    assert_eq!(
        serde_json::from_value::<Revision>(json!("18446744073709551615"))
            .unwrap()
            .value(),
        u64::MAX
    );
    for len in [128, 129] {
        assert_eq!(ProfileId::parse("a".repeat(len)).is_ok(), len == 128);
        assert_eq!(BuildId::parse("a".repeat(len)).is_ok(), len == 128);
    }
    assert!(MethodName::parse("a".repeat(96)).is_ok());
    assert!(MethodName::parse("a".repeat(97)).is_err());
    assert!(SafeMessage::parse("x".repeat(512)).is_ok());
    assert!(SafeMessage::parse("x".repeat(513)).is_err());
    assert!(SafeMessage::parse("secret\nline").is_err());
    assert!(UtcTime::parse("2026-01-01T12:00:00Z").is_ok());
    assert!(UtcTime::parse("2026-01-01T12:00:00+00:00").is_err());
    let mut raw = fixture();
    raw["library_id"] = json!("{01900000-0000-7000-8000-000000000001}");
    assert!(serde_json::from_value::<RequestEnvelope>(raw).is_err());
    assert!(serde_json::from_value::<LibraryId>(json!("not-uuid")).is_err());
    assert!(
        serde_json::from_value::<sm_protocol::application::CheckResult>(
            json!({"check_id":"fixture","outcome":"NOT_RUN"})
        )
        .is_ok()
    );
    assert!(
        serde_json::from_value::<sm_protocol::application::CheckResult>(
            json!({"check_id":"fixture","outcome":"NOT_RUN","duration_ms":null})
        )
        .is_err()
    );
}
#[test]
fn s01_dto_04_safe_message_control_parity_fixtures() {
    let cases: Value = serde_json::from_str(SAFE_MESSAGES).unwrap();
    for case in cases["invalid"].as_array().unwrap() {
        let message = case["safe_message"].as_str().unwrap();
        assert!(message.chars().any(char::is_control), "{}", case["id"]);
        assert!(SafeMessage::parse(message).is_err(), "{}", case["id"]);
        let error = json!({"code":"INVALID_REQUEST", "safe_message":message, "retryable":false});
        assert!(
            serde_json::from_value::<sm_protocol::errors::AppError>(error).is_err(),
            "{}",
            case["id"]
        );
    }
    for case in cases["valid"].as_array().unwrap() {
        let message = case["safe_message"].as_str().unwrap();
        assert!(!message.chars().any(char::is_control), "{}", case["id"]);
        assert!(SafeMessage::parse(message).is_ok(), "{}", case["id"]);
        let error = json!({"code":"INVALID_REQUEST", "safe_message":message, "retryable":false});
        assert!(
            serde_json::from_value::<sm_protocol::errors::AppError>(error).is_ok(),
            "{}",
            case["id"]
        );
    }
}
#[test]
fn s01_dto_03_consent_bounds_and_response_union() {
    for (value, accepted) in [(0, false), (1, true), (300, true), (301, false)] {
        assert_eq!(ConsentLifetime::new(value).is_ok(), accepted);
        let req = json!({"fixture":"synthetic_menu_v1","profile_id":"fixture","allow_load":false,"lifetime_seconds":value});
        assert_eq!(
            serde_json::from_value::<AuthorizeProbeRequest>(req).is_ok(),
            accepted
        );
    }
    let called = Cell::new(0);
    let response = route(fixture(), &called);
    let mut encoded = serde_json::to_value(&response).unwrap();
    assert!(serde_json::from_value::<Response>(encoded.clone()).is_ok());
    encoded["error"] =
        json!({"code":"INVALID_REQUEST","safe_message":"Invalid.","retryable":false});
    assert!(serde_json::from_value::<Response>(encoded).is_err());
    let mut encoded = serde_json::to_value(Response::Error {
        request_id: request(fixture()).request_id,
        error: sm_protocol::errors::AppError::method_not_supported(),
    })
    .unwrap();
    encoded["result"] = json!({"type":"ai_policy","value":{"revision":"1","enabled":true}});
    assert!(serde_json::from_value::<Response>(encoded).is_err());
}
