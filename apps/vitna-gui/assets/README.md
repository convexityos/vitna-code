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

One face, Inter, as the variable TrueType master from Google's `fonts`
repository (`github.com/google/fonts`, `ofl/inter/`), fetched 2026-09-17, with
`OFL-Inter.txt` beside it. The SIL Open Font License permits bundling and
redistribution with software, provided the font is not sold on its own and the
licence travels with it.

| file | sha256 |
|---|---|
| `Inter-Variable.ttf` | `29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031` |

Inter is all of the text in the window, on the owner's instruction: labels,
controls, chips, meta lines, sentences, the composer, the headline, and the
code-like text (paths, hashes, the receipt JSON). The roles survive as weights
on its `wght` axis, set when `theme.rs` registers it: 540 for the display cut,
440 for the interface, 370 for sentences. Its `opsz` axis stays at the default
14. The one real cost is that code-like text is now proportional, so hashes and
JSON lose column alignment; egui's own monospace face stays behind Inter only
as a fallback for glyphs Inter lacks.

How it got here, so nobody re-litigates it from the git log. On 2026-09-17 the
owner asked for a face in the register of Google Sans, which is proprietary and
cannot be bundled. Plus Jakarta Sans went in first as the UI face; a Lato trial
followed and was not kept; Inter was chosen, first for everything except the
header, then for all text. Manrope, Space Grotesk and JetBrains Mono (the three
faces vitna-web's `tokens.css` names, which this directory previously carried)
and Plus Jakarta Sans are therefore no longer bundled, since the window should
carry only what it draws. To bring any of them back, copy the master from
`google/fonts` again and record its sha256 here.

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
