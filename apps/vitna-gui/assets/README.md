# assets

Brand assets, copied verbatim from `convexityos/vitna-web` `public/` at the
commit that merged its PR #306 ("the mark is traced, not constructed"). That
repository is the source of truth; nothing here is edited in place.

- `vitna-mark.svg` is `public/vitna-mark-v3.svg`: four blades turning around
  a centre square, TRACED blade by blade from the artwork. Its header explains
  why the blades are not copies of one another and asks that the drawing never
  be regularised or rebuilt from one blade rotated. Honour that. The window
  renders it through the SVG loader at whatever size a call site asks for,
  which is the whole reason it ships as a vector: the same drawing measures
  0.98 ink-to-grey at 16px as a vector and 0.73 when downscaled from a raster.
- `vitna-wordmark-sm.png` is `public/vitna-wordmark-v3-sm.png`, the 376 x 104
  cut the site chrome renders at 20px tall. It is white ink on transparent
  (BRAND.md: alpha derived from luminance, ink set flat), so the window tints
  it with the ink token. The 2093 x 579 master is not shipped; the chrome does
  not ship it either.
- `icon.png` is `public/icon.png`, rasterised from that SVG upstream, used only
  for the OS window and taskbar icon, which the platform takes as a bitmap.

To update, copy the files again from vitna-web rather than editing these.

## fonts/

The three faces `tokens.css` names, as variable TrueType files. `vitna-web`
self-hosts the same faces (`public/fonts/*.woff2`, latin and latin-ext
subsets), which is what confirms these are the design's faces; egui cannot
read woff2, so the TrueType masters come from the canonical OFL distributions
in Google's `fonts` repository (`github.com/google/fonts`, `ofl/`). Fetched
2026-09-17. Each ships with its `OFL-*.txt`; the SIL Open Font License permits
bundling in an application.

| file | sha256 |
|---|---|
| `Manrope-Variable.ttf` | `3ae11c49db0455a3cc33e37d380f20fdb8c7f8b41dc07625c177e3d87a9d6ae6` |
| `SpaceGrotesk-Variable.ttf` | `acad6de1fc93436f5c0f1f4137751ef04f1aea3063e7036535970ffcfbd79f72` |
| `JetBrainsMono-Variable.ttf` | `48715a42ec242c21e9f02692891e147d022299a52e48d5e413e1a942193ffeda` |

Roles, per the tokens: Space Grotesk is what a view says, so it is the
proportional default for headings, labels, controls and chips, at weight 500,
with a 600 cut for the headline and the wordmark; Manrope is for sentences,
at 500; JetBrains Mono for paths, shas and counts, at 500. Weights are set on
each face's `wght` axis when it is registered (`theme.rs`), so one number
moves every label. Each family keeps egui's default faces behind it so a
glyph these lack still draws.
