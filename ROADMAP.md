# ProCraft Tier-A Roadmap: near-Procreate for daily work

Goal: a credible hobbyist alternative covering ~60–70% of the features
people touch daily. Ranked by user-visible value per effort.

## Cadence (agreed 2026-10-13)

- Work in milestone batches. Local commit per milestone (protects work);
  push only when asked.
- **PC check every 5 milestones** (desktop verify + user verdict).
- **iPad check every 10 milestones** (IPA + sideload verdict).
- Batch 1 (PC check #1): M23–M27. Batch 2 (PC check #2 + iPad check #1):
  M28–M32 (exact slice decided at M27). Batch 3 (PC check #3): M33–M37
  (Gallery + Adjustments start, decided at v0.7.5).
- Batch 5 (PC check #5 + iPad check #3): M43–M47 (slice below).

## Batch 5: M43–M47 (transform depth → hardening → picker → Pencil)

- [x] **M43 — Distort (corner-pin).** Quad remap on layer, bilinear,
  identity is a no-op; masks travel.
- [x] **M44 — Warp mesh.** Movable lattice over the layer (distort's
  freeform sibling).
- [x] **M45 — Stability hardening.** Fuzz project open, OOM guards on
  import.
- [x] **M46 — SAF file picker.** Real Android picker (replaces the
  v0.8.1 "picker coming" notes); tighten desktop fallback.
- [x] **M47 — Palm rejection + Pencil.** Double-tap/hover, touch guards.
  **→ PC checkpoint #5 + iPad check #3 (IPA).**
- Deferred to Batch 6: 8K canvas + lazy compositing (standalone perf
  epic, riskiest — Batch 5 stays shippable without it). Shipped on
  branch `8k-canvas`.

## Batch 6 slice: M48 — Transform handles (done, user-verified)

- [x] **M48 — On-canvas transform handles.** Corner pins (Distort) +
  3x3 warp lattice (Mesh): arm in the Transform panel, drag handles,
  release-apply; live drag preview from a revision-tracked session
  base, Esc/pinch cancel restore, Reset. Retest rounds hardened the
  pin session (substep stabilizer, trail sampling), smoothed strokes
  (midpoint-quadratic `StrokeCurve`), and the Transform-layer buttons
  (session base — no compounding resample). All user-verified good;
  pushed bb5c777.
- Batch 6 remainder: 8K canvas + lazy compositing perf epic — shipped
  on branch `8k-canvas` (tiled storage + tile-undo, LRU composite cache,
  v12 format, viewport fallback).

## Batch 7 — keybinds + settings (planned 2026-10-17)

- [x] **M49 — Settings persistence + keybind remap.** Done 2026-10-17
  (`998cbce`): cross-platform `settings.json` (seed-once from legacy
  ui.json, corrupt-file quarantine + note), Shortcuts editor (rebind
  tools + undo/redo, exact-chord dispatch fixes Ctrl+Shift+Z), drag
  modifiers, Shift+drag = brush size. Defaults: B brush, E eraser, F
  fill, Ctrl+Z/Ctrl+Shift+Z undo/redo. View-verification (restart
  restore, corrupt recovery) pending user run.
- [x] **M50 — Editable text layers.** Done 2026-10-18 (`c0495fd`,
  review fixes `ea4b91d` + `3e240a3`, input fixes `f904b04` +
  `98afdf9`): Procreate-style re-editable text layers (tap to
  select/re-edit/move, Rasterize bakes to pixels), v11 project format
  (PCRAFT11), device-verified. Ships in v0.9.0.
- [ ] **Candidate — animated UI transitions + timelapse card.**
  Discord (MAN/FMHY, 2026-10-16): "like a card with frame inside… bring
  some animation/transition for the ui" for showcasing timelapse-from-
  scratch. Earlier ruling: declined (perf risk); logged per user.
- [ ] **Issue #3 — phone UI report (jawbu, Pixel 8, v0.8.5).** Icon part
  fixed: adaptive launcher icon built from icon.svg geometry (the legacy
  PNG was launcher-wrapped in a padded white plate); CI asserts
  mipmap-anydpi-v26. Remaining: status-bar overlap + unpressable top
  buttons (unreleased phone-layout commits address — retest in next
  APK); camera punch-hole in landscape; bigger/fewer Procreate-like
  buttons (toolbar-minimalism theme); dark mode (exists, buried in the
  theme selector — discoverability + should persist on mobile); portrait
  spacing stretched.
- Close-out: Windows + iOS sideload + Android builds → next GitHub
  release (continue the existing release series).

## B2 — UI modernization (branch `ui-b2-restyle`, 2026-10-27)

Plan: `docs/superpowers/plans/2026-10-27-ui-modernization.md`.

- [x] Design tokens per theme and per device class, with one egui style
  pipeline (`design.rs` `Tokens::apply`).
- [x] Pill button primitive adopted in the top bar; section cards in the
  tools and layers panels; single touch-height source with a
  `PROCRAFT_FORCE_LAYOUT` device-class override; thick quick-bar sliders.
- [x] Three-state theme toggle in the top bar (outside the touch fold,
  right before `About` on desktop and before `View` on touch): cycles
  Dark → Light → Paper and shows the current state — ☾ dark, ☀ light,
  🖹 paper. It is the only theme control; the old View theme combo is
  gone, so the bar carries one square icon pill instead.
- [x] Dark is the default theme for new installs; an existing
  `settings.json` keeps its saved theme.
- [x] Swatch card pass: normal card padding (labels no longer touch the
  border), hand-painted pixel-snapped 24 pt squares with a 4 pt corner
  (crisp at 125%), and the palette actions/name split onto two rows so
  the name no longer clips.
- [x] Device pass + iOS/Android artifacts for the branch: shipped in
  v0.10.0 alongside the mobile shell (iOS IPA, signed Android APK,
  Windows zip on the release page).
- Known, pre-existing: at 1120 pt wide the right panel and the quick bar
  clip — also true of `73e06da`, so not introduced by B2.

## Batch 1: M23–M27 (done, awaiting PC check #1)

- [x] **M23 — Layer groups + group ops.** Nested model with effective
  visibility, panel tree (collapse/eye/rename/+Add/Ungroup), v8 format
  with v1–v7 compat open. Deferred: group opacity, PSD group round-trip.
- [x] **M24 — Smudge tool.** Core-biased finger pickup, stroke walker,
  Strength slider, mask-target hint.
- [x] **M25 — Canvas rotate.** Twist gesture + slider/Reset, GPU-rotated
  mesh view, all overlays mapped (ants march rotated edges).
- [x] **M26 — Finger-tap gestures.** Two-finger tap undo, three-finger
  tap redo (clean quick release only).
- [x] **M27 — Autosave + crash recovery.** Silent v8 snapshot every
  120 s, centered Recover/Discard prompt, cleared on save/load/clean
  exit. **→ PC checkpoint #1 (you are here).**

## Batch 2: M28–M32 (done, awaiting PC check #2 + iPad check #1)

- [x] **M28 — JPEG/TIFF export.** Quality slider for JPEG (1–100),
  plain lossless TIFF (PNG/PSD/GIF existed).
- [x] **M29 — Crop + canvas resize.** Drag-crop rect + pad/trim resize
  with anchor (lossless offsets carry content, history clears).
- [x] **M30 — Group opacity + PSD groups.** Pass-through group opacity
  in compositing/frames/GIF, v9 project format, PSD section-divisor
  round-trip (nested, open/closed, empty groups).
- [x] **M31 — Color from image + loupe.** Pick tool + magnifier ring on
  pick/Alt (touch-first), palette from image file or canvas.
- [x] **M32 — Animation frame UI.** Duplicate frame, per-frame hold
  (v10 format, GIF round-trips timing), onion strength per side.
  **→ PC checkpoint #2 + iPad check #1 (IPA) (you are here).**

## Batch 3: M33–M37 (done — Gallery + Adjustments start)

- [x] **M33 — Gallery/file browser.** New/open/recent/duplicates with
  thumbnails; replaces OS-dialog-only flow (dialogs stay as fallback).
- [x] **M34 — Reference companion window.** Pinned image alongside canvas.
- [x] **M35 — Selection upgrades.** Save/load selection, transform
  selection (nudge/flip); on-canvas fill-tolerance sweep pre-existed.
- [x] **M36 — Adjustments I.** Curves + Levels.
- [x] **M37 — Adjustments II.** HSL + Color Balance + Recolor.
  **→ PC checkpoint #3 (passed, pushed).**

## Batch 4: M38–M42 (done — depth + polish-at-IPA)

- [x] **M38 — Typography.** TTF/OTF import, leading/tracking, outline text.
- [x] **M39 — Brush depth.** Dual-tip, wet-edge/glaze, per-brush blend mode.
- [x] **M40 — Liquify-lite.** Push/smudge-field on layer.
- [x] **M41 — Time-lapse recording.** Staged snapshots.
- [x] **M42 — iPad layout polish pass.**
  **→ PC checkpoint #4 + iPad check #2 (IPA) (you are here).**

## P0 backlog (done)

- [x] Gallery/file browser — moved into Batch 3 above.

## P1 — depth on what exists

- [x] Freeform transform: distort + warp mesh (M43/M44 engine,
  M48 on-canvas handles).
- [ ] Liquify-lite (push field on layer).
- [ ] Adjustments pack: Curves, HSL, Levels, Color Balance, Recolor.
- [ ] Selection upgrades: save/load, transform selection, on-canvas
  color-fill tolerance.
- [ ] Typography: import TTF/OTF, leading/tracking, outline text.
- [ ] Brush depth: dual-tip, wet-edge/glaze, per-brush blend mode.
- [ ] Reference companion window.

## P2 — performance + platform (when P0 hurts)

- [x] 8K canvas, 100+ layers with lazy compositing (device verification pending).
  256² copy-on-write tiles with tile-pointer undo, tile uploads behind an
  LRU composite cache, v12 per-tile project format, viewport fallback for
  GPUs that cannot hold an 8K texture.
- [ ] Palm rejection + Pencil double-tap/hover.
- [ ] Time-lapse recording.
- [ ] iPad layout polish pass.
- [ ] Stability hardening: fuzz project open, OOM guards on import.

## Explicit non-goals (until Tier A ships)

GPU render path · CMYK/P3 color management · cloud sync · App Store
submission process · ML filters · 3D / text-on-path.

## Shipped context (v0.7.1, milestones 1–22)

Layered model + masks + 10 blends, brush engine (dynamics, library,
abr/brushset/kpp/gbr interop), rect/lasso + Selection Studio (weighted
masks, bbox ants), flood + ColorDrop, shapes, symmetry/kaleido + movable
center, guides + Drawing Assist, palettes + harmonies, animation
(frames/onion/GIF), baked text, lossless offsets, PSD interchange,
compressed projects (v7), undo byte-budget, iOS sideload builds.
