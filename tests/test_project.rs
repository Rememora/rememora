mod common;

use rememora::models::project;

#[test]
fn test_add_project() {
    let conn = common::create_test_db();
    let id = project::add(&conn, "myapp", Some("/tmp/myapp"), "My app", &["rust".into()]).unwrap();
    assert!(!id.is_empty());

    let info = project::get_info(&conn, "myapp").unwrap().unwrap();
    assert_eq!(info.name, "myapp");
    assert_eq!(info.path.as_deref(), Some("/tmp/myapp"));
    assert_eq!(info.description, "My app");
    assert_eq!(info.tech_stack, vec!["rust"]);
}

#[test]
fn test_list_projects() {
    let conn = common::create_test_db();
    project::add(&conn, "proj1", None, "First", &[]).unwrap();
    project::add(&conn, "proj2", None, "Second", &[]).unwrap();

    let projects = project::list(&conn).unwrap();
    assert_eq!(projects.len(), 2);
}

#[test]
fn test_get_project() {
    let conn = common::create_test_db();
    project::add(&conn, "testproj", Some("/tmp/test"), "Test", &["rust".into(), "sqlite".into()]).unwrap();

    let record = project::get(&conn, "testproj").unwrap().unwrap();
    assert_eq!(record.name, "testproj");
    assert_eq!(record.context_type, "project");
}

#[test]
fn test_detect_from_cwd_match() {
    let conn = common::create_test_db();
    project::add(&conn, "myapp", Some("/Users/me/projects/myapp"), "My app", &[]).unwrap();

    let detected = project::detect_from_cwd(&conn, "/Users/me/projects/myapp/src").unwrap();
    assert_eq!(detected, Some("myapp".to_string()));
}

#[test]
fn test_detect_from_cwd_no_match() {
    let conn = common::create_test_db();
    project::add(&conn, "myapp", Some("/Users/me/projects/myapp"), "My app", &[]).unwrap();

    let detected = project::detect_from_cwd(&conn, "/Users/me/other/project").unwrap();
    assert_eq!(detected, None);
}
