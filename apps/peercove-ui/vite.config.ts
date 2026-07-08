import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri は固定ポートを期待する(見つからなければ失敗させる)
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    // Tauri は最近の WebView を使うのでダウンレベル変換は不要
    target: "esnext",
    sourcemap: true,
  },
});
