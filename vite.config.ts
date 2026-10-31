import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

import packageJson from "./package.json" with { type: "json" };

// Tauri serves the UI from a fixed port in development and from the bundled
// dist directory in production.
export default defineConfig({
  plugins: [react()],
  // The About block reports the same version the bundle was built from.
  define: {
    __APP_VERSION__: JSON.stringify(packageJson.version)
  },
  clearScreen: false,
  server: {
    port: 5183,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/crates/**"] }
  },
  build: {
    target: "chrome110",
    sourcemap: false,
    outDir: "dist"
  }
});
