import { sveltekit } from '@sveltejs/kit/vite'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'

export default defineConfig({
  define: {
    'process.env.NODE_ENV':
      process.env.NODE_ENV === 'production' ? '"production"' : '"development"'
  },
  build: {
    emptyOutDir: true
  },
  server: {
    proxy: {
      // dfx's local replica, for `pnpm dev` against locally deployed canisters
      '/api': {
        target: 'http://127.0.0.1:4943',
        changeOrigin: true
      }
    }
  },
  plugins: [tailwindcss(), sveltekit()]
})
