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

/// Crop one composite tile to the canvas edge for `set_partial`,
/// returning `(rgba, w, h)`. A tile wholly outside the canvas yields
/// `(empty, 0, 0)`; a missing slice bottom is zero-filled rather than
/// panicking, like the engine's own out-of-canvas pixels.
pub fn tile_image(doc: &Document, tile: &[u8], tx: u32, ty: u32) -> (Vec<u8>, usize, usize) {
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
    use procraft_engine::Document;

    use super::*;

    fn painted(w: u32, h: u32, px: [u8; 4]) -> Document {
        let mut doc = Document::new(w, h);
        doc.layers[0].pixels.set4(0, px);
        doc
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
}
