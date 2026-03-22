use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

pub struct AgentRunArgs {
    pub repo: String,
    pub issue: u64,
    pub model: Option<String>,
    pub max_budget: Option<f64>,
    pub allow_skip_permissions: bool,
}

pub fn run(args: &AgentRunArgs) -> Result<()> {
    // 1. Fetch issue details
    println!("Fetching issue #{} from {}...", args.issue, args.repo);
    let issue = fetch_issue(&args.repo, args.issue)?;
    println!("Issue: {}", issue.title);

    // 2. Determine repo local path and branch
    let repo_root = find_repo_root()?;
    let branch = format!("agent/issue-{}", args.issue);
    let worktree_path = std::env::temp_dir().join(format!("rememora-agent-{}", args.issue));

    // 3. Create worktree
    if worktree_path.exists() {
        println!("Cleaning up existing worktree...");
        Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&worktree_path)
            .current_dir(&repo_root)
            .output()
            .ok();
    }

    println!("Creating worktree at {}...", worktree_path.display());
    let worktree_out = Command::new("git")
        .args(["worktree", "add", "-B", &branch])
        .arg(&worktree_path)
        .current_dir(&repo_root)
        .output()
        .context("Failed to create git worktree")?;

    if !worktree_out.status.success() {
        bail!(
            "git worktree failed: {}",
            String::from_utf8_lossy(&worktree_out.stderr)
        );
    }

    // 4. Start rememora session
    let session_id = start_session(args.issue, &issue.title)?;
    println!("Rememora session: {session_id}");

    // 5. Build the prompt
    let prompt = build_prompt(&issue);

    // 6. Run claude CLI
    println!("Spawning Claude CLI...\n");
    let claude_result = run_claude(&worktree_path, &prompt, args);

    // 7. Handle result
    match claude_result {
        Ok(output) => {
            println!("\nClaude finished.");

            // Check if there are changes to commit/push
            let has_changes = check_has_changes(&worktree_path)?;

            if has_changes {
                // Push branch
                println!("Pushing branch {branch}...");
                let push = Command::new("git")
                    .args(["push", "origin", &branch, "--force-with-lease"])
                    .current_dir(&worktree_path)
                    .output()
                    .context("Failed to push branch")?;

                if !push.status.success() {
                    eprintln!(
                        "Push failed: {}",
                        String::from_utf8_lossy(&push.stderr)
                    );
                } else {
                    // Create PR
                    println!("Creating PR...");
                    create_pr(&args.repo, args.issue, &issue.title, &branch)?;
                }

                // Relabel issue
                relabel_issue(&args.repo, args.issue, "agent-ready", "in-review").ok();
            } else {
                println!("No changes produced by the agent.");
            }

            // End session
            let summary = if has_changes {
                format!("Worked on issue #{}. Created PR with changes.", args.issue)
            } else {
                format!(
                    "Worked on issue #{} but no changes were produced.",
                    args.issue
                )
            };

            end_session(&session_id, &summary)?;

            // Print claude output summary
            if !output.is_empty() {
                let lines: Vec<&str> = output.lines().collect();
                let tail: Vec<&&str> = lines.iter().rev().take(20).collect();
                println!("\n--- Agent output (last 20 lines) ---");
                for line in tail.into_iter().rev() {
                    println!("{line}");
                }
            }
        }
        Err(e) => {
            eprintln!("Claude CLI failed: {e}");
            end_session(
                &session_id,
                &format!("Failed on issue #{}: {e}", args.issue),
            )?;
        }
    }

    // 8. Cleanup worktree
    println!("Cleaning up worktree...");
    Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&worktree_path)
        .current_dir(&repo_root)
        .output()
        .ok();

    Ok(())
}

// --- helpers ---

struct Issue {
    title: String,
    body: String,
    labels: Vec<String>,
}

