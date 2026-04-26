//! `rememora update [--check]` — surface upgrade advice without ever
//! mutating the install. We deliberately do NOT auto-execute the upgrade:
//! mixed install methods (brew vs cargo vs curl|sh), sudo prompts, and
//! the risk of force-upgrading users onto a release we just shipped make
//! "tell, don't do" the safer default. The marketplace plugin lives on
//! its own lineage and updates via `claude plugin update rememora@rememora`.
use anyhow::Result;

use rememora::update_check;

pub fn run(check_only: bool, json: bool) -> Result<()> {
    // `--check` and bare `update` differ only in whether we hit the cache
    // or always go to the network. Bare `update` is the user-initiated
    // path so we force a fresh check; `--check` exists for callers (CI,
    // hooks, scripts) that want to respect the 24h cache window.
    let force = !check_only;
    let advice = update_check::check(force)?;

    match (advice, json) {
        (Some(a), true) => {
            let payload = serde_json::json!({
                "status": "update-available",
                "current": a.current,
                "latest": a.latest,
                "install_method": match a.install_method {
                    update_check::InstallMethod::Homebrew => "homebrew",
                    update_check::InstallMethod::Cargo => "cargo",
                    update_check::InstallMethod::Unknown => "unknown",
                },
                "upgrade_command": a.install_method.upgrade_hint(),
                "release_url": a.html_url,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        (Some(a), false) => {
            println!("{}", a.render_hint());
        }
        (None, true) => {
            println!(
                "{}",
                serde_json::json!({
                    "status": "up-to-date",
                    "current": env!("CARGO_PKG_VERSION"),
                })
            );
        }
        (None, false) => {
            println!(
                "rememora {} is up to date.",
                env!("CARGO_PKG_VERSION"),
            );
        }
    }
    Ok(())
}
