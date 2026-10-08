import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import { VitePWA } from "vite-plugin-pwa";

// The WebAssembly bindings are generated into build/wasm by scripts/build-wasm.mjs;
// the app imports them through the `@wasm` alias so nothing generated lives in src/.
const wasmDir = fileURLToPath(new URL("../build/wasm", import.meta.url));

export default defineConfig({
  resolve: {
    alias: { "@wasm": wasmDir },
  },
  server: {
    fs: { allow: [".", "../build/wasm"] },
  },
  build: {
    outDir: "../build/web",
    emptyOutDir: true,
    target: "es2022",
  },
  worker: {
    format: "es",
  },
  plugins: [
    VitePWA({
      registerType: "autoUpdate",
      includeAssets: ["icons/icon.svg", "audio-worklet.js"],
      manifest: {
        name: "Pipit",
        short_name: "Pipit",
        description: "A clean, free Game Boy Advance emulator",
        theme_color: "#14161c",
        background_color: "#14161c",
        display: "standalone",
        orientation: "any",
        start_url: "/",
        icons: [
          { src: "icons/icon-192.png", sizes: "192x192", type: "image/png" },
          { src: "icons/icon-512.png", sizes: "512x512", type: "image/png" },
          { src: "icons/icon-512.png", sizes: "512x512", type: "image/png", purpose: "maskable" },
        ],
      },
      workbox: {
        globPatterns: ["**/*.{js,css,html,wasm,png,svg,woff2}"],
        maximumFileSizeToCacheInBytes: 8 * 1024 * 1024,
      },
    }),
  ],
});
