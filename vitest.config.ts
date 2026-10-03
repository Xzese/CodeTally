import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    globals: true,
    // UI journeys include several real interactions and rerenders. Allow slower
    // CI runners to finish them; individual findBy/waitFor deadlines stay short.
    testTimeout: 15_000,
    css: true
  }
})
