use anyhow::{bail, Result};

const SCHEME: &str = "rememora://";

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedUri {
    pub full: String,
    pub scope: UriScope,
    pub segments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UriScope {
    Global,
    Project(String),
    Agent(String),
    Session(String),
}

pub fn parse(uri: &str) -> Result<ParsedUri> {
    if !uri.starts_with(SCHEME) {
        bail!("Invalid URI: must start with {SCHEME}");
    }

    let path = &uri[SCHEME.len()..];
    if path.is_empty() {
        bail!("Invalid URI: empty path");
    }

    // Reject path traversal
    if path.contains("..") || path.contains('\\') {
        bail!("Invalid URI: path traversal not allowed");
    }

    let segments: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    if segments.is_empty() {
        bail!("Invalid URI: no segments");
    }

    let scope = match segments[0].as_str() {
        "global" => UriScope::Global,
        "projects" => {
            if segments.len() < 2 {
                bail!("Invalid URI: project name required after 'projects/'");
            }
            UriScope::Project(segments[1].clone())
        }
        "agents" => {
            if segments.len() < 2 {
                bail!("Invalid URI: agent name required after 'agents/'");
            }
            UriScope::Agent(segments[1].clone())
        }
        "sessions" => {
            if segments.len() < 2 {
                bail!("Invalid URI: session id required after 'sessions/'");
            }
            UriScope::Session(segments[1].clone())
        }
        other => bail!("Invalid URI: unknown root segment '{other}'"),
    };

    Ok(ParsedUri {
        full: uri.to_string(),
        scope,
        segments,
    })
}

pub fn parent(uri: &str) -> Result<Option<String>> {
    let parsed = parse(uri)?;
    if parsed.segments.len() <= 1 {
        return Ok(None);
    }
    let parent_segments = &parsed.segments[..parsed.segments.len() - 1];
    Ok(Some(format!("{}{}", SCHEME, parent_segments.join("/"))))
}

pub fn build_memory_uri(project: Option<&str>, category: &str, slug: &str) -> String {
    match project {
        Some(proj) => format!("{SCHEME}projects/{proj}/memories/{category}/{slug}"),
        None => format!("{SCHEME}global/memories/{category}/{slug}"),
    }
}

pub fn build_project_uri(name: &str) -> String {
    format!("{SCHEME}projects/{name}/_meta")
}

pub fn slugify(text: &str) -> String {
    slug::slugify(text)
}

pub fn extract_project(uri: &str) -> Option<String> {
    parse(uri).ok().and_then(|p| match p.scope {
        UriScope::Project(name) => Some(name),
        _ => None,
    })
}

pub fn extract_category(uri: &str) -> Option<String> {
    let parsed = parse(uri).ok()?;
    // Pattern: projects/{name}/memories/{category}/...
    // or: global/memories/{category}/...
    let mem_idx = parsed.segments.iter().position(|s| s == "memories")?;
    parsed.segments.get(mem_idx + 1).cloned()
}
