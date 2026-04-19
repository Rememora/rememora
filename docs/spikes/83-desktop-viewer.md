# Spike: Desktop app as opt-in viewer (issue #83)

Status: design-only. No implementation in this branch.
Owner: TBD
Related: issues #76 (OTEL span export), #69 (agent invocation telemetry — already merged).

---

## 1. Goal + non-goals

### Goal

Ship an optional, user-launched desktop application that answers the question
"what does my agent remember about me?" by reading `~/.rememora/rememora.db`
directly. The app is an independent consumer of the same SQLite database the
CLI owns — nothing more. It is useful out of the box for: browsing memories,
inspecting a session timeline, and seeing cost/token usage from
`agent_invocations`.

### Non-goals

- **Not a memory editor in v0.** Read-only. Write features (delete, supersede,
  re-tag) are future work and raise concurrency questions with the CLI.
- **Not a replacement for `rememora search` / `rememora status`.** The CLI is
  still the primary surface for agents and for power users.
- **Not a cloud product.** Local-first, same as the CLI.

### Explicit anti-patterns (hard rules)

These are the claude-mem mistakes we will not repeat. Each is a BLOCKED design
decision, not a nice-to-have:

- **No CLI-spawned HTTP server.** `rememora` the CLI must never open a
  listening socket for the benefit of the viewer. If we need IPC later, we use
  the DB or a Unix domain socket — never a TCP port.
- **No autostart.** The app is launched by the user, from the Dock / Start
  Menu / Activities. No launchd, no systemd unit, no Windows service, no
  "Start at login" default.
- **No Claude Code / agent hook integration.** No SessionStart hook, no
  PreToolUse hook, nothing in `settings.json` ever references the desktop app.
  The plugin and the desktop app have no knowledge of each other.
- **No port binding from the app's own lifecycle** either. The app talks to
  SQLite on disk; it does not expose an HTTP endpoint for anything.
- **No background daemon when the app is closed.** Quitting the app = zero
  processes, zero sockets, zero file locks held.

These rules exist because violating any one of them re-creates the bug
category claude-mem has been drowning in (see §6).

---

## 2. Architecture

### 2.1 Data source

The app opens `~/.rememora/rememora.db` directly using a local SQLite binding.
The CLI already runs WAL mode (see `src/db.rs:66` —
`PRAGMA journal_mode = WAL`), which is exactly the mode SQLite recommends for
"one writer, many readers across processes". The app is a reader. Concurrent
reads during CLI writes are a solved problem in SQLite; no additional locking
or coordination is required.

Key pragmas the CLI already sets that the app must respect:

- `journal_mode = WAL` — enables multi-process concurrent readers.
- `busy_timeout = 5000` — the app should set the same (or higher) for
  resilience.
- `foreign_keys = ON` — readers don't care, but set it for parity.

The app should open the connection `readonly` by default:
`sqlite3_open_v2(path, SQLITE_OPEN_READONLY | SQLITE_OPEN_URI, ...)`. This
makes it impossible for the app to corrupt state even in the face of bugs.

### 2.2 Live updates — options considered

The app needs to feel live while open (new memories saved by an agent should
show up within a few seconds). Three candidates:

| Option | Mechanism | Pros | Cons |
|---|---|---|---|
| **A. File-mtime polling** | `stat()` `rememora.db-wal` every 1–2s; if mtime or size changed, re-run the visible queries | Dead simple, cross-platform, no SQLite hooks, no extra process | ~1s latency; wasted syscalls when idle; polling is unfashionable |
| **B. SQLite `update_hook`** | Install `sqlite3_update_hook` to get callbacks on INSERT/UPDATE/DELETE | Instant notification on same connection | **Only fires for writes on the same connection.** Useless cross-process. Blocked. |
| **C. WAL-frame tailing** | Open the `-wal` file, track the last-seen frame header, re-query when the header advances | True change-feed, no polling loop | Undocumented territory, WAL format is SQLite-private; fragile across SQLite versions |

**Recommendation: Option A — file-mtime polling.** Poll the `-wal` file every
1000ms while the app is focused, every 5000ms when backgrounded, and not at
all when the main window is hidden. If the `-wal` size/mtime has changed,
re-run the currently-visible queries. This is the approach Datasette,
DBeaver, and Litestream-adjacent tools all use. It is boring, correct, and
survives any future SQLite internal change.

