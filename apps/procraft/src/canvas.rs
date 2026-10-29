//! Canvas display: composite tiles behind an LRU cache.
//!
//! The canvas texture is dense (`doc.w` × `doc.h`), but the composite is
//! produced and uploaded tile by tile so a dab costs one 256² tile pass
//! instead of a full-canvas one. [`CompositeCache`] holds the tiles that
//! were recomposited while the texture survived, and [`tile_image`]
//! crops a tile to the canvas edge so every upload fits the dense
//! texture without changing its size (UVs stay 0..1).
//!
//! The cache is keyed by tile only: a tile is trusted until something
//! evicts it. Content edits evict the tiles their dirty rect touches
//! (see `ProCraftApp::refresh_texture`); dropping the canvas texture
//! (new/open/resize/undo-driven rebuild) clears the whole cache.

use procraft_engine::{Document, StackPlan, MAX_CANVAS_SIDE, TILE};

/// Cache budget on phones and tablets: a 4K canvas fits whole, an 8K
/// one keeps the working set of the tiles being painted.
pub const CACHE_MOBILE: usize = 96 * 1024 * 1024;
/// Cache budget on desktop: an 8K canvas (256 MiB of composite) plus
/// headroom for the neighbouring tiles.
pub const CACHE_DESKTOP: usize = 320 * 1024 * 1024;

/// Whether the display takes the viewport-resolution fallback (spec §4)
/// instead of one canvas-sized texture: forced for captures, native mobile
/// above 4K, or any GPU whose texture limit is below the 8K canvas cap.
pub fn viewport_active(
    forced: bool,
    native_mobile: bool,
    side: u32,
    max_texture_side: usize,
) -> bool {
    forced || (native_mobile && side > 4096) || max_texture_side < MAX_CANVAS_SIDE as usize
}

/// Bytes of one full composite tile, which is what the engine returns.
const TILE_BYTES: usize = (TILE * TILE * 4) as usize;

/// Composite tiles, least-recently-used first when over budget.
pub struct CompositeCache {
    tiles: std::collections::HashMap<(u32, u32), Vec<u8>>,
    /// Recency order, oldest at the front.
    order: std::collections::VecDeque<(u32, u32)>,
    bytes: usize,
    budget: usize,
    /// Bumped by [`invalidate`](Self::invalidate). The viewport texture keys
    /// on it because a content-identity change (new/open/undo rebuild) can
    /// reset `Document::revision()`, so the revision alone cannot tell two
    /// documents apart.
    epoch: u64,
}

impl CompositeCache {
    pub fn new(budget: usize) -> Self {
        Self {
            tiles: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
            bytes: 0,
            // One tile must always fit, or `tile` could not return the
            // tile it just composited.
            budget: budget.max(TILE_BYTES),
            epoch: 0,
        }
    }

