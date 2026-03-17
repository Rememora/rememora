use std::thread;
use tempfile::TempDir;

#[test]
fn test_concurrent_writes() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");

    // Initialize the DB
    let conn = rememora::db::open(&db_path).unwrap();
    drop(conn);

    let db_path1 = db_path.clone();
    let db_path2 = db_path.clone();

    let t1 = thread::spawn(move || {
        let conn = rememora::db::open(&db_path1).unwrap();
        for i in 0..10 {
            rememora::models::context::insert(
                &conn,
                &rememora::models::context::InsertContext {
                    uri: format!("rememora://projects/test/memories/decisions/t1-{i}"),
                    parent_uri: None,
                    context_type: "memory".into(),
                    category: Some("decision".into()),
                    name: format!("Thread 1 item {i}"),
                    abstract_text: "test".into(),
                    overview: "test".into(),
                    content: "test".into(),
                    tags: "[]".into(),
                    source_agent: None,
                    source_session: None,
                    importance: 0.5,
                },
            )
            .unwrap();
        }
    });

    let t2 = thread::spawn(move || {
        let conn = rememora::db::open(&db_path2).unwrap();
        for i in 0..10 {
            rememora::models::context::insert(
                &conn,
                &rememora::models::context::InsertContext {
                    uri: format!("rememora://projects/test/memories/decisions/t2-{i}"),
                    parent_uri: None,
                    context_type: "memory".into(),
                    category: Some("decision".into()),
                    name: format!("Thread 2 item {i}"),
                    abstract_text: "test".into(),
                    overview: "test".into(),
                    content: "test".into(),
                    tags: "[]".into(),
                    source_agent: None,
                    source_session: None,
                    importance: 0.5,
                },
            )
            .unwrap();
        }
    });

    t1.join().unwrap();
    t2.join().unwrap();

    // Verify all 20 records exist
    let conn = rememora::db::open(&db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM contexts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 20);
}
