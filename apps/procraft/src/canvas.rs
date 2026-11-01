//! Canvas display: composite tiles behind an LRU cache.
//!
//! Two targets share that composite. Target A is one dense canvas texture
//! (`doc.w` × `doc.h`) fed by tile uploads: [`full_image`] interleaves the
//! tiles row by row, [`tile_image`] crops them to the canvas edge, and a
//! dab re-uploads the tiles its dirty rect touches, so the texture never
//! changes size (UVs stay 0..1).
//!
//! Target B is the viewport fallback for documents the GPU cannot hold: a
//! screen-sized texture ([`ViewTex`]) covering only the canvas rect the
//! view shows ([`ViewRect`]), sampled from the same composite tiles by
//! [`view_pixels`] when the fallback triggers. A view change re-samples it
//! whole; a dirty revision patches just the texels under the dirty rect.
//!
//! The cache is keyed by tile only: a tile is trusted until something
//! evicts it. Content edits evict the tiles their dirty rect touches
//! (see `ProCraftApp::refresh_texture`); dropping the canvas texture
//! (new/open/resize/undo-driven rebuild) clears the whole cache — with one
//! deliberate exception: the viewport path drops the dense texture without
//! invalidating, because the cache is exactly what it samples.

use procraft_engine::{Document, StackPlan, MAX_CANVAS_SIDE, TILE};

/// Cache budget on phones and tablets: a 4K canvas fits whole, an 8K
/// one keeps the working set of the tiles being painted.
pub const CACHE_MOBILE: usize = 96 * 1024 * 1024;
/// Cache budget on desktop: an 8K canvas (256 MiB of composite) plus
/// headroom for the neighbouring tiles.
pub const CACHE_DESKTOP: usize = 320 * 1024 * 1024;

/// Below this zoom the canvas texture is minified past what its bilinear
/// sampling can hold, so the viewport path ([`view_pixels`], box-filtered)
/// draws the canvas instead. At or above it the 1:1 texture stays exact.
pub const MINIFIED_ZOOM: f32 = 0.75;

/// Canvases above this side always draw through the viewport path. A
/// canvas-sized RGBA texture costs 4 bytes per canvas pixel of VRAM and
/// grows with the square of the side (2048² = 16 MiB, 4K = 64 MiB, 8K =
/// 256 MiB), and on mobile it competes with the composite cache for the
/// same unified memory. Sampling the visible rect costs a few ms of CPU;
/// holding the whole canvas on the GPU costs hundreds of MiB everywhere.
pub const VIEWPORT_SIDE: u32 = 2048;

/// Whether the display takes the viewport-resolution fallback (spec §4)
/// instead of one canvas-sized texture: forced for captures, any canvas
/// above [`VIEWPORT_SIDE`], any canvas the GPU's texture limit cannot
/// hold, or a legacy document above the 8K cap (target A was sized for the
/// cap; a 16384² texture is ~1 GiB even when the GPU nominally allows it).
pub fn viewport_active(forced: bool, side: u32, max_texture_side: usize) -> bool {
    forced || side > VIEWPORT_SIDE || side as usize > max_texture_side || side > MAX_CANVAS_SIDE
}

/// Whether a canvas-sized texture can be allocated at all: within the GPU's
/// texture limit and within the app's own 8K cap. Unlike
/// [`viewport_active`], a GPU that cannot hold the 8K cap is not
/// disqualifying here — a 1K canvas fits a 4K-limit GPU fine.
pub fn canvas_texture_fits(side: u32, max_texture_side: usize) -> bool {
    side as usize <= max_texture_side && side <= MAX_CANVAS_SIDE
}

/// A mask view with nothing to show must not suppress the fallback when the
/// document needs it; with Animation Assist on the viewport is not taken, so
/// leaving the user's View alone is correct.
pub(crate) fn maskless_view_blocks_fallback(
    view_mask: bool,
    has_mask: bool,
    anim_assist: bool,
    needs_fallback: bool,
) -> bool {
    view_mask && !has_mask && !anim_assist && needs_fallback
}

/// Bytes of one full composite tile, which is what the engine returns.
const TILE_BYTES: usize = (TILE * TILE * 4) as usize;

/// Deepest cached mip level. Level L covers the composite at 1/2^L, so 4
/// is a 16× reduction: an 8K canvas at fit zoom needs level 3 or 4, and a
/// deeper chain would only add build time below a screen pixel.
pub(crate) const MAX_MIP_LEVEL: u32 = 4;

/// Composite tiles, least-recently-used first when over budget.
pub struct CompositeCache {
    tiles: std::collections::HashMap<(u32, u32), Vec<u8>>,
    /// Recency order, oldest at the front.
    order: std::collections::VecDeque<(u32, u32)>,
    bytes: usize,
    budget: usize,
    /// Levels 1..=`MAX_MIP_LEVEL`, keyed `(level, tx, ty)`: each texel is
    /// the alpha-weighted average of the 2×2 texels below it. Minified
    /// views sample these, so they get their own (smaller) budget rather
    /// than competing with the level-0 tiles a 1:1 view needs.
    mips: std::collections::HashMap<(u32, u32, u32), Vec<u8>>,
    mip_order: std::collections::VecDeque<(u32, u32, u32)>,
    mip_bytes: usize,
    mip_budget: usize,
    /// Set while a view pass prebuilds and then reads its tiles: a trim
    /// would drop one underneath the threads that sample them.
    pass: bool,
    /// Bumped by [`invalidate`](Self::invalidate). The viewport texture keys
    /// on it because a content-identity change (new/open/undo rebuild) can
    /// reset `Document::revision()`, so the revision alone cannot tell two
    /// documents apart.
    epoch: u64,
}

impl CompositeCache {
    pub fn new(budget: usize) -> Self {
        // A quarter of the level-0 budget holds every level above 1 for
        // any canvas that fits in it, and at least four tiles so one parent
        // and its children can coexist mid-build.
        let mip_budget = (budget / 4).max(4 * TILE_BYTES);
        Self {
            tiles: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
            bytes: 0,
            // One tile must always fit, or `tile` could not return the
            // tile it just composited.
            budget: budget.max(TILE_BYTES),
            mips: std::collections::HashMap::new(),
            mip_order: std::collections::VecDeque::new(),
            mip_bytes: 0,
            mip_budget,
            pass: false,
            epoch: 0,
        }
    }

    /// Bytes currently held: the layers-panel memory readout.
    pub fn bytes(&self) -> usize {
        self.bytes + self.mip_bytes
    }

