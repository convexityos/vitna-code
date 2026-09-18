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

Four variable TrueType faces. Three are the faces `tokens.css` names:
`vitna-web` self-hosts them (`public/fonts/*.woff2`, latin and latin-ext
subsets), which is what confirms they are the design's faces. The fourth,
Plus Jakarta Sans, is NOT one of vitna-web's faces: the owner chose it on
2026-09-17 as this window's UI voice, after comparing it against the bundled
faces and three other OFL candidates, when asked for something in the register
of Google Sans (which is proprietary and cannot be bundled). egui cannot read
woff2, so all four TrueType masters come from the canonical OFL distributions
in Google's `fonts` repository (`github.com/google/fonts`, `ofl/`), fetched
2026-09-17. Each ships with its `OFL-*.txt`; the SIL Open Font License permits
bundling and redistribution with software, provided the fonts are not sold on
their own and the licence travels with them.

| file | sha256 |
|---|---|
| `Manrope-Variable.ttf` | `3ae11c49db0455a3cc33e37d380f20fdb8c7f8b41dc07625c177e3d87a9d6ae6` |
| `SpaceGrotesk-Variable.ttf` | `acad6de1fc93436f5c0f1f4137751ef04f1aea3063e7036535970ffcfbd79f72` |
| `JetBrainsMono-Variable.ttf` | `48715a42ec242c21e9f02692891e147d022299a52e48d5e413e1a942193ffeda` |
| `PlusJakartaSans-Variable.ttf` | `89b3fb38aa0d275d7a731d0d817a4f1622b316b4d7fbdedcf02ee9099ff68bc8` |

Roles, as `theme.rs` registers them: Plus Jakarta Sans is what a view says,
so it is the proportional default for labels, controls, chips and meta lines,
at weight 440; Space Grotesk keeps the display cut for the headline and the
wordmark's "Code", at 540, where its character is the point; Manrope is for
sentences, at 370; JetBrains Mono for paths, shas and counts, at 440. Weights
are set on each face's `wght` axis when it is registered, so one number moves
every label. This paragraph previously gave weights of 500 and 600 that the
window had already moved off, which is why it now names `theme.rs` as its
source instead of restating the tokens. Each family keeps egui's default faces
behind it so a glyph these lack still draws.

## logos/

Provider marks for the model menu, used to identify the provider the way every
coding terminal's picker does. The marks are their owners' trademarks; the
icon FILES come from Simple Icons (CC0 1.0, `LICENSE-simple-icons.md`),
fetched 2026-09-17, with one edit: the path carries `fill="#ffffff"` so the
window can tint it with the ink token, since a tint can only darken a black
source.

- `anthropic.svg`: Simple Icons `anthropic`.
- OpenAI is not here. Simple Icons no longer carries the mark (only "OpenAI
  Gym" remains), which means it was removed, and a mark its owner asked a
  free set to drop is not something to re-fetch from a mirror. A provider
  with no file under `logos/` draws a monogram badge instead. An official
  OpenAI SVG placed here as `openai.svg`, under whatever brand permission you
  hold, is the whole change.
