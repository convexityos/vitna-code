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

Three variable TrueType faces, each the canonical OFL master from Google's
`fonts` repository (`github.com/google/fonts`, `ofl/`), fetched 2026-09-17,
each with its `OFL-*.txt` beside it. The SIL Open Font License permits bundling
and redistribution with software, provided the fonts are not sold on their own
and the licence travels with them.

| file | sha256 |
|---|---|
| `Inter-Variable.ttf` | `29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031` |
| `SpaceGrotesk-Variable.ttf` | `acad6de1fc93436f5c0f1f4137751ef04f1aea3063e7036535970ffcfbd79f72` |
| `JetBrainsMono-Variable.ttf` | `48715a42ec242c21e9f02692891e147d022299a52e48d5e413e1a942193ffeda` |

Roles, as `theme.rs` registers them. Inter is every word in the window except
the header: labels, controls, chips, meta lines, sentences and the composer, at
440 for the interface and 370 for sentences, both set on its `wght` axis (its
`opsz` axis is left at the default 14, which suits text at these sizes). Space
Grotesk keeps only the display cut, at 540, for the "Let's build" headline, the
lockup's "Code" and page titles. JetBrains Mono, at 440, is for paths, hashes,
counts and the receipt JSON, a role no proportional face can fill. Each family
keeps egui's default faces behind it so a glyph these lack still draws.

How it got here, so nobody re-litigates it from the git log: the owner asked on
2026-09-17 for a face in the register of Google Sans, which is proprietary and
cannot be bundled. Plus Jakarta Sans went in first as the UI face, a Lato trial
followed and was not kept, and Inter was chosen. The owner's rule is that a
font change is universal except for the header, so Inter replaced both the UI
face and the prose face; Manrope (one of vitna-web's own `tokens.css` faces,
previously the prose face) and Plus Jakarta Sans are therefore no longer
bundled, since the window should carry only what it draws.

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
