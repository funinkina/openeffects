// @ts-check
import { defineConfig } from 'astro/config';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  site: 'https://openeffects.funinkina.co.in',
  vite: {
    plugins: [tailwindcss()],
  },
});
