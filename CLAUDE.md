@AGENTS.md

# Claude Code

- Shared repo runtime artifacts live under `.agents/`
- Claude-specific subagents live under `.claude/agents/`
- Prefer delegating Rememora workflow tasks to:
  - `.claude/agents/rememora-developer.md` for ticket implementation work
  - `.claude/agents/rememora-triage.md` for project board triage work
- Use `.agents/worktrees/` for any new local git worktrees
