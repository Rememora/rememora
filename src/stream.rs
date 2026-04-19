//! Streaming curator — long-lived producer for Claude Code monitor entries.
//!
//! Reads a session JSONL line-stream from `stdin` (typically
//! `tail -F -n 0 session.jsonl`), accumulates transcript deltas, and calls
//! the signal gate + curator on fresh bytes only. Stdout writes are shaped
//! by a token bucket so Claude Code never sees more than ~1 notification
//! per `notify_secs`.
//!
//! The core [`StreamState`] is pure — it's driven by `push_line` / `flush`
//! and has no threads, timers, or stdin of its own. [`run`] wraps the state
//! machine with a background reader thread so production code can feed
//! it directly from a pipe.
//!
//! Signal gate + curator are invoked via the [`crate::curator::Subagent`]
//! trait, which lets tests substitute a fake that doesn't spend tokens.

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::Connection;

use crate::curator::{Signal, Subagent, SubagentTelemetry};
use crate::jsonl;
use crate::models::agent_invocation::{self, Caller};
use crate::models::watermark;

pub const DEFAULT_FLUSH_MS: u64 = 10_000;
pub const DEFAULT_NOTIFY_SECS: u64 = 30;
/// Flush when the per-cycle buffer crosses this many fresh bytes, even if
/// the idle timer hasn't fired. Keeps signal-gate latency bounded on
/// chatty sessions.
pub const DEFAULT_FLUSH_BYTES: usize = 4_096;

/// Knobs for the streaming curator. Defaults match the plan's 10 s idle
/// flush + 30 s notify interval.
#[derive(Debug, Clone)]
pub struct StreamOpts {
    pub session_id: Option<String>,
    pub project: Option<String>,
    /// If set, [`StreamState`] persists its cumulative byte offset into the
    /// `watermarks` table under this key on every flush. Leave `None` in
    /// tests that want to skip DB persistence.
    pub watermark_path: Option<String>,
    pub flush_ms: u64,
    pub notify_secs: u64,
    pub flush_bytes: usize,
    pub dry_run: bool,
}

impl Default for StreamOpts {
    fn default() -> Self {
        Self {
            session_id: None,
            project: None,
            watermark_path: None,
            flush_ms: DEFAULT_FLUSH_MS,
            notify_secs: DEFAULT_NOTIFY_SECS,
            flush_bytes: DEFAULT_FLUSH_BYTES,
            dry_run: false,
        }
    }
}

/// What a single [`StreamState::flush`] did. Useful for tests and for the
/// top-level driver to decide whether to emit a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushOutcome {
    /// Buffer was empty — nothing to do.
    Nothing,
    /// Signal gate returned No (or short-circuited on length).
    NoSignal,
    /// Curator ran successfully.
    Curated,
}

/// Streaming curator state machine. Pure — no I/O of its own; callers
/// feed it lines via [`push_line`] and time via [`flush`].
pub struct StreamState<'a> {
    opts: StreamOpts,
    subagent: &'a dyn Subagent,
    conn: Option<&'a Connection>,
    buffer: String,
    bytes_since_flush: usize,
    /// Cumulative byte count consumed by the stream (initial watermark
    /// offset + every line accepted via [`push_line`]). Persisted to the
    /// `watermarks` table after each flush when `opts.watermark_path` is set.
    total_bytes: u64,
    last_flush_at: Instant,
    bucket: TokenBucket,
}

impl<'a> StreamState<'a> {
    pub fn new(
        opts: StreamOpts,
        subagent: &'a dyn Subagent,
        conn: Option<&'a Connection>,
        now: Instant,
    ) -> Self {
        let bucket = TokenBucket::new(Duration::from_secs(opts.notify_secs));
        let initial_offset = match (opts.watermark_path.as_deref(), conn) {
            (Some(path), Some(c)) => watermark::get(c, path)
                .ok()
                .flatten()
                .map(|w| w.byte_offset)
                .unwrap_or(0),
            _ => 0,
        };
        Self {
            opts,
            subagent,
            conn,
            buffer: String::new(),
            bytes_since_flush: 0,
            total_bytes: initial_offset,
            last_flush_at: now,
            bucket,
        }
    }

    /// Cumulative byte count processed so far. Exposed for tests.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Push one complete JSONL line (no trailing newline). Always appends
    /// a `\n` to the internal buffer so downstream parse_reader sees a
    /// well-formed JSONL slab.
    pub fn push_line(&mut self, line: &str) {
        self.buffer.push_str(line);
        self.buffer.push('\n');
        let bytes = line.len() + 1;
        self.bytes_since_flush += bytes;
        self.total_bytes += bytes as u64;
    }

