import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  // Relative asset URLs so Tauri's `tauri://localhost/...` protocol resolves
  // <script src="./assets/..."> instead of <script src="/assets/...">. The
  // root-relative form works in some platforms' webviews and not others; the
  // relative form is foolproof.
  base: "./",
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
  },
  build: {
    target: "es2022",
    sourcemap: true,
  },
});
