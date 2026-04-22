// Mirrors the Rust types in src-tauri/src/commands.rs. Keep in sync.

export type DbStatus =
  | { tag: "Ok" }
  | { tag: "DbMissing" }
  | { tag: "DbUnencrypted" }
  | { tag: "KeychainMissing" }
  | { tag: "Other"; message: string };

export interface ContextRow {
  id: string;
  uri: string;
  name: string;
  abstract_text: string;
  category: string | null;
  created_at: string;
}

export interface ListContextsResponse {
  rows: ContextRow[];
  total: number;
}
