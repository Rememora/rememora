# Plan: monitors-based long-lived curator (Issue #65)

**Ticket:** https://github.com/Rememora/rememora/issues/65
**Branch:** `feat/65-monitors-curator`
**Worktree:** `.agents/worktrees/issue-65`
**Scope:** full staged migration — stages 1 → 4, end-to-end.

---

## Goal

Replace the per-turn `Stop`-hook + `plugin/scripts/stop-curate.sh` + `${TMPDIR}`
lockfile architecture with a single Claude Code `monitors` entry that runs a
long-lived curator for the entire session. The monitor pipes the session JSONL
through a new `rememora curate --stream` subcommand that processes transcript
deltas incrementally and emits a rate-limited notification stream.

Structural wins (vs post-#63 state):

- "≤1 curate per session" becomes a language-level guarantee (one `tail | rememora`
  pipeline per session), not a `pgrep`/filesystem invariant.
- Signal-detector / curator runs on *new* transcript bytes only; no full re-scan.
- Back-pressure is natural (blocking pipe between `tail -F` and `rememora`).
- Rate-limiting moves into the Rust process as an explicit token bucket.
- No `${TMPDIR}` lockfiles, no `setsid`/`nohup` detachment gymnastics.

## Non-goals

- Cross-agent liveness monitor (`rememora events --follow`) — separate ticket.
- Skill-scoped ambient search monitor — separate ticket.
- Rewriting `src/curator.rs` signal-gate / curator prompts — reused as-is in
  stage 1 (delta-friendly reshaping is a later follow-up if budget demands).
- Removing `rememora curate` full-transcript mode — kept for headless /
  `claude -p` / CI and for the SessionEnd final-flush.

---

## Files scouted

| Path | Purpose | Touched in stage |
|---|---|---|
| `src/main.rs` | clap `Curate` subcommand, dispatch to `commands::curate::run` (lines 236–261, 671–689). | 1 |
| `src/commands/curate.rs` | Orchestrator: resolves files, calls `curator::signal_gate` + `curator::curate`, records telemetry, updates watermarks. Already has `--from-stdin` path (`curate_stdin`). | 1, 4 |
| `src/curator.rs` | `signal_gate` (Haiku via `claude -p`) and `curate` (Sonnet via `claude -p`) subagents. `MIN_TRANSCRIPT_CHARS=500`. Reused by the streaming path. | 1 (reuse) |
| `src/jsonl.rs` | `parse_file(path, byte_offset)` + `parse_reader<R>`. Offset-aware, caps at 32 KB per call. Reused on every stream chunk. | 1 (reuse) |
| `src/models/watermark.rs` | `(file_path, byte_offset, line_count)` persistence + `log_action`. Stream mode persists watermark per successful chunk. | 1 |
| `src/lib.rs` | Library re-exports. | 1 (add stream module) |
| `plugin/.claude-plugin/plugin.json` | Version bump + optional `monitors` string key. | 1, 3 |
| `plugin/hooks/hooks.json` | Currently `SessionStart` + `Stop` + `SessionEnd`. Stop is removed in stage 3; reshaped to headless-only in stage 4. | 3, 4 |
| `plugin/scripts/stop-curate.sh` | Current Stop-hook entry. Reshaped in stage 3 (env-gated short-circuit), removed in stage 4. | 3, 4 |
| `plugin/scripts/session-end.sh` | SessionEnd flush already runs `rememora curate --file ...`. Stays as final-flush safety net. | unchanged (verify only) |
| `plugin/monitors/monitors.json` | NEW. Declares the `curate` monitor. | 1 (opt-in), 3 (always-on) |
| `plugin/scripts/curate-monitor.sh` | NEW. Wrapper: resolves JSONL path, runs `tail -F -n 0 ... | rememora curate --stream` with a crash-loop wrapper. | 1 |
| `plugin/README.md` | Documents monitors-based curator, Stop-hook retirement. | 3, 4 |

## New modules / files

- **`src/stream.rs`** (library module) — streaming curator state machine.
  - Public entry: `run(opts: StreamOpts, stdin: impl BufRead, stdout: impl Write, conn: &Connection) -> Result<()>`.
  - Internals:
    - Accumulator buffer for unparsed JSONL bytes (lines can arrive partially from `tail -F`).
    - Periodic `flush_tick` (e.g. every `REMEMORA_STREAM_FLUSH_MS`, default `10_000` ms of idle, or after `N` new bytes) to decide whether to call the signal gate.
    - Reuses `jsonl::parse_reader` on the fresh-byte slab, renders via `render_transcript`, calls `curator::signal_gate` → `curator::curate` exactly like `curate_file`, then advances the in-memory byte offset.
    - Persists watermark after each successful cycle via `watermark::set` so SessionEnd / restart resume at the right place.
    - Token-bucket limiter on stdout writes. Capacity 1, refill 1 / `REMEMORA_STREAM_NOTIFY_SECS` (default 30 s). Collapsed notifications are counted and summarized on next emit (`"saved 3 memories (2 decision, 1 case) — 2 notifications coalesced"`).
    - Telemetry via `agent_invocation::try_insert` for both gate + curator calls (same as `curate_file`).
- **`plugin/scripts/curate-monitor.sh`** — bash wrapper, ~40 lines.
  - Resolves `$JSONL_PATH` the same way `stop-curate.sh` does (encoded `cwd` +
    `session_id`), but via env vars Claude Code exposes for monitors: `CLAUDE_SESSION_ID`, `CLAUDE_CWD` (fall back to `$PWD`). Unknown which env vars are actually set — stage 1 dogfood will confirm; if absent we probe the newest `~/.claude/projects/<encoded-cwd>/*.jsonl` by mtime.
  - `while true; do tail -F -n 0 "$JSONL_PATH" | rememora curate --stream --project "$PROJECT" --session "$SESSION_ID"; echo "monitor: curator exited, restarting in 5s"; sleep 5; done`
  - On startup, emits one line: `"rememora curator online (session $SESSION_ID)"`.
  - Short-circuits when `REMEMORA_CURATE_CHILD=1` (curator's own `claude -p` child spawns).
- **`plugin/monitors/monitors.json`** — manifest entry.

---

## Stage 1 — Streaming producer (opt-in via env)

**Outcome:** `rememora curate --stream` exists and is fully tested. The plugin
ships a `curate-monitor.sh` wrapper and `monitors.json`, but the monitor is
**disabled by default** — it short-circuits unless `REMEMORA_USE_MONITOR=1` is
set. The Stop hook is unchanged.

**Rust changes**

1. `src/main.rs`: extend the `Curate` clap variant with `--stream`, `--session`,
   `--stream-flush-ms` (default 10000), `--stream-notify-secs` (default 30).
   `--stream` uses clap `conflicts_with` against `--file`, `--from-stdin`,
   `--auto`, `--reset-watermark`. Dispatch `--stream` to
   `commands::curate::run_stream(&conn, &opts)`.

2. `src/commands/curate.rs`: add `run_stream(conn, opts)` wrapper that builds
   `StreamOpts` and delegates to `rememora::stream::run(...)`.

3. **NEW** `src/stream.rs` (added to `src/lib.rs` as `pub mod stream;`).
   - `StreamOpts { session_id: Option<String>, project: Option<String>, flush_ms: u64, notify_secs: u64, dry_run: bool }`.
   - Main loop reads stdin line-by-line. Each line is appended to a `String` buffer; parsing is driven by `jsonl::parse_reader` called on a `Cursor` over the buffer *only when* the buffer has grown past `MIN_FLUSH_BYTES` or `flush_ms` has elapsed without growth.
   - Successful parses produce `TranscriptEntry`s added to a rolling window (capped at `MAX_TRANSCRIPT_BYTES` — 32 KB — to match `jsonl.rs`). The window represents "transcript delta since last gate call".
   - Signal-gate the delta; if `Signal::Yes`, run the curator exactly like `curate_file`. Afterwards, shrink the window (reset to empty) and advance a "last-curated byte offset" for watermark persistence.
   - Stdout emission is rate-limited: a `TokenBucket { capacity: 1, refill_every: notify_secs }`. When a line can't be emitted, it's folded into a pending `SuppressedStats` struct; on next token, emit one coalesced line.
   - Graceful shutdown: on EOF (tail exited) or SIGTERM/SIGINT, emit a final summary line and return. On SIGPIPE (Claude Code closed the monitor), also exit cleanly.

4. **Tests** (`src/stream.rs` `#[cfg(test)]`):
   - `stream_handles_partial_lines` — feed a JSONL line split across two reads; parse succeeds after second chunk.
   - `stream_coalesces_notifications` — feed N signal-yielding chunks within `notify_secs`; verify `stdout` has exactly 1 line.
   - `stream_advances_watermark` — after a successful curate cycle, `watermark::get` returns the right offset.
   - `stream_persists_across_reopen` — simulate restart: new `run()` call reads existing watermark, skips already-processed bytes. (Uses in-memory DB via `db::open_memory`.)
   - `stream_exits_clean_on_eof` — stdin closes → `run()` returns `Ok(())`.
   - Signal-gate + curator are **stubbed** in tests via a trait or cfg feature to avoid hitting `claude -p`. Prefer lifting `signal_gate` / `curate` behind a `trait Subagent` and injecting a fake — minimal refactor, keeps `curator.rs` pure of test infra.

5. **Integration test** (`tests/stream_integration.rs`):
   - Spawn `rememora curate --stream --session TEST --project X` as a subprocess;
     pipe a recorded-fixture JSONL into stdin; assert process exits cleanly and
     watermark row exists. Signal gate short-circuits on short fixtures
     (< 500 chars), so we can assert the no-signal / watermark-advance path
     without LLM calls.

**Plugin changes**

1. **NEW** `plugin/monitors/monitors.json`:
   ```json
   [
     {
       "name": "curate",
       "command": "bash ${CLAUDE_PLUGIN_ROOT}/scripts/curate-monitor.sh",
       "description": "Incremental rememora curator",
       "when": "always"
     }
   ]
   ```
2. **NEW** `plugin/scripts/curate-monitor.sh`:
   - First line of body: `[ "${REMEMORA_USE_MONITOR:-0}" = "1" ] || exit 0`
     — the monitor is opt-in in stage 1.
   - Next line: `[ -n "${REMEMORA_CURATE_CHILD:-}" ] && exit 0` — so the
     curator's own `claude -p` children never spawn a recursive monitor.
   - Resolves `$JSONL_PATH` + `$PROJECT` + `$SESSION_ID`. Same encoding logic as
     `stop-curate.sh`. On missing path, emits one notification line and exits 0.
   - Runs the `while true; do tail -F -n 0 … | rememora curate --stream … ; sleep 5; done` loop, with a max-restart cap (5 restarts in 60 s → sleep 60 s).
3. `plugin/.claude-plugin/plugin.json`: add a `monitors` pointer:
   `"monitors": "monitors/monitors.json"`.
   Bump `version` to `1.3.0` (minor — new runtime subsystem, opt-in).

**Migration / rollback**

- Stage 1 is opt-in via `REMEMORA_USE_MONITOR=1`. Default behavior is unchanged:
  Stop hook runs, monitor short-circuits.
- Rollback = leave `REMEMORA_USE_MONITOR` unset (default). If shipping the
  wrapper broke something catastrophic, revert `plugin/.claude-plugin/plugin.json`
  to drop the `monitors` key.

**Success criteria**

- `cargo test && cargo clippy` green, including new stream tests.
- Setting `REMEMORA_USE_MONITOR=1` in a real Claude Code session shows one
  `"rememora curator online"` notification at session start.
- During the session, no notifications for short turns; a single rate-limited
  line appears after a curate-worthy chunk.
- `sqlite3 ~/.rememora/rememora.db 'select * from watermarks'` shows the session
  JSONL path with a non-zero, advancing `byte_offset`.

---

## Stage 2 — Dogfood

**Outcome:** I personally run with `REMEMORA_USE_MONITOR=1` for ~1 week across
daily Rememora work. We measure the four success metrics from the issue:

1. `pgrep -fa "rememora curate --stream"` → exactly 1 per Claude Code session.
2. No files matching `${TMPDIR}/rememora-curate-*.last` accumulating (stamp
   files don't fire when monitor short-circuits the Stop hook; verify).
3. Notification count averages ≤1 / 30 s over an 8-hour session.
   Instrument: add a `--notify-log` flag to `--stream` that appends `(ts, kind)`
   tuples to a log file. Tally post-hoc.
4. Signal-detector input char count scales with delta, not session length.
   Instrument: log `detector_input_chars` to `agent_invocation` telemetry —
   reuse the existing table by stuffing the metric into its metadata path; no
   new migration.

**Handling Stop hook during dogfood:** in stage 2, Stop hook remains enabled.
To avoid double-curate, `stop-curate.sh` gains a short pre-flight:

```bash
# If a monitor is processing this session, let it handle curation.
if pgrep -f "rememora curate --stream.*--session ${SESSION_ID}" >/dev/null 2>&1; then
  exit 0
fi
```

This block is added in stage 2 as a minimal patch. (It's a temporary bridge —
removed in stage 4 along with the script itself.)

**Rollback:** `unset REMEMORA_USE_MONITOR` in shell; Stop hook resumes its role.

**Success criteria for stage 2 (gate for stage 3):**

- ≥ 5 consecutive days with monitor-only curation and no stampedes or missed
  curate runs.
- Notification volume ≤1/30 s per the log.
- No new GitHub issues filed against #65 behavior.
- SessionEnd final-flush verified to still work (kill monitor mid-session,
  end session, observe final curate via `rememora session end-active` path).

If any criterion fails, we iterate stage 1 fixes, not advance to stage 3.

---

## Stage 3 — Flip the default (monitor always-on)

**Outcome:** monitors-based curation is the default. Stop hook stops firing
its curate path.

**Changes**

1. `plugin/scripts/curate-monitor.sh`: remove the `REMEMORA_USE_MONITOR` gate.
   The monitor runs by default whenever `monitors` is supported.
2. `plugin/hooks/hooks.json`: **remove the `Stop` block entirely**.
   (Keep SessionStart + SessionEnd.)
3. `plugin/scripts/stop-curate.sh`: keep the file in-tree for one release cycle
   as a no-op stub (immediate `exit 0`) documenting it's deprecated. Removed in
   stage 4.
4. `plugin/README.md`: rewrite section 4 ("After each agent turn…") to describe
   the monitor architecture. Remove `setsid` / `nohup` / lockfile prose.
5. `plugin/.claude-plugin/plugin.json`: bump `version` to `1.4.0` (minor —
   removes a hook and replaces it with a runtime subsystem, users deserve a
   changelog entry).
6. `plugin/scripts/session-end.sh`: verify the final-flush still works with
   monitor up. The monitor handles live turns; SessionEnd should still run
   `rememora curate --file` to capture any unflushed tail (the monitor may be
   mid-cycle when session ends).

**Migration / rollback**

- Users on plugin < 1.4.0 continue to use Stop-hook architecture unchanged.
- Users on plugin ≥ 1.4.0 without Claude Code v2.1.105+ get no curation
  (monitors key ignored, Stop hook absent). Plugin README gains a
  "Requires Claude Code ≥ 2.1.105" bullet at install instructions.
  Alternative (keep both for one release) is rejected: the two-code-paths bug
  surface is worse than forcing a Claude Code upgrade.
- Rollback: revert plugin to 1.3.x.

**Success criteria**

- CI green. Plugin installs cleanly.
- Two dogfood days with zero regressions; no `rememora-curate-*.last` files
  created (`ls $TMPDIR/rememora-curate-*` returns nothing).
- `rememora usage` shows signal-gate + curator invocations flowing as before.
- Stop hook truly absent from hook JSON.

---

## Stage 4 — Remove the Stop-hook curator; keep narrow headless fallback

**Outcome:** `stop-curate.sh` is deleted. Headless (`claude -p`, API, CI) falls
back to SessionEnd's existing final-flush.

**Changes**

1. Delete `plugin/scripts/stop-curate.sh`.
2. Delete the deprecated `Stop` hook block reference (already gone in stage 3).
3. Headless fallback: **Option A (preferred)** — monitors are interactive-only;
   `SessionEnd` already runs a final `rememora curate --file` flush; in headless
   that covers everything. **No new script needed.** `session-end.sh` is the only
   fallback.
   - Option B (escape hatch if Option A proves insufficient): a new
     `plugin/scripts/curate-headless.sh` that checks `CLAUDE_CODE_HEADLESS` and
     is wired as a narrow `Stop` hook. Reintroduces Stop hook for one case.
4. `src/commands/curate.rs`: no logic changes; `curate_file` / `curate_stdin`
   remain the headless code path.
5. `plugin/README.md`: final polish. Remove all `stop-curate.sh` references.
6. `plugin/.claude-plugin/plugin.json`: bump `version` to `1.5.0`.

**Migration / rollback**

- Rollback = revert the plugin to 1.4.x, re-add `stop-curate.sh`. Rust CLI is
  unchanged across stage 3 → stage 4, so no CLI revert is needed.

**Success criteria**

- `git grep stop-curate` returns zero hits outside historical docs.
- `rememora usage` confirms curator is still firing for interactive sessions.
- A `claude -p "hello"` invocation still triggers a SessionEnd flush if the
  transcript crosses `MIN_TRANSCRIPT_CHARS` (or is a deliberate no-op if below).

---

## Test strategy (cross-stage)

### Unit
- `src/stream.rs` tests (see Stage 1) — partial-line parsing, token bucket,
  watermark advance, restart resume, EOF handling. All use in-memory SQLite
  + stubbed subagent.
- `src/stream.rs::tests::token_bucket` — dedicated bucket-math tests without
  streaming scaffolding.

### Integration
- `tests/stream_integration.rs` — new: subprocess `rememora curate --stream`,
  short-transcript path (no LLM call), assert watermark row + exit code.
- Revisit `tests/` for any Stop-hook-dependent tests (none currently; verify).

### Dogfood
- Stage 2: my daily work with `REMEMORA_USE_MONITOR=1`. Log collection
  instrumented via `--notify-log`.
- Stage 3: two days post-flip with monitor as default.
- Stage 4: one week post-Stop-hook-removal to confirm headless flow works.

### Manual smoke checklist per stage
1. Fresh `claude` interactive session → observe `"rememora curator online"`
   notification.
2. Do a curate-worthy sequence (decision + save-worthy code).
3. `sqlite3 ~/.rememora/rememora.db 'select byte_offset, updated_at from watermarks order by updated_at desc limit 1'` shows the watermark advancing.
4. Kill monitor (`pkill -f 'rememora curate --stream'`) — observe wrapper
   restart with a single new notification line.
5. End session → `rememora session end-active` completes; final flush runs
   (visible in curator log).

---

## Open risks (from issue + my scout) and resolution status

| # | Risk | Resolution |
|---|---|---|
| 1 | `"when":"always"` vs `"on-skill-invoke:rememora-save"` gating. Always-on spends Haiku tokens even for sessions that never save. | **Deferred (not blocking).** Stage 1 ships `"always"` because skill-invoke adds a latency hitch before the first save. We track Haiku cost in `rememora usage` during dogfood; if it balloons we switch to skill-invoke in stage 3. Document as a follow-up ticket if triggered. |
| 2 | Headless (`claude -p`, API, CI) has no monitor. | **Accepted & designed around.** Stage 3 keeps Stop hook gone; headless relies on SessionEnd final flush. Stage 4 reaffirms Option A. If insufficient, Option B (reintroduce a narrow headless-only Stop hook) is a clean escape hatch. |
| 3 | No auto-restart of monitor on crash. | **Mitigated.** `curate-monitor.sh` wraps the pipeline in `while true; do …; sleep 5; done` and emits a single "curator restarted" notification per crash. Flag-gated noisier logging via `REMEMORA_STREAM_DEBUG=1`. |
| 4 | Notification noise. | **Mitigated via token bucket.** 1 notification per 30 s, coalesced. Startup + crash lines are the only unconditional emissions. |
| 5 | `$HOME/.claude/projects/<encoded>/<session>.jsonl` is an undocumented path. | **Already accepted** (same risk in current `stop-curate.sh`). `curate-monitor.sh` emits a single notification if the file is missing, then exits; wrapper loop won't retry indefinitely in that case (add max-restart cap: 5 in 60 s, then sleep 60 s). |
| 6 | Downgrade path for users on Claude Code < v2.1.105. | **Resolved via plugin README + requirements bullet.** Accept the break; the plugin marketplace surfaces min-Claude-Code-version through docs. Alternative (keep Stop hook as fallback) is rejected — two code paths are worse. Plugin `requirements` in `plugin.json` does not yet support a Claude-Code version key; if it does by stage 3, use it. |
| 7 | **NEW from scout:** child `claude -p` sessions (signal-gate, curator) still write JSONL under `~/.claude/projects/` and could trip *their own* monitors. | **Resolved.** Curator children already run with `REMEMORA_CURATE_CHILD=1` (see `src/curator.rs::build_subagent_command`). `curate-monitor.sh` short-circuits on that env var at the top of the script. |
| 8 | **NEW from scout:** monitor env vars for `CLAUDE_SESSION_ID` / `CLAUDE_CWD` may differ from hook env. | **Verify in stage 2 dogfood.** Fall-back strategy: pick the newest `*.jsonl` in `~/.claude/projects/<encoded-pwd>/` by mtime. Worst case: brief double-coverage at session start. |

---

## Success criteria (issue-level rollup)

- Replaces Stop hook with a single `monitors` entry; no per-session lockfiles. **(stage 3)**
- ≤1 curate process per session (structural, not via `pgrep` gate). **(stage 1 — by construction of `tail | rememora`)**
- Monitor handles session rotate + Claude Code restart gracefully. **(stage 1 — watermark persistence + wrapper restart loop)**
- Notification volume ≤1 line / 30 s per session. **(stage 1 — token bucket; verified stage 2)**
- Stop hook retained as a narrow final-flush safety net, or explicitly removed if SessionEnd covers it. **(stage 4 — removed; SessionEnd covers headless)**
- Full plan + risks in issue comment. **(this document; PR description will summarize)**
- Delta curation: detector input size scales with new bytes, not session length. **(stage 1 — by construction)**
- Crash recovery observable. **(stage 1 — wrapper loop + notification line)**

## Effort estimate

- Stage 1 (streaming producer + opt-in plugin wiring + tests): ~2 days.
- Stage 2 (dogfood): ~1 week elapsed.
- Stage 3 (flip default, polish README): ~half day.
- Stage 4 (remove Stop-hook, final polish): ~half day.

Total coding time: ~3 days; total elapsed: ~2 weeks including dogfood.