Option B is fundamentally the wrong tool — the hook does not cross process
boundaries and we explicitly reject IPC from the CLI. Option C is a research
project, not a v0 feature.

### 2.3 Data model alignment

The app has two primary views that map cleanly to existing tables:

**List view ← `contexts`**
- Rows come from `list_by_scope` / a direct SELECT on `contexts WHERE
  superseded_by IS NULL`.
- Columns: `category` (chip), `name`, `abstract`, `importance`,
  `active_count`, `updated_at`, `source_agent`, `source_session`.
- FTS5 search is already set up (`contexts_fts` virtual table, see
  `src/migrations/001_initial.sql:69`). The app runs BM25 queries directly —
  no need to go through the CLI.
- Filter chips: `context_type`, `category`, `project` (via
  `uri LIKE 'rememora://projects/<p>/%'`).

**Timeline view ← `sessions` + `agent_invocations` + OTEL spans**
- Top row: sessions sorted by `started_at DESC`, with `agent`, `intent`,
  `summary`, `status` (active/ended/transferred).
- Per-session swimlane: all `agent_invocations` rows where
  `parent_session = sessions.id`, showing `caller`
  (signal_gate/curator/extract/evolve/consolidate/agent_run), `model`,
  `duration_ms`, `cost_usd`, `is_error`.
- When issue #76 lands (OTEL span export), the span stream joins in on
  `child_session` / span trace_id for a full distributed-trace timeline per
  session. The app can consume OTLP JSON files dropped in a well-known
  directory, or read a future spans table — #76 will decide.

**Cost/usage view ← `agent_invocations` aggregation**
- Re-implements `rememora usage` (see `src/models/agent_invocation.rs:223`
  `aggregate()`) as a chart: cost by caller, cost by model, cost by day.
- The SQL is already written in `agent_invocations::aggregate`. The desktop
  app reads the same table directly.

### 2.4 Read-only by default — the write-feature carveout

v0 is read-only. Writes would force us to answer:

- **Concurrency.** CLI writes happen from the curator subprocess, the plugin
  stop-hook, `rememora save`, and direct API tooling. A desktop-initiated
  delete/supersede races all of them. SQLite's `busy_timeout` handles
  transient contention, but the UX question (what does "delete" mean when the
  curator is about to write the same memory) is non-trivial.
- **Encryption.** The CLI supports SQLCipher (`src/db.rs:158`
  `apply_encryption_key`). If the DB is encrypted, the desktop app needs
  access to the key too — either via the same keychain/env var lookup, or via
  an explicit unlock prompt. This is solvable but adds UX surface.
- **Audit trail.** Rememora's design preference is supersession over deletion
  (see project memory). A "delete" button that actually creates a tombstone
  supersession row is the right shape.

**Recommendation:** ship v0 read-only. Revisit writes after v0 ships and we
have a real user telling us what they want to edit.

---

## 3. Platform choice

Evaluated: Tauri v2, Electron, native (SwiftUI + WinUI/native Linux GTK),
web-first (user-launched local webapp).

| Axis | Tauri v2 | Electron | Native per-OS | Web-first |
|---|---|---|---|---|
| Binary size (hello world) | ~3–10 MB | ~120–150 MB | ~5 MB/platform | ~0 (no binary) |
| RAM at idle | ~30–50 MB | ~150–300 MB | ~20–50 MB | browser tab |
| Cross-platform story | mac+win+linux (iOS/Android bonus) | mac+win+linux | 3× code | single codebase, no install |
| SQLite bindings | `rusqlite` — same as the CLI | `better-sqlite3` via Node | platform SQLite | WASM SQLite (OPFS/IDB) |
| Install / distribution | .app / .msi / .AppImage / .deb / Homebrew cask | same | per-OS pkg | served from static host |
| Language/team fit | Rust — matches Rememora's core | JS/TS — different skill set | 3 toolchains | JS/TS, one codebase |
| WAL file access | direct fs — trivial | direct fs — trivial | direct fs — trivial | **blocked** — no arbitrary disk access |
| Consistency risk | WebKitGTK vs WebView2 vs WKWebView rendering quirks | one Chromium — consistent | native widgets | browser-consistent but sandboxed |

**Web-first is disqualified** for the viewer role: a locally-hosted webapp
cannot read `~/.rememora/rememora.db` without either (a) a file picker the
user drags the DB into every session — terrible UX, or (b) a local HTTP
server that reads the file for it — which is precisely the claude-mem
anti-pattern we're ruling out.

