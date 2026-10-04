use sm_cli::{parse_args, Command};
fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}
#[test]
fn search_actions_keep_typed_json_and_reserved_text_with_global_ordering() {
    use sm_protocol::search::SearchRequest;
    let id = "00000000-0000-4000-8000-000000000001";
    for literal in [
        "search",
        "knowledge",
        "processing",
        "import",
        "export",
        "metadata",
    ] {
        for prefix in [true, false] {
            let mut input = args(&["search", "--text", literal]);
            let global = args(&["--json", "--config", "C:/synthetic/config.json"]);
            if prefix {
                input.splice(0..0, global);
            } else {
                input.extend(global);
            }
            let parsed = parse_args(&input).unwrap();
            assert!(parsed.json);
            let Command::Search(command) = parsed.command else {
                panic!("literal selected another command")
            };
            let SearchRequest::Start(request) = serde_json::from_str(&command.0).unwrap() else {
                panic!("start")
            };
            assert_eq!(request.text, literal);
        }
    }
    for (action, flag) in [("page", "--cursor"), ("cancel", "--search-id")] {
        let parsed = parse_args(&args(&[
            "search",
            action,
            flag,
            id,
            "--config",
            "C:/synthetic/config.json",
        ]))
        .unwrap();
        assert!(parsed.json);
        let Command::Search(command) = parsed.command else {
            panic!("search")
        };
        let request: SearchRequest = serde_json::from_str(&command.0).unwrap();
        match request {
            SearchRequest::Page { cursor } => {
                assert_eq!(action, "page");
                assert_eq!(cursor.0.to_string(), id);
            }
            SearchRequest::Cancel { search_id } => {
                assert_eq!(action, "cancel");
                assert_eq!(search_id.0.to_string(), id);
            }
            _ => panic!("unexpected start"),
        }
    }
}

#[test]
fn command_dispatch_does_not_bypass_malformed_global_flags() {
    for input in [
        vec!["--config"],
        vec!["--config", "search", "knowledge", "categories"],
        vec![
            "--config",
            "C:/synthetic/config.json",
            "--config",
            "C:/synthetic/other.json",
            "search",
        ],
        vec![
            "--unknown",
            "search",
            "--config",
            "C:/synthetic/config.json",
            "knowledge",
            "categories",
        ],
        vec![
            "--version",
            "search",
            "--config",
            "C:/synthetic/config.json",
        ],
        vec!["--config", "C:/synthetic/config.json", "search", "--config"],
    ] {
        assert!(parse_args(&args(&input)).is_err(), "{input:?}");
    }
}

#[test]
fn s06_search_json_cli_closed_parser_and_literals() {
    let parsed = parse_args(&args(&[
        "--config",
        "C:/synthetic/config.json",
        "search",
        "--text",
        "Q8_0 OR NEAR settings.json",
        "--limit",
        "1",
        "--json",
    ]))
    .unwrap();
    assert!(parsed.json);
    let Command::Search(c) = parsed.command else {
        panic!("search")
    };
    let j: serde_json::Value = serde_json::from_str(&c.0).unwrap();
    assert_eq!(j["text"], "Q8_0 OR NEAR settings.json");
    assert_eq!(j["limit"], 1);
    for flags in [
        ["--mode", "semantic"],
        ["--allow-model-load", "true"],
        ["--limit", "0"],
    ] {
        assert!(parse_args(&args(&[
            "--config",
            "C:/synthetic/config.json",
            "search",
            flags[0],
            flags[1]
        ]))
        .is_err());
    }
    assert!(parse_args(&args(&[
        "--config",
        "C:/synthetic/config.json",
        "search",
        "page",
        "--cursor",
        "invalid"
    ]))
    .is_err());
}
