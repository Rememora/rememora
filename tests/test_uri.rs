use rememora::uri;

#[test]
fn test_parse_project_memory_uri() {
    let parsed = uri::parse("rememora://projects/myapp/memories/decisions/use-zustand").unwrap();
    assert_eq!(parsed.segments[0], "projects");
    assert_eq!(parsed.segments[1], "myapp");
    assert_eq!(parsed.segments[2], "memories");
    assert_eq!(parsed.segments[3], "decisions");
    assert_eq!(parsed.segments[4], "use-zustand");
    assert!(matches!(parsed.scope, uri::UriScope::Project(ref name) if name == "myapp"));
}

#[test]
fn test_parse_global_uri() {
    let parsed = uri::parse("rememora://global/memories/preferences/dark-mode").unwrap();
    assert!(matches!(parsed.scope, uri::UriScope::Global));
    assert_eq!(parsed.segments.len(), 4);
}

#[test]
fn test_build_memory_uri_with_project() {
    let uri = uri::build_memory_uri(Some("myapp"), "decision", "use-zustand");
    assert_eq!(uri, "rememora://projects/myapp/memories/decision/use-zustand");
}

#[test]
fn test_build_memory_uri_global() {
    let uri = uri::build_memory_uri(None, "preference", "dark-mode");
    assert_eq!(uri, "rememora://global/memories/preference/dark-mode");
}

#[test]
fn test_build_project_uri() {
    let uri = uri::build_project_uri("myapp");
    assert_eq!(uri, "rememora://projects/myapp/_meta");
}

#[test]
fn test_parent_uri() {
    let parent = uri::parent("rememora://projects/myapp/memories/decisions/use-zustand")
        .unwrap()
        .unwrap();
    assert_eq!(parent, "rememora://projects/myapp/memories/decisions");
}

#[test]
fn test_parent_uri_root() {
    let parent = uri::parent("rememora://global").unwrap();
    assert!(parent.is_none());
}

#[test]
fn test_invalid_uri_no_scheme() {
    assert!(uri::parse("http://example.com").is_err());
}

#[test]
fn test_invalid_uri_empty_path() {
    assert!(uri::parse("rememora://").is_err());
}

#[test]
fn test_invalid_uri_path_traversal() {
    assert!(uri::parse("rememora://projects/../secrets").is_err());
}

#[test]
fn test_invalid_uri_unknown_root() {
    assert!(uri::parse("rememora://unknown/path").is_err());
}

#[test]
fn test_slugify() {
    assert_eq!(uri::slugify("Use Zustand for state"), "use-zustand-for-state");
    assert_eq!(uri::slugify("iOS build fails!"), "ios-build-fails");
}

#[test]
fn test_extract_project() {
    let proj = uri::extract_project("rememora://projects/myapp/memories/decisions/foo");
    assert_eq!(proj, Some("myapp".to_string()));

    let none = uri::extract_project("rememora://global/memories/preferences/dark");
    assert_eq!(none, None);
}

#[test]
fn test_extract_category() {
    let cat = uri::extract_category("rememora://projects/myapp/memories/decisions/foo");
    assert_eq!(cat, Some("decisions".to_string()));

    let cat = uri::extract_category("rememora://global/memories/preferences/dark");
    assert_eq!(cat, Some("preferences".to_string()));
}
