# 8K canvas + lazy compositing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Open, paint and pan 8192×8192 ProCraft documents with 100+ layers, keeping stroke latency and pan/zoom feel at today's 4K level, with undo and memory proportional to painted tiles rather than canvas size.

**Architecture:** `Layer::pixels`/`mask` move from dense `Vec<u8>` to `Raster` — a grid of 256² copy-on-write `Arc` tiles (`crates/engine/src/raster.rs`). Undo checkpoints become pointer copies of the tile vector, so a stroke costs only the tiles it touches. The app composites 256² tiles through an LRU `CompositeCache` and uploads only dirty tiles into one canvas texture (target A); when that texture cannot fit (native mobile above 4096 a side, or a GPU `max_texture_side` below 8192) it renders a screen-resolution viewport texture instead (target B). Project format v12 stores per-tile zlib payloads and keeps reading v1–v11.

**Tech Stack:** Rust 1.88+, `egui`/`epaint` 0.36.2, `flate2` (already a dependency), `std::sync::Arc`. No new crates.

**Spec:** `docs/superpowers/specs/2026-10-29-8k-canvas-lazy-compositing-design.md`

## Global Constraints

- Rust 1.88+; workspace crates `crates/engine` (`procraft-engine`) and `apps/procraft` (`procraft`); no new dependencies.
- Tile side is exactly `TILE = 256` pixels. RGBA tile = 262 144 bytes; mask tile = 65 536 bytes.
- `MAX_CANVAS_SIDE` becomes 8192 (task 8). The project-file reader keeps its 16384 dimension tolerance.
- Empty tiles are `None`; reads of `None` return 0; `fill(0)` drops tiles; `replace_dense`/`from_dense` skip all-zero tiles.
- Desktop chrome must not change: every mobile-only branch stays behind `mobile_shell()` / `PROCRAFT_FORCE_SHELL=mobile`; the 8K work is platform-neutral.
- `dist/` stays untracked and is never committed.
- Gates after every task: `cargo fmt --all`, `cargo clippy -p procraft --all-targets -- -D warnings`, `cargo test -p procraft-engine`, `cargo test -p procraft --lib`, and for tasks 8–11 `cargo build --release -p procraft`.
- Branch: create `8k-canvas` from `mobile-polish` (the spec commit `251f1fc` lives there). Do not push, tag, or cut IPA/APK builds without asking.
- Replies and commit messages in English. Commit after each task.

---

### Task 1: `Raster` — tiled copy-on-write pixel storage

**Files:**
- Create: `crates/engine/src/raster.rs`
- Modify: `crates/engine/src/lib.rs:12-33` (module list + re-export)

