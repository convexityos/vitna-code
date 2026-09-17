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
- `icon.png` is `public/icon.png`, rasterised from that SVG upstream, used only
  for the OS window and taskbar icon, which the platform takes as a bitmap.

To update, copy the files again from vitna-web rather than editing these.
