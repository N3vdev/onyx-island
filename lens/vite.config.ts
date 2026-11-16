import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // HTML entry points: the overlay (default), the lazily-opened settings
  // window, one pin-to-screen window per pinned selection (all sharing this
  // one page, differentiated at runtime by their own window label — see
  // pin.ts), and the share-sheet anchor window (share.rs — an ordinary,
  // undecorated-free window the native Windows share sheet can actually
  // attach to, since the overlay's own transparent/topmost/skip-taskbar
  // combination isn't a window shape Shell surfaces will anchor to). Vite
  // defaults to single-entry index.html otherwise.
  build: {
    rollupOptions: {
      input: {
        overlay: fileURLToPath(new URL("./index.html", import.meta.url)),
        settings: fileURLToPath(new URL("./settings.html", import.meta.url)),
        pin: fileURLToPath(new URL("./pin.html", import.meta.url)),
        share: fileURLToPath(new URL("./share.html", import.meta.url)),
      },
    },
  },
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
