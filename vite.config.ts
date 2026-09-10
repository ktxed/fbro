import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

import { collectBuildInfo } from "./scripts/build-info.mjs";

export default defineConfig(({ command }) => {
  const rootDir = fileURLToPath(new URL(".", import.meta.url));
  const profile = command === "serve"
    ? "development"
    : process.env.TAURI_DEBUG
      ? "debug"
      : "release";

  return {
    plugins: [react()],
    clearScreen: false,
    define: {
      __FBRO_BUILD_INFO__: JSON.stringify(collectBuildInfo({ rootDir, profile })),
    },
    server: {
      port: 5173,
      strictPort: true,
      watch: {
        ignored: ["**/src-tauri/**"],
      },
      fs: {
        strict: false,
      },
    },
    envPrefix: ["VITE_", "TAURI_"],
    build: {
      target: ["es2021", "chrome105", "safari13"],
      minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
      sourcemap: !!process.env.TAURI_DEBUG,
    },
  };
});