    /// Identity of the content the cached tiles belong to: changes on every
    /// [`invalidate`](Self::invalidate), which the app calls exactly when the
    /// document behind the cache is replaced or rebuilt.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Drop every tile: the next read recomposites from the document.
    /// Called wherever the canvas texture is dropped, because that is
    /// when a full upload will read every tile again.
    pub fn invalidate(&mut self) {
        self.tiles.clear();
        self.order.clear();
        self.bytes = 0;
        self.mips.clear();
        self.mip_order.clear();
        self.mip_bytes = 0;
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Hold off trimming until [`end_pass`](Self::end_pass). A sampling
    /// pass builds every tile it is about to read, and the sampling threads
    /// cannot composite one back if it is dropped underneath them. The
    /// overshoot is bounded by the tiles one pass touches.
    pub fn begin_pass(&mut self) {
        self.pass = true;
    }

    /// Release a [`begin_pass`](Self::begin_pass) and trim back to budget.
    pub fn end_pass(&mut self) {
        self.pass = false;
        self.trim();
        self.trim_mips();
    }

    /// Recomposite only `rect` (inclusive canvas bounds) of every cached
    /// tile it touches. What a dab changed is a band, not the 256² tiles
    /// around it: patching keeps the two-thirds of each tile the event did
    /// not touch instead of recompositing the whole square. Tiles that are
    /// not cached stay missing and are composited fresh on the next read.
    pub fn patch(&mut self, doc: &Document, plan: &StackPlan, rect: (u32, u32, u32, u32)) {
        let (x0, y0, x1, y1) = rect;
        if x0 > x1 || y0 > y1 {
            return;
        }
        for ty in y0 / TILE..=y1 / TILE {
            for tx in x0 / TILE..=x1 / TILE {
                if let Some(tile) = self.tiles.get_mut(&(tx, ty)) {
                    doc.composite_tile_into(plan, tx, ty, Some(rect), tile);
                }
            }
        }
        self.patch_mips(rect);
    }

    /// Bring the cached mip tiles overlapping `rect` up to date, bottom up:
    /// each level is re-averaged from the level below, which was just
    /// patched. Only texels whose 2^L square intersects the rect change, so
    /// a dab costs a handful of averages per level instead of rebuilding
    /// the tile. A tile whose children are not all cached is dropped and
    /// rebuilt from the document on the next read.
    fn patch_mips(&mut self, rect: (u32, u32, u32, u32)) {
        let (x0, y0, x1, y1) = rect;
        for level in 1..=MAX_MIP_LEVEL {
            let span = TILE << level;
            for ty in y0 / span..=y1 / span {
                for tx in x0 / span..=x1 / span {
                    let key = (level, tx, ty);
                    if !self.mips.contains_key(&key) {
                        continue;
                    }
                    let Some(mut tile) = self.mips.remove(&key) else {
                        continue;
                    };
                    let complete = {
                        let (ax, ay) = (tx * 2, ty * 2);
                        let kids = [
                            self.level_tile(level - 1, ax, ay),
                            self.level_tile(level - 1, ax + 1, ay),
                            self.level_tile(level - 1, ax, ay + 1),
                            self.level_tile(level - 1, ax + 1, ay + 1),
                        ];
                        match kids.map(|k| k.filter(|t| t.len() == TILE_BYTES)) {
                            [Some(a), Some(b), Some(c), Some(d)] => {
                                let kids = [a, b, c, d];
                                let mx0 = (x0 >> level).max(tx * TILE);
                                let mx1 = (x1 >> level).min(tx * TILE + TILE - 1);
                                let my0 = (y0 >> level).max(ty * TILE);
                                let my1 = (y1 >> level).min(ty * TILE + TILE - 1);
                                for my in my0..=my1 {
                                    for mx in mx0..=mx1 {
                                        average_texel(
                                            kids,
                                            (mx - tx * TILE) as usize,
                                            (my - ty * TILE) as usize,
                                            &mut tile,
                                        );
                                    }
                                }
                                true
                            }
                            _ => false,
                        }
                    };
                    if complete {
                        self.mips.insert(key, tile);
                    } else {
                        self.mip_bytes -= TILE_BYTES;
                        self.mip_order.retain(|k| *k != key);
                    }
                }
            }
        }
    }

    /// A level tile from either the level-0 cache (level 0) or the mip
    /// cache, by its tile index at that level.
    fn level_tile(&self, level: u32, tx: u32, ty: u32) -> Option<&[u8]> {
        if level == 0 {
            self.tiles.get(&(tx, ty)).map(Vec::as_slice)
        } else {
            self.mips.get(&(level, tx, ty)).map(Vec::as_slice)
        }
    }

    /// One tile of level `level` (1..=`MAX_MIP_LEVEL`), `TILE * TILE * 4`
    /// bytes: a hit returns the cached bytes, a miss averages the level
    /// below and caches the result. Level L tile `(tx, ty)` covers the
    /// canvas square `tx * TILE << L`.
    pub fn mip_tile(
        &mut self,
        doc: &Document,
        plan: &StackPlan,
        level: u32,
        tx: u32,
        ty: u32,
    ) -> &[u8] {
        let key = (level, tx, ty);
        if !self.mips.contains_key(&key) {
            let bytes = self.build_mip(doc, plan, level, tx, ty);
            self.mip_bytes += bytes.len();
            self.mips.insert(key, bytes);
            self.mip_order.push_back(key);
            self.trim_mips();
        }
        self.mips.get(&key).map(Vec::as_slice).unwrap_or(&[])
    }

    fn build_mip(
        &mut self,
        doc: &Document,
        plan: &StackPlan,
        level: u32,
        tx: u32,
        ty: u32,
    ) -> Vec<u8> {
        let (ax, ay) = (tx * 2, ty * 2);
        let children = [(ax, ay), (ax + 1, ay), (ax, ay + 1), (ax + 1, ay + 1)];
        if level == 1 {
            for (cx, cy) in children {
                self.tile(doc, plan, cx, cy);
            }
        } else {
            for (cx, cy) in children {
                self.mip_tile(doc, plan, level - 1, cx, cy);
            }
        }
        let mut out = vec![0u8; TILE_BYTES];
        {
            let kids = [
                self.level_tile(level - 1, ax, ay),
                self.level_tile(level - 1, ax + 1, ay),
                self.level_tile(level - 1, ax, ay + 1),
                self.level_tile(level - 1, ax + 1, ay + 1),
            ];
            let kids = match kids.map(|k| k.filter(|t| t.len() == TILE_BYTES)) {
                [Some(a), Some(b), Some(c), Some(d)] => [a, b, c, d],
                // A child that would not build (empty document): leave the
                // tile transparent and let the next read rebuild it.
                _ => return out,
            };
            for my in 0..TILE as usize {
                for mx in 0..TILE as usize {
                    average_texel(kids, mx, my, &mut out);
                }
            }
        }
        out
    }

    fn trim_mips(&mut self) {
        if self.pass {
            return;
        }
        while self.mip_bytes > self.mip_budget && self.mip_order.len() > 1 {
            let Some(key) = self.mip_order.pop_front() else {
                break;
            };
            if let Some(old) = self.mips.remove(&key) {
                self.mip_bytes -= old.len();
            }
        }
    }

    /// One tile of the composite, `TILE * TILE * 4` bytes: a hit returns
    /// the cached bytes, a miss composites and caches them.
    pub fn tile(&mut self, doc: &Document, plan: &StackPlan, tx: u32, ty: u32) -> &[u8] {
        let key = (tx, ty);
        if self.tiles.contains_key(&key) {
            self.touch(key);
        } else {
            let bytes = doc.composite_tile(plan, tx, ty);
            self.bytes += bytes.len();
            self.tiles.insert(key, bytes);
            self.order.push_back(key);
            self.trim();
        }
        self.tiles.get(&key).map(Vec::as_slice).unwrap_or(&[])
    }

    fn touch(&mut self, key: (u32, u32)) {
        if self.order.back() == Some(&key) {
            return;
        }
        self.order.retain(|k| *k != key);
        self.order.push_back(key);
    }

    fn trim(&mut self) {
        if self.pass {
            return;
        }
        while self.bytes > self.budget && self.order.len() > 1 {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.tiles.remove(&key) {
                self.bytes -= old.len();
            }
        }
    }
}

/// Assemble the full canvas image (`w * h * 4`, row-major) from composite
/// tiles: tiles are interleaved row by row within each tile row.
///
/// The texture is dense, so appending whole tiles in tile-block order
/// scrambles every canvas with more than one tile column. This walks the
/// cache one tile row at a time, crops each tile to the canvas edge (edge
/// tiles share the band's cropped height, so the first entry gives the row
/// count) and emits each canvas row as one slice per tile in `tx` order.
pub fn full_image(doc: &Document, plan: &StackPlan, cache: &mut CompositeCache) -> Vec<u8> {
    let w = doc.w as usize;
    let mut img = Vec::with_capacity(w * doc.h as usize * 4);
    for ty in 0..doc.tiles_y() {
        let band: Vec<(Vec<u8>, usize, usize)> = (0..doc.tiles_x())
            .map(|tx| {
                let tile = cache.tile(doc, plan, tx, ty);
                tile_image(doc, tile, tx, ty)
            })
            .collect();
        // Every tile in a band crops to the same height; a tile outside the
        // canvas would be `(empty, 0, 0)`, so the first entry is the band.
        let rows = band.first().map_or(0, |(_, _, h)| *h);
        for row in 0..rows {
            for (rgba, tw, _) in &band {
                let start = row * tw * 4;
                img.extend_from_slice(&rgba[start..start + tw * 4]);
            }
        }
    }
    debug_assert_eq!(img.len(), w * doc.h as usize * 4);
    img
}

/// Crop one composite tile to the canvas edge for `set_partial`,
/// returning `(rgba, w, h)`. A tile wholly outside the canvas yields
/// `(empty, 0, 0)`; a short slice is zero-filled in release builds rather
/// than panicking, like the engine's own out-of-canvas pixels (debug
/// builds assert the engine's full-tile size instead).
pub fn tile_image(doc: &Document, tile: &[u8], tx: u32, ty: u32) -> (Vec<u8>, usize, usize) {
    debug_assert_eq!(
        tile.len(),
        TILE_BYTES,
        "composite tile is not TILE * TILE * 4 bytes"
    );
    let x0 = tx.saturating_mul(TILE);
    let y0 = ty.saturating_mul(TILE);
    let w = doc.w.saturating_sub(x0).min(TILE) as usize;
    let h = doc.h.saturating_sub(y0).min(TILE) as usize;
    if w == 0 || h == 0 {
        return (Vec::new(), 0, 0);
    }
    let stride = TILE as usize * 4;
    let row_bytes = w * 4;
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let src = row * stride;
        match tile.get(src..src + row_bytes) {
            Some(px) => out.extend_from_slice(px),
            None => out.resize(out.len() + row_bytes, 0),
        }
    }
    (out, w, h)
}

/// Crop the `rect` part (inclusive canvas bounds) of one composite tile to
/// the canvas edge for `set_partial`, returning
/// `(rgba, x, y, w, h)` at the clamped canvas-space origin. Empty when the
/// rect misses the tile or the canvas.
pub fn tile_image_region(
    doc: &Document,
    tile: &[u8],
    tx: u32,
    ty: u32,
    rect: (u32, u32, u32, u32),
) -> (Vec<u8>, usize, usize, usize, usize) {
    let empty = (Vec::new(), 0, 0, 0, 0);
    if tile.len() != TILE_BYTES {
        return empty;
    }
    let (x0, y0) = (tx * TILE, ty * TILE);
    let x0 = x0.min(doc.w);
    let y0 = y0.min(doc.h);
    let x1 = (x0 + TILE).min(doc.w).saturating_sub(1);
    let y1 = (y0 + TILE).min(doc.h).saturating_sub(1);
    let (rx0, ry0) = (rect.0.max(x0), rect.1.max(y0));
    let (rx1, ry1) = (rect.2.min(x1), rect.3.min(y1));
    if rx0 > rx1 || ry0 > ry1 {
        return empty;
    }
    let (w, h) = ((rx1 - rx0 + 1) as usize, (ry1 - ry0 + 1) as usize);
    let stride = TILE as usize * 4;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in ry0 - y0..=ry1 - y0 {
        let start = y as usize * stride + (rx0 - x0) as usize * 4;
        out.extend_from_slice(&tile[start..start + w * 4]);
    }
    (out, rx0 as usize, ry0 as usize, w, h)
}

/// The canvas-pixel rect a viewport shows (target B): what the view
/// texture samples, and where the draw path paints that texture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl ViewRect {
    /// Canvas-pixel rect covered by a `sw`x`sh` screen rect at `zoom`,
    /// starting at canvas pixel `cx, cy` (the screen rect's top-left),
    /// clamped to the `cw`x`ch` canvas.
    pub fn for_screen(cx: f32, cy: f32, sw: f32, sh: f32, cw: f32, ch: f32, zoom: f32) -> Self {
        let zoom = zoom.max(0.0001);
        Self {
            x0: cx.clamp(0.0, cw),
            y0: cy.clamp(0.0, ch),
            x1: (cx + sw / zoom).clamp(0.0, cw),
            y1: (cy + sh / zoom).clamp(0.0, ch),
        }
    }

    /// Bounding box of a screen-space quad's corners mapped back to canvas
    /// pixels, clamped to the `cw`x`ch` canvas. A rotated view's visible
    /// region is not the axis-aligned intersection of the page and the
    /// panel, so the corners come from the four edges of the panel itself.
    /// At zero rotation this covers the same region as [`for_screen`]
    /// (`for_screen` is still the mapping the draw path runs unrotated).
    ///
    /// [`for_screen`]: Self::for_screen
    pub fn from_canvas_points(points: [(f32, f32); 4], cw: f32, ch: f32) -> Self {
        let mut b = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for (x, y) in points {
            b.0 = b.0.min(x);
            b.1 = b.1.min(y);
            b.2 = b.2.max(x);
            b.3 = b.3.max(y);
        }
        Self {
            x0: b.0.clamp(0.0, cw),
            y0: b.1.clamp(0.0, ch),
            x1: b.2.clamp(0.0, cw),
            y1: b.3.clamp(0.0, ch),
        }
    }
}

/// Output texels whose sample centers fall inside the canvas band
/// `band_lo..band_hi`, as a half-open range clamped to `len`.
fn texel_span(origin: f32, step: f32, len: usize, band_lo: f32, band_hi: f32) -> (usize, usize) {
    if step <= 0.0 || !step.is_finite() {
        return (0, 0);
    }
    let lo = ((band_lo - origin) / step - 0.5).ceil();
    let hi = ((band_hi - origin) / step - 0.5).ceil();
    let lo = (lo.max(0.0) as usize).min(len);
    let hi = (hi.max(0.0) as usize).min(len);
    (lo, hi.max(lo))
}

/// Taps per axis for one output texel: 1 while the canvas is at least
/// screen resolution (nearest is exact), then a box filter widening with
/// the footprint, capped at 4 per axis. The GPU texture path has no
/// mipmaps (`mip_level_count: 1`), so minification has to be filtered
/// here or 1-pixel details alias into stripes.
fn tap_count(step: f32) -> usize {
    if step <= 1.0 {
        1
    } else {
        (step.ceil() as usize).clamp(2, 4)
    }
}

