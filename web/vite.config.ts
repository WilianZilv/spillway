import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// The panel is embedded into the spillway binary at build time (see build.rs).
// In dev, API calls are proxied to a running master.
const master = process.env.SPILLWAY_MASTER ?? 'http://localhost:7070'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': path.resolve(__dirname, './src') } },
  server: {
    port: 5173,
    proxy: { '/api': master, '/healthz': master },
  },
  build: { outDir: 'dist', chunkSizeWarningLimit: 1500 },
})
