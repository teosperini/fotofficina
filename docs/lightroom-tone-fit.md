# Highlights, Shadows, Whites and Blacks fitted to Lightroom (Fotofficina)

LightCraft's tone sliders read fixed levels: Whites worked only near display white, Blacks only in the deepest shadows
(±5 levels at ±100), and Highlights only above about 0.4 of the displayed range, so on a dark photo Highlights and
Whites did nothing at all. Lightroom's sliders read the photo's own tonal range and reach much further into the midtones.
Fotofficina follows Lightroom here.

## What was measured

Eight exports from Lightroom Classic 15.6 of one high-contrast raw (IMG_8554, EOS 6D Mark II), each with one slider at
−100 or +100, and the unadjusted export. For each, the mean change in 8-bit sRGB luminance of the pixels at each
level of the unadjusted export (`~/Pictures/prova`, 16 level bins). The same raw rendered by Fotofficina with the same
slider values gives the same table; `scratchpad/lrcmp.sh` in the author's working folder reruns the comparison (mean gap
between the two tables: 4.4 levels before, 2.1 after; Whites 0.5–1.8, Blacks 1.4–1.8, Shadows 1.3–2.6, Highlights 1.5–5.7).

## What changed

- **Whites and Blacks** (`pipeline/src/tone.rs`, `lr_whites_blacks`): in the display-encoded domain, the change at each
  level comes from Lightroom's measured rows (`WHITES_LOW/HIGH`, `BLACKS_LOW/HIGH`), scaled linearly by the slider. They
  live in the tone LUT, so CPU and GPU share them.
- **Highlights and Shadows** (`finish.rs` and `wgsl/finish.wgsl`, same constants): the Highlights window is now
  `smooth(−2.3, 1.8)` in EV, eased by 30 % between 2.0 and 3.4 EV and back to full above 4.2 EV (blown lights still come
  down, see the equivalence test); Highlights strength 1.7 EV, Shadows 2.3 (was 1.7).
- **Anchoring to the photo** (`local::frame_white_ev`, `FinishParams::hl_shift`): the scene EV of the frame's 99.5th
  percentile luminance. When it is under `WHITE_REF_EV` (2.0 EV over grey, after exposure) the Highlights window moves
  down by the difference (at most 6 EV), the Whites/Blacks rows are read against the photo's own range, and +Highlights is
  eased by 1/(1 − 0.5·shift). A photo that reaches white reads exactly as without the anchor.

## Limits

Only one photo was measured, only at the two ends of each slider, and linearly scaled in between. Lightroom's response on
a dark photo was not measured: the anchoring is a design choice (Lightroom's sliders adapt to the histogram), tuned so
that the two ends of Highlights and Whites move a dark photo by about as much as the measured one. Contrast, Exposure,
Clarity and the rest are unchanged. Edits made with the old sliders render differently.
