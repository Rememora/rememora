use anyhow::{Context, Result};
use std::process::Command;
use std::time::Duration;

use super::agent_run::{self, AgentRunArgs};

pub struct AgentLoopArgs {
    pub repo: String,
    pub label: String,
    pub poll_secs: u64,
    pub model: Option<String>,
    pub max_budget: Option<f64>,
    pub allow_skip_permissions: bool,
    pub once: bool,
}

pub fn run(args: &AgentLoopArgs) -> Result<()> {
    println!(
        "Agent loop started — watching {} for label '{}'",
        args.repo, args.label
    );
    if !args.once {
        println!("Polling every {}s. Ctrl+C to stop.\n", args.poll_secs);
    }

    loop {
        let issues = find_labeled_issues(&args.repo, &args.label)?;

        if issues.is_empty() {
            if args.once {
                println!("No issues with label '{}' found.", args.label);
                return Ok(());
            }
        } else {
            println!("Found {} issue(s) to work on.", issues.len());

            for (number, title) in &issues {
                println!("\n========================================");
                println!("Working on issue #{number}: {title}");
                println!("========================================\n");

                let result = agent_run::run(&AgentRunArgs {
                    repo: args.repo.clone(),
                    issue: *number,
                    model: args.model.clone(),
                    max_budget: args.max_budget,
                    allow_skip_permissions: args.allow_skip_permissions,
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

fn find_labeled_issues(repo: &str, label: &str) -> Result<Vec<(u64, String)>> {
    let output = Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            repo,
            "--label",
            label,
            "--state",
            "open",
            "--json",
            "number,title",
            "--limit",
            "10",
        ])
        .output()
        .context("Failed to run gh issue list")?;

    if !output.status.success() {
        anyhow::bail!(
            "gh issue list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let items: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).context("Failed to parse issue list")?;

    Ok(items
        .iter()
        .filter_map(|item| {
            let number = item["number"].as_u64()?;
            let title = item["title"].as_str()?.to_string();
            Some((number, title))
        })
        .collect())
}