/// Mip level to sample when a texel covers `dx`x`dy` canvas pixels: the
/// deepest level whose texel is at most half the footprint, so a 2×2
/// bilinear of mip texels spans it. Level 0 below 2:1 minification.
///
/// Without this a fit-zoomed 8K viewport reads 16 full-resolution pixels
/// per texel (≈38 M reads per pass, 100+ ms); with it, 4 averaged ones.
pub(crate) fn mip_shift(dx: f32, dy: f32) -> u32 {
    let step = dx.min(dy);
    if step <= 1.0 {
        return 0;
    }
    (step.log2().floor() as i32).clamp(0, MAX_MIP_LEVEL as i32) as u32
}

/// One texel of a mip tile: the alpha-weighted mean of the 2×2 block in
/// the four children (`a, b` top row; `c, d` bottom row, left to right).
/// `mx`/`my` are tile-local texel coordinates of the parent.
fn average_texel(kids: [&[u8]; 4], mx: usize, my: usize, out: &mut [u8]) {
    let half = (TILE / 2) as usize;
    let stride = TILE as usize * 4;
    let (left, right) = if my < half {
        (kids[0], kids[1])
    } else {
        (kids[2], kids[3])
    };
    let (child, sx) = if mx < half {
        (left, mx * 2)
    } else {
        (right, (mx % half) * 2)
    };
    let sy = (my % half) * 2;
    let o = sy * stride + sx * 4;
    let (p0, p1) = (&child[o..o + 4], &child[o + 4..o + 8]);
    let o2 = o + stride;
    let (p2, p3) = (&child[o2..o2 + 4], &child[o2 + 4..o2 + 8]);
    let (a0, a1, a2, a3) = (p0[3] as u32, p1[3] as u32, p2[3] as u32, p3[3] as u32);
    let a_sum = a0 + a1 + a2 + a3;
    let dst = (my * TILE as usize + mx) * 4;
    if a_sum == 0 {
        out[dst..dst + 4].fill(0);
        return;
    }
    // Weighting by alpha keeps the canvas edge from darkening: level 0 is
    // opaque on the canvas and transparent outside it.
    for k in 0..3 {
        let s = p0[k] as u32 * a0 + p1[k] as u32 * a1 + p2[k] as u32 * a2 + p3[k] as u32 * a3;
        out[dst + k] = ((s + a_sum / 2) / a_sum) as u8;
    }
    // Opaque inside the canvas by construction: a partly covered edge texel
    // keeps the edge colour rather than turning translucent, like level 0's
    // clamped taps.
    out[dst + 3] = 255;
}

/// Sample a viewport out of the composite tiles: `out` texels, row-major
/// RGBA. `region` is a half-open `[col0, row0, col1, row1]` texel rect to
/// render (`None`: all of it).
///
/// Tile-major on purpose: a fit-zoomed viewport is millions of texels, and
/// one cache lookup per texel would dominate the frame. Each canvas tile is
/// fetched once, then every texel whose footprint lands in it is sampled —
/// the inner loop is pure sampling.
///
/// At 1:1 or larger the texel takes its center pixel. Minified, it takes an
/// alpha-weighted box of up to 4×4 taps across its footprint: each tap is
/// accumulated under the tile that contains it, so a texel straddling a
/// tile seam still averages both sides and every tile is read once.
pub(crate) fn view_pixels(
    doc: &Document,
    plan: &StackPlan,
    cache: &mut CompositeCache,
    view: ViewRect,
    out: [usize; 2],
    region: Option<[usize; 4]>,
) -> Vec<u8> {
    let out = [out[0].max(1), out[1].max(1)];
    let [c0, r0, c1, r1] = match region {
        Some([c0, r0, c1, r1]) => [
            c0.min(out[0]),
            r0.min(out[1]),
            c1.min(out[0]),
            r1.min(out[1]),
        ],
        None => [0, 0, out[0], out[1]],
    };
    let (cols, rows) = (c1.saturating_sub(c0), r1.saturating_sub(r0));
    if cols == 0 || rows == 0 || doc.w == 0 || doc.h == 0 {
        return vec![0u8; cols * rows * 4];
    }
    let (vx, vy) = (view.x1 - view.x0, view.y1 - view.y0);
    if vx <= 0.0 || vy <= 0.0 {
        return vec![0u8; cols * rows * 4];
    }
    let dx = vx / out[0] as f32;
    let dy = vy / out[1] as f32;
    let shift = mip_shift(dx, dy);
    if shift > 0 {
        return view_pixels_mip(doc, plan, cache, view, out, [c0, r0, c1, r1], shift);
    }
    let tile = TILE as usize;
    let (max_x, max_y) = ((doc.w - 1) as f32, (doc.h - 1) as f32);
    let (taps_x, taps_y) = (tap_count(dx), tap_count(dy));
    let filtered = taps_x > 1 || taps_y > 1;
    // A center past the tile grid is outside the document: the unfiltered
    // path never visits those texels and leaves them transparent.
    let grid = (doc.tiles_x() as usize * tile, doc.tiles_y() as usize * tile);
    let (tiles_x, tiles_y) = (doc.tiles_x(), doc.tiles_y());
    // Composite every tile the region samples before the bands start: they
    // only read the cache, and trimming must not drop a tile a band still
    // has to sample.
    cache.begin_pass();
    for ty in 0..tiles_y {
        let band = ((ty * TILE) as f32, ((ty + 1) * TILE) as f32);
        let half = if filtered { dy * 0.5 } else { 0.0 };
        let (row_a, row_b) = texel_span(view.y0, dy, out[1], band.0 - half, band.1 + half);
        let (row_a, row_b) = (row_a.max(r0), row_b.min(r1));
        if row_a >= row_b {
            continue;
        }
        for tx in 0..tiles_x {
            let band = ((tx * TILE) as f32, ((tx + 1) * TILE) as f32);
            let half = if filtered { dx * 0.5 } else { 0.0 };
            let (col_a, col_b) = texel_span(view.x0, dx, out[0], band.0 - half, band.1 + half);
            let (col_a, col_b) = (col_a.max(c0), col_b.min(c1));
            if col_a >= col_b {
                continue;
            }
            cache.tile(doc, plan, tx, ty);
        }
    }
    let mut img = vec![0u8; cols * rows * 4];
    let pass = TilePass {
        cache: &*cache,
        view,
        out,
        region: [c0, r0, c1, r1],
        cols,
        dx,
        dy,
        max_x,
        max_y,
        taps_x,
        taps_y,
        filtered,
        grid,
        tiles: (tiles_x, tiles_y),
    };
    sample_in_bands(&mut img, cols, r0..r1, |row_a, row_b, dst| {
        pass.band(row_a, row_b, dst)
    });
    cache.end_pass();
    img
}

/// Level-0 sampling of a band of output rows: the texel math of
/// [`view_pixels`], with every projection clipped to `region` and every
/// tile read from the (already built) cache.
struct TilePass<'a> {
    cache: &'a CompositeCache,
    view: ViewRect,
    out: [usize; 2],
    region: [usize; 4],
    cols: usize,
    dx: f32,
    dy: f32,
    max_x: f32,
    max_y: f32,
    taps_x: usize,
    taps_y: usize,
    filtered: bool,
    grid: (usize, usize),
    tiles: (u32, u32),
}

impl TilePass<'_> {
    fn band(&self, row_a: usize, row_b: usize, dst: &mut [u8]) {
        let [c0, _, c1, _] = self.region;
        let tile = TILE as usize;
        let cols = self.cols;
        // Alpha-weighted RGBA sums over each texel's taps: r*a, g*a, b*a, a.
        let mut acc = if self.filtered {
            vec![[0u32; 4]; (row_b - row_a) * cols]
        } else {
            Vec::new()
        };
        for ty in 0..self.tiles.1 {
            let band = ((ty * TILE) as f32, ((ty + 1) * TILE) as f32);
            let half = if self.filtered { self.dy * 0.5 } else { 0.0 };
            let (ra, rb) = texel_span(
                self.view.y0,
                self.dy,
                self.out[1],
                band.0 - half,
                band.1 + half,
            );
            let (ra, rb) = (ra.max(row_a), rb.min(row_b));
            if ra >= rb {
                continue;
            }
            for tx in 0..self.tiles.0 {
                let band = ((tx * TILE) as f32, ((tx + 1) * TILE) as f32);
                let half = if self.filtered { self.dx * 0.5 } else { 0.0 };
                let (ca, cb) = texel_span(
                    self.view.x0,
                    self.dx,
                    self.out[0],
                    band.0 - half,
                    band.1 + half,
                );
                let (ca, cb) = (ca.max(c0), cb.min(c1));
                if ca >= cb {
                    continue;
                }
                let pixels = self
                    .cache
                    .level_tile(0, tx, ty)
                    .expect("tile missing during a sampling pass");
                for row in ra..rb {
                    let dst_row = (row - row_a) * cols * 4;
                    if !self.filtered {
                        let y = (self.view.y0 + (row as f32 + 0.5) * self.dy)
                            .floor()
                            .clamp(0.0, self.max_y) as usize;
                        let src_row = (y % tile) * tile * 4;
                        for col in ca..cb {
                            let x = (self.view.x0 + (col as f32 + 0.5) * self.dx)
                                .floor()
                                .clamp(0.0, self.max_x)
                                as usize;
                            let src = src_row + (x % tile) * 4;
                            let slot_dst = dst_row + (col - c0) * 4;
                            if let (Some(px), Some(slot)) = (
                                pixels.get(src..src + 4),
                                dst.get_mut(slot_dst..slot_dst + 4),
                            ) {
                                slot.copy_from_slice(px);
                            }
                        }
                        continue;
                    }
                    let center_y = self.view.y0 + (row as f32 + 0.5) * self.dy;
                    if center_y >= self.grid.1 as f32 {
                        continue;
                    }
                    let mut tap_rows = [0usize; 4];
                    let mut rows_here = 0;
                    for (j, slot) in tap_rows.iter_mut().enumerate().take(self.taps_y) {
                        let tap_y = self.view.y0
                            + (row as f32 + (j as f32 + 0.5) / self.taps_y as f32) * self.dy;
                        let py = tap_y.floor().clamp(0.0, self.max_y) as usize;
                        *slot = py;
                        rows_here += (py / tile == ty as usize) as usize;
                    }
                    if rows_here == 0 {
                        continue;
                    }
                    for col in ca..cb {
                        let center_x = self.view.x0 + (col as f32 + 0.5) * self.dx;
                        if center_x >= self.grid.0 as f32 {
                            continue;
                        }
                        let mut tap_cols = [0usize; 4];
                        let mut cols_here = 0;
                        for (i, slot) in tap_cols.iter_mut().enumerate().take(self.taps_x) {
                            let tap_x = self.view.x0
                                + (col as f32 + (i as f32 + 0.5) / self.taps_x as f32) * self.dx;
                            let px = tap_x.floor().clamp(0.0, self.max_x) as usize;
                            *slot = px;
                            cols_here += (px / tile == tx as usize) as usize;
                        }
                        if cols_here == 0 {
                            continue;
                        }
                        let mut sum = [0u32; 4];
                        for &py in &tap_rows[..self.taps_y] {
                            if py / tile != ty as usize {
                                continue;
                            }
                            let src_row = (py % tile) * tile * 4;
                            for &px in &tap_cols[..self.taps_x] {
                                if px / tile != tx as usize {
                                    continue;
                                }
                                let src = src_row + (px % tile) * 4;
                                let slot = &pixels[src..src + 4];
                                let a = slot[3] as u32;
                                sum[0] += slot[0] as u32 * a;
                                sum[1] += slot[1] as u32 * a;
                                sum[2] += slot[2] as u32 * a;
                                sum[3] += a;
                            }
                        }
                        let s = &mut acc[(row - row_a) * cols + (col - c0)];
                        for k in 0..4 {
                            s[k] += sum[k];
                        }
                    }
                }
            }
        }
        if !self.filtered {
            return;
        }
        let taps = (self.taps_x * self.taps_y) as u32;
        for (slot, s) in dst.chunks_mut(4).zip(acc) {
            let a = s[3];
            if a == 0 {
                continue;
            }
            slot[0] = ((s[0] + a / 2) / a) as u8;
            slot[1] = ((s[1] + a / 2) / a) as u8;
            slot[2] = ((s[2] + a / 2) / a) as u8;
            slot[3] = ((a + taps / 2) / taps) as u8;
        }
    }
}