**Native per-OS is disqualified by cost.** Three codebases for an optional
viewer is a bad trade.

**Electron vs. Tauri.** Both work. Electron is faster to prototype if you're
JS-native. Tauri wins on every runtime metric (size, memory, startup time),
reuses Rememora's existing Rust/rusqlite stack, and aligns with the project's
"single binary, fast startup" aesthetic. The WebKitGTK rendering-quirks risk
is real but manageable for a dense data-heavy UI with no exotic CSS.

**Recommendation: Tauri v2.** Ship the frontend in SolidJS or Svelte (small
bundle, good tables), the backend in Rust reusing Rememora's existing query
code as a library crate. Distribute signed DMG for macOS, MSI for Windows,
AppImage + .deb for Linux.

---

## 4. v0 feature scope

Cut to the bone. Anything outside this list is v0.1+.

1. **Memory list.** `contexts` table, default sort by `importance DESC,
   updated_at DESC`. Filter chips for `category` and `project`. FTS5 search
   box using the existing `contexts_fts` index.
2. **Memory detail view.** Click a row → side panel with `abstract`,
   `overview`, `content`, `tags`, `source_agent`, `source_session`,
   `importance`, supersession chain if any.
3. **Session timeline.** `sessions` list, newest first, with expandable rows
   showing `agent_invocations` children (caller, model, duration, cost).
4. **Cost chart.** One page re-implementing `rememora usage` as a chart —
   cost by caller and cost by day, last 30 days.

That's it. Four screens. No editing, no settings beyond DB path, no account,
no sync, no onboarding flow.

Explicitly **cut** from v0:

