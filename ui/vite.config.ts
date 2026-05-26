import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  plugins: [solid()],

  // Tauri attend un port fixe et échoue si Vite en choisit un autre.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: {
      // Ignore les fichiers générés par l'app (DB SQLCipher + WAL/SHM, salt,
      // backups, exports) — sinon Vite voit muter `*.sqlite-wal` à chaque
      // écriture et déclenche un full-reload qui fait croire à un redémarrage.
      ignored: [
        "**/src-tauri/**",
        "**/data/**",
        "**/*.sqlite",
        "**/*.sqlite-*",
        "**/*.salt",
        "**/backups/**",
        "**/exports/**",
      ],
    },
  },
}));
