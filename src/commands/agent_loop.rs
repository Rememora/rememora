use anyhow::{Context, Result};
use std::process::Command;
use std::time::Duration;

use super::agent_run::{self, AgentRunArgs};

const STATUS_DONE: &str = "98236657";

pub struct AgentLoopArgs {
    pub repo: String,
    pub poll_secs: u64,
    pub model: Option<String>,
    pub max_budget: Option<f64>,
    pub allow_skip_permissions: bool,
    pub once: bool,
    pub retries: u32,
}

pub fn run(args: &AgentLoopArgs) -> Result<()> {
    println!(
        "Agent loop started — watching {} project board",
        args.repo
    );
    if !args.once {
        println!("Polling every {}s. Ctrl+C to stop.\n", args.poll_secs);
    }

    let owner = args.repo.split('/').next().unwrap_or("Rememora");

    loop {
        // 1. Process Cherry-Picked → Done (merged PRs)
        let cherry_picked = find_items_by_status(owner, "Cherry-Picked")?;
        for (item_id, number, title) in &cherry_picked {
            if is_pr_merged(&args.repo, *number) {
                println!("Issue #{number} ({title}) — PR merged, moving to Done");
                agent_run::move_to_column(item_id, STATUS_DONE).ok();
            }
        }

        // 2. Pick up Ready-For-Dev items
        let ready = find_items_by_status(owner, "Ready-For-Dev")?;

        if ready.is_empty() {
            if args.once {
                println!("No issues in Ready-For-Dev.");
                return Ok(());
            }
        } else {
            println!("Found {} issue(s) ready for dev.", ready.len());

            for (_item_id, number, title) in &ready {
                println!("\n========================================");
                println!("Working on issue #{number}: {title}");
                println!("========================================\n");

                let result = agent_run::run(&AgentRunArgs {
                    repo: args.repo.clone(),
                    issue: *number,
                    model: args.model.clone(),
                    max_budget: args.max_budget,
                    allow_skip_permissions: args.allow_skip_permissions,
                    retries: args.retries,
                });

                match result {
                    Ok(()) => println!("\nIssue #{number} completed."),
                    Err(e) => eprintln!("\nIssue #{number} failed: {e}"),
                }
            }
        }

        if args.once {
            return Ok(());
        }

        println!(
            "\nSleeping {}s before next poll...",
            args.poll_secs
        );
        std::thread::sleep(Duration::from_secs(args.poll_secs));
    }
}

fn find_items_by_status(owner: &str, status: &str) -> Result<Vec<(String, u64, String)>> {
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
        anyhow::bail!(
            "gh project item-list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let data: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse project items")?;

    let items = data["items"]
        .as_array()
        .map(|arr| arr.as_slice())
        .unwrap_or(&[]);

    Ok(items
        .iter()
        .filter_map(|item| {
            let item_status = item["status"].as_str()?;
            if item_status != status {
                return None;
            }
            let item_id = item["id"].as_str()?.to_string();
            let number = item["content"]["number"].as_u64()?;
            let title = item["content"]["title"].as_str()?.to_string();
            Some((item_id, number, title))
        })
        .collect())
}

fn is_pr_merged(repo: &str, issue_number: u64) -> bool {
    // Check if there's a merged PR that closes this issue
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            repo,
            "--search",
            &format!("#{issue_number}"),
            "--state",
            "merged",
            "--json",
            "number",
            "--limit",
            "1",
        ])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            // If we get a non-empty array, there's a merged PR
            text.trim() != "[]"
        }
        _ => false,
    }
}
