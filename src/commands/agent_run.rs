use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

// GitHub Project board IDs for Rememora/rememora project #3
const PROJECT_ID: &str = "PVT_kwDOCB405M4BSdN1";
const STATUS_FIELD_ID: &str = "PVTSSF_lADOCB405M4BSdN1zg__B7M";
const STATUS_IN_PROGRESS: &str = "47fc9ee4";
const STATUS_READY_FOR_REVIEW: &str = "7e86c92f";
const STATUS_READY_FOR_DEV: &str = "eafe2cca";

pub struct AgentRunArgs {
    pub repo: String,
    pub issue: u64,
    pub model: Option<String>,
    pub max_budget: Option<f64>,
    pub allow_skip_permissions: bool,
    pub retries: u32,
}

pub fn run(args: &AgentRunArgs) -> Result<()> {
    // 1. Fetch issue details
    println!("Fetching issue #{} from {}...", args.issue, args.repo);
    let issue = fetch_issue(&args.repo, args.issue)?;
    println!("Issue: {}", issue.title);

    // 2. Move to In Progress on the project board
    if let Some(ref item_id) = issue.project_item_id {
        println!("Moving to In Progress...");
        move_to_column(item_id, STATUS_IN_PROGRESS).ok();
    }

    // 3. Set up worktree
    let repo_root = find_repo_root()?;
    let branch = format!("agent/issue-{}", args.issue);
    let worktree_path = std::env::temp_dir().join(format!("rememora-agent-{}", args.issue));

    if worktree_path.exists() {
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

    // 5. Quality loop: run claude, check quality, retry if needed
    let mut last_error = String::new();
    let mut success = false;

    for attempt in 1..=args.retries {
        println!("\n--- Attempt {}/{} ---", attempt, args.retries);

        // Build prompt (include previous error if retrying)
        let prompt = if last_error.is_empty() {
            build_prompt(&issue)
        } else {
            build_retry_prompt(&issue, &last_error)
        };

        // Run claude
        println!("Spawning Claude CLI...");
        match run_claude(&worktree_path, &prompt, args) {
            Ok(_output) => {
                println!("Claude finished. Running quality checks...");
            }
            Err(e) => {
                last_error = format!("Claude CLI failed: {e}");
                eprintln!("{last_error}");
                continue;
            }
        }

        // Quality gate
        match run_quality_checks(&worktree_path) {
            Ok(quality) => {
                if quality.all_pass() {
                    println!("All quality checks passed!");
                    success = true;
                    break;
                } else {
                    last_error = quality.format_failures();
                    eprintln!("Quality checks failed:\n{last_error}");
                }
            }
            Err(e) => {
                last_error = format!("Quality check error: {e}");
                eprintln!("{last_error}");
            }
        }
    }

    // 6. Handle outcome
    if success && check_has_changes(&worktree_path)? {
        // Push and create PR
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
            println!("Creating PR...");
            create_pr(&args.repo, args.issue, &issue.title, &branch)?;

            // Move to Ready-For-Review
            if let Some(ref item_id) = issue.project_item_id {
                println!("Moving to Ready-For-Review...");
                move_to_column(item_id, STATUS_READY_FOR_REVIEW).ok();
            }
        }

        end_session(
            &session_id,
            &format!("Implemented issue #{}. PR created.", args.issue),
        )?;
    } else if !success {
        eprintln!(
            "Failed after {} attempts. Moving back to Ready-For-Dev.",
            args.retries
        );

        // Comment on the issue with failure details
        add_issue_comment(
            &args.repo,
            args.issue,
            &format!(
                "Agent failed after {} attempts.\n\nLast error:\n```\n{}\n```",
                args.retries, last_error
            ),
        )
        .ok();

        // Move back to Ready-For-Dev
        if let Some(ref item_id) = issue.project_item_id {
            move_to_column(item_id, STATUS_READY_FOR_DEV).ok();
        }

        end_session(
            &session_id,
            &format!("Failed on issue #{}: {}", args.issue, last_error),
        )?;
    } else {
        println!("No changes produced by the agent.");
        end_session(
            &session_id,
            &format!(
                "No changes produced for issue #{}.",
                args.issue
            ),
        )?;

        if let Some(ref item_id) = issue.project_item_id {
            move_to_column(item_id, STATUS_READY_FOR_DEV).ok();
        }
    }

    // 7. Cleanup worktree
    println!("Cleaning up worktree...");
    Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&worktree_path)
        .current_dir(&repo_root)
        .output()
        .ok();

    Ok(())
}

// --- Data types ---

pub struct Issue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub project_item_id: Option<String>,
}

struct QualityResult {
    tests_pass: bool,
    tests_output: String,
    clippy_pass: bool,
    clippy_output: String,
    has_commits: bool,
}

impl QualityResult {
    fn all_pass(&self) -> bool {
        self.tests_pass && self.clippy_pass && self.has_commits
    }