- ~~Write operations (delete/supersede/re-tag)~~
- ~~Graph view of `relations`~~
- ~~Per-memory edit history / diff view~~
- ~~Multi-DB support~~
- ~~OTEL span waterfall (blocked on #76 landing)~~ — stub the lane, fill it
  when #76 ships
- ~~Themes / preferences~~
- ~~Cross-device sync~~

---

## 5. Lifecycle + install

### 5.1 Distribution

**Recommendation: separate repo `Rememora/rememora-app`**, with the backend
Rust crate depending on `rememora` as a git or path dependency. Rationale:

- Main repo stays CLI-focused. Desktop code has very different release
  cadence, bug surface, and reviewer set.
- The desktop app can ship before the CLI cuts a new release, and vice
  versa.
- Homebrew cask (`brew install --cask rememora`) alongside the existing
  `brew install rememora` formula. Linux via `.deb` + AppImage. Windows via
  signed MSI on GitHub Releases.
- No marketplace entanglement. The plugin already ships via Claude Code's
  marketplace (see project memory "plugin vs CLI versioning"); the desktop
  app is a third, independent lineage. This is fine.

### 5.2 DB path discovery

Follow the CLI exactly (see `src/db.rs:182` `default_db_path`):

1. If `$REMEMORA_DB` is set, use it.
2. Otherwise, `~/.rememora/rememora.db`.

Expose this in a **settings pane** with a file picker, for users who run
rememora with a non-standard location. Store the override in the app's own
prefs (platform-standard location), not in the rememora DB.

### 5.3 Error handling

Three pathological states the app must handle without crashing:

- **DB missing.** Show an empty-state screen: "No Rememora database found at
  `<path>`. Run `rememora save ...` from your terminal to get started, or
  point the app at a different DB." Do not create the DB from the app — the
  CLI owns schema creation.
- **DB locked.** Retry with exponential backoff up to the `busy_timeout`.
  After that, show a non-blocking toast: "Database busy, retrying…" and keep
  showing stale data.
- **DB corrupt / SQLCipher-locked without key.** Show a clear error with a
  link to the CLI's recovery commands (`rememora encrypt`, etc.). Do not
  attempt repair from the app.
- **Schema drift.** The CLI is ahead of the app. The app reads via explicit
  column lists (never `SELECT *`) and tolerates unknown columns. If a table
  the app expects is missing, show a "please update the desktop app" notice.

---

## 6. Claude-mem comparison — concrete bugs avoided

Each claude-mem issue below exists because their viewer relies on a
CLI-spawned HTTP worker on port 37777. Our design kills the whole category.

- **[#1392](https://github.com/thedotmack/claude-mem/issues/1392)** — zombie
  sockets hold port 37777 after worker crash, blocking restart until reboot.
  *Avoided: we bind no ports.*
- **[#1531](https://github.com/thedotmack/claude-mem/issues/1531)** — worker
  daemon never shuts down; ghost TCP socket blocks all subsequent sessions.
  *Avoided: no daemon, no session-coupled lifecycle.*
- **[#1603](https://github.com/thedotmack/claude-mem/issues/1603)** — Windows
  kernel leaves port 37777 in LISTEN after abrupt worker exit.
  *Avoided: the only long-lived process is a GUI app the user closes
  themselves.*
- **[#1616](https://github.com/thedotmack/claude-mem/issues/1616)** — dozens
  of ghost CLOSE_WAIT / FIN_WAIT_2 connections survive process kill.
  *Avoided: no HTTP sockets anywhere.*
- **[#1747](https://github.com/thedotmack/claude-mem/issues/1747)** — Stop
  hook synchronously waits 3–7s for summarization, adding minutes of dead
  time per session. *Avoided: we do not integrate with Stop hooks at all —
  their existing Stop-hook curator is unchanged and unrelated to the
  viewer.*
- **[#1870](https://github.com/thedotmack/claude-mem/issues/1870)** — Stop
  hook blocks CLI ~110s when SDK pool saturates; workers have no max-age,
  feedback loops on re-enqueue. *Avoided: no worker pool, no queue, no
  lifecycle to saturate.*
- **[#1089](https://github.com/thedotmack/claude-mem/issues/1089)** — worker
  daemon spawns SDK subprocesses that never terminate; ~45 MB RSS leaks per
  orphan. *Avoided: the app spawns no background processes.*
- **[#484](https://github.com/thedotmack/claude-mem/issues/484)** — plugin
  version N+1 hooks talk to worker version N; 400 errors on API shape drift.
  *Avoided: the desktop app and CLI share the DB schema (a versioned
  migration chain), not an HTTP API contract. The app tolerates schema
  drift as described in §5.3.*

The structural lesson: **a long-lived process owned by the CLI is a bug
generator.** By making the lifecycle entirely user-driven (the app window is
open ↔ the process is running), we remove the category.

---

## 7. Open questions

Decisions needed from the user before implementation kicks off:

1. **Encryption story.** If the user has enabled SQLCipher (`rememora
   encrypt`), how does the app get the key? Options: (a) re-use the CLI's
   keychain lookup — requires linking `rememora::crypto`; (b) prompt on
   unlock and cache in-session only; (c) require an env var. My lean: (a) +
   (b) as fallback.
2. **Repo location.** New `Rememora/rememora-app` repo vs. folder in the
   existing monorepo (`app/`). New repo is cleaner; monorepo keeps CI and
   versioning in one place.
3. **Tauri vs. Electron.** Tauri is the recommendation but Electron's
   ecosystem maturity (esp. code signing + auto-update tooling) is
   genuinely better. Worth a short prototype week to validate Tauri's DX on
   all three target OSes before committing.
4. **v0 platform scope.** Ship all three OSes at once, or macOS-only first?
   My lean: macOS-only v0, because the maintainer's daily driver is macOS
   and feedback loop matters more than coverage. Windows + Linux in v0.1.
5. **OTEL span integration cadence.** Do we ship the cost chart in v0 and
   stub the span lane, or wait for #76 to land so the timeline is
   complete? Lean: ship v0 without spans; the four screens in §4 are
   already useful.
6. **Telemetry from the app itself.** Do we record "the user opened the
   list view" in `agent_invocations` or a new `app_events` table? Lean:
   no telemetry in v0. Local-first means local-only.

---

## 8. Recommended next step

Spend one week on a Tauri v2 prototype: a single window that opens
`~/.rememora/rememora.db` read-only, paginates the `contexts` table in a
virtualized list, and implements FTS5 search against `contexts_fts`. No
timeline, no cost chart, no settings. Ship it to the project owner's Mac
only. If Tauri's DX holds up — build + sign + run + reload loop under ~5
minutes end-to-end — we commit to Tauri for v0 and flesh out the remaining
three screens over ~2 more weeks for a macOS-only v0. If Tauri's DX is
painful enough to threaten the timeline, fall back to Electron without
further debate. Total effort through v0: 3 weeks, one engineer.