    /// Enough new bytes have accumulated that we should flush now.
    pub fn should_flush_by_bytes(&self) -> bool {
        self.bytes_since_flush >= self.opts.flush_bytes
    }

    /// Idle timer has elapsed since the last flush and the buffer has
    /// something in it.
    pub fn should_flush_by_time(&self, now: Instant) -> bool {
        !self.buffer.is_empty()
            && now.duration_since(self.last_flush_at) >= Duration::from_millis(self.opts.flush_ms)
    }

    /// Drive one flush cycle: parse buffer → signal gate → maybe curate →
    /// emit → persist watermark → clear buffer.
    pub fn flush<W: Write>(&mut self, out: &mut W, now: Instant) -> Result<FlushOutcome> {
        if self.buffer.is_empty() {
            return Ok(FlushOutcome::Nothing);
        }

        let cursor = std::io::Cursor::new(self.buffer.as_bytes());
        let parse = jsonl::parse_reader(cursor)?;
        let transcript = jsonl::render_transcript(&parse.entries);

        let gate = self.subagent.signal_gate(&transcript)?;
        if let Some(t) = &gate.telemetry {
            self.record(Caller::SignalGate, self.opts.project.clone(), t);
        }

        let outcome = if gate.signal == Signal::Yes {
            let project = self.opts.project.as_deref().unwrap_or("unknown");
            let result = self.subagent.curate(&transcript, project, self.opts.dry_run)?;
            if let Some(t) = &result.telemetry {
                self.record(Caller::Curator, Some(project.to_string()), t);
            }
            let msg = format!(
                "rememora curated {} transcript entries ({})",
                parse.entries.len(),
                project
            );
            self.bucket.try_emit(out, &msg, now)?;
            FlushOutcome::Curated
        } else {
            FlushOutcome::NoSignal
        };

        self.persist_watermark(parse.lines_processed as u64)?;
        self.buffer.clear();
        self.bytes_since_flush = 0;
        self.last_flush_at = now;

        Ok(outcome)
    }

    /// Flush any pending bucket summary. Called at shutdown.
    pub fn finalize<W: Write>(&mut self, out: &mut W) -> Result<()> {
        self.bucket.drain(out)
    }

    fn record(&self, caller: Caller, project: Option<String>, t: &SubagentTelemetry) {
        if let Some(conn) = self.conn {
            agent_invocation::try_insert(
                conn,
                &agent_invocation::record_from_subagent(
                    caller,
                    project,
                    self.opts.session_id.clone(),
                    t,
                ),
            );
        }
    }

    fn persist_watermark(&self, lines: u64) -> Result<()> {
        if let (Some(path), Some(conn)) = (self.opts.watermark_path.as_deref(), self.conn) {
            watermark::set(conn, path, self.total_bytes, lines)?;
        }
        Ok(())
    }
}

/// Rate limiter for stdout notifications. Capacity-1 bucket with a refill
/// interval — when the token is available, we emit; otherwise we count
/// suppressions and emit them in a coalesced summary on the next token.
pub struct TokenBucket {
    interval: Duration,
    last_emit: Option<Instant>,
    suppressed: u32,
    last_suppressed_msg: Option<String>,
}

impl TokenBucket {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_emit: None,
            suppressed: 0,
            last_suppressed_msg: None,
        }
    }

    /// Try to emit `msg` at `now`. Returns `Ok(true)` if written, `Ok(false)`
    /// if rate-limited into the suppressed bucket.
    pub fn try_emit<W: Write>(&mut self, out: &mut W, msg: &str, now: Instant) -> Result<bool> {
        let ready = self
            .last_emit
            .map(|t| now.duration_since(t) >= self.interval)
            .unwrap_or(true);
        if ready {
            let line = if self.suppressed > 0 {
                let text = format!("{msg} ({} notifications coalesced)", self.suppressed);
                self.suppressed = 0;
                self.last_suppressed_msg = None;
                text
            } else {
                msg.to_string()
            };
            writeln!(out, "{line}")?;
            out.flush()?;
            self.last_emit = Some(now);
            Ok(true)
        } else {
            self.suppressed += 1;
            self.last_suppressed_msg = Some(msg.to_string());
            Ok(false)
        }
    }

    /// Emit any pending suppressed-summary line. Called at shutdown so the
    /// last observation isn't lost.
    pub fn drain<W: Write>(&mut self, out: &mut W) -> Result<()> {
        if self.suppressed == 0 {
            return Ok(());
        }
        let msg = self.last_suppressed_msg.take().unwrap_or_default();
        writeln!(
            out,
            "{msg} ({} notifications coalesced, stream ending)",
            self.suppressed
        )?;
        out.flush()?;
        self.suppressed = 0;
        Ok(())
    }
}

