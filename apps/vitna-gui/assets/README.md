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

## icons/

Every icon in the window, from Lucide (`lucide-static` 1.47.0 on npm, fetched
2026-09-18 from `cdn.jsdelivr.net/npm/lucide-static@1.47.0/icons/`), under the
ISC licence in `LICENSE-lucide.txt`. They replaced icons drawn from line
primitives, which rendered at slightly different weights on slightly different
grids. Each file has one edit: `stroke="currentColor"` became
`stroke="#ffffff"`, because the window tints an icon by multiplying it and a
black source would multiply every tone to black. The sha256 below is of the
file as fetched, before that edit, so the upstream copy can be checked and the
edit reapplied; `icons.rs` has a test that every bundled file loads and is
stroked white.

| file | sha256 as fetched |
|---|---|
| `arrow-up.svg` | `a2939fbea24b0323c6dcece2d27f68ccc8830df2a28bae8bb30839cc47281d2e` |
| `check.svg` | `7cb2a63edf5a9701361656f9bfbf13b1e7b084027f7fb2c992b82cd94105d9c1` |
| `chevron-left.svg` | `4a6ec61e17068836f3399349533d79d34ed7a533dd62ffd71bd33e2b108e2fa3` |
| `circle-alert.svg` | `14c478ad719aa49d3a0a074d7568781f6fa6390a33d0ffed72253e2d97b3109f` |
| `circle-check.svg` | `ed2f8446b59638ef70ebec698b2c0a8969dd9bbc61f1269767f70a25d4bcc7cd` |
| `circle-x.svg` | `99f86f531be18a2b2cac7fcea3fe5db1f2f5de7304c4eccda79e06c9e3538759` |
| `clock.svg` | `a5dea15fc6fbad0c836640f949b5db1ce5223be13993b7b463759fd411760a50` |
| `corner-down-left.svg` | `e973688fa3246d8bcffec85376d5b69fc7a622a08faf159c901877b2e04bf591` |
| `folder.svg` | `e0d4fefcca6cb86d5cebe9563e03149da88df1a2d76e5b9f8bc0bf2927721dad` |
| `git-merge.svg` | `722c8b9b19ce60137a6275f494867d3bceb8c949e75088028ddb74b7795a0d6b` |
| `keyboard.svg` | `e950413303a54b21decb9efe13e936e8a462425c97116c678e7fa0c3c4af199b` |
| `layout-grid.svg` | `f626b2a2abfd6b2985a002fbc338d799b2c21e86133c073b82b4d98a16be13d3` |
| `menu.svg` | `3d8a2baeb4d373403e9734e42239e9e65197bdcb87b89ec53000e8c3676d853d` |
| `message-square.svg` | `cce61ea2bfe80a69325ecd5811f35c9d4ae354cd071a97ce2bf33aceab95da5b` |
| `panel-left.svg` | `6d49a5bcce2c51c8847a4f0c1db0ee51222673ff3c97accf48ca54801290ba12` |
| `plus.svg` | `b0c6dc396b96741954b4ac7aaa247b9df73628e877826924e5d622e2677e2960` |
| `search.svg` | `5523ab5ec4d821183ad2b43b18142f36ea5080f1c90ffbf980c96a2b6cbbe5c6` |
| `server.svg` | `295bcac93ca3d2b8565a4cf25b50c79cc034e14db2bebf4adc52c997985f9615` |
| `sliders-horizontal.svg` | `60812bcc922487e7231d4ce75bacb0d1e3ad4d738fec9b17edaba7424a823273` |
| `sparkles.svg` | `b6398bc0c005340918475369e80921db0f6781a91af48d45f9cb85ec444a30d8` |
| `x.svg` | `3fb7f6004b7d52fb5019d11dff0d3ebc4236aab0651315742ea2fa5bfd6711a2` |

`LICENSE-lucide.txt` is the package's `LICENSE` as fetched, sha256
`b495047bd93a9b06913511076f504daba17d5bbeb3e0650f3bb53a4220329c57`. To add an
icon, fetch it from the same pinned version, apply the one edit, add its row
here and its line in `icons.rs`.