**Interfaces:**
- Consumes: nothing (new leaf module).
- Produces: `Raster`, `TILE: u32`; methods `rgba(w,h)`, `mask(w,h)`, `new(w,h,bpp)`, `from_dense(w,h,bpp,&[u8])`, `replace_dense(&[u8])`, `to_dense() -> Vec<u8>`, `w()`, `h()`, `bpp()`, `tiles_x()`, `tiles_y()`, `len()`, `tile_len()`, `tile(tx,ty) -> Option<&[u8]>`, `tile_mut(tx,ty) -> &mut [u8]`, `allocated_tiles()`, `alloc_bytes()`, `get(usize) -> u8`, `set(usize,u8)`, `get4(usize) -> [u8;4]`, `set4(usize,[u8;4])`, `fill(u8)`, `fingerprint() -> u64`, `PartialEq`/`PartialEq<Vec<u8>>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/engine/src/raster.rs` with tests at the bottom (the implementation comes in step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_roundtrip_matches_input() {
        let w, h = 300u32, 200u32;
        let bytes: Vec<u8> = (0..(w * h * 4) as usize).map(|i| (i % 251) as u8).collect();
        let r = Raster::from_dense(w, h, 4, &bytes);
        assert_eq!(r.len(), bytes.len());
        assert_eq!(r.to_dense(), bytes);
        assert_eq!(r.get(0), bytes[0]);
        assert_eq!(r.get(bytes.len() - 1), bytes[bytes.len() - 1]);
        assert_eq!(r.get(bytes.len()), 0, "reads past the end are zero");
    }

    #[test]
    fn writes_cross_tile_seams() {
        let mut r = Raster::rgba(600, 600); // 3x3 tiles
        let at = |x: u32, y: u32| ((y * 600 + x) * 4) as usize;
        r.set4(at(255, 10), [1, 2, 3, 4]);
        r.set4(at(256, 10), [5, 6, 7, 8]);
        assert_eq!(r.get4(at(255, 10)), [1, 2, 3, 4]);
        assert_eq!(r.get4(at(256, 10)), [5, 6, 7, 8]);
        assert_eq!(r.alloc_bytes(), 2 * r.tile_len());
    }

    #[test]
    fn blank_raster_costs_nothing() {
        let mut r = Raster::rgba(8192, 8192);
        assert_eq!(r.alloc_bytes(), 0);
        assert_eq!(r.get4(0), [0; 4]);
        r.set(5, 9);
        assert_eq!(r.alloc_bytes(), r.tile_len());
        r.fill(0);
        assert_eq!(r.alloc_bytes(), 0);
    }

    #[test]
    fn zero_tiles_are_not_materialised_from_dense() {
        let w, h = 512u32, 512u32;
        let mut bytes = vec![0u8; (w * h * 4) as usize];
        bytes[((511 * w + 511) * 4) as usize] = 7;
        let r = Raster::from_dense(w, h, 4, &bytes);
        assert_eq!(r.alloc_bytes(), r.tile_len(), "only the touched tile exists");
        assert_eq!(r.to_dense(), bytes);
    }

    #[test]
    fn clones_share_tiles_until_written() {
        let mut a = Raster::rgba(300, 300);
        a.set4(0, [9, 9, 9, 9]);
        let mut b = a.clone();
        assert!(a.shares_tile(&b, 0, 0));
        b.set4(4, [1, 1, 1, 1]);
        assert!(!a.shares_tile(&b, 0, 0), "write copies the tile");
        assert_eq!(a.get4(0), [9, 9, 9, 9]);
        assert_eq!(b.get4(0), [9, 9, 9, 9]);
        assert_eq!(b.get4(4), [1, 1, 1, 1]);
    }

    #[test]
    fn edge_tiles_pad_without_reading_past_the_row() {
        let r = Raster::from_dense(300, 5, 4, &vec![3u8; 300 * 5 * 4]);
        assert_eq!(r.tiles_x(), 2);
        assert_eq!(r.tiles_y(), 1);
        assert_eq!(r.get(((5 * 300 - 1) * 4) as usize), 3);
        assert_eq!(r.get(((5 * 300) * 4) as usize), 0);
    }

    #[test]
    fn mask_raster_uses_one_byte_per_pixel() {
        let mut m = Raster::mask(300, 300);
        m.set((299 * 300 + 299) as usize, 255);
        assert_eq!(m.len(), 300 * 300);
        assert_eq!(m.tile_len(), 65536);
        assert_eq!(m.get((299 * 300 + 299) as usize), 255);
        assert_eq!(m.to_dense().len(), 300 * 300);
    }

    #[test]
    fn fingerprint_notices_tile_writes() {
        let mut a = Raster::rgba(600, 600);
        let before = a.fingerprint();
        assert_eq!(a.fingerprint(), before);
        a.set4(((590 * 600 + 590) * 4) as usize, [1, 2, 3, 255]);
        assert_ne!(a.fingerprint(), before);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p procraft-engine raster`
Expected: compile error — `Raster` not defined.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/engine/src/raster.rs`:

```rust
//! Dense 8K layers are stored as a grid of copy-on-write 256² tiles.
//!
//! A `Raster` reads like a dense buffer (`get`/`set` take the same byte
//! index a `Vec<u8>` used) but only allocates tiles that hold content:
//! an untouched 8K layer costs one `Vec` of `Option` pointers, and undo
//! and duplicate share every untouched tile through `Arc`.

use std::sync::Arc;

/// Tile side in pixels. 256² RGBA is 256 KiB; 256² masks are 64 KiB.
pub const TILE: u32 = 256;

#[derive(Clone, Debug, Default)]
pub struct Raster {
    w: u32,
    h: u32,
    bpp: u8,
    tiles_x: u32,
    tiles_y: u32,
    tiles: Vec<Option<Arc<Vec<u8>>>>,
}

impl Raster {
    pub fn new(w: u32, h: u32, bpp: u8) -> Self {
        let tiles_x = w.div_ceil(TILE);
        let tiles_y = h.div_ceil(TILE);
        Self {
            w,
            h,
            bpp: bpp.max(1),
            tiles_x,
            tiles_y,
            tiles: vec![None; (tiles_x * tiles_y) as usize],
        }
    }

    pub fn rgba(w: u32, h: u32) -> Self {
        Self::new(w, h, 4)
    }

    pub fn mask(w: u32, h: u32) -> Self {
        Self::new(w, h, 1)
    }

    pub fn w(&self) -> u32 {
        self.w
    }

    pub fn h(&self) -> u32 {
        self.h
    }

    pub fn bpp(&self) -> usize {
        self.bpp as usize
    }

    pub fn tiles_x(&self) -> u32 {
        self.tiles_x
    }

    pub fn tiles_y(&self) -> u32 {
        self.tiles_y
    }

    /// Dense byte length (`w * h * bpp`): the index space of `get`/`set`.
    pub fn len(&self) -> usize {
        (self.w as usize) * (self.h as usize) * self.bpp()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn tile_len(&self) -> usize {
        (TILE * TILE) as usize * self.bpp()
    }

    pub fn tile(&self, tx: u32, ty: u32) -> Option<&[u8]> {
        self.tile_arc(tx, ty).map(Vec::as_slice)
    }

    fn tile_arc(&self, tx: u32, ty: u32) -> Option<&Arc<Vec<u8>>> {
        if tx >= self.tiles_x || ty >= self.tiles_y {
            return None;
        }
        self.tiles[(ty * self.tiles_x + tx) as usize].as_ref()
    }

    /// Mutable tile access; materialises the tile on first write and
    /// copies it when it is still shared with a snapshot.
    pub fn tile_mut(&mut self, tx: u32, ty: u32) -> &mut [u8] {
        let len = self.tile_len();
        let idx = (ty * self.tiles_x + tx) as usize;
        let arc = self.tiles[idx].get_or_insert_with(|| Arc::new(vec![0u8; len]));
        Arc::make_mut(arc).as_mut_slice()
    }

    /// Allocated tiles in row-major order: `(tx, ty, bytes)`.
    pub fn allocated_tiles(&self) -> impl Iterator<Item = (u32, u32, &[u8])> {
        self.tiles.iter().enumerate().filter_map(|(i, t)| {
            let t = t.as_deref()?;
            let i = i as u32;
            Some((i % self.tiles_x, i / self.tiles_x, t.as_slice()))
        })
    }

    /// Bytes actually held by tiles (what the memory readout reports).
    pub fn alloc_bytes(&self) -> usize {
        self.tiles.iter().filter(|t| t.is_some()).count() * self.tile_len()
    }

    fn locate(&self, i: usize) -> Option<(usize, usize)> {
        if i >= self.len() {
            return None;
        }
        let bpp = self.bpp();
        let px = i / bpp;
        let k = i % bpp;
        let (x, y) = (
            (px % self.w as usize) as u32,
            (px / self.w as usize) as u32,
        );
        let (tx, ty) = (x / TILE, y / TILE);
        let slot = (ty * self.tiles_x + tx) as usize;
        let off = ((y % TILE) * TILE + (x % TILE)) as usize * bpp + k;
        Some((slot, off))
    }

    pub fn get(&self, i: usize) -> u8 {
        let Some((slot, off)) = self.locate(i) else {
            return 0;
        };
        self.tiles[slot].as_deref().map_or(0, |t| t[off])
    }

    pub fn set(&mut self, i: usize, v: u8) {
        let Some((slot, off)) = self.locate(i) else {
            return;
        };
        let len = self.tile_len();
        let arc = self.tiles[slot].get_or_insert_with(|| Arc::new(vec![0u8; len]));
        Arc::make_mut(arc)[off] = v;
    }

    pub fn get4(&self, i: usize) -> [u8; 4] {
        [self.get(i), self.get(i + 1), self.get(i + 2), self.get(i + 3)]
    }

    pub fn set4(&mut self, i: usize, px: [u8; 4]) {
        for (k, v) in px.into_iter().enumerate() {
            self.set(i + k, v);
        }
    }

    pub fn to_dense(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.len()];
        let step = self.bpp();
        let w = self.w as usize;
        for (tx, ty, tile) in self.allocated_tiles() {
            let x0 = tx as usize * TILE as usize;
            let y0 = ty as usize * TILE as usize;
            let n = (w - x0).min(TILE as usize) * step;
            for row in 0..TILE as usize {
                let y = y0 + row;
                if y >= self.h as usize {
                    break;
                }
                let src = row * TILE as usize * step;
                let dst = (y * w + x0) * step;
                out[dst..dst + n].copy_from_slice(&tile[src..src + n]);
            }
        }
        out
    }

    /// Replace every tile from a dense buffer sized for this geometry.
    /// Rows shorter than `w` read as zero; all-zero tiles stay `None`.
    pub fn replace_dense(&mut self, bytes: &[u8]) {
        for t in &mut self.tiles {
            *t = None;
        }
        let step = self.bpp();
        let w = self.w as usize;
        let row = w * step;
        for y in 0..self.h as usize {
            let src = y * row;
            if src >= bytes.len() {
                break;
            }
            let row_end = (src + row).min(bytes.len());
            let mut x = 0usize;
            while x < w {
                let tx = (x as u32) / TILE;
                let tx_end = (((tx + 1) * TILE) as usize).min(w);
                let a = x * step;
                let b = (tx_end * step).min(row_end - src);
                if a < b {
                    let chunk = &bytes[src + a..src + b];
                    if chunk.iter().any(|&v| v != 0) {
                        let local_x = x - tx as usize * TILE as usize;
                        let local = ((y % TILE as usize) * TILE as usize + local_x) * step;
                        let t = self.tile_mut(tx, y as u32 / TILE);
                        t[local..local + (b - a)].copy_from_slice(chunk);
                    }
                }
                x = tx_end;
            }
        }
    }

    pub fn from_dense(w: u32, h: u32, bpp: u8, bytes: &[u8]) -> Self {
        let mut r = Raster::new(w, h, bpp);
        r.replace_dense(bytes);
        r
    }

    /// Fill every pixel. `fill(0)` drops all tiles.
    pub fn fill(&mut self, v: u8) {
        if v == 0 {
            for t in &mut self.tiles {
                *t = None;
            }
            return;
        }
        let len = self.tile_len();
        for t in &mut self.tiles {
            let arc = t.get_or_insert_with(|| Arc::new(vec![0u8; len]));
            Arc::make_mut(arc).fill(v);
        }
    }

    /// Cheap content fingerprint for preview-guard comparisons: samples
    /// every allocated tile, never materialises dense bytes.
    pub fn fingerprint(&self) -> u64 {
        let mut h = 0xcbf29ce484222325u64 ^ (self.len() as u64);
        for (i, t) in self.tiles.iter().enumerate() {
            let Some(t) = t else {
                continue;
            };
            h ^= i as u64;
            h = h.wrapping_mul(0x100000001b3);
            for (k, &b) in t.iter().enumerate().step_by(4096) {
                h ^= (b as u64).wrapping_add(k as u64);
                h = h.wrapping_mul(0x100000001b3);
            }
        }
        h
    }

    #[cfg(test)]
    fn shares_tile(&self, other: &Raster, tx: u32, ty: u32) -> bool {
        match (self.tile_arc(tx, ty), other.tile_arc(tx, ty)) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl PartialEq for Raster {
    fn eq(&self, other: &Self) -> bool {
        self.w == other.w
            && self.h == other.h
            && self.bpp == other.bpp
            && self.to_dense() == other.to_dense()
    }
}

impl PartialEq<Vec<u8>> for Raster {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.to_dense() == *other
    }
}

impl PartialEq<Raster> for Vec<u8> {
    fn eq(&self, other: &Raster) -> bool {
        *self == other.to_dense()
    }
}
```

Register the module in `crates/engine/src/lib.rs`:

```rust
mod raster;
...
pub use raster::{Raster, TILE};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p procraft-engine raster`
Expected: 8 passed.

- [ ] **Step 5: Run the gates and commit**

```bash
cargo fmt --all
cargo clippy -p procraft-engine --all-targets -- -D warnings
cargo test -p procraft-engine
git add crates/engine/src/raster.rs crates/engine/src/lib.rs
git commit -m "feat(engine): add tiled copy-on-write Raster storage"
```

---

### Task 2: Engine document on `Raster` (layers, undo, bulk ops)

**Files:**
- Modify: `crates/engine/src/document.rs` (`Layer` 261-292, `Document::blank` 313-333, `UndoEntry` 424-444, `snapshot`/`restore`/`undo`/`redo` 792-865, `composite_pixel` 867-916, and the 30 functions listed below)
- Test: `crates/engine/src/document.rs` (unit tests at the bottom)

**Interfaces:**
- Consumes: `Raster`, `TILE` from task 1.
- Produces: `Layer.pixels: Raster`, `Layer.mask: Option<Raster>`, `Document::layer_alloc_bytes() -> usize`, `Document::tiles_x()/tiles_y()`, `UndoEntry.pixels: Raster`, unchanged public API for everything else (`dab`, `flatten`, `flatten_region`, `undo`, `save_project`, …).

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` in `document.rs`:

```rust
#[test]
fn blank_layers_allocate_no_tiles() {
    let mut doc = Document::new(2048, 2048);
    for i in 0..100 {
        doc.add_layer(format!("L{i}"));
    }
    assert_eq!(doc.layers.len(), 101);
    assert_eq!(doc.layer_alloc_bytes(), 0);
}

#[test]
fn undo_checkpoint_costs_touched_tiles_only() {
    let mut doc = Document::new(600, 600); // 3x3 tiles
    let brush = Brush::default();
    doc.dab(&brush, Color([255, 0, 0, 255]), 10.0, 10.0);
    doc.checkpoint();
    doc.dab(&brush, Color([0, 0, 255, 255]), 550.0, 550.0);
    assert!(
        doc.undo_bytes <= 2 * 256 * 1024,
        "one checkpoint of a two-tile layer: {} bytes",
        doc.undo_bytes
    );
    assert!(doc.undo());
    assert_eq!(doc.layers[0].pixels.get4(((550 * 600 + 550) * 4) as usize), [0; 4]);
    assert_eq!(doc.layers[0].pixels.get(((10 * 600 + 10) * 4 + 3) as usize), 255);
}

#[test]
fn duplicate_layer_shares_tiles_until_painted() {
    let mut doc = Document::new(600, 600);
    doc.dab(&Brush::default(), Color([0, 255, 0, 255]), 12.0, 12.0);
    doc.duplicate_layer(0);
    // Per-layer accounting counts the shared slot twice; the process holds
    // one tile because the Arc is shared.
    assert_eq!(doc.layer_alloc_bytes(), 2 * 256 * 1024);
    doc.active = 1;
    doc.dab(&Brush::default(), Color([1, 2, 3, 255]), 300.0, 300.0);
    assert_eq!(doc.layer_alloc_bytes(), 3 * 256 * 1024);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p procraft-engine blank_layers_allocate` and `cargo test -p procraft-engine undo_checkpoint_costs`
Expected: compile errors — `layer_alloc_bytes` missing, `Raster` not the field type.

- [ ] **Step 3: Convert `Layer`, undo and the documented bulk paths**

3a. Field types and constructors:

```rust
pub struct Layer {
    ...
    pub pixels: Raster,
    pub mask: Option<Raster>,
    ...
}

impl Layer {
    pub fn blank(name: String, w: u32, h: u32) -> Self {
        Self { ..., pixels: Raster::rgba(w, h), mask: None, ... }
    }
}
```

Add `use crate::raster::{Raster, TILE};` at the top of `document.rs`.

3b. Undo (phase 3 lands here because it is a two-line change):

```rust
struct UndoEntry {
    layer: usize,
    pixels: Raster,
    mask: Option<Raster>,
    ox: i32,
    oy: i32,
    text: Option<crate::text::TextBlock>,
}

impl UndoEntry {
    fn byte_size(&self) -> usize {
        self.pixels.alloc_bytes()
            + self.mask.as_ref().map_or(0, Raster::alloc_bytes)
            + self.text.as_ref().map_or(0, |t| {
                t.text.len() + t.style.font_bytes.as_ref().map_or(0, Vec::len)
            })
    }
}
```

`snapshot()` keeps `pixels: layer.pixels.clone()` (now a pointer copy). `restore_last_checkpoint` keeps `layer.pixels.clone_from(&entry.pixels)`. `undo`/`redo` keep `layer.pixels = entry.pixels;`.

3c. Add the accounting helpers next to `revision()`:

```rust
/// Tiles held by live layers (not history): what the memory readout shows.
pub fn layer_alloc_bytes(&self) -> usize {
    self.layers
        .iter()
        .map(|l| l.pixels.alloc_bytes() + l.mask.as_ref().map_or(0, Raster::alloc_bytes))
        .sum()
}

pub fn tiles_x(&self) -> u32 {
    self.w.div_ceil(TILE)
}

pub fn tiles_y(&self) -> u32 {
    self.h.div_ceil(TILE)
}
```

3d. Convert the 30 functions with `.pixels`/`.mask` sites using exactly two rules:

**Rule A — cold paths use the dense bridge.** For functions that already treat a layer as one dense buffer (geometry, filters, warp, merge, file I/O, thumbnails), read `let mut d = layer.pixels.to_dense();`, keep the existing math on `d`, then write back with `layer.pixels.replace_dense(&d)` (or `Raster::from_dense(w, h, 4, &d)` when the geometry changes). Masks: `to_dense`/`Raster::mask(w, h)`.

Functions on Rule A: `absorb_offset` (1045), `rehome_canvas` (1126), `affine_layer` (1247), `content_bounds` (1289), `distort_layer` (1372), `warp_layer` (1427), `warp_layer_tris` (1504), `flip_layer_horizontal` (1574), `flip_layer_vertical` (1606), `merge_down` (1676), `add_mask`/`clear_mask` (1786/1798), `mask_view` (1807), `baked_layer` (1831), `select_opaque` (2293), `adjust_tone` (2457), `blur_layer` (2494), `sharpen_layer` (2532), `apply_lut` (2570), `adjust_hsl` (2614), `color_balance` (2664), `colorize` (2706), `liquify_push` (2816), `load_background` (2951), `save_project` (3071, dense until task 9), `open_project` (3154, wrap the decompressed payload in `Raster::from_dense(w, h, 4, &pixels)` and masks in `Raster::from_dense(w, h, 1, &mask)`).

Example, `absorb_offset`:

```rust
let (w, h) = (self.w, self.h);
let mut d = self.layers[index].pixels.to_dense();
Self::slide_buffer(&mut d, w, h, 4, sx, sy);
self.layers[index].pixels.replace_dense(&d);
if let Some(m) = self.layers[index].mask.as_mut() {
    let mut md = m.to_dense();
    Self::slide_buffer(&mut md, w, h, 1, sx, sy);
    m.replace_dense(&md);
}
```

Example, `rehome_canvas` layer loop:

```rust
for layer in self.layers.iter_mut() {
    let (sx, sy) = (-dx - layer.ox, -dy - layer.oy);
    let dense = layer.pixels.to_dense();
    let next = carry(&dense, old_w, old_h, new_w, new_h, 4, sx, sy);
    layer.pixels = Raster::from_dense(new_w, new_h, 4, &next);
    if let Some(m) = layer.mask.take() {
        let md = m.to_dense();
        layer.mask = Some(Raster::from_dense(new_w, new_h, 1, &carry(
            &md, old_w, old_h, new_w, new_h, 1, sx, sy,
        )));
    }
    layer.ox = 0;
    layer.oy = 0;
    ...
}
```

**Rule B — per-pixel paths index through `get`/`set`.** `composite_pixel` (867) becomes:

```rust
let Some(o) = layer.local_offset(x, y, self.w, self.h) else { continue };
if o + 4 > layer.pixels.len() { continue; }
let px = layer.pixels.get4(o);
let mut a = px[3] as f32 / 255.0 * layer.opacity.clamp(0.0, 1.0) * gop;
if let Some(m) = layer.mask.as_ref() {
    a *= m.get(o / 4) as f32 / 255.0;
}
...
r = blend.apply(px[0] as f32, r) * a + r * (1.0 - a);
```

`Layer::clear` (342) becomes `self.pixels.fill(0); self.mask = None;` (match the existing body — if it only zeroes pixels, keep that).

3e. Update the existing unit tests mechanically:

| Old pattern | New pattern |
|---|---|
| `doc.layers[i].pixels = vec![..]` | `doc.layers[i].pixels = Raster::from_dense(w, h, 4, &[..])` |
| `doc.layers[i].mask = Some(vec![..])` | `Some(Raster::from_dense(w, h, 1, &[..]))` |
| `doc.layers[i].pixels[o]` (read) | `doc.layers[i].pixels.get(o)` |
| `doc.layers[i].pixels[o] = v` | `doc.layers[i].pixels.set(o, v)` |
| `pixels.iter().any(..)`, `pixels.chunks(4)` | `pixels.to_dense().iter().any(..)` |
| `pixels.truncate(n)` corruption tests | build the wrong geometry instead (`Raster::new(1, 1, 4)`) |
| `assert_eq!(pixels, vec![..])` | unchanged (PartialEq impl) |

- [ ] **Step 4: Run the engine suite until green**

Run: `cargo test -p procraft-engine`
Expected: all tests pass, including the three new ones. Iterate on compile errors — every remaining `&layer.pixels` / `pixels[..]` slice site is a Rule A candidate.

- [ ] **Step 5: Verify the app still compiles against the new field types is NOT expected yet; commit engine-only**

The app breaks in this task (field type changed). Commit the engine step so task 5 has a base:

```bash
cargo fmt --all
cargo clippy -p procraft-engine --all-targets -- -D warnings
cargo test -p procraft-engine
git add crates/engine/src/document.rs
git commit -m "refactor(engine): store layers as tiled rasters"
```

---

### Task 3: Engine brush paths on tiles

**Files:**
- Modify: `crates/engine/src/brush.rs` (129 `.pixels` sites: dabs 554-1088, smudge 1192-1395, shapes/fill 1952-2125, flood fill and liquify helpers)
- Test: `crates/engine/src/brush.rs` tests

**Interfaces:**
- Consumes: `Raster::get`/`set`/`get4`/`set4`/`tile_mut`/`fill` from task 1; `Layer.pixels: Raster` from task 2.
- Produces: unchanged public brush API (`dab`, `stroke_to`, `smudge_to`, `fill_rect`, `flood_fill`, …).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn dab_paints_across_a_tile_seam() {
    let mut doc = Document::new(600, 100);
    let brush = Brush {
        size: 24.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.dab(&brush, Color([255, 0, 0, 255]), 256.0, 50.0);
    let px = &doc.layers[0].pixels;
    assert_eq!(px.get4(((50 * 600 + 250) * 4) as usize)[3], 255);
    assert_eq!(px.get4(((50 * 600 + 262) * 4) as usize)[3], 255);
    assert_eq!(px.alloc_bytes(), 2 * 256 * 1024, "both tiles exist");
}

#[test]
fn eraser_clears_but_keeps_scene_undoable() {
    let mut doc = Document::new(300, 300);
    let brush = Brush {
        size: 40.0,
        softness: 0.0,
        ..Default::default()
    };
    doc.dab(&brush, Color([10, 20, 30, 255]), 150.0, 150.0);
    doc.checkpoint();
    let eraser = Brush { eraser: true, ..brush };
    doc.dab(&eraser, Color([0, 0, 0, 255]), 150.0, 150.0);
    assert_eq!(doc.layers[0].pixels.get4(((150 * 300 + 150) * 4) as usize), [0; 4]);
    assert!(doc.undo());
    assert_eq!(doc.layers[0].pixels.get(((150 * 300 + 150) * 4 + 3) as usize), 255);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p procraft-engine dab_paints_across_a_tile_seam`
Expected: compile error on `get4`/`alloc_bytes` only if task 2 did not land; otherwise it fails because `dab` still indexes a dense buffer.

- [ ] **Step 3: Convert `composite_dab` and the per-pixel loops**

`composite_dab` signature and body:

```rust
fn composite_dab(
    pixels: &mut crate::raster::Raster,
    o: usize,
    color: [u8; 4],
    src_a: f32,
    mode: super::document::BlendMode,
) {
    let dst = pixels.get4(o);
    let dst_a = dst[3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return;
    }
    let mut out = [0u8; 4];
    for ch in 0..3 {
        let b = mode.apply(color[ch] as f32, dst[ch] as f32);
        out[ch] = ((b * src_a + dst[ch] as f32 * dst_a * (1.0 - src_a)) / out_a).round() as u8;
    }
    out[3] = (out_a * 255.0).round() as u8;
    pixels.set4(o, out);
}
```

Apply these conversions throughout `brush.rs`:

- Read-modify-write per pixel (`dab_one_resolved` 853-960, `dab_stamp_resolved` 964-1088, `put_one` 1952-2005, `text.rs`-style blends): `let mut px = layer.pixels.get4(o); ...; layer.pixels.set4(o, px);`
- Bounds guard: `if o + 4 > layer.pixels.len() { continue; }`
- Stamp/tip reads (`layer.pixels.get(o..o+4)`): `let px = layer.pixels.get4(o);`
- Smudge swap loop (1381-1385):

```rust
let mut px = layer.pixels.get4(o);
for (d, f) in px.iter_mut().zip(finger.iter()) {
    *d = (*d as f32 * (1.0 - t) + f * t).round() as u8;
}
layer.pixels.set4(o, px);
```

- Mask painting (`layer.mask.as_mut()` / `as_deref()`): masks are `Raster` too — `layer.mask.as_ref().map(|m| m.get(i))` for reads, `m.set(i, v)` for writes, `m.fill(255)` where a mask is created.
- Flood fill, liquify push, and any function that walks a whole layer: Rule A dense bridge from task 2 (`to_dense` → existing code → `replace_dense`).

- [ ] **Step 4: Run the engine suite**

Run: `cargo test -p procraft-engine`
Expected: all pass, including the two new tests. Brush golden tests (`tests/brush_corpus.rs`) must be byte-identical — they are the correctness gate for this task.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p procraft-engine --all-targets -- -D warnings
git add crates/engine/src/brush.rs
git commit -m "refactor(engine): paint through tile accessors"
```

---

### Task 4: Engine text, PSD, animation, probrush on tiles

**Files:**
- Modify: `crates/engine/src/text.rs` (26 sites), `crates/engine/src/psd.rs` (14), `crates/engine/src/animation.rs` (11), `crates/engine/src/probrush.rs` (6)
- Test: the unit tests inside each file

**Interfaces:**
- Consumes: `Raster` API from task 1.
- Produces: unchanged public behaviour (`TextBlock` rerender, `save_psd`/`open_psd`, `rasterize_frame`, GIF I/O, `probrush` tips).

- [ ] **Step 1: Run the four suites to see the breakage**

Run: `cargo test -p procraft-engine text::` / `psd::` / `animation::` / `probrush::`
Expected: compile errors (dense `Vec<u8>` assumptions).

- [ ] **Step 2: Apply the two conversion rules**

- `text.rs`: per-pixel coverage writes → `get4`/`set4` (same pattern as task 3); full clears → `pixels.fill(0)`; layer creation → `Raster::rgba(w, h)`.
- `psd.rs`: the size guard becomes a geometry check and the export/import path uses the dense bridge:

```rust
for layer in &doc.layers {
    if layer.pixels.w() != w || layer.pixels.h() != h {
        return Err("layer has wrong pixel size".to_string());
    }
    if let Some(m) = layer.mask.as_ref() {
        if m.w() != w || m.h() != h {
            return Err("layer mask has wrong size".to_string());
        }
    }
}
```

The corrupt-size test changes from `doc.layers[0].pixels.truncate(3)` to:

```rust
doc.layers[0].pixels = procraft_engine::Raster::new(1, 1, 4);
```

- `animation.rs`: `frame_pixel` and `onion_ghost` read `layer.pixels.get4(o)` / `get(o + ch)`; keep everything else identical.
- `probrush.rs`: dense bridge around its buffer work.

- [ ] **Step 3: Run all engine tests**

Run: `cargo test -p procraft-engine`
Expected: all pass. PSD round-trip tests are the gate for this task.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
cargo clippy -p procraft-engine --all-targets -- -D warnings
cargo test -p procraft-engine
git add crates/engine/src/text.rs crates/engine/src/psd.rs crates/engine/src/animation.rs crates/engine/src/probrush.rs
git commit -m "refactor(engine): text, PSD and animation on tiled rasters"
```

---

### Task 5: App on `Raster`

**Files:**
- Modify: `apps/procraft/src/lib.rs` (106 `.pixels` sites: fill preview 360-400, xform base 7800-7900, transform session 8680-8830, thumbnails 2109/6344, refresh_texture 1894-1960, and the unit tests from 13360 onward)
- Test: `apps/procraft/src/lib.rs` tests

**Interfaces:**
- Consumes: `Raster`, `Raster::fingerprint`, `Raster::to_dense`, `Raster::from_dense`, `Document::flatten` (unchanged signature).
- Produces: unchanged app behaviour; `pixels_hash` keeps its `&[u8]` signature for selection masks.

- [ ] **Step 1: Apply the conversions**

- `FillPreview.pixels: Raster`, `XformBase.pixels: Raster`, `TformBase.pixels: Raster` — field types only; `.clone()` stays a pointer copy, `clone_from` stays.
- Preview guards: `pixels_hash(&l.pixels)` → `l.pixels.fingerprint()` (lines 390, 1831, 5913, 5937, 5988, 11413, 11459, 11489). Keep `pixels_hash` for `&[u8]` masks.
- `ColorImage` builders (2109, 6344, and anywhere `from_rgba_unmultiplied(.., &layer.pixels)`) take `&layer.pixels.to_dense()`.
- Flips in the transform session (8811-8823) use a local helper instead of `flip_buf_*` on a slice:

```rust
fn flip_raster(r: &mut procraft_engine::Raster, horizontal: bool) {
    let (w, h, bpp) = (r.w() as usize, r.h() as usize, r.bpp());
    let mut d = r.to_dense();
    if horizontal {
        flip_buf_h(&mut d, w, h, bpp);
    } else {
        flip_buf_v(&mut d, w, h, bpp);
    }
    r.replace_dense(&d);
}
```

- Tests: `l.pixels = vec![0u8; 8 * 8 * 4]` → `l.pixels = procraft_engine::Raster::from_dense(8, 8, 4, &[0u8; 8 * 8 * 4])`; `for p in l.pixels.chunks_mut(4)` → build a dense vec, mutate, then `replace_dense`; `pixels_hash(&l.pixels)` in tests → `l.pixels.fingerprint()`.

- [ ] **Step 2: Compile and run the app suite**

Run: `cargo test -p procraft --lib`
Expected: all pass. Then run the app once:

```bash
PROCRAFT_FORCE_SHELL=mobile cargo run --release -p procraft
```

Paint a stroke, undo it, open Gallery, and confirm the canvas updates — this is the end-to-end check that the dense bridge still feeds `refresh_texture`.

- [ ] **Step 3: Screenshot check against the polish baseline**

Run the capture harness with `PROCRAFT_FORCE_SHELL=mobile` set in the calling shell, compare canvas, layers panel and Gallery against `dist/mobile-shell/2026-10-29/`. Expect pixel-identical chrome; canvas content identical.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
cargo clippy -p procraft --all-targets -- -D warnings
cargo test -p procraft --lib
git add apps/procraft/src/lib.rs
git commit -m "refactor(ui): app on tiled rasters"
```

---

### Task 6: Composite plan + tile compositor in the engine

**Files:**
- Create: `crates/engine/src/composite.rs`
- Modify: `crates/engine/src/lib.rs` (module + re-export), `crates/engine/src/document.rs` (`composite_pixel` stays; `flatten`/`flatten_region` delegate)
- Test: `crates/engine/src/composite.rs`

**Interfaces:**
- Consumes: `Raster`, `Document::layers`, `Document::background`, `BlendMode`.
- Produces:
  - `pub struct StackPlan` (opaque; `Clone + Debug`)
  - `Document::stack_plan(&self) -> StackPlan`
  - `Document::composite_tile(&self, plan: &StackPlan, tx: u32, ty: u32) -> Vec<u8>` (always `TILE*TILE*4` bytes; bytes outside the canvas are 0)
  - `flatten()` / `flatten_region()` rewritten on top of them (same results, tile-driven).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn tile_composite_matches_per_pixel_reference() {
    let mut doc = doc_with_everything(); // 300x200: blend modes, mask, clip, group, offsets
    let plan = doc.stack_plan();
    for ty in 0..doc.tiles_y() {
        for tx in 0..doc.tiles_x() {
            let tile = doc.composite_tile(&plan, tx, ty);
            assert_eq!(tile.len(), 256 * 256 * 4);
            for row in 0..256u32 {
                for col in 0..256u32 {
                    let (x, y) = (tx * 256 + col, ty * 256 + row);
                    let o = ((row * 256 + col) * 4) as usize;
                    let want = if x < doc.w && y < doc.h {
                        doc.composite_pixel(x, y)
                    } else {
                        [0, 0, 0, 0]
                    };
                    assert_eq!(&tile[o..o + 4], &want, "pixel {x},{y}");
                }
            }
        }
    }
}

#[test]
fn flatten_uses_tiles_and_matches_pixel_reference() {
    let doc = doc_with_everything();
    let out = doc.flatten();
    for y in 0..doc.h {
        for x in 0..doc.w {
            let o = ((y * doc.w + x) * 4) as usize;
            assert_eq!(&out[o..o + 4], &doc.composite_pixel(x, y));
        }
    }
}
```

`doc_with_everything()` helper (write it in the test module):

```rust
fn doc_with_everything() -> Document {
    let mut doc = Document::new(300, 200);
    doc.background = [250, 240, 230, 255];
    doc.layers[0].pixels.replace_dense(&[200u8; 300 * 200 * 4]);
    doc.layers[0].pixels.fill(0);
    for y in 0..120u32 {
        for x in 0..180u32 {
            doc.layers[0].pixels.set4(((y * 300 + x) * 4) as usize, [10, 60, 200, 255]);
        }
    }
    doc.add_layer("mult".into());
    doc.layers[1].blend = BlendMode::Multiply;
    doc.layers[1].pixels.fill(0);
    for y in 0..200u32 {
        for x in 0..300u32 {
            if (x + y) % 3 == 0 {
                doc.layers[1].pixels.set4(((y * 300 + x) * 4) as usize, [255, 40, 40, 180]);
            }
        }
    }
    doc.add_layer("masked".into());
    doc.layers[2].pixels.fill(0);
    doc.layers[2].ox = 25;
    doc.layers[2].oy = -10;
    doc.layers[2].pixels.set4(0, [0, 255, 0, 255]);
    doc.layers[2].mask = Some(Raster::mask(300, 200));
    doc.layers[2].mask.as_mut().unwrap().fill(128);
    doc.add_layer("clipped".into());
    doc.layers[3].clip = true;
    doc.layers[3].pixels.fill(0);
    doc.layers[3].pixels.set4(4, [0, 0, 0, 200]);
    doc
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p procraft-engine composite::`
Expected: compile error — `stack_plan`/`composite_tile` missing.

- [ ] **Step 3: Implement `composite.rs`**

```rust
//! Tile compositing: the engine side of lazy display.
//!
//! `StackPlan` freezes the per-layer blend inputs (visibility, effective
//! opacity, clip, offsets) once per document revision; `composite_tile`
//! then blends one 256² tile by reading each layer's tile directly.

use crate::document::{BlendMode, Document};
use crate::raster::TILE;

#[derive(Clone, Debug)]
struct PlannedLayer {
    index: usize,
    ox: i32,
    oy: i32,
    opacity: f32,
    blend: BlendMode,
    clip: bool,
}

#[derive(Clone, Debug, Default)]
pub struct StackPlan {
    layers: Vec<PlannedLayer>,
}

impl Document {
    /// Per-layer blend inputs for the current visibility and groups.
    pub fn stack_plan(&self) -> StackPlan {
        let mut layers = Vec::new();
        for index in 0..self.layers.len() {
            if !self.is_layer_shown(index) {
                continue;
            }
            let layer = &self.layers[index];
            let opacity = layer.opacity.clamp(0.0, 1.0) * self.group_opacity(index);
            if opacity <= 0.0 {
                continue;
            }
            layers.push(PlannedLayer {
                index,
                ox: layer.ox,
                oy: layer.oy,
                opacity,
                blend: layer.blend,
                clip: layer.clip,
            });
        }
        StackPlan { layers }
    }

    /// One tile of the composite, `TILE * TILE * 4` bytes, row-major.
    /// Pixels outside the canvas are zero.
    pub fn composite_tile(&self, plan: &StackPlan, tx: u32, ty: u32) -> Vec<u8> {
        let mut out = vec![0u8; (TILE * TILE * 4) as usize];
        let x0 = tx * TILE;
        let y0 = ty * TILE;
        let tw = self.w.saturating_sub(x0).min(TILE);
        let th = self.h.saturating_sub(y0).min(TILE);
        let (br, bg_, bb) = (
            self.background[0] as f32,
            self.background[1] as f32,
            self.background[2] as f32,
        );
        for row in 0..th {
            let y = y0 + row;
            for col in 0..tw {
                let x = x0 + col;
                let (mut r, mut g, mut b) = (br, bg_, bb);
                let mut below_a = 0.0f32;
                for pl in &plan.layers {
                    let lx = x as i32 - pl.ox;
                    let ly = y as i32 - pl.oy;
                    if lx < 0 || ly < 0 || lx >= self.w as i32 || ly >= self.h as i32 {
                        continue;
                    }
                    let layer = &self.layers[pl.index];
                    let (lx, ly) = (lx as u32, ly as u32);
                    let Some(tile) = layer.pixels.tile(lx / TILE, ly / TILE) else {
                        continue;
                    };
                    let off =
                        ((ly % TILE) * TILE + (lx % TILE)) as usize * 4;
                    let px = [tile[off], tile[off + 1], tile[off + 2], tile[off + 3]];
                    let mut a = px[3] as f32 / 255.0 * pl.opacity;
                    if let Some(m) = layer.mask.as_ref() {
                        let moff = ((ly % TILE) * TILE + (lx % TILE)) as usize;
                        a *= m.get(moff) as f32 / 255.0;
                    }
                    if pl.clip {
                        a *= below_a;
                    }
                    if a <= 0.0 {
                        continue;
                    }
                    r = pl.blend.apply(px[0] as f32, r) * a + r * (1.0 - a);
                    g = pl.blend.apply(px[1] as f32, g) * a + g * (1.0 - a);
                    b = pl.blend.apply(px[2] as f32, b) * a + b * (1.0 - a);
                    below_a = a + below_a * (1.0 - a);
                }
                let o = ((row * TILE + col) * 4) as usize;
                out[o] = r.round().clamp(0.0, 255.0) as u8;
                out[o + 1] = g.round().clamp(0.0, 255.0) as u8;
                out[o + 2] = b.round().clamp(0.0, 255.0) as u8;
                out[o + 3] = 255;
            }
        }
        out
    }
}
```

Note `m.get(moff)` is a dense-index call on the mask raster for this pixel's local position — correct because the mask shares the layer's geometry.

Then rewrite `flatten` and `flatten_region` in `document.rs`:

```rust
pub fn flatten(&self) -> Vec<u8> {
    let plan = self.stack_plan();
    let mut out = vec![0u8; (self.w * self.h * 4) as usize];
    for ty in 0..self.tiles_y() {
        for tx in 0..self.tiles_x() {
            let tile = self.composite_tile(&plan, tx, ty);
            let x0 = (tx * TILE) as usize;
            let y0 = (ty * TILE) as usize;
            let tw = (self.w as usize - x0).min(TILE as usize);
            let th = (self.h as usize - y0).min(TILE as usize);
            for row in 0..th {
                let src = row * TILE as usize * 4;
                let dst = ((y0 + row) * self.w as usize + x0) * 4;
                out[dst..dst + tw * 4].copy_from_slice(&tile[src..src + tw * 4]);
            }
        }
    }
    out
}

pub fn flatten_region(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<u8> {
    let x1 = x1.min(self.w.saturating_sub(1));
    let y1 = y1.min(self.h.saturating_sub(1));
    let (x0, y0) = (x0.min(x1), y0.min(y1));
    let plan = self.stack_plan();
    let mut out = Vec::with_capacity(((x1 - x0 + 1) * (y1 - y0 + 1) * 4) as usize);
    let (tx0, tx1) = (x0 / TILE, x1 / TILE);
    for ty in y0 / TILE..=y1 / TILE {
        // One composite per tile in this tile row; rows are emitted in
        // canvas row order so the caller's row-major buffer stays valid.
        let row_tiles: Vec<Vec<u8>> = (tx0..=tx1)
            .map(|tx| self.composite_tile(&plan, tx, ty))
            .collect();
        for y in y0.max(ty * TILE)..=y1.min((ty + 1) * TILE - 1) {
            let row_in_tile = (y % TILE) as usize * TILE as usize;
            for (i, tx) in (tx0..=tx1).enumerate() {
                let cx0 = (tx * TILE).max(x0);
                let cx1 = ((tx + 1) * TILE - 1).min(x1);
                for x in cx0..=cx1 {
                    let o = (row_in_tile + (x % TILE) as usize) * 4;
                    out.extend_from_slice(&row_tiles[i][o..o + 4]);
                }
            }
        }
    }
    out
}
```

Add `mod composite;` and `pub use composite::StackPlan;` to `crates/engine/src/lib.rs`. `is_layer_shown` becomes `pub(crate)`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p procraft-engine`
Expected: the two new tests pass and every existing flatten/merge/PSD test stays green (byte-identical output).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p procraft-engine --all-targets -- -D warnings
git add crates/engine/src/composite.rs crates/engine/src/document.rs crates/engine/src/lib.rs
git commit -m "feat(engine): composite stack plan and tile compositor"
```

---

### Task 7: App composite cache + per-tile texture uploads

**Files:**
- Create: `apps/procraft/src/canvas.rs`
- Modify: `apps/procraft/src/lib.rs` (`ProCraftApp` fields ~856, `refresh_texture` 1894-1960, `mod` list, every `self.texture = None` invalidation stays valid)
- Test: `apps/procraft/src/canvas.rs`

**Interfaces:**
- Consumes: `Document::stack_plan`, `Document::composite_tile`, `TILE`, `Document::take_dirty`.
- Produces:
  - `pub struct CompositeCache` with `new(budget: usize)`, `tile(&mut self, doc: &Document, plan: &StackPlan, tx, ty) -> &[u8]`, `invalidate(&mut self)`, `bytes()`
  - `pub const CACHE_MOBILE: usize = 96 * 1024 * 1024; pub const CACHE_DESKTOP: usize = 320 * 1024 * 1024;`
  - `pub fn tile_image(doc: &Document, tile: &[u8], tx: u32, ty: u32) -> (Vec<u8>, usize, usize)` — crops to the canvas edge, returns `(rgba, w, h)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn cache_evicts_oldest_tiles_over_budget() {
    let mut doc = Document::new(1024, 1024);
    doc.layers[0].pixels.set4(0, [7, 8, 9, 255]);
    let plan = doc.stack_plan();
    let mut cache = CompositeCache::new(2 * 256 * 1024);
    for (tx, ty) in [(0, 0), (1, 0), (2, 0)] {
        cache.tile(&doc, &plan, tx, ty);
    }
    assert!(cache.bytes() <= 2 * 256 * 1024);
    assert_eq!(cache.bytes(), 2 * 256 * 1024);
}

#[test]
fn tile_image_crops_at_the_canvas_edge() {
    let doc = Document::new(300, 5);
    let plan = doc.stack_plan();
    let mut cache = CompositeCache::new(256 * 1024);
    let tile = cache.tile(&doc, &plan, 1, 0);
    let (rgba, w, h) = tile_image(&doc, &tile.to_vec(), 1, 0);
    assert_eq!((w, h), (44, 5));
    assert_eq!(rgba.len(), 44 * 5 * 4);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p procraft --lib canvas::`
Expected: compile error — module missing.

- [ ] **Step 3: Implement `canvas.rs`**

```rust
//! Canvas display: composite tiles behind an LRU cache.

use procraft_engine::{Document, StackPlan, TILE};

pub const CACHE_MOBILE: usize = 96 * 1024 * 1024;
pub const CACHE_DESKTOP: usize = 320 * 1024 * 1024;

pub struct CompositeCache {
    tiles: std::collections::HashMap<(u32, u32), Vec<u8>>,
    order: std::collections::VecDeque<(u32, u32)>,
    bytes: usize,
    budget: usize,
}

impl CompositeCache {
    pub fn new(budget: usize) -> Self {
        Self {
            tiles: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
            bytes: 0,
            budget: budget.max(TILE as usize * TILE as usize * 4),
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn invalidate(&mut self) {
        self.tiles.clear();
        self.order.clear();
        self.bytes = 0;
    }

    pub fn tile(&mut self, doc: &Document, plan: &StackPlan, tx: u32, ty: u32) -> &[u8] {
        if !self.tiles.contains_key(&(tx, ty)) {
            let bytes = doc.composite_tile(plan, tx, ty);
            self.bytes += bytes.len();
            self.tiles.insert((tx, ty), bytes);
            self.order.push_back((tx, ty));
            while self.bytes > self.budget && self.order.len() > 1 {
                let key = self.order.pop_front().expect("non-empty");
                if let Some(old) = self.tiles.remove(&key) {
                    self.bytes -= old.len();
                }
            }
        }
        self.tiles.get(&(tx, ty)).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Crop one composite tile to the canvas edge for `set_partial`.
pub fn tile_image(doc: &Document, tile: &[u8], tx: u32, ty: u32) -> (Vec<u8>, usize, usize) {
    let x0 = tx * TILE;
    let y0 = ty * TILE;
    if x0 >= doc.w || y0 >= doc.h {
        return (Vec::new(), 0, 0);
    }
    let w = (doc.w - x0).min(TILE) as usize;
    let h = (doc.h - y0).min(TILE) as usize;
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let src = (row * TILE as usize) * 4;
        out.extend_from_slice(&tile[src..src + w * 4]);
    }
    (out, w, h)
}

#[cfg(test)]
mod tests { /* the two tests above */ }
```

- [ ] **Step 4: Wire it into `refresh_texture`**

Add to `ProCraftApp`: `composite: CompositeCache,` initialised in the constructor with

```rust
composite: CompositeCache::new(if cfg!(any(target_os = "android", target_os = "ios")) {
    canvas::CACHE_MOBILE
} else {
    canvas::CACHE_DESKTOP
}),
```

Replace the dense branches of `refresh_texture` (`apps/procraft/src/lib.rs:1935-1953`) with:

```rust
self.anim_shown = None;
let (tw, th) = (self.doc.tiles_x(), self.doc.tiles_y());
if self.texture.is_none() {
    let plan = self.doc.stack_plan();
    let mut img = Vec::with_capacity((self.doc.w * self.doc.h * 4) as usize);
    for ty in 0..th {
        for tx in 0..tw {
            let (rgba, _, _) = canvas::tile_image(
                &self.doc,
                self.composite.tile(&self.doc, &plan, tx, ty),
                tx,
                ty,
            );
            img.extend_from_slice(&rgba);
        }
    }
    let image = ColorImage::from_rgba_unmultiplied(
        [self.doc.w as usize, self.doc.h as usize],
        &img,
    );
    self.texture = Some(ctx.load_texture("canvas", image, TextureOptions::LINEAR));
    self.doc.take_dirty();
    return;
}
if let Some((x0, y0, x1, y1)) = self.doc.take_dirty() {
    let plan = self.doc.stack_plan();
    for ty in y0 / TILE..=y1 / TILE {
        for tx in x0 / TILE..=x1 / TILE {
            let tile = self.composite.tile(&self.doc, &plan, tx, ty).to_vec();
            let (rgba, w, h) = canvas::tile_image(&self.doc, &tile, tx, ty);
            if w == 0 || h == 0 {
                continue;
            }
            let image = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            if let Some(handle) = &mut self.texture {
                handle.set_partial(
                    [(tx * TILE) as usize, (ty * TILE) as usize],
                    image,
                    TextureOptions::LINEAR,
                );
            }
        }
    }
}
```

`TILE` comes from `procraft_engine::TILE`. Every existing `self.texture = None` invalidation must also clear the cache (`self.composite.invalidate()`), otherwise a re-upload composites from stale tiles — add the call at each site (lines 2001, 2019, 3820, 3840, 4696, 4807, 6241, 6296, 9990).

Note: the full-upload loop above is row-major over tiles; canvas dimensions not divisible by 256 leave the last column/row cropped by `tile_image`, which keeps the dense buffer exactly `w*h*4`.

- [ ] **Step 5: Verify in the running app and commit**

```bash
cargo fmt --all
cargo clippy -p procraft --all-targets -- -D warnings
cargo test -p procraft --lib
cargo run --release -p procraft
```

Paint, undo, toggle layer visibility, scroll the layers panel: the canvas must update on every change and never show a stale tile (a stale tile means a missing `take_dirty` tile or a cache not invalidated on structural edits).

```bash
git add apps/procraft/src/canvas.rs apps/procraft/src/lib.rs
git commit -m "feat(ui): composite cache and per-tile canvas uploads"
```

---

### Task 8: 8K cap, presets, memory readout

**Files:**
- Modify: `crates/engine/src/document.rs` (`MAX_CANVAS_SIDE`, doc comments, validation tests), `apps/procraft/src/lib.rs` (Gallery new-canvas row ~9727, canvas-size dialog ~2564, layers panel footer)
- Test: engine + app unit tests

**Interfaces:**
- Consumes: `Document::layer_alloc_bytes()` (task 2).
- Produces: `MAX_CANVAS_SIDE = 8192`; Gallery 4K/8K preset buttons; a layers-panel allocation label.

- [ ] **Step 1: Write the failing tests**

In `document.rs`:

```rust
#[test]
fn canvas_cap_allows_8k_but_not_more() {
    assert_eq!(MAX_CANVAS_SIDE, 8192);
    assert!(Document::new(8192, 8192).rehome_canvas(8192, 8192, 0, 0).is_ok());
    assert!(Document::new(64, 64).rehome_canvas(8193, 64, 0, 0).is_err());
}
```

In `apps/procraft/src/lib.rs` tests:

```rust
#[test]
fn new_canvas_clamps_at_the_engine_cap() {
    assert_eq!(MAX_CANVAS_SIDE, 8192);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p procraft-engine canvas_cap_allows_8k`
Expected: FAIL — `left: 4096, right: 8192`.

- [ ] **Step 3: Raise the cap and add the UI**

`document.rs`:

```rust
/// Canvas side cap for crop/resize: bounds reallocations (tiles are
/// allocated on demand, so an 8192² layer that is never painted costs
/// nothing) and matches the 8K export target.
pub const MAX_CANVAS_SIDE: u32 = 8192;
```

Update the two error-message assertions in the existing tests (`want 1-4096 each` → `want 1-8192 each`), and the doc comment on `import_image` that quotes 4096.

Gallery new-canvas row: add a preset strip under the W/H row:

```rust
ui.horizontal_wrapped(|ui| {
    for (label, w, h) in [
        ("1080p", 1920u32, 1080u32),
        ("4K", 4096, 4096),
        ("8K", 8192, 8192),
    ] {
        if ui.button(label).clicked() {
            self.gallery_new_w = w;
            self.gallery_new_h = h;
        }
    }
    ui.label(format!(
        "8K: {} tiles/layer, painted tiles only",
        (8192u32.div_ceil(procraft_engine::TILE)).pow(2)
    ));
});
```

Layers panel footer (next to the layer count):

```rust
let bytes = self.doc.layer_alloc_bytes();
ui.label(format!(
    "{} layers · {}",
    self.doc.layers.len(),
    procraft_engine::file_size_label(bytes as u64)
));
```

- [ ] **Step 4: Run the gates**

Run: `cargo test -p procraft-engine`, `cargo test -p procraft --lib`
Expected: all pass. Then create an 8192 canvas in the running app, paint, and pan: the window must stay responsive. Check the layers label reads `1 layers · 0 KB` before the first stroke and grows by 256 KB per touched tile.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p procraft --all-targets -- -D warnings
git add crates/engine/src/document.rs apps/procraft/src/lib.rs
git commit -m "feat: 8K canvas cap, presets and tile memory readout"
```

---

### Task 9: Project format v12 with tile tables

**Files:**
- Modify: `crates/engine/src/document.rs` (`save_project` 2963-3110, `open_project` 3154-3300+, format tests 3860-4110)
- Test: `document.rs` tests

**Interfaces:**
- Consumes: `Raster::allocated_tiles`, `Raster::tile_mut`, `Raster::from_dense`.
- Produces: `PCRAFT12` writer/reader; v1–v11 still readable.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn v12_roundtrip_keeps_sparse_layers_small() {
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v12-sparse.procraft");
    let mut doc = Document::new(8192, 8192);
    doc.add_layer("second".into());
    doc.layers[0].pixels.set4(0, [1, 2, 3, 255]);
    doc.save_project(&path).unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size < 64 * 1024, "sparse 8K project is {size} bytes");
    let back = Document::open_project(&path).unwrap();
    assert_eq!(back.layers[0].pixels.get4(0), [1, 2, 3, 255]);
    assert_eq!(back.layers[0].pixels.alloc_bytes(), 256 * 1024);
    assert_eq!(back.layers[1].pixels.alloc_bytes(), 0);
    let _ = std::fs::remove_file(&path);
}
```

The v11 → v12 test is an addition to the existing hand-built v11 record test (the one that writes `b"PCRAFT11"` into a buffer around line 4098): after it opens the crafted document, save it and assert the version move:

```rust
back.save_project(&resave).unwrap();
let raw = std::fs::read(&resave).unwrap();
assert_eq!(&raw[..8], b"PCRAFT12");
assert_eq!(&head[..8], b"PCRAFT11", "the crafted input stays v11");
let _ = std::fs::remove_file(&resave);
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p procraft-engine v12_`
Expected: `assert_eq!(&raw[..8], b"PCRAFT12")` fails (writer still says 11).

- [ ] **Step 3: Write the v12 payload**

Writer helpers near `save_project`:

```rust
fn write_tile_table(
    f: &mut std::fs::File,
    raster: &Raster,
) -> Result<(), String> {
    use std::io::Write;
    let count = raster.allocated_tiles().count() as u32;
    f.write_all(&count.to_le_bytes()).map_err(|e| e.to_string())?;
    for (tx, ty, tile) in raster.allocated_tiles() {
        let mut enc =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(tile).map_err(|e| e.to_string())?;
        let packed = enc.finish().map_err(|e| e.to_string())?;
        f.write_all(&(tx as u16).to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.write_all(&(ty as u16).to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.write_all(&(packed.len() as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.write_all(&packed).map_err(|e| e.to_string())?;
    }
    Ok(())
}
```

`save_project`: magic → `b"PCRAFT12"`; replace the dense pixel block

```rust
let expect = (self.w * self.h * 4) as usize;
if layer.pixels.len() != expect { return Err(..); }
let mut enc = ...; enc.write_all(&layer.pixels)...;
```

with

```rust
if layer.pixels.w() != self.w || layer.pixels.h() != self.h {
    return Err("layer has wrong pixel size".to_string());
}
write_tile_table(&mut f, &layer.pixels)?;
```

and replace the mask block with the flag followed by `write_tile_table` when the mask has the canvas geometry.

Reader helpers inside `open_project`. The existing `take` closure stays as-is; pass it by `&mut` into a generic helper (the returned slice lifetime is the buffer's, not the borrow's):

```rust
fn read_tile_table<'a>(
    take: &mut impl FnMut(usize) -> Result<&'a [u8], String>,
    w: u32,
    h: u32,
    bpp: u8,
) -> Result<Raster, String> {
    let mut raster = Raster::new(w, h, bpp);
    let count = u32::from_le_bytes(
        take(4)?[..4].try_into().map_err(|_| "bad tile count".to_string())?,
    );
    let slots = (w.div_ceil(TILE) as u64) * (h.div_ceil(TILE) as u64);
    if count as u64 > slots {
        return Err("project has bad tile count".to_string());
    }
    for _ in 0..count {
        let tx = u16::from_le_bytes(
            take(2)?[..2].try_into().map_err(|_| "bad tile x".to_string())?,
        ) as u32;
        let ty = u16::from_le_bytes(
            take(2)?[..2].try_into().map_err(|_| "bad tile y".to_string())?,
        ) as u32;
        if tx >= raster.tiles_x() || ty >= raster.tiles_y() {
            return Err("project has out-of-range tile".to_string());
        }
        let len = u32::from_le_bytes(
            take(4)?[..4].try_into().map_err(|_| "bad tile length".to_string())?,
        ) as usize;
        let packed = take(len)?;
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(packed)
            .read_to_end(&mut raw)
            .map_err(|e| e.to_string())?;
        let dst = raster.tile_mut(tx, ty);
        let n = dst.len().min(raw.len());
        dst[..n].copy_from_slice(&raw[..n]);
    }
    Ok(raster)
}
```

Call it as `read_tile_table(&mut take, w, h, 4)?` / `read_tile_table(&mut take, w, h, 1)?`.

Version flags:

```rust
let v12 = magic == b"PCRAFT12";
let v11 = v12 || magic == b"PCRAFT11";
let v10 = v11 || magic == b"PCRAFT10";
// ... rest unchanged
```

Layer read: at the pixel payload, branch

```rust
let pixels = if v12 {
    read_tile_table(&mut take, w, h, 4)?
} else {
    let expect = (w * h * 4) as usize;
    let len = u32::from_le_bytes(take(4)?[..4].try_into().map_err(|_| "bad pixel length".to_string())?) as usize;
    let packed = take(len)?;
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(packed).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    if raw.len() != expect return Err("layer has wrong pixel size")?;
    Raster::from_dense(w, h, 4, &raw)
};
```

Mask: the existing `1u8` flag is followed by the same branch with `bpp = 1`.

- [ ] **Step 4: Update the magic assertions and run the suite**

Existing assertions at `document.rs:3866`, `:3917`, `:3985`, `:4089` (and any in `psd.rs`/app) move from `PCRAFT11` to `PCRAFT12`. The old-version tests at `:3935` (`PCRAFT08`), `:4000` (`PCRAFT09`), `:4049` (`PCRAFT10`) keep their magic and their dense payload, since the v12 branch only triggers on `PCRAFT12`.

Run: `cargo test -p procraft-engine`
Expected: all pass, including both new tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p procraft --all-targets -- -D warnings
git add crates/engine/src/document.rs
git commit -m "feat(engine): project format v12 with per-tile payloads"
```

---

### Task 10: Mobile viewport fallback (target B)

**Files:**
- Modify: `apps/procraft/src/canvas.rs` (viewport renderer + trigger), `apps/procraft/src/lib.rs` (`ProCraftApp` fields, `refresh_texture` early branch, canvas draw at ~10528, `paint_textured_quad` 9077)
- Test: `apps/procraft/src/canvas.rs`

**Interfaces:**
- Consumes: `CompositeCache`, `Document::stack_plan`, `Document::composite_tile`.
- Produces:
  - `pub fn viewport_active(forced: bool, native_mobile: bool, side: u32, max_texture_side: usize) -> bool`
  - `pub struct ViewTex { pub tex: Option<egui::TextureHandle>, key: Option<u64> }` with `render(&mut self, ctx, doc, cache, view: ViewRect, out: [usize; 2])`
  - `pub struct ViewRect { pub x0: f32, pub y0: f32, pub x1: f32, pub y1: f32 }` (canvas pixels)
  - `paint_textured_quad_uv(painter, tex, corners, uv: egui::Rect, tint)` in `lib.rs`
  - env override `PROCRAFT_FORCE_VIEWPORT_CANVAS=1`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn viewport_trigger_follows_size_and_texture_limit() {
    assert!(!viewport_active(false, false, 8192, 8192));
    assert!(viewport_active(false, true, 8192, 8192), "mobile above 4K");
    assert!(!viewport_active(false, true, 4096, 8192), "4K still fits on mobile");
    assert!(viewport_active(false, false, 8192, 4096), "GPU cannot hold the canvas");
    assert!(viewport_active(true, false, 1024, 8192), "forced for captures");
}

#[test]
fn view_rect_maps_screen_zoom_to_canvas_pixels() {
    // zoom 2: 200x100 screen px cover 100x50 canvas px
    let v = ViewRect::for_screen(0.0, 0.0, 200.0, 100.0, 8192.0, 8192.0, 2.0);
    assert_eq!((v.x0, v.y0, v.x1, v.y1), (0.0, 0.0, 100.0, 50.0));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p procraft --lib viewport_`
Expected: compile error — missing items.

- [ ] **Step 3: Implement the fallback**

In `canvas.rs`:

```rust
pub fn viewport_active(forced: bool, native_mobile: bool, side: u32, max_texture_side: usize) -> bool {
    forced || (native_mobile && side > 4096) || (side as usize) > max_texture_side
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl ViewRect {
    /// Canvas-pixel rect covered by a `sw`x`sh` screen rect at `zoom`,
    /// starting at canvas pixel `cx, cy` (the screen rect's top-left).
    pub fn for_screen(cx: f32, cy: f32, sw: f32, sh: f32, cw: f32, ch: f32, zoom: f32) -> Self {
        let zoom = zoom.max(0.0001);
        Self {
            x0: cx.clamp(0.0, cw),
            y0: cy.clamp(0.0, ch),
            x1: (cx + sw / zoom).clamp(0.0, cw),
            y1: (cy + sh / zoom).clamp(0.0, ch),
        }
    }
}

pub struct ViewTex {
    pub tex: Option<egui::TextureHandle>,
    key: Option<u64>,
}

impl ViewTex {
    pub fn new() -> Self {
        Self { tex: None, key: None }
    }

    /// Re-render when the document revision, the visible canvas rect or
    /// the output size changed. Returns the texture covering `view`.
    pub fn render(
        &mut self,
        ctx: &egui::Context,
        doc: &mut Document,
        cache: &mut CompositeCache,
        view: ViewRect,
        out: [usize; 2],
    ) -> &egui::TextureHandle {
        let key = doc
            .revision()
            .wrapping_mul(31)
            .wrapping_add(f32::to_bits(view.x0) as u64)
            .wrapping_mul(31)
            .wrapping_add(f32::to_bits(view.y0) as u64)
            .wrapping_mul(31)
            .wrapping_add(f32::to_bits(view.x1) as u64)
            .wrapping_mul(31)
            .wrapping_add(f32::to_bits(view.y1) as u64)
            .wrapping_mul(31)
            .wrapping_add((out[0] as u64) << 32 | out[1] as u64);
        if self.tex.is_none() || self.key != Some(key) {
            let plan = doc.stack_plan();
            let (ow, oh) = (out[0].max(1), out[1].max(1));
            let mut img = Vec::with_capacity(ow * oh * 4);
            let sx = (view.x1 - view.x0) / ow as f32;
            let sy = (view.y1 - view.y0) / oh as f32;
            for row in 0..oh {
                for col in 0..ow {
                    let x = (view.x0 + (col as f32 + 0.5) * sx).floor() as u32;
                    let y = (view.y0 + (row as f32 + 0.5) * sy).floor() as u32;
                    let x = x.min(doc.w.saturating_sub(1));
                    let y = y.min(doc.h.saturating_sub(1));
                    let tile = cache.tile(doc, &plan, x / TILE, y / TILE);
                    let o = (((y % TILE) * TILE + (x % TILE)) * 4) as usize;
                    img.extend_from_slice(&tile[o..o + 4]);
                }
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([ow, oh], &img);
            self.tex = Some(ctx.load_texture("canvas-view", image, egui::TextureOptions::LINEAR));
            self.key = Some(key);
            doc.take_dirty(); // tiles were recomposited; nothing to upload
        }
        self.tex.as_ref().expect("built above")
    }
}
```

`doc.take_dirty()` cannot be called on `&Document` — make `render` take `&mut Document`, or drain dirty in the caller. Use `&mut Document` and call `doc.take_dirty()` before/after; in `refresh_texture`, the fallback branch returns before the tile-upload path, so it must clear the dirty rect itself.

In `lib.rs`:

```rust
fn viewport_forced() -> bool {
    std::env::var("PROCRAFT_FORCE_VIEWPORT_CANVAS").is_ok_and(|v| v == "1")
}
```

`ProCraftApp` gains `view_tex: canvas::ViewTex` (init `ViewTex::new()`).

`refresh_texture` top branch (mask view and Animation Assist keep their own full uploads, so they opt out):

```rust
if !self.view_mask
    && !self.anim_assist
    && canvas::viewport_active(
        viewport_forced(),
        cfg!(any(target_os = "android", target_os = "ios")),
        self.doc.w.max(self.doc.h),
        ctx.input(|i| i.max_texture_side),
    )
{
    return; // the draw path renders the viewport texture
}
```

Draw path: delete the unconditional `let tex = self.texture.as_ref().unwrap().id();` at ~10401 and replace the `paint_textured_quad(...)` block at ~10528 with

```rust
let side = self.doc.w.max(self.doc.h);
let max_side = ui.ctx().input(|i| i.max_texture_side);
if !self.view_mask
    && !self.anim_assist
    && canvas::viewport_active(viewport_forced(), cfg!(any(target_os = "android", target_os = "ios")), side, max_side)
{
    let visible = view_rect.intersect(rect);
    let out = [
        (visible.width() * ui.ctx().pixels_per_point()).round().max(1.0) as usize,
        (visible.height() * ui.ctx().pixels_per_point()).round().max(1.0) as usize,
    ];
    let (ox, oy) = (
        (visible.min.x - rect.min.x) / z,
        (visible.min.y - rect.min.y) / z,
    );
    let view = canvas::ViewRect::for_screen(ox, oy, visible.width(), visible.height(), w, h, z);
    let view_tex = &mut self.view_tex;
    let composite = &mut self.composite;
    let tex = view_tex.render(&ctx, &mut self.doc, composite, view, out).id();
    let uv = egui::Rect::new(view.x0 / w, view.y0 / h, view.x1 / w, view.y1 / h);
    paint_textured_quad_uv(
        ui.painter(),
        tex,
        [
            rot_pt(visible.min, cc, ang),
            rot_pt(egui::pos2(visible.max.x, visible.min.y), cc, ang),
            rot_pt(visible.max, cc, ang),
            rot_pt(egui::pos2(visible.min.x, visible.max.y), cc, ang),
        ],
        uv,
        Color32::WHITE,
    );
} else {
    let tex = self.texture.as_ref().expect("canvas texture").id();
    paint_textured_quad(
        ui.painter(),
        tex,
        [
            to_screen(0.0, 0.0),
            to_screen(w, 0.0),
            to_screen(w, h),
            to_screen(0.0, h),
        ],
        Color32::WHITE,
    );
}
```

`paint_textured_quad_uv` is `paint_textured_quad` with UV corners taken from `uv: egui::Rect` (`min`, `(max.x,min.y)`, `max`, `(min.x,max.y)`).

Because the fallback path renders on every dirty frame and every view change, it must not also upload tiles: `refresh_texture` returns early, and `ViewTex::render` drains `take_dirty`.

- [ ] **Step 4: Verify on the desktop with the override, then on device**

```bash
cargo test -p procraft --lib
PROCRAFT_FORCE_VIEWPORT_CANVAS=1 PROCRAFT_FORCE_SHELL=mobile cargo run --release -p procraft
```

Create an 8192 canvas, paint and pinch-zoom: the image must stay sharp at 100 % once the gesture ends, and panning must not blank the canvas. Then cut a debug APK (ask first) and repeat on the Android tablet; record the frame feel in the plan's Measurement log (task 11).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p procraft --all-targets -- -D warnings
git add apps/procraft/src/canvas.rs apps/procraft/src/lib.rs
git commit -m "feat(ui): viewport-resolution canvas fallback for large documents"
```

---

### Task 11: `tilebench`, measurement log, docs

**Files:**
- Create: `crates/engine/examples/tilebench.rs`
- Modify: `ROADMAP.md:29,42,168`, `docs/superpowers/plans/2026-10-29-8k-canvas-lazy-compositing.md` (Measurement log section at the bottom)
- Test: the example itself is the check (`cargo run --release -p procraft-engine --example tilebench`)

**Interfaces:**
- Consumes: `Document::stack_plan`, `Document::composite_tile`, `Document::flatten`, `Raster`.
- Produces: printed composite timings; the docs updates.

- [ ] **Step 1: Write the benchmark**

```rust
//! Composite cost per tile at 4K and 8K for 1/10/100 layers.
//!
//! Run: cargo run --release -p procraft-engine --example tilebench

use procraft_engine::Document;
use std::time::Instant;

fn bench(side: u32, layers: usize) -> (f64, f64) {
    let mut doc = Document::new(side, side);
    let tiles_x = doc.tiles_x();
    for i in 0..layers {
        if i > 0 {
            doc.add_layer(format!("L{i}"));
        }
        let layer = doc.layers.last_mut().expect("layer");
        layer.opacity = 0.6;
        // One 256² patch of content per layer, in a different tile, so
        // 100 layers cost 25 MB instead of 25 GB.
        let t = i as u32 % (tiles_x * tiles_x);
        let (px0, py0) = ((t % tiles_x) * 256, (t / tiles_x) * 256);
        for y in py0..(py0 + 256).min(side) {
            for x in (px0..(px0 + 256).min(side)).step_by(4) {
                layer
                    .pixels
                    .set4(((y * side + x) * 4) as usize, [200, 40, 40, 200]);
            }
        }
    }
    let plan = doc.stack_plan();
    let (tx, ty) = (doc.tiles_x() / 2, doc.tiles_y() / 2);
    let n = 20;
    let start = Instant::now();
    for _ in 0..n {
        std::hint::black_box(doc.composite_tile(&plan, tx, ty));
    }
    let per_tile = start.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let start = Instant::now();
    std::hint::black_box(doc.flatten());
    let full = start.elapsed().as_secs_f64() * 1000.0;
    (per_tile, full)
}

fn main() {
    println!("{:>6} {:>7} {:>12} {:>12}", "side", "layers", "tile ms", "flatten ms");
    for side in [4096u32, 8192] {
        for layers in [1usize, 10, 100] {
            let (tile, full) = bench(side, layers);
            println!("{side:>6} {layers:>7} {tile:>12.2} {full:>12.1}");
        }
    }
}
```
```

Drop the `let _ = (TILE, ...)` line and the unused imports once the file compiles; keep only what is used.

- [ ] **Step 2: Run it and record the numbers**

```bash
cargo run --release -p procraft-engine --example tilebench
```

Append to this plan file:

```markdown
## Measurement log

| Date | Host | 4K / 1L tile | 4K / 100L tile | 8K / 1L tile | 8K / 100L tile | 8K flatten (100L) |
|---|---|---|---|---|---|---|
| 2026-10-29 | (fill in) | | | | | |
```

Fill in the measured values, plus one line for the Android tablet: canvas texture (`set_partial` per tile) vs viewport fallback feel, and the `layer_alloc_bytes()` reading for a 100-layer 8K document.

- [ ] **Step 3: Update ROADMAP**

- `ROADMAP.md:168`: `- [x] 8K canvas, 100+ layers with lazy compositing.`
- `ROADMAP.md:29,42`: move the Batch 6 remainder line to a short "Shipped in v…" note that names the tile storage and v12 format.

- [ ] **Step 4: Full gates**

```bash
cargo fmt --all
cargo clippy -p procraft --all-targets -- -D warnings
cargo test -p procraft-engine
cargo test -p procraft --lib
cargo build --release -p procraft
```

Expected: green. Then a final smoke run at 8K with 100 layers: create the document, add layers until the panel reads 100+, paint on several, save, reopen, export PNG.

- [ ] **Step 5: Commit**

```bash
git add crates/engine/examples/tilebench.rs ROADMAP.md docs/superpowers/plans/2026-10-29-8k-canvas-lazy-compositing.md
git commit -m "perf(engine): tilebench and 8K measurements"
```

---

## Self-review notes

- Spec §1 (tiled storage) → task 1; §2 (undo) → task 2; §3 (target A display) → tasks 6–7; §4 (fallback B) → task 10; §5 (limits + v12) → tasks 8 and 9; §6 (memory guard) → tasks 2 and 8; phases 1–6 map to tasks 1–11 in order.
- Conversion cost: tasks 2–5 are mechanical sweeps over 542 `.pixels` sites; the two rules (dense bridge for cold paths, `get`/`set` for per-pixel paths) keep them reviewable per file. Brush corpus byte-equality is the hard gate that phase 1 stayed behaviour-neutral.
- Type names used consistently: `Raster`, `TILE`, `StackPlan`, `CompositeCache`, `ViewRect`, `ViewTex`, `viewport_active`, `tile_image`, `layer_alloc_bytes`, `alloc_bytes`.

## Measurement log

`cargo run --release -p procraft-engine --example tilebench` (release build). Tile
figures are one 256² centre tile, 20 repetitions averaged; `flatten` is a single
full pass (n = 1). Every layer holds a full 256² patch in that tile — the fill is
dense on purpose, because the compositor skips a layer whose effective alpha is
0, so a sparse patch would measure fast paths rather than a solid tile. 100L
therefore measures 100 blended layers, not empty-tile fast paths. Two consecutive
runs agreed within ~1.6 % on the tile figures and within ~3 % on the 8K/100L
`flatten` (a single pass, so it moves more: 22.0–22.7 s across those two runs;
~7 % across five runs on this 15 W host).

| Date | Host | 4K / 1L tile | 4K / 100L tile | 8K / 1L tile | 8K / 100L tile | 8K flatten (100L) |
|---|---|---|---|---|---|---|
| 2026-10-29 | Windows 11 (x86_64), Intel Core i5-8250U @ 1.60 GHz, ~7.9 GiB (8 GiB installed) — desktop | 1.83 ms | 102.68 ms | 1.84 ms | 108.35 ms | 21963 ms (22.0 s) |

- Host `layer_alloc_bytes()` for the 8K/100-layer document: 26,214,400 bytes
  (100 loaded tiles, 25.0 MiB). That is a floor for this benchmark shape (one
  tile per layer), not a typical 100-layer document's footprint; the tile column
  above is that memory's cost.
- Android tablet: **pending a device run** — canvas-texture feel with per-tile
  `set_partial` (target A) vs forced `PROCRAFT_FORCE_VIEWPORT_CANVAS=1`, plus
  that document's `layer_alloc_bytes()`; controller/project owner close this.