fn fetch_issue(repo: &str, number: u64) -> Result<Issue> {
    let output = Command::new("gh")
        .args([
            "issue",
            "view",
            &number.to_string(),
            "--repo",
            repo,
            "--json",
            "title,body,labels",
        ])
        .output()
        .context("Failed to run gh issue view")?;

    if !output.status.success() {
        bail!(
            "gh issue view failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let v: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse issue JSON")?;

    Ok(Issue {
        title: v["title"].as_str().unwrap_or("").to_string(),
        body: v["body"].as_str().unwrap_or("").to_string(),
        labels: v["labels"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|l| l["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn find_repo_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to find git repo root")?;

    if !output.status.success() {
        bail!("Not in a git repository");
    }

    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

fn build_prompt(issue: &Issue) -> String {
    let labels = if issue.labels.is_empty() {
        String::new()
    } else {
        format!("\nLabels: {}", issue.labels.join(", "))
    };

    format!(
        r#"You are working on a GitHub issue. Implement the requested changes.

## Issue: {}
{}
{}

## Instructions
1. Read the relevant files in the codebase to understand the current state
2. Implement the changes described in the issue
3. Run `cargo test` to verify your changes don't break anything
4. Run `cargo clippy` to check for warnings
5. If tests pass, commit your changes with a descriptive message referencing the issue
6. If tests fail, fix the issues and try again

Do NOT push or create PRs — that will be handled automatically.
Keep your changes focused on what the issue asks for. Do not refactor unrelated code."#,
        issue.title, issue.body, labels
    )
}

fn run_claude(worktree: &PathBuf, prompt: &str, args: &AgentRunArgs) -> Result<String> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(prompt);
    cmd.arg("--output-format").arg("text");
    cmd.current_dir(worktree);

    if let Some(ref model) = args.model {
        cmd.arg("--model").arg(model);
    }

    if let Some(budget) = args.max_budget {
        cmd.arg("--max-budget-usd").arg(budget.to_string());
    }

    if args.allow_skip_permissions {
        cmd.arg("--dangerously-skip-permissions");
    } else {
        // Allow common dev tools without prompting
        cmd.arg("--allowedTools")
            .arg("Bash(cargo:*) Bash(git:*) Bash(rememora:*) Read Edit Write Glob Grep");
    }

    let output = cmd.output().context("Failed to run claude CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("claude exited with error: {stderr}");
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn check_has_changes(worktree: &PathBuf) -> Result<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree)
        .output()
        .context("Failed to check git status")?;

    let status = String::from_utf8_lossy(&output.stdout);

    // Also check if there are commits ahead of the base
    let log = Command::new("git")
        .args(["log", "HEAD", "--not", "--remotes", "--oneline"])
        .current_dir(worktree)
        .output()
        .context("Failed to check git log")?;

    let commits = String::from_utf8_lossy(&log.stdout);

    Ok(!status.trim().is_empty() || !commits.trim().is_empty())
}

fn start_session(issue: u64, title: &str) -> Result<String> {
    let output = Command::new("rememora")
        .args([
            "session",
            "start",
            "--agent",
            "claude-code-auto",
            "--project",
            "rememora",
            "--intent",
            &format!("Issue #{issue}: {title}"),
        ])
        .output()
        .context("Failed to start rememora session")?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn end_session(id: &str, summary: &str) -> Result<()> {
    Command::new("rememora")
        .args(["session", "end", id, "--summary", summary])
        .output()
        .context("Failed to end rememora session")?;
    Ok(())
}

fn create_pr(repo: &str, issue: u64, title: &str, branch: &str) -> Result<()> {
    let pr_title = format!("#{issue}: {title}");
    let pr_body = format!("Automated implementation for #{issue}.\n\nCloses #{issue}");

    let output = Command::new("gh")
        .args([
            "pr",
            "create",
            "--repo",
            repo,
            "--title",
            &pr_title,
            "--body",
            &pr_body,
            "--head",
            branch,
        ])
        .output()
        .context("Failed to create PR")?;

    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout);
        println!("PR created: {}", url.trim());
    } else {
        eprintln!(
            "PR creation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

fn relabel_issue(repo: &str, issue: u64, remove: &str, add: &str) -> Result<()> {
    Command::new("gh")
        .args([
            "issue",
            "edit",
            &issue.to_string(),
            "--repo",
            repo,
            "--remove-label",
            remove,
            "--add-label",
            add,
        ])
        .output()
        .context("Failed to relabel issue")?;
    Ok(())
}
