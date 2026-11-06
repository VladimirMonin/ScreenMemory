use super::*;
#[test]
fn s07_access_migration8_to9_rollback_preserves_shipped_prefix_and_uuid() {
    let mut db = Connection::open_in_memory().unwrap();
    db.execute_batch(BOOTSTRAP).unwrap();
    for (n, (name, sql)) in SHIPPED.iter().take(8).enumerate() {
        db.execute_batch(sql).unwrap();
        db.execute(
            "INSERT INTO schema_migrations VALUES(?1,?2,?3,'synthetic')",
            (n + 1, name, checksum_of(sql)),
        )
        .unwrap();
    }
    let library = Uuid::new_v4().to_string();
    db.execute(
        "INSERT INTO library_meta VALUES(1,?1,1,'synthetic')",
        [&library],
    )
    .unwrap();
    for key in ["ai.enabled", "model.autostart"] {
        db.execute("INSERT INTO library_settings VALUES(?1,'false',1)", [key])
            .unwrap();
    }
    let before = objects(&db).unwrap();
    let checksums: Vec<String> = db
        .prepare("SELECT checksum FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(applied_version(&db).unwrap(), 8);
    validate_applied(&db, 8).unwrap();
    assert!(initialize(&mut db, false, true).is_err());
    assert_eq!(objects(&db).unwrap(), before);
    assert_eq!(applied_version(&db).unwrap(), 8);
    validate_applied(&db, 8).unwrap();
    assert_eq!(initialize(&mut db, false, false).unwrap().schema_version, 9);
    assert_eq!(
        db.query_row("SELECT library_id FROM library_meta", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        library
    );
    let after: Vec<String> = db
        .prepare("SELECT checksum FROM schema_migrations WHERE version<=8 ORDER BY version")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(checksums, after);
    assert_eq!(initialize(&mut db, false, false).unwrap().schema_version, 9);
}
