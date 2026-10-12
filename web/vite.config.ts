import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import { readFileSync } from 'node:fs'

export default defineConfig({
  plugins: [react(), {
    name: 'bundled-font-license',
    apply: 'build',
    generateBundle() {
      this.emitFile({
        type: 'asset',
        fileName: 'licenses/IBM-Plex-Mono-OFL.txt',
        source: readFileSync(new URL('./src/fonts/OFL.txt', import.meta.url)),
      })
    },
  }],
  test: {
    environment: 'jsdom',
    setupFiles: './src/test-setup.ts',
    include: ['src/**/*.test.{ts,tsx}'],
  },
})