/// Output rows a view pass splits across threads: more workers than this
/// only adds spawn cost, and a band under [`MIN_BAND_ROWS`] rows spends
/// more time setting up than sampling.
const MAX_SAMPLE_WORKERS: usize = 8;
const MIN_BAND_ROWS: usize = 8;
/// Below this many texels a pass is not worth spawning threads for.
const MIN_PARALLEL_TEXELS: usize = 96 * 1024;

/// Run `sample` over `rows` of a `cols`-wide RGBA image, one band per
/// worker, and return the number of bands. Every band writes only its own
/// rows and each texel accumulates only from tiles (never from another
/// texel), so any split produces the same bytes as a single band.
fn sample_in_bands(
    img: &mut [u8],
    cols: usize,
    rows: std::ops::Range<usize>,
    sample: impl Fn(usize, usize, &mut [u8]) + Sync,
) -> usize {
    let len = rows.len();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let workers = if cols * len < MIN_PARALLEL_TEXELS {
        1
    } else {
        cores
            .min(MAX_SAMPLE_WORKERS)
            .min((len / MIN_BAND_ROWS).max(1))
    };
    sample_in_bands_with(workers, img, cols, rows, sample)
}

/// [`sample_in_bands`] with the worker count given.
fn sample_in_bands_with(
    workers: usize,
    img: &mut [u8],
    cols: usize,
    rows: std::ops::Range<usize>,
    sample: impl Fn(usize, usize, &mut [u8]) + Sync,
) -> usize {
    if workers <= 1 {
        sample(rows.start, rows.end, img);
        return 1;
    }
    let per = rows.len().div_ceil(workers);
    let mut bands = 0;
    std::thread::scope(|scope| {
        let mut rest = img;
        let mut a = rows.start;
        while a < rows.end {
            let b = (a + per).min(rows.end);
            let (head, tail) = rest.split_at_mut((b - a) * cols * 4);
            rest = tail;
            let sample = &sample;
            scope.spawn(move || sample(a, b, head));
            bands += 1;
            a = b;
        }
    });
    bands
}

/// Minified sampling: a 2×2 bilinear of level-`shift` mip texels per output
/// texel. Same tile-major shape as [`view_pixels`] (each mip tile is read
/// once, taps land in the tile that owns them), with weights instead of a
/// uniform box because the mip texel is not aligned to the footprint.
fn view_pixels_mip(
    doc: &Document,
    plan: &StackPlan,
    cache: &mut CompositeCache,
    view: ViewRect,
    out: [usize; 2],
    region: [usize; 4],
    shift: u32,
) -> Vec<u8> {
    let [c0, r0, c1, r1] = region;
    let (cols, rows) = (c1 - c0, r1 - r0);
    let dx = (view.x1 - view.x0) / out[0] as f32;
    let dy = (view.y1 - view.y0) / out[1] as f32;
    let scale = (1u32 << shift) as f32;
    let mw = (doc.w + (1 << shift) - 1) >> shift;
    let mh = (doc.h + (1 << shift) - 1) >> shift;
    if mw == 0 || mh == 0 {
        return vec![0u8; cols * rows * 4];
    }
    let (max_x, max_y) = ((mw - 1) as f32, (mh - 1) as f32);
    let (mtx, mty) = (mw.div_ceil(TILE) as usize, mh.div_ceil(TILE) as usize);
    let (mw_us, mh_us) = (mw as usize, mh as usize);
    let tile = TILE as usize;
    // Build every mip tile the region samples before the bands start: the
    // bands only read the cache. A fit-zoomed 8K pass needs all of level 1
    // (64 MiB of mips) just to average level 2, more than the mip budget.
    cache.begin_pass();
    for mt_y in 0..mty {
        let band = (
            (mt_y * tile) as f32 * scale,
            ((mt_y + 1) * tile) as f32 * scale,
        );
        let (row_a, row_b) = texel_span(view.y0, dy, out[1], band.0 - dy * 0.5, band.1 + dy * 0.5);
        let (row_a, row_b) = (row_a.max(r0), row_b.min(r1));
        if row_a >= row_b {
            continue;
        }
        for mt_x in 0..mtx {
            let band = (
                (mt_x * tile) as f32 * scale,
                ((mt_x + 1) * tile) as f32 * scale,
            );
            let (col_a, col_b) =
                texel_span(view.x0, dx, out[0], band.0 - dx * 0.5, band.1 + dx * 0.5);
            let (col_a, col_b) = (col_a.max(c0), col_b.min(c1));
            if col_a >= col_b {
                continue;
            }
            cache.mip_tile(doc, plan, shift, mt_x as u32, mt_y as u32);
        }
    }
    let mut img = vec![0u8; cols * rows * 4];
    let pass = MipPass {
        cache: &*cache,
        view,
        out,
        region,
        cols,
        shift,
        scale,
        dx,
        dy,
        mw: mw_us,
        mh: mh_us,
        max_x,
        max_y,
        mtx,
        mty,
    };
    sample_in_bands(&mut img, cols, r0..r1, |row_a, row_b, dst| {
        pass.band(row_a, row_b, dst)
    });
    cache.end_pass();
    img
}

/// Minified sampling of a band of output rows: the texel math of
/// [`view_pixels_mip`], with the tile walk clipped to the band.
struct MipPass<'a> {
    cache: &'a CompositeCache,
    view: ViewRect,
    out: [usize; 2],
    region: [usize; 4],
    cols: usize,
    shift: u32,
    scale: f32,
    dx: f32,
    dy: f32,
    mw: usize,
    mh: usize,
    max_x: f32,
    max_y: f32,
    mtx: usize,
    mty: usize,
}

impl MipPass<'_> {
    fn band(&self, row_a: usize, row_b: usize, dst: &mut [u8]) {
        let [c0, _, c1, _] = self.region;
        let tile = TILE as usize;
        let stride = tile * 4;
        let cols = self.cols;
        // Alpha-weighted sums per texel: r·a·w, g·a·w, b·a·w, a·w. Weights
        // are 0..=255 per axis, so a texel's weights sum to 65025 and the
        // largest term (255·255·65025) still fits u32.
        let mut acc = vec![[0u32; 4]; (row_b - row_a) * cols];
        for mt_y in 0..self.mty {
            let band = (
                (mt_y * tile) as f32 * self.scale,
                ((mt_y + 1) * tile) as f32 * self.scale,
            );
            let (ra, rb) = texel_span(
                self.view.y0,
                self.dy,
                self.out[1],
                band.0 - self.dy * 0.5,
                band.1 + self.dy * 0.5,
            );
            let (ra, rb) = (ra.max(row_a), rb.min(row_b));
            if ra >= rb {
                continue;
            }
            for mt_x in 0..self.mtx {
                let band = (
                    (mt_x * tile) as f32 * self.scale,
                    ((mt_x + 1) * tile) as f32 * self.scale,
                );
                let (ca, cb) = texel_span(
                    self.view.x0,
                    self.dx,
                    self.out[0],
                    band.0 - self.dx * 0.5,
                    band.1 + self.dx * 0.5,
                );
                let (ca, cb) = (ca.max(c0), cb.min(c1));
                if ca >= cb {
                    continue;
                }
                let pixels = self
                    .cache
                    .level_tile(self.shift, mt_x as u32, mt_y as u32)
                    .expect("mip tile missing during a sampling pass");
                for row in ra..rb {
                    let u = (self.view.y0 + (row as f32 + 0.5) * self.dy) / self.scale - 0.5;
                    let y0f = u.floor();
                    let fy = ((u - y0f).clamp(0.0, 1.0) * 255.0).round() as u32;
                    let wy = [255 - fy, fy];
                    let ya = [
                        y0f.clamp(0.0, self.max_y) as usize,
                        (y0f + 1.0).clamp(0.0, self.max_y) as usize,
                    ];
                    for col in ca..cb {
                        let ux = (self.view.x0 + (col as f32 + 0.5) * self.dx) / self.scale - 0.5;
                        let x0f = ux.floor();
                        let fx = ((ux - x0f).clamp(0.0, 1.0) * 255.0).round() as u32;
                        let wx = [255 - fx, fx];
                        let xa = [
                            x0f.clamp(0.0, self.max_x) as usize,
                            (x0f + 1.0).clamp(0.0, self.max_x) as usize,
                        ];
                        let mut sum = [0u32; 4];
                        for (j, &my) in ya.iter().enumerate() {
                            if my / tile != mt_y || my >= self.mh {
                                continue;
                            }
                            let src_row = (my % tile) * stride;
                            for (i, &mx) in xa.iter().enumerate() {
                                if mx / tile != mt_x || mx >= self.mw {
                                    continue;
                                }
                                let w = wx[i] * wy[j];
                                let src = src_row + (mx % tile) * 4;
                                let px = &pixels[src..src + 4];
                                let a = px[3] as u32 * w;
                                sum[0] += px[0] as u32 * a;
                                sum[1] += px[1] as u32 * a;
                                sum[2] += px[2] as u32 * a;
                                sum[3] += a;
                            }
                        }
                        let s = &mut acc[(row - row_a) * cols + (col - c0)];
                        for k in 0..4 {
                            s[k] += sum[k];
                        }
                    }
                }
            }
        }
        const TOTAL: u32 = 255 * 255;
        for (slot, s) in dst.chunks_mut(4).zip(acc) {
            let a = s[3];
            if a == 0 {
                continue;
            }
            slot[0] = ((s[0] + a / 2) / a) as u8;
            slot[1] = ((s[1] + a / 2) / a) as u8;
            slot[2] = ((s[2] + a / 2) / a) as u8;
            slot[3] = ((a + TOTAL / 2) / TOTAL).min(255) as u8;
        }
    }
}

