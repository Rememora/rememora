// Shared invoke wrapper — uses Tauri when available, mock data in browser

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { mockInvoke } from "./mock.js";

const isTauri = typeof window !== "undefined" && window.__TAURI_INTERNALS__;

export const invoke = isTauri ? tauriInvoke : mockInvoke;