    fn format_failures(&self) -> String {
        let mut out = String::new();
        if !self.has_commits {
            out.push_str("No commits found — you must commit your changes.\n");
        }
        if !self.tests_pass {
            out.push_str(&format!("cargo test FAILED:\n{}\n", self.tests_output));
        }
        if !self.clippy_pass {
            out.push_str(&format!(
                "cargo clippy FAILED:\n{}\n",
                self.clippy_output
            ));
        }
        out
    }
}

// --- Helpers ---

pub fn fetch_issue(repo: &str, number: u64) -> Result<Issue> {
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

    let project_item_id = find_project_item_id(repo, number).ok();

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
        project_item_id,
    })
}

pub fn find_project_item_id(repo: &str, issue_number: u64) -> Result<String> {
    let owner = repo.split('/').next().unwrap_or("Rememora");

    let output = Command::new("gh")
        .args([
            "project",
            "item-list",
            "3",
            "--owner",
            owner,
            "--format",
            "json",
        ])
        .output()
        .context("Failed to list project items")?;

    if !output.status.success() {
        bail!("gh project item-list failed");
    }

    let data: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse project items")?;

    let items = data["items"].as_array().context("No items array")?;

    for item in items {
        if let Some(num) = item["content"]["number"].as_u64() {
            if num == issue_number {
                if let Some(id) = item["id"].as_str() {
                    return Ok(id.to_string());
                }
            }
        }
    }

    bail!("Issue #{issue_number} not found in project board")
}

pub fn move_to_column(item_id: &str, status_option_id: &str) -> Result<()> {
    let output = Command::new("gh")
        .args([
            "project",
            "item-edit",
            "--id",
            item_id,
            "--field-id",
            STATUS_FIELD_ID,
            "--project-id",
            PROJECT_ID,
            "--single-select-option-id",
            status_option_id,
        ])
        .output()
        .context("Failed to update project item status")?;

    if !output.status.success() {
        bail!(
            "gh project item-edit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
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
4. Run `cargo clippy -- -D warnings` to check for warnings
5. If tests pass, commit your changes with a descriptive message referencing the issue
6. If tests fail, fix the issues and try again

Do NOT push or create PRs — that will be handled automatically.
Keep your changes focused on what the issue asks for. Do not refactor unrelated code."#,
        issue.title, issue.body, labels
    )
}

fn build_retry_prompt(issue: &Issue, last_error: &str) -> String {
    let base = build_prompt(issue);
    format!(
        r#"{base}

## Previous attempt failed

The previous attempt did not pass quality checks. Here is the error:

```
{last_error}
```

Fix these issues. Make sure `cargo test` passes and `cargo clippy -- -D warnings` is clean before committing."#
    )
}

fn run_claude(worktree: &Path, prompt: &str, args: &AgentRunArgs) -> Result<String> {
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

fn run_quality_checks(worktree: &Path) -> Result<QualityResult> {
    // Check for commits
    let log = Command::new("git")
        .args(["log", "HEAD", "--not", "--remotes", "--oneline"])
        .current_dir(worktree)
        .output()
        .context("Failed to check git log")?;
    let has_commits = !String::from_utf8_lossy(&log.stdout).trim().is_empty();

    // cargo test
    let test_out = Command::new("cargo")
        .args(["test"])
        .current_dir(worktree)
        .output()
        .context("Failed to run cargo test")?;
    let tests_pass = test_out.status.success();
    let tests_output = format!(
        "{}{}",
        String::from_utf8_lossy(&test_out.stdout),
        String::from_utf8_lossy(&test_out.stderr)
    );

    // cargo clippy
    let clippy_out = Command::new("cargo")
        .args(["clippy", "--", "-D", "warnings"])
        .current_dir(worktree)
        .output()
        .context("Failed to run cargo clippy")?;
    let clippy_pass = clippy_out.status.success();
    let clippy_output = format!(
        "{}{}",
        String::from_utf8_lossy(&clippy_out.stdout),
        String::from_utf8_lossy(&clippy_out.stderr)
    );

    Ok(QualityResult {
        tests_pass,
        tests_output,
        clippy_pass,
        clippy_output,
        has_commits,
    })
}

fn check_has_changes(worktree: &Path) -> Result<bool> {
    let log = Command::new("git")
        .args(["log", "HEAD", "--not", "--remotes", "--oneline"])
        .current_dir(worktree)
        .output()
        .context("Failed to check git log")?;

    Ok(!String::from_utf8_lossy(&log.stdout).trim().is_empty())
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

fn add_issue_comment(repo: &str, issue: u64, body: &str) -> Result<()> {
    Command::new("gh")
        .args([
            "issue",
            "comment",
            &issue.to_string(),
            "--repo",
            repo,
            "--body",
            body,
        ])
        .output()
        .context("Failed to add issue comment")?;
    Ok(())
}
