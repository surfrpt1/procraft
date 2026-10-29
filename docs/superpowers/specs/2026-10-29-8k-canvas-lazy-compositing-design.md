# 8K canvas + lazy compositing — Design

**Scope:** the document engine (`crates/engine`) plus the canvas display path in
`apps/procraft`. Desktop, phone and tablet share one implementation; the only
platform split is the display fallback in phase 5. No UI redesign: toolbars,
drawers and sheets keep their current behaviour.

**Why:** the engine stores every layer (and its mask) as one dense RGBA buffer
(`Layer::pixels`,
[crates/engine/src/document.rs:261](../../crates/engine/src/document.rs#L261)),
caps the canvas at 4096 a side (`MAX_CANVAS_SIDE`,
[crates/engine/src/document.rs:422](../../crates/engine/src/document.rs#L422)),
snapshots the whole layer per stroke for undo
([crates/engine/src/document.rs:792](../../crates/engine/src/document.rs#L792)),
and composites by walking every layer for every pixel
([crates/engine/src/document.rs:867](../../crates/engine/src/document.rs#L867)).
At 8192² that is 256 MB per layer, 256 MB per undo step and ~26 GB for 100
layers, so "8K, 100+ layers" is impossible without changing storage first.

**Goal:** open and paint 8192×8192 documents with 100+ layers, keeping stroke
latency and pan/zoom identical to today's feel, with undo bounded by what a
stroke actually touched rather than canvas size.

**Non-goals:** GPU layer compositing; cross-session tile compression; layer
hibernation to disk; changing the brush engine's sampling.

## 1. Tiled layer storage

`Layer.pixels` (and `Layer.mask`) become `Raster`:

```rust
const TILE: u32 = 256;          // TILE * TILE * 4 = 256 KiB per tile
type Tile = Arc<Vec<u8>>;       // len = TILE * TILE * bpp

pub struct Raster {
    w: u32, h: u32,             // canvas size (shared by all layers)
    bpp: u8,                    // 4 = RGBA layer, 1 = mask
    tiles_x: u32, tiles_y: u32, // ceil(w / TILE), ceil(h / TILE)
    tiles: Vec<Option<Tile>>,   // None = fully transparent
}
```

- Empty tiles are `None`, never zero-filled allocations: 100 blank layers at 8K
  cost 100 pointer arrays (1024 slots × 8 B ≈ 8 KB each), not 26 GB. A tile is
  materialised the first time a write touches it.
- Reads: `get(i) -> u8` / `get4(i) -> [u8; 4]` on the dense byte index the old
  `Vec<u8>` used (zero on `None` or past the end), `tile(tx, ty)` and
  `allocated_tiles()` for row-wise work, `to_dense()` and `from_dense()` as
  migration bridges for export, PSD and thumbnail paths.
- Writes: `set`, `set4`, `tile_mut`, `fill`, always through `Arc::make_mut`, so
  untouched tiles stay shared between history steps and duplicate layers.
- Mask rasters are 1 byte/pixel with the same geometry and tiling (64 KiB per
  tile).
- Edge tiles are full 256² and padded with zeros; canvas bounds gate all reads
  so padding is never visible.
- `fill(0)`, `from_dense` and `replace_dense` drop all-zero tiles; erasing
  pixel by pixel leaves an allocated zero tile until the next whole-layer
  write, so "erased costs nothing" holds on disk and after any full rewrites,
  not per erased pixel.

**Invariant:** a `None` slot and an all-zero tile are equivalent. Tests assert
`get`/`region` equality between both forms, and that zero tiles are never
materialised by dense loads. The eraser canonicalises every pixel it takes to
alpha 0 to `[0, 0, 0, 0]` (masks already write 0), so alpha-0 pixels carry no
colour and an all-zero tile is exactly an invisible tile.

## 2. Undo

- `Layer` checkpoints clone the tile vector (pointer copy) instead of pixel
  bytes; `UndoEntry.pixels` becomes a `Raster`, `mask` an `Option<Raster>`, and
  `text` is unchanged.
- A stroke writes through `Arc::make_mut`, so the first touch of a tile in a
  stroke allocates its 256 KiB copy once; the checkpoint keeps the old pointer.
- Cost per stroke = touched tiles × 256 KiB (typically 1–5 MB at 8K) instead of
  256 MB. At that size the 32-entry cap (`MAX_UNDO_ENTRIES`) binds long before
  the 256 MiB byte budget, so 8K history keeps 32 steps where 4K keeps ~6.
- `UndoEntry::byte_size` charges every allocated tile slot per entry. Tiles
  shared with a neighbouring entry are counted once per entry — a deliberate
  over-estimate, so `MAX_UNDO_BYTES` stays a safe upper bound.
- Non-stroke operations (resize, transform bake, filters, merge) keep a
  document-level checkpoint that clones every layer's tile vector: ~0.8 MB of
  pointers for 100 layers at 8K, with COW making the actual copies as they are
  written.

## 3. Compositing and display (target A)

- `CompositeCache` lives in the app: a grid of 256² RGBA8 tiles, LRU-capped at
  96 MB on native mobile and 320 MB elsewhere.
- Building a composite tile precomputes the stack plan once (visible layers,
  effective group opacity, clip flags), then blends rows — no per-pixel
  `is_layer_shown` or `group_opacity` calls.
- `Document` gains `composite_tile(&self, plan: &StackPlan, tx, ty) -> Vec<u8>`
  (always `TILE * TILE * 4` bytes, zero outside the canvas) so
  blending stays in the engine; the app owns the cache, the GPU texture and the
  dirty-tile bookkeeping.
- `Document::take_dirty` keeps returning the rect the app already consumes; the
  app maps it to tile indices and re-composites exactly those tiles.
- One canvas texture (8192² RGBA8 = 268 MB VRAM) receives `set_partial` per
  recomposited tile. Pan, zoom and rotation never recomposite.
- Mask view and Animation Assist keep their dedicated upload paths, but both
  read through the same tile accessor so neither special-cases storage.
- Both still upload the full canvas texture (mask view now rebuilds it once and
  then patches the dirty rect), so they require a canvas that fits the GPU
  limit; a larger canvas shows a status message instead of the upload.
- The canvas texture needs a GPU `max_texture_size` of at least 8192. When the
  device reports less, the app takes the fallback in section 4 instead.

## 4. Mobile fallback (B)

- Trigger: native mobile target and `max(w, h) > 4096`, or `max_texture_size`
  below 8192, or `PROCRAFT_FORCE_VIEWPORT_CANVAS=1` for host captures.
- Instead of the canvas texture, the app composites the visible rect into a
  screen-sized texture (iPad Pro 2732×2048 ≈ 22 MB) from the same composite
  tiles, at 1:1 when zoom ≥ 100 % and downscaled when zoomed out.
- Re-render on pan/zoom end, dirty tiles that intersect the view, and window
  resize; mid-gesture frames reuse the previous texture with the current
  transform so pinch stays responsive.
- Quality: after any gesture settles, the visible region is re-rendered from
  canvas-resolution tiles, so text and thin strokes stay sharp at 100 %.

## 5. Limits and project format

- `MAX_CANVAS_SIDE` 4096 → 8192; offsets keep their ±16384 limit.
- Project format v12 (`PCRAFT12`): the v11 header, symmetry, group and layer
  fields are byte-identical. Only the pixel payload changes — instead of one
  zlib blob per layer, each layer writes a tile table: `u32 count`, then per
  tile `u16 tx, u16 ty, u32 len, len bytes` of zlib-compressed RGBA. The mask
  flag is followed by the same table at one byte per pixel. Empty tiles are
  omitted entirely, so saving an unpainted 8K layer writes 4 bytes instead of
  compressing 256 MB of zeros.
- Readers keep v1–v11: dense payloads are decompressed and split into tiles on
  load. Writers emit v12 only; older app versions refuse v12 at the magic.
- New-canvas presets gain an 8K entry; the canvas-size dialog accepts up to
  8192 with the same validation message.

## 6. Memory guard

- The new-canvas dialog shows the canvas size and the allocation a blank
  document will use (pointer arrays only) on mobile, where the previous 4096
  cap kept documents small enough that this never mattered.
- The layers panel shows the live allocation next to the layer count: allocated
  tiles × 256 KiB plus mask tiles × 64 KiB, summed over layers and undo history.
- No hard refusal beyond the 8192 cap. `UndoEntry::byte_size` and the 32-entry
  cap already bound history; the panel exists so a user can see why a 100-layer
  8K document is heavy before the OS kills the app.

## Phases

1. **Raster + bridges.** Introduce `Raster` with `to_dense`/`from_dense`, move
   `Layer::pixels`/`mask` and every call site onto it mechanically. Behaviour
   unchanged: the existing engine and app suites stay green and the 2026-10-29
   screenshots match.
2. **Tile writes + composite cache.** Convert the paint paths (brush dabs,
   eraser, smudge, fill, liquify, text rerender) to tile writes, add
   `CompositeCache`, recomposite dirty tiles, upload per tile (target A).
3. **Tile-aware undo.** Replace byte snapshots with tile-vector clones; keep
   `restore_last_checkpoint` and the taper replay intact.
4. **8K + v12.** Raise the cap, add the 8K preset, write/read v12 with v11
   back-compat, keep export/PSD working through the dense bridge.
5. **Mobile viewport fallback (B).** Screen-sized composite texture behind the
   trigger above, including the host override.
6. **Measurement.** `cargo run --release -p procraft-engine --example tilebench`
   reports composite time per tile for 1/10/100 layers at 4K and 8K; on-device
   numbers recorded in the plan log. Measured caveat: `flatten` at 8K/100 layers
   takes ~22 s on the 15 W host and that figure is a floor, because the
   compositor does not skip empty layers — export at that size is a batch
   operation, not interactive.

Phases 1–4 complete the milestone; 5 is the VRAM escape hatch and 6 gates a
"done" claim. If the schedule slips, 5 is the piece to cut.

## Testing

- Unit: raster get/put/bounds/edge tiles, None-equals-zero, COW sharing
  (`Arc::strong_count`), undo bytes after a stroke equal touched tiles × 256 KiB.
- Golden: composite output of a tiled document must equal the dense reference
  byte-for-byte across blend modes, masks, clip layers and group opacity.
- Format: the in-memory v2–v11 records the current tests already build still
  open and re-save as v12 with identical flattened output; the existing
  `PCRAFT11` magic assertions move to `PCRAFT12` with the writer; a v12
  round-trip keeps tile sparsity (empty layers and masks stay empty on disk).
- App: existing screenshot harness re-run at 4K and 8K on desktop and forced
  mobile, comparing sheets, drawer and canvas against the 2026-10-29 baseline.

## Risks

- **Surface area.** ~250 dense access sites in the engine and 105 in the app;
  phase 1 must land as a behaviour-neutral refactor before any perf work.
- **Mask and text layers** share the storage path; both are covered in phase 1
  so later phases never special-case them.
- **VRAM.** Target A needs 268 MB at 8K; the fallback in phase 5 is the
  mitigation and is measured on the Android tablet before shipping.
- **Export cost.** A full 8K flatten over 100 layers takes seconds; it stays on
  the explicit export/thumbnail path, never in the frame loop.
