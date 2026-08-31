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

// --- Worktree project resolution -------------------------------------------
//
// Regression coverage for the recall bug: agent work happens in git worktrees
// (mandated by AGENTS.md), but a worktree lives outside the registered project
// path, so the prefix match in `detect_from_cwd` misses. Callers used to fall
// back to `basename(cwd)`, which fabricates a name that matches no project —
// and since the search project filter is a hard URI prefix match, that
// silently excluded every project memory.

/// Create a real git repo plus a linked worktree, so the resolution path is
/// exercised against actual git plumbing rather than a mock.
fn git_repo_with_worktree(root: &std::path::Path) -> (String, String) {
    let main = root.join("main");
    let wt = root.join("wt");
    std::fs::create_dir_all(&main).unwrap();

    let git = |args: &[&str], cwd: &std::path::Path| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git available");
    };

    git(&["init", "-q", "-b", "main"], &main);
    git(&["config", "user.email", "t@example.com"], &main);
    git(&["config", "user.name", "t"], &main);
    std::fs::write(main.join("f.txt"), "x").unwrap();
    git(&["add", "-A"], &main);
    git(&["commit", "-qm", "init"], &main);
    git(&["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "wt"], &main);

    (
        main.to_string_lossy().into_owned(),
        wt.to_string_lossy().into_owned(),
    )
}

#[test]
fn resolve_for_cwd_maps_worktree_to_registered_project() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, wt) = git_repo_with_worktree(tmp.path());

    let conn = common::create_test_db();
    project::add(&conn, "myproj", Some(&main), "Main checkout", &[]).unwrap();

    // The plain prefix match cannot see the worktree — this is the bug.
    assert_eq!(project::detect_from_cwd(&conn, &wt).unwrap(), None);

    // Resolution walks back to the main checkout via --git-common-dir.
    assert_eq!(
        project::resolve_for_cwd(&conn, &wt).unwrap().as_deref(),
        Some("myproj"),
        "a git worktree must resolve to the project registered for its main checkout"
    );

    // The direct case still works.
    assert_eq!(
        project::resolve_for_cwd(&conn, &main).unwrap().as_deref(),
        Some("myproj")
    );
}

#[test]
fn resolve_for_cwd_returns_none_rather_than_guessing() {
    let tmp = tempfile::tempdir().unwrap();
    let unrelated = tmp.path().join("nowhere");
    std::fs::create_dir_all(&unrelated).unwrap();

    let conn = common::create_test_db();
    project::add(&conn, "myproj", Some("/some/registered/path"), "P", &[]).unwrap();

    // Must be None (search everything), never `basename(cwd)` == "nowhere",
    // which would hard-filter the corpus down to nothing.
    assert_eq!(
        project::resolve_for_cwd(&conn, unrelated.to_str().unwrap()).unwrap(),
        None,
        "unresolvable cwd must not fabricate a project name"
    );
}
