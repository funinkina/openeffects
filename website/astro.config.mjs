import { execSync } from 'node:child_process';
import { defineConfig } from 'astro/config';
import tailwindcss from '@tailwindcss/vite';

// Latest release tag, read from git at build time (falls back if git is absent).
let version = 'v0.1.4';
try {
  version = execSync('git describe --tags --abbrev=0', { encoding: 'utf8' }).trim();
} catch {
  /* git unavailable (e.g. built from a tarball); keep the fallback */
}

export default defineConfig({
  site: 'https://openeffects.funinkina.co.in',
  vite: {
    plugins: [tailwindcss()],
    define: {
      __APP_VERSION__: JSON.stringify(version),
    },
  },
});
