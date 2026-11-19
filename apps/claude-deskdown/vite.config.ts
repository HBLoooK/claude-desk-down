import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  build: {
    // macOS uses system WebKit: min macOS 12 (Safari 15), same build target as Codex++.
    target: ["es2020", "safari14", "chrome87", "edge88", "firefox78"],
    cssTarget: ["safari14", "chrome87", "edge88", "firefox78"],
  },
  server: { host: "127.0.0.1", port: 1440, strictPort: true },
});
