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

use procraft_engine::{Document, StackPlan, TILE};

/// Cache budget on phones and tablets: a 4K canvas fits whole, an 8K
/// one keeps the working set of the tiles being painted.
pub const CACHE_MOBILE: usize = 96 * 1024 * 1024;
/// Cache budget on desktop: an 8K canvas (256 MiB of composite) plus
/// headroom for the neighbouring tiles.
pub const CACHE_DESKTOP: usize = 320 * 1024 * 1024;

/// Bytes of one full composite tile, which is what the engine returns.
const TILE_BYTES: usize = (TILE * TILE * 4) as usize;

/// Composite tiles, least-recently-used first when over budget.
pub struct CompositeCache {
    tiles: std::collections::HashMap<(u32, u32), Vec<u8>>,
    /// Recency order, oldest at the front.
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
            // One tile must always fit, or `tile` could not return the
            // tile it just composited.
            budget: budget.max(TILE_BYTES),
        }
    }

    /// Bytes currently held (tests, and the memory readout that will show
    /// it next to the layer total).
    #[allow(dead_code)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Drop every tile: the next read recomposites from the document.
    /// Called wherever the canvas texture is dropped, because that is
    /// when a full upload will read every tile again.
    pub fn invalidate(&mut self) {
        self.tiles.clear();
        self.order.clear();
        self.bytes = 0;
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
}