/// Output texel rect covering a dirty rect's canvas pixels, or `None` when
/// the dirty area is not sampled at all. Widened by one texel each way:
/// float rounding must never leave a stale texel behind, and one extra
/// texel costs nothing.
fn dirty_texels(view: ViewRect, out: [usize; 2], rect: (u32, u32, u32, u32)) -> Option<[usize; 4]> {
    let (x0, y0, x1, y1) = rect;
    if x0 > x1 || y0 > y1 || view.x1 <= view.x0 || view.y1 <= view.y0 {
        return None;
    }
    let span = |origin: f32, step: f32, len: usize, a: u32, b: u32| {
        let lo = ((a as f32 - origin) / step - 0.5).ceil();
        let hi = (((b + 1) as f32 - origin) / step - 0.5).ceil();
        let lo = (lo.max(0.0) as usize).saturating_sub(1).min(len);
        let hi = (hi.max(0.0) as usize).saturating_add(1).min(len);
        (lo, hi.max(lo))
    };
    let (c0, c1) = span(view.x0, (view.x1 - view.x0) / out[0] as f32, out[0], x0, x1);
    let (r0, r1) = span(view.y0, (view.y1 - view.y0) / out[1] as f32, out[1], y0, y1);
    if c0 >= c1 || r0 >= r1 {
        None
    } else {
        Some([c0, r0, c1, r1])
    }
}

/// What a [`ViewTex::render`] call has to do: rewrite the whole texture,
/// patch one texel rect, or nothing at all.
#[derive(Debug, PartialEq)]
pub(crate) enum ViewUpdate {
    Full,
    Partial([usize; 4]),
    None,
}

/// The pure full/partial/none decision behind [`ViewTex::render`].
///
/// A new view or output size re-samples everything. So does a revision
/// change with no dirty rect (the dirt is unknown). A dirty rect that maps
/// to no texel of the view — a symmetric dab on the hidden half, a filtered
/// region off-screen — is a no-op, not a full pass. A rect covering at
/// least half the texture is no cheaper than a full pass.
fn plan_update(
    key_matches: bool,
    rev_matches: bool,
    dirty: Option<(u32, u32, u32, u32)>,
    view: ViewRect,
    out: [usize; 2],
) -> ViewUpdate {
    if !key_matches {
        return ViewUpdate::Full;
    }
    // Dropping the pending dirt here is safe only because the engine sets
    // `dirty` and bumps `revision` together: no dirty rect is ever marked
    // without a revision change.
    if rev_matches {
        return ViewUpdate::None;
    }
    let Some(rect) = dirty else {
        return ViewUpdate::Full;
    };
    match dirty_texels(view, out, rect) {
        Some([c0, r0, c1, r1]) if (c1 - c0) * (r1 - r0) * 2 < out[0] * out[1] => {
            ViewUpdate::Partial([c0, r0, c1, r1])
        }
        Some(_) => ViewUpdate::Full,
        None => ViewUpdate::None,
    }
}

/// View key: what the texture currently in hand depicts. Revision is not
/// part of it — a dirty revision takes the cheaper partial path instead of
/// a full re-render. The cache epoch stands in for document identity,
/// because a replaced document can reuse a revision number.
fn view_key(view: ViewRect, out: [usize; 2], doc: (u32, u32), epoch: u64) -> u64 {
    let mut key = epoch;
    for part in [
        view.x0.to_bits() as u64,
        view.y0.to_bits() as u64,
        view.x1.to_bits() as u64,
        view.y1.to_bits() as u64,
        out[0] as u64,
        out[1] as u64,
        ((doc.0 as u64) << 32) | doc.1 as u64,
    ] {
        key = key.wrapping_mul(31).wrapping_add(part);
    }
    key
}

/// The screen-sized canvas texture of target B: the composite of the
/// visible rect, re-sampled when the view, the output size or the document
/// changes. Dirty revisions re-sample only the texels under the dirty rect
/// (a fit-zoomed 8K view is 256 MiB of composite against a 96 MiB mobile
/// cache, so a full pass per dab would thrash).
#[derive(Default)]
pub struct ViewTex {
    tex: Option<egui::TextureHandle>,
    /// The canvas rect the texture depicts: the draw path paints it there
    /// even on frames that reuse the texture (mid-gesture).
    view: Option<ViewRect>,
    key: Option<u64>,
    /// Document revision the texture was sampled at.
    rev: u64,
}

impl ViewTex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Canvas rect the current texture covers, if it has one.
    pub fn view(&self) -> Option<ViewRect> {
        self.view
    }

    /// The texture itself, if it has one.
    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.tex.as_ref()
    }

    /// Drop the texture and everything describing it, freeing its VRAM. A
    /// cleared view is a missing texture, so the next [`render`](Self::render)
    /// takes the full path.
    pub fn clear(&mut self) {
        self.tex = None;
        self.view = None;
        self.key = None;
        self.rev = 0;
    }

    /// Bring the texture up to date for `view` (canvas pixels) laid out as
    /// `out` texels. Full pass on a new view or output size, partial pass
    /// over the dirty rect otherwise; no work at all when nothing changed.
    pub fn render(
        &mut self,
        ctx: &egui::Context,
        doc: &mut Document,
        cache: &mut CompositeCache,
        view: ViewRect,
        out: [usize; 2],
    ) {
        let out = [out[0].max(1), out[1].max(1)];
        let key = view_key(view, out, (doc.w, doc.h), cache.epoch());
        let rev = doc.revision();
        // The viewport path is the only dirty consumer while it is active
        // (`refresh_texture` returns early), so this is its one drain.
        let dirty = doc.take_dirty();
        // One plan for the patch and the sampling pass: it snapshots the
        // revision, which the next dab bumps.
        let plan = doc.stack_plan();
        if let Some(rect) = dirty {
            cache.patch(doc, &plan, rect);
        }
        let update = plan_update(self.key == Some(key), rev == self.rev, dirty, view, out);
        // A dirty mip texel averages a 2^shift square, so the change spills
        // up to that far past the dirty rect in canvas pixels; widen the
        // re-sampled texels to cover it. Level 0 has no spill.
        let update = match update {
            ViewUpdate::Partial([c0, r0, c1, r1]) => {
                let dx = (view.x1 - view.x0) / out[0] as f32;
                let dy = (view.y1 - view.y0) / out[1] as f32;
                let shift = mip_shift(dx, dy);
                let pad = if shift == 0 {
                    0
                } else {
                    ((1u32 << shift) as f32 / dx.min(dy)).ceil() as usize + 1
                };
                ViewUpdate::Partial([
                    c0.saturating_sub(pad),
                    r0.saturating_sub(pad),
                    (c1 + pad).min(out[0]),
                    (r1 + pad).min(out[1]),
                ])
            }
            other => other,
        };
        match update {
            ViewUpdate::Full => self.full(ctx, doc, &plan, cache, view, out),
            ViewUpdate::Partial(region) => self.partial(doc, &plan, cache, view, out, region),
            ViewUpdate::None => {}
        }
        // Every path, including the no-op one, records what the texture now
        // depicts, so the next frame does not redo a pass the dirty rect
        // could not make cheaper.
        self.key = Some(key);
        self.rev = rev;
        self.view = Some(view);
    }

    fn full(
        &mut self,
        ctx: &egui::Context,
        doc: &Document,
        plan: &StackPlan,
        cache: &mut CompositeCache,
        view: ViewRect,
        out: [usize; 2],
    ) {
        let pixels = view_pixels(doc, plan, cache, view, out, None);
        let image = egui::ColorImage::from_rgba_unmultiplied(out, &pixels);
        // Update in place when possible: a reload would churn the texture id
        // and free the old one on every full pass (forced/desktop wheel).
        if let Some(handle) = &mut self.tex {
            handle.set(image, egui::TextureOptions::LINEAR);
        } else {
            self.tex = Some(ctx.load_texture("canvas-view", image, egui::TextureOptions::LINEAR));
        }
    }

    fn partial(
        &mut self,
        doc: &Document,
        plan: &StackPlan,
        cache: &mut CompositeCache,
        view: ViewRect,
        out: [usize; 2],
        region: [usize; 4],
    ) {
        let [c0, r0, c1, r1] = region;
        let pixels = view_pixels(doc, plan, cache, view, out, Some(region));
        let image = egui::ColorImage::from_rgba_unmultiplied([c1 - c0, r1 - r0], &pixels);
        if let Some(handle) = &mut self.tex {
            handle.set_partial([c0, r0], image, egui::TextureOptions::LINEAR);
        }
    }
}

#[cfg(test)]
mod tests {
    use procraft_engine::{BlendMode, Document, Raster};

    use super::*;

    fn painted(w: u32, h: u32, px: [u8; 4]) -> Document {
        let mut doc = Document::new(w, h);
        doc.layers[0].pixels.set4(0, px);
        doc
    }

