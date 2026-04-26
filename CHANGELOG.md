# Changelog

All notable changes to Rememora will be documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.4.1] — 2026-04-26

Patch release. Surfaced by running `claude plugin validate plugin/` end-to-end as part of the marketplace-install acceptance test that 1.4.0 had skipped.

### Fixed

- **`rememora-init` skill frontmatter parse failure** (#124). `argument-hint: [save|search|status] [text]` put YAML in flow-sequence mode where `|` is a reserved block-scalar marker, so the entire frontmatter block was discarded by Claude Code's plugin loader at install time. The `/rememora` slash command shipped in 1.4.0 with empty metadata. Quoting the value as a plain string scalar fixes the parse.

### Removed

- **Unrecognized `requirements` key** in `plugin/.claude-plugin/plugin.json` (#124). Claude Code's plugin schema doesn't include a `requirements` field; the `binaries: ["rememora"]` declaration was silently ignored. The Setup hook in `setup-check.sh` already covers the same intent (verify `rememora` is on PATH; surface an install hint if not).

[1.4.1]: https://github.com/Rememora/rememora/compare/v1.4.0...v1.4.1

## [1.4.0] — 2026-04-26

This release makes the autonomous memory pipeline actually work end-to-end on modern Claude Code. The Stop-hook curator was silently inert in 1.2.x for most users — every curator-spawned `claude -p` call bailed before the LLM's answer was parsed, so the plugin chain "looked busy" but no memories were ever saved. 1.4.0 closes the entire loop: capture, dedup, and recall all verified in a Docker sandbox against the real Claude Code CLI.

### Added

- **Docker sandbox** for end-to-end plugin validation (`docker/scripts/{build,up,login,exec,down}.sh`). Multi-stage build, OAuth via mounted volume, tmux-driven command injection, libsecret deliberately omitted to force the file-fallback key path. (#99)
- **Hook recursion-gate observability** via `rememora debug record-hook-event`. `rememora usage --hooks` aggregates per-hook gate outcomes (`passed_through`, `cooldown_short_circuit`, `pgrep_short_circuit`, `env_var_short_circuit`). (#82, #96)
- **OTEL telemetry export** layer over `agent_invocations` for local-first usage analytics. (#76, #93)
- **UserPromptSubmit hook** prepends top-3 FTS5 hits to every prompt's additional-context channel. (#77, #87)
- **Cross-agent transfer scenario** (Claude → Codex) added to the eval bench. (#29, #94)
- **Codex rollout curation** via `rememora watch-transcript`. (#81, #89)
- **Progressive-disclosure search CLI** — `search → timeline → get` flow. (#80, #88)
- **Setup hook install hint** when rememora isn't on PATH. (#78, #85)
- **`REMEMORA_DISABLE_HOOKS=1`** kill-switch for all hooks. (#79, #84)
- **Desktop viewer v0** (Tauri, macOS) — opt-in app reading the SQLite DB read-only. (#83, #91, #95)
- **`agent_invocations` per-attempt telemetry** for `rememora agent-run`. (#70, #75)

### Fixed

- **Curator JSON parser** (#119, #120). Modern Claude Code emits `--output-format json` as a single result object; the parser required an array and silently bailed on every signal_gate / AUDN call. **End-to-end effect: the autonomous memory pipeline did nothing on 1.2.x.** Parser now accepts both shapes; sandbox iter 9 verified end-to-end save/dedup.
- **Curator child session explosion** (#117, #118). Only `stop-curate.sh` honored `REMEMORA_CURATE_CHILD`; the other three hooks ran their full bodies inside curator-spawned children, producing ~30 spurious session rows per real user turn. All four hooks now share the gate. **28× reduction** (33 → 1 session row per turn) verified in sandbox iter 7.
- **Concurrent Stop+SessionEnd curate dups** (#121, #122). In `claude -p` both hooks fired back-to-back and spawned curate on the same transcript; two AUDN agents searched in parallel before either's saves committed, producing near-duplicate memories. SessionEnd now skips the tail-pass when Stop's curate is in flight. **~50% cost reduction** ($0.15 → $0.07 per turn).
- **`setup --apply` falsely "already configured"** when DB encrypted but key missing (#113, #116). Setup now probes the env→file→keychain chain after the encryption check; prompts for recovery interactively or surfaces a clear error in non-interactive environments instead of leaving the user with an unopenable DB.
- **`SessionEnd` no-op'd in `claude -p` from unregistered cwds** (#114, #116). Hook called `session end-active` without `--project`; `detect_from_cwd` returned None for `/tmp/<scratch>` paths and silently no-op'd, leaking `[active]` rows. Hook now passes `--project "$(basename $CWD)"` and the CLI gained a `basename(cwd)` fallback.
- **Hook scripts not redeployed on CLI upgrade** (#115, #116). `setup --apply` skipped `deploy_hook_scripts` when settings.json was already configured; users on upgraded binaries kept running stale shell scripts. Deploy now runs unconditionally on `--apply`.
- **Keychain silent-success on Linux without libsecret** (#109, #112). The keyring crate's `set` returned Ok but `get` returned NotFound, so setup falsely reported "stored in OS keychain" while leaving the user unable to open the DB. New `crypto::persist_key_with(setter, getter)` round-trip-verifies and falls back to a 0600 file with a clear UX message.
- **`.claude.json` schema warning in sandbox** (#110, #112). Entrypoint now seeds `~/.claude.json = {}` so non-interactive `claude` doesn't warn at startup.
- **Hook scripts deployed to `~/.rememora/hooks/`** instead of inline 200-char one-liners (#111, #112). `_emit` instrumentation now actually runs, populating `hook_invocations`.
- **Canonical `settings.json` hook envelope schema** (#107, #108). `setup --apply` was writing `{type, command}` directly into the matcher array; Claude Code rejected the entire hook config silently. Setup now embeds `plugin/hooks/hooks.json` via `include_str!` so the deployed shape always matches the schema. **Critical fix** — without it, `setup --apply` left every hook off.
- **Sandbox `known_hosts` collision on rebuild** (#106, #108). `up.sh` now clears `~/.ssh/known_hosts.rememora-sandbox` so each fresh image's sshd keys don't trip "REMOTE HOST IDENTIFICATION HAS CHANGED".
- **FTS5 `OR` queries silently failing** (#103, #105). Search now wraps queries in a safe-OR fallback grammar; the UserPromptSubmit hook also strips FTS5-reserved punctuation before injection.
- **`context --auto` returned empty in Global mode** (#104, #105). Now aggregates across all projects with `[project]` prefixes when no scope is specified.
- **`setup --apply` silently failed on no-keychain Linux** (#100, #102). Now writes a 0600 key file when the keychain backend is unavailable, materializes the DB, and reports the trade-off explicitly.
- **`setup --apply` didn't wire the UserPromptSubmit hook** (#101, #102). The hardcoded canonical hook list was missing the entry; now driven from the embedded plugin manifest.

### Changed

- `setup --apply` now redeploys `plugin/scripts/*.sh` to `~/.rememora/hooks/` on every invocation (idempotent overwrite). Hook script changes ship in lockstep with the CLI binary.
- `--output-format json` parser docs updated to clarify both single-object and array shapes are accepted.
- All four bundled hook scripts (`session-start.sh`, `session-end.sh`, `prompt-search.sh`, `stop-curate.sh`) now early-exit when `REMEMORA_DISABLE_HOOKS` or `REMEMORA_CURATE_CHILD` is set — consistent gate semantics across the chain.

### Known issues

- 6 research/spike issues remain open and are not bug fixes: #25, #26, #27, #28 (research experiments), #29 (cross-agent transfer follow-up), #65 (monitors-based curator spike).
- Crypto unit tests were flaking under parallel `cargo test` (env-var contention). Mitigated with a module-level mutex in `crypto::tests` since 1.4.0.

### Sandbox acceptance

- 12 iterations of Docker sandbox validation against the real Claude Code CLI
- Iter 12 gold-standard: 3 substantive turns in a fresh project → 7 memories captured autonomously (no manual `rememora save`), ~$0.21 total. A 4th `claude -p` in a fresh session correctly recalled all 3 architectural decisions with full reasoning intact.

[1.4.0]: https://github.com/Rememora/rememora/compare/v1.2.1...v1.4.0
