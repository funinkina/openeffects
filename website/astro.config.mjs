// @ts-check
import { defineConfig } from 'astro/config';
import tailwindcss from '@tailwindcss/vite';

// Update `site` to your real domain before deploying (used for canonical + OG URLs).
export default defineConfig({
  site: 'https://openeffects.funinkina.co.in',
  vite: {
    plugins: [tailwindcss()],
  },
});