    /// Bytes currently held: the layers-panel memory readout.
    pub fn bytes(&self) -> usize {
        self.bytes
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
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Drop one tile so the next [`tile`](Self::tile) recomposites it.
    /// The dirty-rect path evicts before reading: a cached tile there
    /// would be the pre-edit composite of the very tile being repainted.
    pub fn evict(&mut self, tx: u32, ty: u32) {
        if let Some(old) = self.tiles.remove(&(tx, ty)) {
            self.bytes -= old.len();
            self.order.retain(|k| *k != (tx, ty));
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

/// Evict every tile a dirty rect (inclusive canvas bounds) touches, so the
/// next read recomposites them. Mirrors `ProCraftApp::refresh_texture`'s
/// dirty loop: without it the viewport would sample pre-edit tiles.
pub(crate) fn evict_tiles(cache: &mut CompositeCache, rect: (u32, u32, u32, u32)) {
    let (x0, y0, x1, y1) = rect;
    if x0 > x1 || y0 > y1 {
        return;
    }
    for ty in y0 / TILE..=y1 / TILE {
        for tx in x0 / TILE..=x1 / TILE {
            cache.evict(tx, ty);
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

/// Sample a viewport out of the composite tiles: `out` texels, nearest at
/// each texel center, row-major RGBA. `region` is a half-open
/// `[col0, row0, col1, row1]` texel rect to render (`None`: all of it).
///
/// Tile-major on purpose: a fit-zoomed viewport is millions of texels, and
/// one cache lookup per texel would dominate the frame. Each canvas tile is
/// fetched once, then every texel whose center lands in it is copied — the
/// inner loop is pure sampling.
pub fn view_pixels(
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
    let mut img = vec![0u8; cols * rows * 4];
    if cols == 0 || rows == 0 || doc.w == 0 || doc.h == 0 {
        return img;
    }
    let (vx, vy) = (view.x1 - view.x0, view.y1 - view.y0);
    if vx <= 0.0 || vy <= 0.0 {
        return img;
    }
    let dx = vx / out[0] as f32;
    let dy = vy / out[1] as f32;
    let tile = TILE as usize;
    let (max_x, max_y) = ((doc.w - 1) as f32, (doc.h - 1) as f32);
    for ty in 0..doc.tiles_y() {
        let band = ((ty * TILE) as f32, ((ty + 1) * TILE) as f32);
        let (row_a, row_b) = texel_span(view.y0, dy, out[1], band.0, band.1);
        let (row_a, row_b) = (row_a.max(r0), row_b.min(r1));
        if row_a >= row_b {
            continue;
        }
        for tx in 0..doc.tiles_x() {
            let band = ((tx * TILE) as f32, ((tx + 1) * TILE) as f32);
            let (col_a, col_b) = texel_span(view.x0, dx, out[0], band.0, band.1);
            let (col_a, col_b) = (col_a.max(c0), col_b.min(c1));
            if col_a >= col_b {
                continue;
            }
            // One cache read per tile: the loops below only copy bytes.
            let pixels = cache.tile(doc, plan, tx, ty);
            for row in row_a..row_b {
                let y = (view.y0 + (row as f32 + 0.5) * dy)
                    .floor()
                    .clamp(0.0, max_y) as usize;
                let src_row = (y % tile) * tile * 4;
                let dst_row = (row - r0) * cols * 4;
                for col in col_a..col_b {
                    let x = (view.x0 + (col as f32 + 0.5) * dx)
                        .floor()
                        .clamp(0.0, max_x) as usize;
                    let src = src_row + (x % tile) * 4;
                    let dst = dst_row + (col - c0) * 4;
                    if let (Some(px), Some(slot)) =
                        (pixels.get(src..src + 4), img.get_mut(dst..dst + 4))
                    {
                        slot.copy_from_slice(px);
                    }
                }
            }
        }
    }
    img
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
    ) -> &egui::TextureHandle {
        let out = [out[0].max(1), out[1].max(1)];
        let key = view_key(view, out, (doc.w, doc.h), cache.epoch());
        let rev = doc.revision();
        // The viewport path is the only dirty consumer while it is active
        // (`refresh_texture` returns early), so this is its one drain.
        let dirty = doc.take_dirty();
        if let Some(rect) = dirty {
            evict_tiles(cache, rect);
        }
        if self.tex.is_none() || self.key != Some(key) {
            self.full(ctx, doc, cache, view, out);
        } else if rev != self.rev {
            let region = dirty.and_then(|r| dirty_texels(view, out, r));
            // A dirty area covering half the texture is no cheaper than a
            // full pass (and a missing rect means the dirt is unknown).
            match region {
                Some([c0, r0, c1, r1]) if (c1 - c0) * (r1 - r0) * 2 < out[0] * out[1] => {
                    self.partial(doc, cache, view, out, [c0, r0, c1, r1]);
                }
                _ => self.full(ctx, doc, cache, view, out),
            }
        }
        self.key = Some(key);
        self.rev = rev;
        self.view = Some(view);
        self.tex.as_ref().expect("render built a texture")
    }

    fn full(
        &mut self,
        ctx: &egui::Context,
        doc: &Document,
        cache: &mut CompositeCache,
        view: ViewRect,
        out: [usize; 2],
    ) {
        let plan = doc.stack_plan();
        let pixels = view_pixels(doc, &plan, cache, view, out, None);
        let image = egui::ColorImage::from_rgba_unmultiplied(out, &pixels);
        self.tex = Some(ctx.load_texture("canvas-view", image, egui::TextureOptions::LINEAR));
    }

    fn partial(
        &mut self,
        doc: &Document,
        cache: &mut CompositeCache,
        view: ViewRect,
        out: [usize; 2],
        region: [usize; 4],
    ) {
        let [c0, r0, c1, r1] = region;
        let plan = doc.stack_plan();
        let pixels = view_pixels(doc, &plan, cache, view, out, Some(region));
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
    fn cache_serves_the_cached_tile_until_it_is_evicted() {
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

        // Evicting the tile is what makes the dirty path recomposite it.
        cache.evict(0, 0);
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
        cache.evict(0, 0);
        assert_eq!(cache.bytes(), 2 * TILE_BYTES);
        cache.invalidate();
        assert_eq!(cache.bytes(), 0);
        assert!(cache.tiles.is_empty());
        assert!(cache.order.is_empty());
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
        assert!(!viewport_active(false, false, 8192, 8192));
        assert!(viewport_active(false, true, 8192, 8192), "mobile above 4K");
        assert!(
            !viewport_active(false, true, 4096, 8192),
            "4K still fits on mobile"
        );
        assert!(
            viewport_active(false, false, 8192, 4096),
            "GPU cannot hold the canvas"
        );
        assert!(
            viewport_active(true, false, 1024, 8192),
            "forced for captures"
        );
        assert!(
            viewport_active(false, false, 1024, 4096),
            "GPU below the 8K ceiling takes the fallback"
        );
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
        let view = ViewRect {
            x0: 0.0,
            y0: 0.0,
            x1: w as f32,
            y1: h as f32,
        };
        // Half size: centers fall on canvas (1,1), (3,1) ... not (0,0).
        let got = view_pixels(&doc, &plan, &mut cache, view, [4, 2], None);
        let mut want = Vec::new();
        for y in [1u32, 3] {
            for x in [1u32, 3, 5, 7] {
                want.extend_from_slice(&color(x, y));
            }
        }
        assert_same_bytes(&got, &want, "texel centers");
        assert_ne!(&got[0..4], &color(0, 0), "top-left corner is not sampled");
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
        // 600 x 150 is three tiles wide, so evicting tile (1,0) is visible
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

        // A dab inside tile (1,0). Sampling before the eviction serves the
        // cached pre-edit tile, which is exactly why it must be evicted.
        doc.layers[0].pixels.set4(at(300, 50), [200, 100, 50, 255]);
        doc.mark_full_dirty();
        let dirty = doc.take_dirty().expect("dab marked the doc dirty");
        assert_eq!(dirty, (0, 0, w - 1, h - 1));
        let plan = doc.stack_plan();
        let stale = view_pixels(&doc, &plan, &mut cache, view, out, None);
        assert_eq!(&stale[px..px + 4], &[10, 20, 30, 255], "cached tile");

        // The app evicts the tiles the dab's rect touches, then samples.
        evict_tiles(&mut cache, (300, 50, 300, 50));
        assert!(!cache.tiles.contains_key(&(1, 0)), "dirty tile evicted");
        assert!(cache.tiles.contains_key(&(0, 0)));
        assert!(cache.tiles.contains_key(&(2, 0)));
        let fresh = view_pixels(&doc, &plan, &mut cache, view, out, None);
        assert_eq!(&fresh[px..px + 4], &[200, 100, 50, 255], "recomposited");
    }
}
