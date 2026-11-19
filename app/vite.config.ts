/// <reference types="vitest/config" />
import { fileURLToPath } from "node:url";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

const here = (p: string) => fileURLToPath(new URL(p, import.meta.url));

// https://vite.dev/config/
// The frontend source lives in src/ui; the build goes to app/build/ui for Tauri to bundle.
export default defineConfig({
    root: here("src/ui"),
    plugins: [svelte({ configFile: here("svelte.config.js") })],
    clearScreen: false,
    build: {
        outDir: here("build/ui"),
        emptyOutDir: true,
    },
    server: {
        port: 5173,
        strictPort: true,
        watch: { ignored: ["**/src/desktop/**"] },
    },
    test: {
        include: ["**/*.test.ts"],
        passWithNoTests: true,
    },
});
