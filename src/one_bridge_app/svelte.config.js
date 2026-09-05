// adapter-static with an index.html fallback puts the site in SPA mode: the
// asset canister serves one page and the router runs in the browser
// See: https://svelte.dev/docs/kit/single-page-apps
import adapter from '@sveltejs/adapter-static'
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte'

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      fallback: 'index.html'
    }),
    alias: {
      $src: './src',
      $declarations: './src/declarations'
    }
  }
}

export default config
