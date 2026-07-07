# OpenEffects — landing page

Marketing site for [OpenEffects](https://github.com/funinkina/openeffects). Static, single page.

**Stack:** [Astro](https://astro.build) 7 + [Tailwind CSS](https://tailwindcss.com) 4. Google Fonts (Space Grotesk / Inter / IBM Plex Mono). No client framework — the only browser JS is copy-to-clipboard and the install tabs.

## Commands

Run from `website/`:

| Command           | Action                                       |
| :---------------- | :------------------------------------------- |
| `npm install`     | Install dependencies                         |
| `npm run dev`     | Dev server at `localhost:4321`               |
| `npm run build`   | Build static site to `./dist/`               |
| `npm run preview` | Serve the built `./dist/` locally            |

## Structure

```
src/
├── layouts/Layout.astro        # <head>, fonts, SEO/OG, favicon
├── components/
│   ├── Viewfinder.astro        # hero signature (simulated webcam preview)
│   ├── FeatureCard.astro       # effect card + inline brand glyphs
│   └── CodeBlock.astro         # copy-to-clipboard code block
├── pages/index.astro           # the page — all content + section markup
└── styles/global.css           # Tailwind theme tokens + component classes
public/
├── openeffects.svg             # logo / favicon (copied from ../data/icons)
├── openeffects-512.png         # apple-touch-icon
└── og.png                      # 1200×630 social preview
```

All brand tokens (colors, fonts, the viewfinder animations) live in `src/styles/global.css` under `@theme`.

## Deploying

Output is fully static — host `dist/` anywhere (GitHub Pages, Netlify, Cloudflare Pages, any static host).

Before deploying, set your real domain in [`astro.config.mjs`](astro.config.mjs) (`site:`) so canonical + Open Graph URLs are correct.

- **Custom domain or user Pages** (`user.github.io`): paths are root-relative, works as-is.
- **Project Pages** (`user.github.io/openeffects`): also set `base: '/openeffects'` in `astro.config.mjs`.

## Regenerating the OG image / icons

From `website/public/`:

```sh
rsvg-convert -w 512 -h 512 openeffects.svg -o openeffects-512.png
```

The `og.png` banner is composited from the logo with ImageMagick — see the project notes if you need to rebuild it.
