import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port and to serve from `src/`.
// @ts-expect-error — Node env is available during Vite runtime.
const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: {
      // Tauri's Rust sources mutate `src-tauri/` during dev; ignore them so
      // Vite does not trigger a full-reload on every `cargo` recompile.
      ignored: ["**/src-tauri/**"],
    },
  },
}));