    /// Content crossing tile seams in both axes: a semi-transparent base,
    /// a multiply layer, and an offset layer revealed through a mask.
    fn busy_doc(w: u32, h: u32) -> Document {
        let mut doc = Document::new(w, h);
        doc.background = [12, 34, 56, 255];
        let stride = w as usize;
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize * stride + x as usize) * 4;
                doc.layers[0].pixels.set4(
                    i,
                    [
                        (x * 3 % 256) as u8,
                        (y * 5 % 256) as u8,
                        ((x + y) * 7 % 256) as u8,
                        (64 + (x + y) % 192) as u8,
                    ],
                );
            }
        }
        doc.add_layer("multiply".into());
        doc.layers[1].blend = BlendMode::Multiply;
        for y in 0..h {
            for x in 0..w {
                if (x + y) % 3 == 0 {
                    let i = (y as usize * stride + x as usize) * 4;
                    doc.layers[1].pixels.set4(i, [255, 40, 220, 180]);
                }
            }
        }
        doc.add_layer("masked".into());
        doc.layers[2].opacity = 0.73;
        doc.layers[2].ox = 25;
        doc.layers[2].oy = -9;
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize * stride + x as usize) * 4;
                let v = ((x * 11 + y * 7) % 256) as u8;
                doc.layers[2].pixels.set4(i, [v, 255 - v, 90, 255]);
            }
        }
        let mask = doc.layers[2].mask.get_or_insert_with(|| Raster::mask(w, h));
        for y in 0..h {
            for x in 0..w {
                mask.set(
                    y as usize * stride + x as usize,
                    ((x * 13 + y * 5) % 256) as u8,
                );
            }
        }
        doc
    }

    fn assert_full_image_matches_flatten(w: u32, h: u32) {
        let doc = busy_doc(w, h);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let img = full_image(&doc, &plan, &mut cache);
        let flat = doc.flatten();
        assert_eq!(img.len(), w as usize * h as usize * 4);
        assert_eq!(img.len(), flat.len());
        // Compare in place: the vectors are ~1 MB, so a failing `assert_eq!`
        // would dump both. The first differing byte is the useful report.
        if let Some(i) = img.iter().zip(&flat).position(|(a, b)| a != b) {
            panic!(
                "first mismatch at byte {i}: full_image {:?} != flatten {:?}",
                &img[i..(i + 4).min(img.len())],
                &flat[i..(i + 4).min(flat.len())]
            );
        }
    }

    #[test]
    fn cache_serves_the_cached_tile_until_the_dirty_rect_is_patched() {
        let mut doc = painted(512, 512, [10, 20, 30, 255]);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        // Miss: composites from the document.
        assert_eq!(&cache.tile(&doc, &plan, 0, 0)[0..4], &[10, 20, 30, 255]);

        // Repaint and dirty the doc, but keep the texture: the next read of
        // an untouched cache entry must return the bytes it already holds.
        doc.layers[0].pixels.set4(0, [40, 50, 60, 255]);
        doc.mark_full_dirty();
        let plan = doc.stack_plan();
        assert_eq!(&cache.tile(&doc, &plan, 0, 0)[0..4], &[10, 20, 30, 255]);

        // Patching the dirty rect is what makes the cached tile current.
        cache.patch(&doc, &plan, (0, 0, 0, 0));
        assert_eq!(&cache.tile(&doc, &plan, 0, 0)[0..4], &[40, 50, 60, 255]);
    }

    #[test]
    fn cache_evicts_oldest_tiles_over_budget() {
        let mut doc = Document::new(1024, 1024);
        doc.layers[0].pixels.set4(0, [7, 8, 9, 255]);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(2 * TILE_BYTES);
        for (tx, ty) in [(0, 0), (1, 0), (2, 0)] {
            cache.tile(&doc, &plan, tx, ty);
        }
        assert!(cache.bytes() <= 2 * TILE_BYTES);
        assert_eq!(cache.bytes(), 2 * TILE_BYTES);
        assert!(!cache.tiles.contains_key(&(0, 0)), "oldest tile evicted");
        assert!(cache.tiles.contains_key(&(1, 0)));
        assert!(cache.tiles.contains_key(&(2, 0)));
    }

    #[test]
    fn cache_hits_refresh_lru_order() {
        let doc = Document::new(1024, 1024);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(2 * TILE_BYTES);
        cache.tile(&doc, &plan, 0, 0);
        cache.tile(&doc, &plan, 1, 0);
        // Touch the oldest so the middle one is next to go.
        cache.tile(&doc, &plan, 0, 0);
        cache.tile(&doc, &plan, 2, 0);
        assert!(cache.tiles.contains_key(&(0, 0)));
        assert!(!cache.tiles.contains_key(&(1, 0)));
        assert!(cache.tiles.contains_key(&(2, 0)));
    }

    #[test]
    fn tiny_budget_still_holds_one_tile() {
        let doc = Document::new(512, 256);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(0);
        cache.tile(&doc, &plan, 0, 0);
        cache.tile(&doc, &plan, 1, 0);
        assert_eq!(cache.bytes(), TILE_BYTES);
        assert_eq!(cache.tile(&doc, &plan, 1, 0).len(), TILE_BYTES);
    }

    #[test]
    fn tile_image_crops_at_the_canvas_edge() {
        let doc = Document::new(300, 5);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(256 * 1024);
        let tile = cache.tile(&doc, &plan, 1, 0);
        let (rgba, w, h) = tile_image(&doc, tile, 1, 0);
        assert_eq!((w, h), (44, 5));
        assert_eq!(rgba.len(), 44 * 5 * 4);
    }

    #[test]
    fn tile_image_region_clamps_to_tile_and_canvas() {
        let doc = Document::new(300, 5);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(256 * 1024);
        let tile = cache.tile(&doc, &plan, 1, 0).to_vec();
        // A rect straddling the tile seam: the upload covers only the part
        // inside tile (1,0) and the canvas (x < 300).
        let (rgba, ox, oy, w, h) = tile_image_region(&doc, &tile, 1, 0, (250, 2, 320, 900));
        assert_eq!((ox, oy, w, h), (256, 2, 44, 3));
        assert_eq!(rgba.len(), 44 * 3 * 4);
        // The first byte is canvas (256, 2): tile-local (0, 2).
        assert_eq!(&rgba[0..4], &tile[(2 * TILE * 4) as usize..][0..4]);
        // A rect that misses the tile yields nothing to upload.
        let (empty, _, _, w, _) = tile_image_region(&doc, &tile, 1, 0, (0, 0, 255, 4));
        assert!(empty.is_empty());
        assert_eq!(w, 0);
    }

    #[test]
    fn tile_image_keeps_interior_tiles_whole() {
        let mut doc = Document::new(512, 300);
        doc.layers[0].pixels.set4(0, [1, 2, 3, 255]);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let tile = cache.tile(&doc, &plan, 0, 0).to_vec();
        let (rgba, w, h) = tile_image(&doc, &tile, 0, 0);
        assert_eq!((w, h), (256, 256));
        assert_eq!(rgba, tile);
        // Bottom tile row is the 44 px remainder.
        let (rgba, w, h) = tile_image(&doc, &tile, 0, 1);
        assert_eq!((w, h), (256, 44));
        assert_eq!(rgba.len(), 256 * 44 * 4);
        // Past the edge there is nothing to upload.
        assert_eq!(tile_image(&doc, &tile, 2, 0), (Vec::new(), 0, 0));
    }

    #[test]
    fn bytes_tracks_cache_contents() {
        let doc = Document::new(256 * 3, 256);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        assert_eq!(cache.bytes(), 0);
        for tx in 0..3 {
            cache.tile(&doc, &plan, tx, 0);
        }
        assert_eq!(cache.bytes(), 3 * TILE_BYTES);
        // Hits are not double counted.
        cache.tile(&doc, &plan, 0, 0);
        assert_eq!(cache.bytes(), 3 * TILE_BYTES);
        // Mips are counted too, and cleared with the level-0 tiles.
        cache.mip_tile(&doc, &plan, 1, 0, 0);
        assert_eq!(
            cache.bytes(),
            (cache.tiles.len() + cache.mips.len()) * TILE_BYTES
        );
        cache.invalidate();
        assert_eq!(cache.bytes(), 0);
        assert!(cache.tiles.is_empty());
        assert!(cache.order.is_empty());
        assert!(cache.mips.is_empty());
    }

    #[test]
    fn full_image_matches_flatten() {
        // 3 × 2 tiles with edge cropping in both axes (88 px last column,
        // 44 px last row); the tile order bug shows up on the first row.
        assert_full_image_matches_flatten(600, 300);
    }

    #[test]
    fn full_image_matches_flatten_on_an_exact_tile_grid() {
        // 2 × 1 tiles, no cropping: ordering alone has to be right.
        assert_full_image_matches_flatten(512, 256);
    }

    #[test]
    fn full_image_is_row_major() {
        let mut doc = Document::new(600, 300);
        doc.background = [0, 0, 0, 255];
        let left = [200, 30, 40, 255];
        let right = [10, 220, 90, 255];
        for y in 0..TILE {
            for x in 0..2 * TILE {
                let px = if x < TILE { left } else { right };
                doc.layers[0].pixels.set4((y * doc.w + x) as usize * 4, px);
            }
        }
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let img = full_image(&doc, &plan, &mut cache);
        assert_eq!(img.len(), doc.w as usize * doc.h as usize * 4);
        // Canvas row 0 holds tile (0,0) then tile (1,0), side by side.
        assert_eq!(&img[0..4], &left);
        assert_eq!(&img[TILE as usize * 4..TILE as usize * 4 + 4], &right);
        // Tile-block order would have laid tile (1,0) a whole tile later.
        assert_ne!(
            &img[TILE_BYTES..TILE_BYTES + 4],
            &right,
            "tile (1,0) must not start a whole tile into the buffer"
        );
    }

    /// First differing byte instead of a multi-megabyte `assert_eq!` dump.
    fn assert_same_bytes(got: &[u8], want: &[u8], what: &str) {
        assert_eq!(got.len(), want.len(), "{what}: length");
        if let Some(i) = got.iter().zip(want).position(|(a, b)| a != b) {
            panic!(
                "{what}: first mismatch at byte {i}: {:?} != {:?}",
                &got[i..(i + 4).min(got.len())],
                &want[i..(i + 4).min(want.len())]
            );
        }
    }

    /// Two solid layers over a dark page, the top one translucent: the
    /// oracle is a real blend, and 600 x 300 crosses tile seams both ways.
    fn layered_doc(w: u32, h: u32, top_opacity: f32) -> Document {
        let mut doc = Document::new(w, h);
        doc.background = [7, 11, 13, 255];
        let base = [40, 90, 160, 255];
        for y in 0..h {
            for x in 0..w {
                doc.layers[0]
                    .pixels
                    .set4((y as usize * w as usize + x as usize) * 4, base);
            }
        }
        doc.add_layer("top".into());
        doc.layers[1].opacity = top_opacity;
        let top = [220, 60, 30, 255];
        for y in 0..h {
            for x in 0..w {
                doc.layers[1]
                    .pixels
                    .set4((y as usize * w as usize + x as usize) * 4, top);
            }
        }
        doc
    }

    #[test]
    fn viewport_trigger_follows_size_and_texture_limit() {
        assert!(
            !viewport_active(false, 2048, 16384),
            "2K is the largest canvas texture worth holding"
        );
        assert!(viewport_active(false, 4096, 16384), "4K takes the viewport");
        assert!(viewport_active(false, 8192, 16384), "8K takes the viewport");
        assert!(
            viewport_active(false, 8192, 4096),
            "GPU cannot hold the canvas"
        );
        assert!(viewport_active(true, 1024, 16384), "forced for captures");
        assert!(
            !viewport_active(false, 1024, 4096),
            "a small canvas still fits a 4K GPU"
        );
        assert!(
            viewport_active(false, 16384, 16384),
            "a legacy oversized canvas never asks for a too-large texture"
        );
    }

    #[test]
    fn canvas_texture_fit_ignores_a_cap_the_canvas_does_not_reach() {
        assert!(
            canvas_texture_fits(1024, 4096),
            "a small canvas fits a 4K GPU"
        );
        assert!(canvas_texture_fits(4096, 4096));
        assert!(
            !canvas_texture_fits(8192, 4096),
            "the canvas itself is too big"
        );
        assert!(canvas_texture_fits(8192, 8192));
        assert!(
            !canvas_texture_fits(16384, 16384),
            "legacy document above the app's own cap"
        );
    }

    #[test]
    fn maskless_view_yields_to_the_fallback() {
        assert!(maskless_view_blocks_fallback(true, false, false, true));
        assert!(!maskless_view_blocks_fallback(true, true, false, true));
        assert!(!maskless_view_blocks_fallback(false, false, false, true));
        assert!(!maskless_view_blocks_fallback(true, false, false, false));
        // Assist keeps its own upload path; do not touch the user's View.
        assert!(!maskless_view_blocks_fallback(true, false, true, true));
    }

    #[test]
    fn view_rect_maps_screen_zoom_to_canvas_pixels() {
        // zoom 2: 200x100 screen px cover 100x50 canvas px
        let v = ViewRect::for_screen(0.0, 0.0, 200.0, 100.0, 8192.0, 8192.0, 2.0);
        assert_eq!((v.x0, v.y0, v.x1, v.y1), (0.0, 0.0, 100.0, 50.0));
    }

    #[test]
    fn view_rect_clamps_to_the_canvas() {
        // Starts off the top-left corner and runs past the far edges.
        let v = ViewRect::for_screen(-50.0, -20.0, 400.0, 300.0, 300.0, 200.0, 1.0);
        assert_eq!((v.x0, v.y0, v.x1, v.y1), (0.0, 0.0, 300.0, 200.0));
        // Wholly off the right edge: an empty rect pinned to the edge.
        let v = ViewRect::for_screen(500.0, 10.0, 100.0, 100.0, 300.0, 200.0, 1.0);
        assert_eq!((v.x0, v.y0, v.x1, v.y1), (300.0, 10.0, 300.0, 110.0));
        // A rotated view maps the panel's four corners, not an axis-aligned
        // intersection, and clamps the box to the canvas.
        let v = ViewRect::from_canvas_points(
            [(10.0, 20.0), (40.0, 5.0), (60.0, 70.0), (-5.0, 30.0)],
            300.0,
            200.0,
        );
        assert_eq!((v.x0, v.y0, v.x1, v.y1), (0.0, 5.0, 60.0, 70.0));
    }

    #[test]
    fn view_pixels_matches_flatten_when_one_to_one() {
        // 3 x 2 tiles with edge cropping in both axes.
        let doc = layered_doc(600, 300, 0.65);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: 600.0,
            y1: 300.0,
        };
        let got = view_pixels(&doc, &plan, &mut cache, view, [600, 300], None);
        assert_same_bytes(&got, &doc.flatten(), "1:1 view");
    }

    #[test]
    fn view_pixels_point_samples_at_texel_centers() {
        // Every canvas pixel has its own color, so which pixel a texel took
        // is readable from the bytes.
        let (w, h) = (8u32, 4u32);
        let mut doc = Document::new(w, h);
        let color = |x: u32, y: u32| [x as u8 * 10 + 1, y as u8 * 10 + 2, 7, 255];
        for y in 0..h {
            for x in 0..w {
                doc.layers[0]
                    .pixels
                    .set4((y as usize * w as usize + x as usize) * 4, color(x, y));
            }
        }
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        // Shifted half a pixel: each texel center lands a whole pixel right
        // and down, so which pixel a texel took is readable from the bytes.
        let view = ViewRect {
            x0: 0.5,
            y0: 0.5,
            x1: w as f32 + 0.5,
            y1: h as f32 + 0.5,
        };
        let got = view_pixels(
            &doc,
            &plan,
            &mut cache,
            view,
            [w as usize, h as usize],
            None,
        );
        let mut want = Vec::new();
        for y in 0..h {
            for x in 0..w {
                want.extend_from_slice(&color((x + 1).min(w - 1), (y + 1).min(h - 1)));
            }
        }
        assert_same_bytes(&got, &want, "texel centers");
        assert_ne!(&got[0..4], &color(0, 0), "top-left corner is not sampled");
    }

    #[test]
    fn minified_view_averages_its_footprint() {
        // 4 px stripes minified 8x: point sampling returns pure black and
        // white texels (the stairstep strokes show at fit zoom); a box
        // filter returns the mean.
        let (w, h) = (64u32, 64u32);
        let mut doc = Document::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if (x / 4) % 2 == 0 { 255u8 } else { 0 };
                doc.layers[0]
                    .pixels
                    .set4((y as usize * w as usize + x as usize) * 4, [v, v, v, 255]);
            }
        }
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: w as f32,
            y1: h as f32,
        };
        let got = view_pixels(&doc, &plan, &mut cache, view, [8, 8], None);
        for texel in got.chunks(4) {
            assert!(
                (100..=160).contains(&texel[0]),
                "aliased texel {}",
                texel[0]
            );
            assert_eq!(texel[3], 255);
        }
    }

    #[test]
    fn minified_view_averages_across_tile_seams() {
        // Red tile side, blue tile side, one texel wide enough (7.88 px) to
        // straddle the seam at x = 256: it must mix, not pick a side.
        let (w, h) = (512u32, 8u32);
        let mut doc = Document::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let px = if x < 256 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                };
                doc.layers[0]
                    .pixels
                    .set4((y as usize * w as usize + x as usize) * 4, px);
            }
        }
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: w as f32,
            y1: h as f32,
        };
        let got = view_pixels(&doc, &plan, &mut cache, view, [65, 1], None);
        let seam = (256.0 / (w as f32 / 65.0)).floor() as usize;
        let texel = &got[seam * 4..seam * 4 + 4];
        assert!(
            (60..=195).contains(&texel[0]) && (60..=195).contains(&texel[2]),
            "seam texel did not mix: {texel:?}"
        );
        assert_eq!(texel[3], 255);
        // Away from the seam the sides stay pure.
        assert_eq!(&got[0..4], [255, 0, 0, 255]);
        assert_eq!(&got[(64 * 4)..(64 * 4 + 4)], [0, 0, 255, 255]);
    }

    #[test]
    fn mip_level_follows_the_texel_footprint() {
        // Below 2:1 the full-resolution path is exact; above it each
        // octave halves the mip resolution, capped at the deepest level.
        assert_eq!(mip_shift(1.0, 1.0), 0);
        assert_eq!(mip_shift(1.9, 1.9), 0);
        assert_eq!(mip_shift(2.0, 2.0), 1);
        assert_eq!(mip_shift(3.9, 3.9), 1);
        assert_eq!(mip_shift(13.0, 13.0), 3);
        assert_eq!(mip_shift(100.0, 100.0), MAX_MIP_LEVEL);
    }

    #[test]
    fn mip_tiles_average_the_level_below() {
        // 1024² opaque ramp, so level 2 has a full 2×2 of level-1 parents.
        let (w, h) = (1024u32, 1024u32);
        let mut doc = Document::new(w, h);
        for y in 0..h {
            for x in 0..w {
                doc.layers[0].pixels.set4(
                    (y as usize * w as usize + x as usize) * 4,
                    [x as u8, y as u8, 9, 255],
                );
            }
        }
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let base = cache.tile(&doc, &plan, 0, 0).to_vec();
        let m1 = cache.mip_tile(&doc, &plan, 1, 0, 0).to_vec();
        for my in 0..32usize {
            for mx in 0..32usize {
                let o = (my * TILE as usize + mx) * 4;
                let p = |x: usize, y: usize| -> [u8; 4] {
                    let s = ((y * 2) * TILE as usize + x * 2) * 4;
                    [base[s], base[s + 1], base[s + 2], base[s + 3]]
                };
                let (a, b, c, d) = (p(mx, my), p(mx + 1, my), p(mx, my + 1), p(mx + 1, my + 1));
                for k in 0..3 {
                    let want = (a[k] as u32 + b[k] as u32 + c[k] as u32 + d[k] as u32 + 2) / 4;
                    assert_eq!(m1[o + k], want as u8, "level 1 {mx},{my} channel {k}");
                }
                assert_eq!(m1[o + 3], 255);
            }
        }
        let m2 = cache.mip_tile(&doc, &plan, 2, 0, 0).to_vec();
        // Parent texel (0,0) averages the 2×2 of its first child tile.
        let q = |x: usize, y: usize| m1[(y * TILE as usize + x) * 4];
        let want = (q(0, 0) as u32 + q(1, 0) as u32 + q(0, 1) as u32 + q(1, 1) as u32 + 2) / 4;
        assert_eq!(m2[0], want as u8, "level 2 is the mean of level 1");
    }

    #[test]
    fn solid_canvas_stays_solid_at_every_mip_level() {
        // Seams and translucent edges would show as a different colour at
        // the mip boundary; a flat canvas must render flat at every zoom.
        let (w, h) = (1000u32, 760u32);
        let mut doc = Document::new(w, h);
        for y in 0..h {
            for x in 0..w {
                doc.layers[0].pixels.set4(
                    (y as usize * w as usize + x as usize) * 4,
                    [30, 120, 200, 255],
                );
            }
        }
        let plan = doc.stack_plan();
        for out in [[500, 380], [250, 190], [125, 95], [63, 48]] {
            let mut cache = CompositeCache::new(CACHE_DESKTOP);
            let view = ViewRect {
                x0: 0.0,
                y0: 0.0,
                x1: w as f32,
                y1: h as f32,
            };
            assert!(mip_shift(w as f32 / out[0] as f32, h as f32 / out[1] as f32) > 0);
            let got = view_pixels(&doc, &plan, &mut cache, view, out, None);
            for (i, texel) in got.chunks(4).enumerate() {
                assert_eq!(texel, [30, 120, 200, 255], "texel {i} at out {out:?}");
            }
        }
    }

    #[test]
    fn dirty_patch_updates_the_mips_over_it() {
        let (w, h) = (1024u32, 512u32);
        let mut doc = Document::new(w, h);
        doc.background = [0, 0, 0, 255];
        // A transparent layer over a black page: the composite is pure
        // black, so the dab is the only content to average.
        doc.layers[0].pixels.fill(0);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        // One level-1 tile spans 512 canvas px, one level-2 tile 1024.
        for level in 1..=2 {
            for tx in 0..(w >> (8 + level)) {
                cache.mip_tile(&doc, &plan, level, tx, 0);
            }
        }
        assert_eq!(cache.mips.len(), 5, "four level-1 tiles and one level-2");
        let l2_before = cache.mip_tile(&doc, &plan, 2, 0, 0).to_vec();
        let m0_before = cache.mip_tile(&doc, &plan, 1, 0, 0).to_vec();

        // Dab at canvas (600, 10): level-1 tile 1, texel (44, 5).
        doc.layers[0]
            .pixels
            .set4((10 * w + 600) as usize * 4, [200, 200, 200, 255]);
        let plan = doc.stack_plan();
        cache.patch(&doc, &plan, (600, 10, 600, 10));

        // The mip tiles stay cached but their affected texels are re-averaged:
        // one opaque pixel among three transparent ones.
        assert_eq!(cache.mips.len(), 5, "nothing dropped");
        let m1 = cache.mip_tile(&doc, &plan, 1, 1, 0).to_vec();
        let local = (5 * TILE as usize + 44) * 4;
        assert_eq!(&m1[local..local + 4], &[50, 50, 50, 255], "level 1 texel");
        let untouched = (5 * TILE as usize + 40) * 4;
        assert_eq!(&m1[untouched..untouched + 4], &[0, 0, 0, 255]);
        let m2 = cache.mip_tile(&doc, &plan, 2, 0, 0).to_vec();
        assert_ne!(m2, l2_before, "level 2 re-averaged from level 1");
        let m0 = cache.mip_tile(&doc, &plan, 1, 0, 0).to_vec();
        assert_eq!(m0, m0_before, "a tile the dab missed is untouched");
    }

    #[test]
    fn minified_partial_after_a_dab_matches_a_full_pass() {
        // The dirty rect is widened for the mip spill; if it is not, a
        // texel next to the repainted one keeps the stale average.
        let (w, h) = (2048u32, 1024u32);
        let mut doc = Document::new(w, h);
        doc.layers[0].pixels.fill(255);
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: w as f32,
            y1: h as f32,
        };
        let out = [160, 80];
        assert!(mip_shift(w as f32 / out[0] as f32, h as f32 / out[1] as f32) > 0);
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let first = view_pixels(&doc, &plan, &mut cache, view, out, None);
        let before = dirty_texels(view, out, (1000, 500, 1100, 560)).unwrap();
        assert!(first.chunks(4).all(|t| t == [255, 255, 255, 255]));

        // A dab marks the doc dirty; patch the tiles and the mips, then
        // re-sample only the region the app would.
        for y in 500..=560 {
            for x in 1000..=1100 {
                doc.layers[0].pixels.set4(
                    (y as usize * w as usize + x as usize) * 4,
                    [20, 20, 20, 255],
                );
            }
        }
        doc.mark_full_dirty();
        doc.take_dirty();
        let plan = doc.stack_plan();
        cache.patch(&doc, &plan, (1000, 500, 1100, 560));
        let pad = ((1u32 << 4) as f32 / 12.8).ceil() as usize + 1;
        let [c0, r0, c1, r1] = before;
        let region = [
            c0.saturating_sub(pad),
            r0.saturating_sub(pad),
            (c1 + pad).min(out[0]),
            (r1 + pad).min(out[1]),
        ];
        let part = view_pixels(&doc, &plan, &mut cache, view, out, Some(region));
        let full = view_pixels(&doc, &plan, &mut cache, view, out, None);
        let (pw, ph) = ((region[2] - region[0]), (region[3] - region[1]));
        let mut want = Vec::new();
        for row in 0..ph {
            let start = ((row + region[1]) * out[0] + region[0]) * 4;
            want.extend_from_slice(&full[start..start + pw * 4]);
        }
        assert_same_bytes(&part, &want, "minified partial");
    }

    #[test]
    fn view_region_equals_the_full_render_sub_rect() {
        let doc = layered_doc(300, 300, 0.65);
        let view = ViewRect {
            x0: 10.0,
            y0: 5.0,
            x1: 290.0,
            y1: 295.0,
        };
        let out = [140, 145];
        let region = [17, 9, 131, 141];
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let full = view_pixels(&doc, &plan, &mut cache, view, out, None);
        // Fresh plan and cache: the region pass must stand on its own.
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let part = view_pixels(&doc, &plan, &mut cache, view, out, Some(region));
        let [c0, r0, c1, r1] = region;
        let mut want = Vec::new();
        for row in r0..r1 {
            let start = (row * out[0] + c0) * 4;
            want.extend_from_slice(&full[start..start + (c1 - c0) * 4]);
        }
        assert_same_bytes(&part, &want, "dirty sub-rect");
    }

    #[test]
    fn view_render_uses_fresh_tiles_after_a_dab() {
        // 600 x 150 is three tiles wide, so patching tile (1,0) is visible
        // in the cache contents as well as in the pixels.
        let (w, h) = (600u32, 150u32);
        let mut doc = Document::new(w, h);
        let at = |x: u32, y: u32| (y as usize * w as usize + x as usize) * 4;
        doc.layers[0].pixels.set4(at(300, 50), [10, 20, 30, 255]);
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: w as f32,
            y1: h as f32,
        };
        let out = [w as usize, h as usize];
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let first = view_pixels(&doc, &plan, &mut cache, view, out, None);
        let px = at(300, 50);
        assert_eq!(&first[px..px + 4], &[10, 20, 30, 255]);
        assert!(cache.tiles.contains_key(&(1, 0)));

        // A dab inside tile (1,0). Sampling before the patch serves the
        // cached pre-edit tile, which is exactly why it must be patched.
        doc.layers[0].pixels.set4(at(300, 50), [200, 100, 50, 255]);
        doc.mark_full_dirty();
        let dirty = doc.take_dirty().expect("dab marked the doc dirty");
        assert_eq!(dirty, (0, 0, w - 1, h - 1));
        let plan = doc.stack_plan();
        let stale = view_pixels(&doc, &plan, &mut cache, view, out, None);
        assert_eq!(&stale[px..px + 4], &[10, 20, 30, 255], "cached tile");

        // The app patches only the dab's rect, then samples. The tile stays
        // cached (its other pixels are still valid) but its rect is fresh.
        cache.patch(&doc, &plan, (300, 50, 300, 50));
        let local = ((50 * TILE + (300 - TILE)) * 4) as usize;
        let tile = cache.tile(&doc, &plan, 1, 0);
        assert_eq!(
            &tile[local..local + 4],
            &[200, 100, 50, 255],
            "patched tile"
        );
        let fresh = view_pixels(&doc, &plan, &mut cache, view, out, None);
        assert_eq!(&fresh[px..px + 4], &[200, 100, 50, 255], "patched");
    }

    #[test]
    fn band_splits_write_disjoint_rows() {
        // Each band owns exactly its rows of the image, in order: the
        // threads must never overlap, or a pass would be nondeterministic.
        let (cols, rows) = (3usize, 64usize);
        let mut img = vec![0u8; cols * rows * 4];
        let bands = sample_in_bands_with(4, &mut img, cols, 0..rows, |a, _b, dst| {
            for (i, px) in dst.chunks_mut(4).enumerate() {
                let row = a + i / cols;
                px.copy_from_slice(&[row as u8, (i % cols) as u8, 7, 255]);
            }
        });
        assert!(bands > 1, "4 workers over 64 rows must split");
        for row in 0..rows {
            for col in 0..cols {
                let o = (row * cols + col) * 4;
                assert_eq!(
                    &img[o..o + 4],
                    &[row as u8, col as u8, 7, 255],
                    "{row},{col}"
                );
            }
        }
    }

    #[test]
    fn a_split_pass_matches_one_row_at_a_time() {
        // The threaded full pass must produce the same bytes as sampling
        // every row on its own (which is below the parallel threshold).
        let (w, h) = (800u32, 800u32);
        let mut doc = Document::new(w, h);
        doc.layers[0].pixels.fill(255);
        let stride = w as usize;
        for y in 0..h as usize {
            for x in 0..w as usize {
                if (x / 40 + y / 40) % 3 == 0 {
                    doc.layers[0]
                        .pixels
                        .set4((y * stride + x) * 4, [200, 30, 90, 255]);
                }
            }
        }
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: w as f32,
            y1: h as f32,
        };
        let out = [400, 400];
        let plan = doc.stack_plan();
        let mut cache = CompositeCache::new(CACHE_DESKTOP);
        let full = view_pixels(&doc, &plan, &mut cache, view, out, None);
        assert_eq!(full.len(), 400 * 400 * 4);
        let mut serial = Vec::with_capacity(full.len());
        for row in 0..out[1] {
            let band = view_pixels(
                &doc,
                &plan,
                &mut cache,
                view,
                out,
                Some([0, row, out[0], row + 1]),
            );
            serial.extend_from_slice(&band);
        }
        assert_eq!(full, serial);
    }

    /// The texel columns/rows whose sample centers land on dirty canvas
    /// pixels, brute-forced from the sampler's own formula: the ground truth
    /// `dirty_texels` must cover, never replace.
    fn texels_touching(
        view: ViewRect,
        out: [usize; 2],
        rect: (u32, u32, u32, u32),
    ) -> Option<[usize; 4]> {
        let (dx, dy) = (
            (view.x1 - view.x0) / out[0] as f32,
            (view.y1 - view.y0) / out[1] as f32,
        );
        let (rows, cols): (Vec<usize>, Vec<usize>) = (
            (0..out[1])
                .filter(|row| {
                    let y = (view.y0 + (*row as f32 + 0.5) * dy).floor() as u32;
                    (rect.1..=rect.3).contains(&y)
                })
                .collect(),
            (0..out[0])
                .filter(|col| {
                    let x = (view.x0 + (*col as f32 + 0.5) * dx).floor() as u32;
                    (rect.0..=rect.2).contains(&x)
                })
                .collect(),
        );
        match (rows.first(), rows.last(), cols.first(), cols.last()) {
            (Some(r0), Some(r1), Some(c0), Some(c1)) => Some([*c0, *r0, c1 + 1, r1 + 1]),
            _ => None,
        }
    }

    #[test]
    fn dirty_texels_covers_every_texel_sampling_the_dirty_pixels() {
        // 8K canvas, 2732-texel viewport: a fractional 2.9985 canvas px per
        // texel, the 3.0 device case where rounding is the hazard.
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: 8192.0,
            y1: 8192.0,
        };
        let out = [2732, 2732];
        let dirty = (1000, 2000, 1300, 2100);
        let got = dirty_texels(view, out, dirty).expect("dirty rect is on screen");
        let want = texels_touching(view, out, dirty).expect("some texel samples it");
        assert!(
            got[0] <= want[0] && got[1] <= want[1] && got[2] >= want[2] && got[3] >= want[3],
            "got {got:?} must cover every sampling texel {want:?}"
        );
        // Small enough to be worth a partial upload.
        assert_eq!(
            plan_update(true, false, Some(dirty), view, out),
            ViewUpdate::Partial(got)
        );
    }

    #[test]
    fn off_view_dirty_rects_are_a_no_op() {
        // The symmetry mirror dabs the hidden half; nothing to upload.
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: 1000.0,
            y1: 1000.0,
        };
        let out = [500, 500];
        let dirty = (2000, 100, 2100, 200);
        assert_eq!(dirty_texels(view, out, dirty), None);
        assert_eq!(texels_touching(view, out, dirty), None);
        assert_eq!(
            plan_update(true, false, Some(dirty), view, out),
            ViewUpdate::None
        );
    }

    #[test]
    fn a_canvas_sized_dirty_rect_takes_the_full_pass() {
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: 8192.0,
            y1: 8192.0,
        };
        let out = [2732, 2732];
        let dirty = (0, 0, 8191, 8191);
        let region = dirty_texels(view, out, dirty).expect("covers the view");
        assert_eq!((region[0], region[1]), (0, 0));
        assert!(region[2] >= out[0] - 1 && region[3] >= out[1] - 1);
        assert_eq!(
            plan_update(true, false, Some(dirty), view, out),
            ViewUpdate::Full
        );
    }

    #[test]
    fn a_new_view_or_an_unknown_dirty_rect_takes_the_full_pass() {
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: 8192.0,
            y1: 8192.0,
        };
        let out = [2732, 2732];
        // View/output key changed (even with the same revision).
        assert_eq!(plan_update(false, true, None, view, out), ViewUpdate::Full);
        // Revision moved but the dirty rect was already consumed: the dirt
        // is unknown, so everything is rewritten.
        assert_eq!(plan_update(true, false, None, view, out), ViewUpdate::Full);
    }

    #[test]
    fn an_unchanged_revision_does_no_work() {
        let view = ViewRect {
            x0: 4000.0,
            y0: 4000.0,
            x1: 5000.0,
            y1: 5000.0,
        };
        let out = [1024, 1024];
        assert_eq!(plan_update(true, true, None, view, out), ViewUpdate::None);
        assert_eq!(
            plan_update(true, true, Some((4100, 4100, 4200, 4200)), view, out),
            ViewUpdate::None
        );
    }
}
