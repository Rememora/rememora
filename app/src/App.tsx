import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ContextRow, DbStatus, ListContextsResponse } from "./types";

const PAGE_SIZE = 200;

function truncate(text: string, max: number): string {
  if (text.length <= max) return text;
  return text.slice(0, max - 1) + "…";
}

function formatCreatedAt(iso: string): string {
  // Keep the date local; fall back to the raw string if parsing fails.
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

function StatusMessage({ status }: { status: DbStatus }) {
  let title: string;
  let body: string;
  switch (status.tag) {
    case "DbMissing":
      title = "Database not found";
      body =
        "Rememora has no database at ~/.rememora/rememora.db yet. Run `rememora init` in a terminal to create one.";
      break;
    case "DbUnencrypted":
      title = "Database is unencrypted";
      body =
        "The Rememora database exists but is not encrypted. Run `rememora encrypt` in a terminal to migrate it, then reopen the app.";
      break;
    case "KeychainMissing":
      title = "Encryption key not available";
      body =
        "The database is encrypted but no key was found in REMEMORA_KEY or the OS keychain. Run `rememora init` in a terminal first so the desktop app can decrypt it.";
      break;
    case "Other":
      title = "Could not open database";
      body = status.message;
      break;
    default:
      title = "Unknown error";
      body = "An unexpected error occurred.";
  }
  return (
    <div className="status">
      <h2>{title}</h2>
      <p>{body}</p>
    </div>
  );
}

export default function App() {
  const [status, setStatus] = useState<DbStatus | null>(null);
  const [rows, setRows] = useState<ContextRow[]>([]);
  const [total, setTotal] = useState<number>(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      try {
        const s = await invoke<DbStatus>("get_db_status");
        setStatus(s);
        if (s.tag === "Ok") {
          await loadMore(0, []);
        }
      } catch (e) {
        setError(String(e));
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function loadMore(offset: number, existing: ContextRow[]) {
    setLoading(true);
    try {
      const res = await invoke<ListContextsResponse>("list_contexts", {
        offset,
        limit: PAGE_SIZE,
      });
      setRows([...existing, ...res.rows]);
      setTotal(res.total);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  if (error) {
    return (
      <div className="status">
        <h2>Error</h2>
        <p>{error}</p>
      </div>
    );
  }

  if (status === null) {
    return (
      <div className="status">
        <p>Opening database…</p>
      </div>
    );
  }

  if (status.tag !== "Ok") {
    return <StatusMessage status={status} />;
  }

  const hasMore = rows.length < total;

  return (
    <div className="app">
      <header>
        <h1>Rememora</h1>
        <span className="count">
          {rows.length.toLocaleString()} / {total.toLocaleString()} contexts
        </span>
      </header>
      <table>
        <thead>
          <tr>
            <th className="col-category">Category</th>
            <th className="col-uri">URI</th>
            <th className="col-name">Name</th>
            <th className="col-abstract">Abstract</th>
            <th className="col-created">Created</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.id}>
              <td className="col-category">{r.category ?? "—"}</td>
              <td className="col-uri" title={r.uri}>
                {r.uri}
              </td>
              <td className="col-name">{r.name}</td>
              <td className="col-abstract" title={r.abstract_text}>
                {truncate(r.abstract_text, 140)}
              </td>
              <td className="col-created">{formatCreatedAt(r.created_at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <footer>
        {hasMore ? (
          <button
            onClick={() => loadMore(rows.length, rows)}
            disabled={loading}
          >
            {loading ? "Loading…" : "Load more"}
          </button>
        ) : rows.length > 0 ? (
          <span className="end">End of list.</span>
        ) : (
          <span className="end">No contexts yet.</span>
        )}
      </footer>
    </div>
  );
}
