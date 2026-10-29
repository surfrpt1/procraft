//! Dense 8K layers are stored as a grid of copy-on-write 256² tiles.
//!
//! A `Raster` reads like a dense buffer (`get`/`set` take the same byte
//! index a `Vec<u8>` used) but only allocates tiles that hold content:
//! an untouched 8K layer costs one `Vec` of `Option` pointers, and undo
//! and duplicate share every untouched tile through `Arc`.

use std::fmt;
use std::sync::Arc;

/// Tile side in pixels. 256² RGBA is 256 KiB; 256² masks are 64 KiB.
pub const TILE: u32 = 256;

/// Largest canvas side the project-file reader accepts. `new` trips a
/// `debug_assert` past it so misuse fails legibly in debug builds.
const MAX_SIDE: u32 = 16384;

#[derive(Clone)]
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
        debug_assert!(
            w <= MAX_SIDE && h <= MAX_SIDE,
            "canvas {w}x{h} exceeds the {MAX_SIDE}px side cap"
        );
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
        self.tile_arc(tx, ty).map(|t| t.as_slice())
    }

    fn tile_arc(&self, tx: u32, ty: u32) -> Option<&Arc<Vec<u8>>> {
        if tx >= self.tiles_x || ty >= self.tiles_y {
            return None;
        }
        self.tiles[(ty * self.tiles_x + tx) as usize].as_ref()
    }

    /// Mutable tile access; materialises the tile on first write and
    /// copies it when it is still shared with a snapshot.
    ///
    /// # Panics
    ///
    /// Panics if `tx >= tiles_x` or `ty >= tiles_y`. Out-of-range indices
    /// are rejected rather than wrapped: `tx == tiles_x` would otherwise
    /// alias the first tile of the next row.
    pub fn tile_mut(&mut self, tx: u32, ty: u32) -> &mut [u8] {
        assert!(
            tx < self.tiles_x && ty < self.tiles_y,
            "tile ({tx},{ty}) outside {}x{}",
            self.tiles_x,
            self.tiles_y
        );
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
        let (x, y) = ((px % self.w as usize) as u32, (px / self.w as usize) as u32);
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

    /// Write one dense byte. A zero written into a `None` slot is a no-op
    /// and allocates nothing; a zero written into an allocated tile still
    /// goes through `Arc::make_mut`, so a shared snapshot keeps its byte.
    pub fn set(&mut self, i: usize, v: u8) {
        let Some((slot, off)) = self.locate(i) else {
            return;
        };
        if let Some(arc) = self.tiles[slot].as_mut() {
            Arc::make_mut(arc)[off] = v;
        } else if v != 0 {
            let mut tile = vec![0u8; self.tile_len()];
            tile[off] = v;
            self.tiles[slot] = Some(Arc::new(tile));
        }
    }

    /// Read four consecutive dense *bytes* at byte offset `i` (the engine's
    /// existing convention), not four pixels. On a `bpp == 1` mask this
    /// spans four pixels.
    pub fn get4(&self, i: usize) -> [u8; 4] {
        [
            self.get(i),
            self.get(i + 1),
            self.get(i + 2),
            self.get(i + 3),
        ]
    }

    /// Write four consecutive dense *bytes* at byte offset `i`. Like `get4`
    /// this always spans 4 bytes, so on a `bpp == 1` mask it touches four
    /// pixels; a byte outside the canvas is dropped.
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
        self.tiles.fill(None);
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
            self.tiles.fill(None);
            return;
        }
        let len = self.tile_len();
        for t in &mut self.tiles {
            let arc = t.get_or_insert_with(|| Arc::new(vec![0u8; len]));
            Arc::make_mut(arc).fill(v);
        }
    }

    /// Cheap content fingerprint for preview-guard comparisons: never
    /// materialises dense bytes.
    ///
    /// Sampling: one byte per 4096 bytes of *each allocated tile*. An
    /// already-allocated tile whose edited byte falls between two samples
    /// is not noticed, so equal fingerprints do not prove equal content.
    /// This is a preview guard, never a correctness oracle.
    ///
    /// `w`, `h` and `bpp` are mixed into the seed, so equal-length rasters
    /// of different geometry cannot collide.
    pub fn fingerprint(&self) -> u64 {
        let mut h = 0xcbf29ce484222325u64 ^ (self.len() as u64);
        for v in [self.w as u64, self.h as u64, self.bpp as u64] {
            h ^= v;
            h = h.wrapping_mul(0x100000001b3);
        }
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

/// A zero-size RGBA canvas, i.e. the same value as `Raster::rgba(0, 0)`.
/// The derived `Default` left `bpp == 0`, a geometry no constructor makes
/// (`new` clamps `0` up to `1`); picking `4` keeps the default RGBA-shaped.
impl Default for Raster {
    fn default() -> Self {
        Self::new(0, 0, 4)
    }
}

/// Geometry and allocation only, never the bytes: an 8K raster in an
/// `assert_eq!` failure message must not print hundreds of megabytes.
impl fmt::Debug for Raster {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let allocated = self.tiles.iter().filter(|t| t.is_some()).count();
        f.debug_struct("Raster")
            .field("w", &self.w)
            .field("h", &self.h)
            .field("bpp", &self.bpp)
            .field("tiles_x", &self.tiles_x)
            .field("tiles_y", &self.tiles_y)
            .field("allocated_tiles", &allocated)
            .finish()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_roundtrip_matches_input() {
        let (w, h) = (300u32, 200u32);
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
        let (w, h) = (512u32, 512u32);
        let mut bytes = vec![0u8; (w * h * 4) as usize];
        bytes[((511 * w + 511) * 4) as usize] = 7;
        let r = Raster::from_dense(w, h, 4, &bytes);
        assert_eq!(
            r.alloc_bytes(),
            r.tile_len(),
            "only the touched tile exists"
        );
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

    #[test]
    fn fingerprint_mixes_geometry_into_its_seed() {
        let wide = Raster::rgba(600, 1); // 2400 dense bytes
        let tall = Raster::rgba(300, 2); // 2400 dense bytes, other geometry
        let deep = Raster::new(300, 1, 8); // 2400 dense bytes, other bpp
        assert_eq!(wide.len(), tall.len());
        assert_eq!(wide.len(), deep.len());
        assert_ne!(
            wide.fingerprint(),
            tall.fingerprint(),
            "w/h are mixed into the seed"
        );
        assert_ne!(
            wide.fingerprint(),
            deep.fingerprint(),
            "bpp is mixed into the seed"
        );
    }

    #[test]
    fn truncated_dense_leaves_the_tail_zero_and_unallocated() {
        let bytes = vec![1u8; 300 * 4 * 2]; // two of five rows
        let r = Raster::from_dense(300, 5, 4, &bytes);
        assert_eq!(r.get(0), 1);
        assert_eq!(r.get((299 * 4) as usize), 1);
        assert_eq!(r.get((300 * 4) as usize), 1);
        assert_eq!(r.get((2 * 300 * 4 - 1) as usize), 1);
        assert_eq!(r.get((2 * 300 * 4) as usize), 0, "the tail reads as zero");
        assert_eq!(r.get((5 * 300 * 4 - 1) as usize), 0);
        let mut expected = vec![0u8; 300 * 5 * 4];
        expected[..bytes.len()].copy_from_slice(&bytes);
        assert_eq!(r.to_dense(), expected);
        assert_eq!(
            r.allocated_tiles()
                .map(|(tx, ty, _)| (tx, ty))
                .collect::<Vec<_>>(),
            vec![(0, 0), (1, 0)],
            "rows 0-1 span both tile columns of the single tile row"
        );
        assert_eq!(r.allocated_tiles().count(), 2);
    }

    #[test]
    fn zero_writes_do_not_materialise_tiles() {
        let mut r = Raster::rgba(600, 600);
        r.set(0, 0);
        r.set4(0, [0, 0, 0, 0]);
        r.set(((300 * 600 + 300) * 4) as usize, 0);
        assert_eq!(
            r.allocated_tiles().count(),
            0,
            "zero into a None slot allocates nothing"
        );
        assert_eq!(r.alloc_bytes(), 0);

        r.set(0, 9);
        let mut copy = r.clone();
        copy.set(0, 0);
        assert_eq!(r.get(0), 9, "CoW: the snapshot keeps the old byte");
        assert_eq!(copy.get(0), 0);
        assert_eq!(
            copy.allocated_tiles().count(),
            1,
            "a zero write keeps the allocated tile"
        );
    }

    #[test]
    #[should_panic(expected = "tile (3,0) outside 3x3")]
    fn tile_mut_rejects_a_column_past_the_grid() {
        let mut r = Raster::rgba(600, 600); // 3x3 tiles
        r.tile_mut(3, 0);
    }

    #[test]
    #[should_panic(expected = "tile (0,3) outside 3x3")]
    fn tile_mut_rejects_a_row_past_the_grid() {
        let mut r = Raster::rgba(600, 600); // 3x3 tiles
        r.tile_mut(0, 3);
    }

    #[test]
    fn debug_prints_geometry_not_bytes() {
        let mut r = Raster::rgba(300, 200);
        r.set(0, 7);
        assert_eq!(
            format!("{r:?}"),
            "Raster { w: 300, h: 200, bpp: 4, tiles_x: 2, tiles_y: 1, allocated_tiles: 1 }"
        );
    }

    #[test]
    fn default_is_a_zero_size_rgba_canvas() {
        let d = Raster::default();
        assert_eq!(d.bpp(), 4, "not the derived bpp == 0");
        assert_eq!(d.len(), 0);
        assert_eq!(d.tiles_x(), 0);
        assert_eq!(d, Raster::rgba(0, 0), "default is the RGBA canvas");
    }
}
