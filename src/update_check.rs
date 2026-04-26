//! Best-effort GitHub-Releases update check.
//!
//! Hits `https://api.github.com/repos/Rememora/rememora/releases/latest`,
//! compares the `tag_name` against `env!("CARGO_PKG_VERSION")`, and returns
//! a hint with the right install command derived from the binary's path.
//!
//! Caches the result at `<data-dir>/.update-check` for 24h so opportunistic
//! callers (`rememora setup`, `rememora status`) don't spam the API.
//!
//! Network and cache errors are non-fatal — every public function returns
//! `Result<Option<UpdateAdvice>>` so the caller can render "no advice
//! available" without distinguishing "up-to-date", "offline", or "rate-
//! limited". Set `REMEMORA_NO_UPDATE_CHECK=1` to disable entirely (CI,
//! sandboxes, privacy-conscious users).
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/Rememora/rememora/releases/latest";
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How the running binary was probably installed. Drives the upgrade hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    /// Path matches a known Homebrew prefix.
    Homebrew,
    /// Path looks like `~/.cargo/bin/`.
    Cargo,
    /// Anything else (manual download, custom prefix). Hint points at the
    /// release URL since we can't infer a single command.
    Unknown,
}

impl InstallMethod {
    /// Render the upgrade command appropriate for this install method.
    pub fn upgrade_hint(self) -> &'static str {
        match self {
            Self::Homebrew => "brew update; brew upgrade rememora",
            Self::Cargo => "cargo install rememora --force",
            Self::Unknown => "download the latest release from \
                https://github.com/Rememora/rememora/releases/latest",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpdateAdvice {
    pub current: String,
    pub latest: String,
    pub install_method: InstallMethod,
    pub html_url: String,
}

impl UpdateAdvice {
    pub fn render_hint(&self) -> String {
        format!(
            "Update available: rememora {current} -> {latest}\n  Run: {cmd}\n  Notes: {url}",
            current = self.current,
            latest = self.latest,
            cmd = self.install_method.upgrade_hint(),
            url = self.html_url,
        )
    }
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    /// UNIX seconds when the check ran.
    checked_at: u64,
    /// `tag_name` returned by GitHub, normalized to bare semver (no `v`).
    latest: String,
    /// `html_url` from the release payload.
    html_url: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

/// Detect the install method from the running binary's filesystem path.
/// Falls back to `Unknown` when the path can't be read.
pub fn detect_install_method() -> InstallMethod {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return InstallMethod::Unknown,
    };
    detect_install_method_from_path(&exe)
}

fn detect_install_method_from_path(exe: &Path) -> InstallMethod {
    let s = exe.to_string_lossy();
    // Apple Silicon brew + Intel brew + Linuxbrew default prefixes.
    if s.starts_with("/opt/homebrew/")
        || s.starts_with("/usr/local/Cellar/")
        || s.starts_with("/usr/local/opt/rememora/")
        || s.starts_with("/home/linuxbrew/")
    {
        return InstallMethod::Homebrew;
    }
    // `cargo install` writes to `~/.cargo/bin/` by default. Also accept the
    // plain string `.cargo/bin/` as a non-anchored substring for users with
    // unusual `$HOME` (CI, root, etc.).
    if s.contains("/.cargo/bin/") {
        return InstallMethod::Cargo;
    }
    InstallMethod::Unknown
}

/// Strip a leading `v` from a version string. GitHub tags are usually `v1.4.1`
/// while in-repo `version.py` writes `1.4.1`.
fn normalize_tag(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// Compare two semver-shaped version strings. Returns `true` when `latest`
/// is strictly newer than `current`. Falls back to plain string inequality
/// for non-numeric segments rather than attempting full semver semantics.
pub fn is_newer(current: &str, latest: &str) -> bool {
    let cur = normalize_tag(current);
    let new = normalize_tag(latest);
    if cur == new {
        return false;
    }
    let cur_parts: Vec<u64> = cur.split('.').filter_map(|p| p.parse().ok()).collect();
    let new_parts: Vec<u64> = new.split('.').filter_map(|p| p.parse().ok()).collect();
    if cur_parts.is_empty() || new_parts.is_empty() {
        // Couldn't parse either side as numeric semver — bail out conservatively.
        return false;
    }
    for i in 0..cur_parts.len().max(new_parts.len()) {
        let c = cur_parts.get(i).copied().unwrap_or(0);
        let n = new_parts.get(i).copied().unwrap_or(0);
        match n.cmp(&c) {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
            std::cmp::Ordering::Equal => continue,
        }
    }
    false
}

fn cache_path() -> PathBuf {
    if let Ok(p) = std::env::var("REMEMORA_DB") {
        if let Some(parent) = Path::new(&p).parent() {
            return parent.join(".update-check");
        }
    }
    let mut path = dirs::home_dir().expect("Could not determine home directory");
    path.push(".rememora");
    path.push(".update-check");
    path
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache(path: &Path) -> Option<CacheEntry> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(path: &Path, entry: &CacheEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let raw = serde_json::to_string(entry).context("serialize update cache")?;
    std::fs::write(path, raw).context("write update cache")?;
    Ok(())
}

fn fetch_latest() -> Result<GithubRelease> {
    let agent = ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .build();
    let resp = agent
        .get(RELEASES_LATEST_URL)
        .set("Accept", "application/vnd.github+json")
        .set(
            "User-Agent",
            &format!("rememora-cli/{CURRENT_VERSION} update-check"),
        )
        .call()
        .context("GitHub releases API call failed")?;
    let release: GithubRelease = resp
        .into_json()
        .context("GitHub releases API returned non-JSON")?;
    Ok(release)
}

/// Run the update check.
///
/// - Honours `REMEMORA_NO_UPDATE_CHECK=1` (returns `Ok(None)`).
/// - When `force` is `false`, uses the on-disk cache if it's younger than
///   24h. When `force` is `true`, always hits the API.
/// - Returns `Ok(Some(advice))` only when a newer release was found. All
///   other cases (up-to-date, offline, parse error, rate-limited) return
///   `Ok(None)` so opportunistic callers stay quiet.
pub fn check(force: bool) -> Result<Option<UpdateAdvice>> {
    if std::env::var("REMEMORA_NO_UPDATE_CHECK")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return Ok(None);
    }

    let cache_file = cache_path();
    let now = now_unix();

    // Cache hit: serve from disk without touching the network.
    if !force {
        if let Some(entry) = read_cache(&cache_file) {
            let age = now.saturating_sub(entry.checked_at);
            if age < CACHE_TTL.as_secs() {
                if is_newer(CURRENT_VERSION, &entry.latest) {
                    return Ok(Some(UpdateAdvice {
                        current: CURRENT_VERSION.to_string(),
                        latest: entry.latest,
                        install_method: detect_install_method(),
                        html_url: entry.html_url,
                    }));
                }
                return Ok(None);
            }
        }
    }

    // Cache miss / forced: hit the API.
    let release = match fetch_latest() {
        Ok(r) => r,
        Err(_) => return Ok(None), // network failure is silent
    };
    let latest = normalize_tag(&release.tag_name).to_string();

    // Best-effort cache write. Failures are silent; next call will retry.
    let _ = write_cache(
        &cache_file,
        &CacheEntry {
            checked_at: now,
            latest: latest.clone(),
            html_url: release.html_url.clone(),
        },
    );

    if is_newer(CURRENT_VERSION, &latest) {
        Ok(Some(UpdateAdvice {
            current: CURRENT_VERSION.to_string(),
            latest,
            install_method: detect_install_method(),
            html_url: release.html_url,
        }))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn normalize_tag_strips_leading_v() {
        assert_eq!(normalize_tag("v1.4.1"), "1.4.1");
        assert_eq!(normalize_tag("1.4.1"), "1.4.1");
        assert_eq!(normalize_tag(""), "");
    }

    #[test]
    fn is_newer_basic_cases() {
        assert!(is_newer("1.2.0", "1.4.1"));
        assert!(is_newer("1.2.0", "v1.4.1"));
        assert!(is_newer("1.4.0", "1.4.1"));
        assert!(!is_newer("1.4.1", "1.4.1"));
        assert!(!is_newer("1.4.1", "1.4.0"));
        assert!(!is_newer("2.0.0", "1.9.9"));
    }

    #[test]
    fn is_newer_handles_unparseable_versions() {
        assert!(!is_newer("not-semver", "1.4.1"));
        assert!(!is_newer("1.4.1", "not-semver"));
        assert!(!is_newer("", "1.4.1"));
    }

    #[test]
    fn is_newer_treats_missing_segments_as_zero() {
        assert!(is_newer("1.4", "1.4.1"));
        assert!(!is_newer("1.4.0", "1.4"));
    }

    #[test]
    fn detect_install_method_maps_known_prefixes() {
        let cases = [
            ("/opt/homebrew/bin/rememora", InstallMethod::Homebrew),
            (
                "/usr/local/Cellar/rememora/1.4.1/bin/rememora",
                InstallMethod::Homebrew,
            ),
            ("/home/linuxbrew/.linuxbrew/bin/rememora", InstallMethod::Homebrew),
            ("/Users/me/.cargo/bin/rememora", InstallMethod::Cargo),
            ("/root/.cargo/bin/rememora", InstallMethod::Cargo),
            ("/Users/me/.local/bin/rememora", InstallMethod::Unknown),
            ("/usr/local/bin/rememora", InstallMethod::Unknown),
        ];
        for (path, expected) in cases {
            assert_eq!(
                detect_install_method_from_path(&PathBuf::from(path)),
                expected,
                "path {path}",
            );
        }
    }

    #[test]
    fn upgrade_hint_branches_per_install_method() {
        assert!(InstallMethod::Homebrew.upgrade_hint().contains("brew"));
        assert!(InstallMethod::Cargo.upgrade_hint().contains("cargo install"));
        assert!(InstallMethod::Unknown
            .upgrade_hint()
            .contains("github.com"));
    }
}
