use sm_protocol::{application::KnownParams, search::*};
#[test]
fn s06_shared_search_fixtures_closed_shape() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/protocol/search-cases.json"
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
fn s06_limits_dates_filters_refused_without_sql() {
    let mut r:SearchStart=serde_json::from_value(serde_json::json!({"search_id":"00000000-0000-4000-8000-000000000001","text":"literal","limit":30,"candidate_depth":100})).unwrap();
    assert!(validate(&r).is_ok());
    r.limit = 0;
    assert!(validate(&r).is_err());
    r.limit = 30;
    r.text = "😀".repeat(1025);
    assert!(validate(&r).is_err());
    r.text = "x".into();
    r.filters.date = DateFilter::UtcRange {
        axis: DateAxis::Ingested,
        from_inclusive: "2026-10-03T13:00:00+01:00".into(),
        to_exclusive: "2026-10-03T12:00:01Z".into(),
    };
    assert!(validate(&r).is_ok());
    r.filters.date = DateFilter::UtcRange {
        axis: DateAxis::Ingested,
        from_inclusive: "2026-02-29T00:00:00Z".into(),
        to_exclusive: "2026-03-01T00:00:00Z".into(),
    };
    assert!(validate(&r).is_err());
    r.filters.date = DateFilter::TakenAbsent {};
    assert_eq!(
        validate(&r).unwrap_err(),
        sm_protocol::errors::ErrorCode::SearchFilterUnsupported
    );
}