/// Top-level driver: spawns a reader thread, ticks the idle timer, and
/// drives [`StreamState`] until `reader` reaches EOF. Intended for the
/// production `rememora curate --stream` entry point.
pub fn run<R, W>(
    opts: StreamOpts,
    reader: R,
    mut out: W,
    subagent: &dyn Subagent,
    conn: Option<&Connection>,
) -> Result<()>
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let session_note = opts
        .session_id
        .as_deref()
        .map(|s| format!(" (session {s})"))
        .unwrap_or_default();
    writeln!(out, "rememora curator online{session_note}")?;
    out.flush()?;

    let flush_ms = opts.flush_ms;
    let mut state = StreamState::new(opts, subagent, conn, Instant::now());

    let (tx, rx) = mpsc::channel::<Option<String>>();
    let _reader_handle = thread::spawn(move || {
        let mut reader = reader;
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(None);
                    break;
                }
                Ok(_) => {
                    let line = buf.trim_end_matches(['\n', '\r']).to_string();
                    if tx.send(Some(line)).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = tx.send(None);
                    break;
                }
            }
        }
    });

    loop {
        match rx.recv_timeout(Duration::from_millis(flush_ms)) {
            Ok(Some(line)) => {
                state.push_line(&line);
                if state.should_flush_by_bytes() {
                    state.flush(&mut out, Instant::now())?;
                }
            }
            Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                state.flush(&mut out, Instant::now())?;
                state.finalize(&mut out)?;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                if state.should_flush_by_time(now) {
                    state.flush(&mut out, now)?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curator::{CurationResult, SignalGateResult, SubagentTelemetry};
    use crate::db;
    use std::cell::Cell;
    use std::io::Cursor;

    /// Fake subagent with pluggable responses. Never calls `claude -p`.
    struct FakeSubagent {
        gate_signal: Signal,
        gate_calls: Cell<u32>,
        curate_calls: Cell<u32>,
    }

    impl FakeSubagent {
        fn new(signal: Signal) -> Self {
            Self {
                gate_signal: signal,
                gate_calls: Cell::new(0),
                curate_calls: Cell::new(0),
            }
        }
    }

    // Single-threaded Cell is fine because tests don't share across threads.
    unsafe impl Send for FakeSubagent {}
    unsafe impl Sync for FakeSubagent {}

    impl Subagent for FakeSubagent {
        fn signal_gate(&self, _transcript: &str) -> Result<SignalGateResult> {
            self.gate_calls.set(self.gate_calls.get() + 1);
            Ok(SignalGateResult {
                signal: self.gate_signal,
                telemetry: None,
            })
        }
        fn curate(
            &self,
            _transcript: &str,
            _project: &str,
            dry_run: bool,
        ) -> Result<CurationResult> {
            self.curate_calls.set(self.curate_calls.get() + 1);
            Ok(CurationResult {
                signal: Signal::Yes,
                curator_output: Some("stub".into()),
                dry_run,
                telemetry: Some(SubagentTelemetry::default()),
            })
        }
    }

    fn user_line(text: &str) -> String {
        serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": text},
            "timestamp": "2026-04-19T00:00:00Z",
            "uuid": "test"
        })
        .to_string()
    }

    /// Build a transcript line long enough to pass the 500-char signal-gate
    /// floor — needed when the fake says Yes but the free-function
    /// `signal_gate` short-circuit would otherwise fire. Our fake bypasses
    /// that floor (it's injected via the trait), so this is mostly a sanity
    /// shape.
    fn long_user_line(seed: usize) -> String {
        user_line(&format!("{seed}: {}", "x".repeat(600)))
    }

    #[test]
    fn pushes_lines_and_advances_total_bytes() {
        let fake = FakeSubagent::new(Signal::No);
        let mut state = StreamState::new(StreamOpts::default(), &fake, None, Instant::now());

        assert_eq!(state.total_bytes(), 0);
        state.push_line("hello");
        // "hello\n" is 6 bytes
        assert_eq!(state.total_bytes(), 6);
        state.push_line("world!");
        assert_eq!(state.total_bytes(), 13);
    }

    #[test]
    fn flush_with_no_signal_emits_nothing_but_clears_buffer() {
        let fake = FakeSubagent::new(Signal::No);
        let mut state = StreamState::new(StreamOpts::default(), &fake, None, Instant::now());
        state.push_line(&user_line("hi"));

        let mut out = Vec::new();
        let outcome = state.flush(&mut out, Instant::now()).unwrap();
        assert_eq!(outcome, FlushOutcome::NoSignal);
        assert!(out.is_empty(), "no-signal should not emit");
        assert_eq!(fake.gate_calls.get(), 1);
        assert_eq!(fake.curate_calls.get(), 0);
    }

    #[test]
    fn flush_with_signal_yes_runs_curator_and_emits() {
        let fake = FakeSubagent::new(Signal::Yes);
        let opts = StreamOpts {
            project: Some("rememora".into()),
            ..StreamOpts::default()
        };
        let mut state = StreamState::new(opts, &fake, None, Instant::now());
        state.push_line(&long_user_line(1));

        let mut out = Vec::new();
        let outcome = state.flush(&mut out, Instant::now()).unwrap();
        assert_eq!(outcome, FlushOutcome::Curated);
        let emitted = String::from_utf8(out).unwrap();
        assert!(
            emitted.contains("rememora curated"),
            "expected curator notification, got {emitted:?}"
        );
        assert_eq!(fake.gate_calls.get(), 1);
        assert_eq!(fake.curate_calls.get(), 1);
    }

    #[test]
    fn flush_on_empty_buffer_is_noop() {
        let fake = FakeSubagent::new(Signal::Yes);
        let mut state = StreamState::new(StreamOpts::default(), &fake, None, Instant::now());
        let mut out = Vec::new();
        let outcome = state.flush(&mut out, Instant::now()).unwrap();
        assert_eq!(outcome, FlushOutcome::Nothing);
        assert_eq!(fake.gate_calls.get(), 0);
    }

    #[test]
    fn token_bucket_emits_once_within_interval_and_coalesces_on_next_token() {
        let mut bucket = TokenBucket::new(Duration::from_secs(30));
        let mut out = Vec::new();
        let t0 = Instant::now();

        // First emit goes through.
        assert!(bucket.try_emit(&mut out, "a", t0).unwrap());
        // Within interval — suppressed.
        assert!(!bucket.try_emit(&mut out, "b", t0 + Duration::from_secs(5)).unwrap());
        assert!(!bucket.try_emit(&mut out, "c", t0 + Duration::from_secs(15)).unwrap());
        // After interval — new emit carries coalesced count.
        assert!(bucket
            .try_emit(&mut out, "d", t0 + Duration::from_secs(31))
            .unwrap());

        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "a");
        assert!(
            lines[1].starts_with("d") && lines[1].contains("2 notifications coalesced"),
            "expected coalesced line, got {:?}",
            lines[1]
        );
    }

    #[test]
    fn token_bucket_drain_flushes_pending_summary() {
        let mut bucket = TokenBucket::new(Duration::from_secs(30));
        let mut out = Vec::new();
        let t0 = Instant::now();
        assert!(bucket.try_emit(&mut out, "first", t0).unwrap());
        assert!(!bucket
            .try_emit(&mut out, "second", t0 + Duration::from_secs(5))
            .unwrap());

        bucket.drain(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("second"));
        assert!(text.contains("1 notifications coalesced, stream ending"));
    }

    #[test]
    fn watermark_advances_and_resumes_across_restart() {
        let conn = db::open_memory().unwrap();
        let fake = FakeSubagent::new(Signal::No);
        let path = "/tmp/test-session.jsonl";
        let opts = StreamOpts {
            watermark_path: Some(path.into()),
            ..StreamOpts::default()
        };

        {
            let mut state = StreamState::new(opts.clone(), &fake, Some(&conn), Instant::now());
            state.push_line(&user_line("one"));
            state.push_line(&user_line("two"));
            state.flush(&mut Vec::new(), Instant::now()).unwrap();
        }

        let mid = watermark::get(&conn, path).unwrap().unwrap();
        assert!(mid.byte_offset > 0);
        assert!(mid.line_count >= 2);

        // A new StreamState picks up the watermark as its starting offset.
        let fake2 = FakeSubagent::new(Signal::No);
        let state2 = StreamState::new(opts, &fake2, Some(&conn), Instant::now());
        assert_eq!(state2.total_bytes(), mid.byte_offset);
    }

    #[test]
    fn run_drains_reader_and_exits_clean_on_eof() {
        let fake = FakeSubagent::new(Signal::No);
        // Force fast flush-by-time so the reader loop doesn't block long.
        let opts = StreamOpts {
            flush_ms: 50,
            ..StreamOpts::default()
        };
        let input = format!("{}\n{}\n", user_line("a"), user_line("b"));
        let cursor = Cursor::new(input.into_bytes());
        let mut out = Vec::new();
        run(opts, cursor, &mut out, &fake, None).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("rememora curator online"),
            "expected startup banner, got {text:?}"
        );
    }
}
