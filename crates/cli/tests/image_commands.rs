use sm_cli::{parse_args, Command, ImageCommand};
fn parse(words: &[&str]) -> Result<sm_cli::CliArgs, sm_cli::ArgError> {
    let mut argv = vec![
        "--config".to_owned(),
        r"C:\synthetic\preview.json".to_owned(),
        "--json".into(),
    ];
    argv.extend(words.iter().map(|s| s.to_string()));
    parse_args(&argv)
}
const ID: &str = "12345678-1234-4123-8123-123456789abc";
#[test]
fn image_paths_profiles_controls_and_stable_id_are_explicit() {
    let args = parse(&[
        "import",
        "--path",
        r"C:\synthetic\folder",
        "--path",
        r"C:\synthetic\image.png",
        "--profile",
        "avif_sdr90",
        "--duplicates",
        "keep_both",
        "--batch-id",
        ID,
        "--wait-ms",
        "1000",
    ])
    .unwrap();
    match args.command {
        Command::Image(ImageCommand::Import {
            paths,
            batch,
            profile,
            wait_ms,
            ..
        }) => {
            assert_eq!(paths.len(), 2);
            assert_eq!(batch.as_deref(), Some(ID));
            assert_eq!(serde_json::to_value(profile).unwrap(), "avif_sdr90");
            assert_eq!(wait_ms, 1000);
        }
        _ => panic!("wrong command"),
    }
    for control in ["status", "report", "pause", "cancel", "resume", "retry"] {
        assert!(parse(&["import", control, "--batch-id", ID]).is_ok());
    }
    let export = parse(&[
        "export",
        "--record-id",
        ID,
        "--operation-id",
        ID,
        "--format",
        "jpeg",
        "--output",
        r"C:\synthetic\output.jpg",
        "--jpeg-background",
        "255,0,127",
        "--max-side",
        "1024",
    ])
    .unwrap();
    assert!(matches!(
        export.command,
        Command::Image(ImageCommand::Export {
            background: Some([255, 0, 127]),
            max_side: Some(1024),
            ..
        })
    ));
    assert!(parse(&["metadata", "--record-id", ID]).is_ok());
}
#[test]
fn image_cli_refuses_ambiguous_or_unrelated_options() {
    for words in [
        vec!["import", "--path", "relative.png"],
        vec![
            "import",
            "--path",
            r"C:\synthetic\a.png",
            "--profile",
            "raw",
        ],
        vec![
            "import",
            "status",
            "--batch-id",
            ID,
            "--path",
            r"C:\synthetic\a.png",
        ],
        vec![
            "import",
            "--path",
            r"C:\synthetic\a.png",
            "--wait-ms",
            "180001",
        ],
        vec!["export", "--record-id", ID, "--jpeg-background", "256,0,0"],
        vec![
            "export",
            "--record-id",
            ID,
            "--output",
            r"C:\synthetic\a.jpg:stream",
        ],
        vec!["metadata", "--record-id", ID, "--operation-id", ID],
        vec!["import", "status", "--batch-id", ID, "--batch-id", ID],
    ] {
        assert!(parse(&words).is_err(), "accepted {words:?}");
    }
}
