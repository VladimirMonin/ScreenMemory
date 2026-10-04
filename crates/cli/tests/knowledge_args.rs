use sm_cli::{parse_args, Command};
fn argv(args: &[&str]) -> Vec<String> {
    [vec!["--config", "C:\\owned\\client.json"], args.to_vec()]
        .concat()
        .into_iter()
        .map(str::to_owned)
        .collect()
}
#[test]
fn reserved_verbs_in_manual_text_do_not_select_a_command() {
    let id = "00000000-0000-4000-8000-000000000001";
    for literal in [
        "search",
        "knowledge",
        "processing",
        "import",
        "export",
        "metadata",
        "status",
        "doctor",
        "capture",
        "host",
    ] {
        for field in ["--title", "--description"] {
            let input = argv(&[
                "knowledge",
                "edit",
                "--record-id",
                id,
                "--operation-id",
                id,
                "--expected-revision",
                "1",
                "--catalog-revision",
                "1",
                field,
                literal,
            ]);
            let parsed =
                parse_args(&input).unwrap_or_else(|error| panic!("{field} {literal}: {error:?}"));
            assert!(parsed.json);
            let Command::Knowledge(command) = parsed.command else {
                panic!("manual text selected another command: {field} {literal}")
            };
            let value: serde_json::Value = serde_json::from_str(&command.0).unwrap();
            assert_eq!(value["patch"][&field[2..]]["value"], literal);
        }
    }
}

#[test]
fn manual_cli_closed_patch_vocabulary_revisions_unicode_and_no_authority() {
    let id = "00000000-0000-4000-8000-000000000001";
    let good = argv(&[
        "knowledge",
        "edit",
        "--record-id",
        id,
        "--operation-id",
        id,
        "--expected-revision",
        "1",
        "--catalog-revision",
        "1",
        "--description",
        "",
        "--clear-category",
        "true",
    ]);
    let parsed = parse_args(&good).unwrap();
    assert!(parsed.json);
    let Command::Knowledge(c) = parsed.command else {
        panic!("knowledge")
    };
    let v: serde_json::Value = serde_json::from_str(&c.0).unwrap();
    assert_eq!(v["patch"]["title"]["op"], "keep");
    assert_eq!(v["patch"]["description"]["value"], "");
    assert!(v["patch"]["category_id"]["value"].is_null());
    for flags in [
        vec!["--principal-id", id],
        vec!["--root", "C:\\foreign"],
        vec!["--model-load", "true"],
        vec!["--inherit-description", "true"],
        vec!["--clear-category", "false"],
    ] {
        let mut bad = good.clone();
        bad.extend(flags.iter().map(|s| s.to_string()));
        assert!(parse_args(&bad).is_err());
    }
    let mut overflow = good;
    overflow[9] = "9223372036854775808".into();
    assert!(parse_args(&overflow).is_err());
}
