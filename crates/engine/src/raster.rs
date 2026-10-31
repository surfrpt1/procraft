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

/// Zero-filled buffer of `len` bytes, allocated fallibly: `vec![0; len]`
/// aborts the process on OOM, which is the wrong answer for a big export
/// on a tablet where the app can still show an error.
pub(crate) fn try_zeroed(len: usize) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    buf.try_reserve_exact(len)
        .map_err(|e| format!("out of memory for {len} bytes: {e}"))?;
    buf.resize(len, 0);
    Ok(buf)
}

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

    /// Copy `run` pixels of source row `sy` onto row `dy` of `self`,
    /// starting at `sx`/`dx`. Both rasters share this geometry. The run is
    /// split at tile borders, so a 8K-wide band costs one `memcpy` per
    /// 256-px segment instead of a `get`/`set` per byte. A zero source
    /// segment leaves an unallocated destination tile alone.
    pub fn copy_run(&mut self, src: &Raster, sx: u32, sy: u32, dx: u32, dy: u32, run: u32) {
        debug_assert_eq!((self.w, self.h, self.bpp), (src.w, src.h, src.bpp));
        let bpp = self.bpp();
        let mut done = 0;
        while done < run {
            let x = dx + done;
            let tx = x / TILE;
            let px = x % TILE;
            let n = (TILE - px).min(run - done);
            let sxx = sx + done;
            let stx = sxx / TILE;
            let spx = sxx % TILE;
            let len = n as usize * bpp;
            let dst_off = ((dy % TILE) * TILE + px) as usize * bpp;
            let src_off = ((sy % TILE) * TILE + spx) as usize * bpp;
            let slot = ((dy / TILE) * self.tiles_x + tx) as usize;
            match src.tile(stx, sy / TILE) {
                Some(st) => {
                    let chunk = &st[src_off..src_off + len];
                    if chunk.iter().any(|&b| b != 0) || self.tiles[slot].is_some() {
                        let t = self.tile_mut(tx, dy / TILE);
                        t[dst_off..dst_off + len].copy_from_slice(chunk);
                    }
                }
                None => {
                    if self.tiles[slot].is_some() {
                        let t = self.tile_mut(tx, dy / TILE);
                        t[dst_off..dst_off + len].fill(0);
                    }
                }
            }
            done += n;
        }
    }

    /// Paint `run` pixels of row `y` from `x` with `v`. Same tile-splitting
    /// as [`Raster::copy_run`]; a zero fill into an unallocated tile is a
    /// no-op so blank rows stay free.
    pub fn fill_run(&mut self, y: u32, x: u32, run: u32, v: u8) {
        let bpp = self.bpp();
        let mut done = 0;
        while done < run {
            let cx = x + done;
            let tx = cx / TILE;
            let px = cx % TILE;
            let n = (TILE - px).min(run - done);
            let ty = y / TILE;
            let slot = (ty * self.tiles_x + tx) as usize;
            if v != 0 || self.tiles[slot].is_some() {
                let off = ((y % TILE) * TILE + px) as usize * bpp;
                let len = n as usize * bpp;
                let t = self.tile_mut(tx, ty);
                t[off..off + len].fill(v);
            }
            done += n;
        }
    }

    pub fn to_dense(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.len()];
        self.fill_dense(&mut out);
        out
    }

    /// [`Self::to_dense`] with a fallible allocation: one 8K RGBA layer is
    /// ~132 MB, so an export that cannot get the buffer reports an error
    /// instead of aborting.
    pub fn try_to_dense(&self) -> Result<Vec<u8>, String> {
        let mut out = try_zeroed(self.len())?;
        self.fill_dense(&mut out);
        Ok(out)
    }

    fn fill_dense(&self, out: &mut [u8]) {
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

    /// Slide all content by (`sx`, `sy`) pixels with zero fill; content that
    /// leaves the canvas is dropped. Tile-granular: destination tiles that
    /// receive only zeros stay `None`, and only tiles that receive non-zero
    /// bytes are allocated.
    ///
    /// Same rule as the dense slide this replaces: destination pixel
    /// `(x, y)` takes source pixel `(x - sx, y - sy)`, zero outside the old
    /// bounds. Works for both pixel (`bpp == 4`) and mask (`bpp == 1`)
    /// rasters, and copies per source-tile row run, so no whole-layer
    /// buffer is ever materialised.
    pub fn slide_in_place(&mut self, sx: i32, sy: i32) {
        if (sx == 0 && sy == 0) || self.is_empty() {
            return;
        }
        let bpp = self.bpp();
        let tile_len = self.tile_len();
        let (w, h) = (self.w as i64, self.h as i64);
        let (sx, sy) = (sx as i64, sy as i64);
        let side = TILE as i64;
        // Read from the old grid while the new one is assembled; `next` is
        // only the per-raster pointer row, never a full-canvas byte buffer.
        let old = std::mem::take(&mut self.tiles);
        let mut next: Vec<Option<Arc<Vec<u8>>>> = vec![None; old.len()];
        // Reused scratch for one destination tile at a time. It only goes
        // back into the pool when it came out all zero, so a fetched
        // buffer never needs re-clearing.
        let mut scratch: Option<Vec<u8>> = None;
        for dty in 0..self.tiles_y {
            for dtx in 0..self.tiles_x {
                let (dx0, dy0) = (dtx as i64 * side, dty as i64 * side);
                let dx1 = (dx0 + side).min(w);
                let dy1 = (dy0 + side).min(h);
                // Part of the destination tile that reads from inside the
                // old canvas; the rest of the tile stays zero.
                let (x0, x1) = (dx0.max(sx), dx1.min(sx + w));
                let (y0, y1) = (dy0.max(sy), dy1.min(sy + h));
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                let mut out = scratch.take().unwrap_or_else(|| vec![0u8; tile_len]);
                for y in y0..y1 {
                    let src_y = y - sy;
                    let (sty, local_y) = ((src_y / side) as u32, (src_y % side) as usize);
                    let src_row = sty * self.tiles_x;
                    let dst_row = (y - dy0) as usize * TILE as usize;
                    let mut x = x0;
                    while x < x1 {
                        let src_x = x - sx;
                        let stx = (src_x / side) as u32;
                        // A run never straddles a source tile boundary.
                        let seg_end = x1.min((stx + 1) as i64 * side + sx);
                        if let Some(src) = old[(src_row + stx) as usize].as_deref() {
                            let n = (seg_end - x) as usize;
                            let s = (local_y * TILE as usize + (src_x % side) as usize) * bpp;
                            let d = (dst_row + (x - dx0) as usize) * bpp;
                            out[d..d + n * bpp].copy_from_slice(&src[s..s + n * bpp]);
                        }
                        x = seg_end;
                    }
                }
                if out.iter().any(|&b| b != 0) {
                    next[(dty * self.tiles_x + dtx) as usize] = Some(Arc::new(out));
                } else {
                    scratch = Some(out);
                }
            }
        }
        self.tiles = next;
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
    /// of different geometry do not share a seed.
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
    fn try_zeroed_reports_impossible_sizes_instead_of_aborting() {
        assert_eq!(try_zeroed(3).unwrap(), vec![0u8; 3]);
        // A request no allocator can satisfy must be an Err, not an abort.
        assert!(try_zeroed(usize::MAX).is_err());
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
    fn copy_run_spans_tile_borders_and_skips_blank_sources() {
        let (w, h) = (600u32, 2u32);
        let at = |x: u32, y: u32| ((y * w + x) * 4) as usize;
        let mut src = Raster::rgba(w, h);
        for x in 0..w {
            src.set4(at(x, 0), [x as u8, 1, 2, 3]);
        }
        let mut dst = Raster::rgba(w, h);
        dst.copy_run(&src, 0, 0, 0, 1, w);
        for x in 0..w {
            assert_eq!(dst.get4(at(x, 1)), [x as u8, 1, 2, 3], "x={x}");
        }
        assert_eq!(
            dst.get4(at(0, 0)),
            [0; 4],
            "the source row itself is untouched"
        );

        // A blank source neither writes nor allocates.
        let empty = Raster::rgba(w, h);
        let mut fresh = Raster::rgba(w, h);
        fresh.copy_run(&empty, 0, 0, 0, 1, w);
        assert_eq!(fresh.alloc_bytes(), 0);
    }

    #[test]
    fn fill_run_spans_tile_borders_and_skips_blank_tiles() {
        let mut m = Raster::mask(600, 1);
        m.fill_run(0, 10, 580, 255);
        let dense = m.to_dense();
        assert!(dense[..10].iter().all(|&v| v == 0));
        assert!(dense[10..590].iter().all(|&v| v == 255));
        assert!(dense[590..].iter().all(|&v| v == 0));
        // Clearing blank tiles with zeros allocates nothing.
        let mut blank = Raster::mask(600, 2);
        blank.fill_run(1, 0, 600, 0);
        assert_eq!(blank.alloc_bytes(), 0);
    }

    /// Per-pixel reference for the slide rule: `dst(x, y)` takes
    /// `src(x - sx, y - sy)`, zero outside the old bounds. Deliberately
    /// dense and naive.
    fn dense_slide(bytes: &[u8], w: u32, h: u32, bpp: usize, sx: i32, sy: i32) -> Vec<u8> {
        let mut out = vec![0u8; bytes.len()];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let (ox, oy) = (x - sx, y - sy);
                if ox < 0 || oy < 0 || ox >= w as i32 || oy >= h as i32 {
                    continue;
                }
                let s = (oy as usize * w as usize + ox as usize) * bpp;
                let d = (y as usize * w as usize + x as usize) * bpp;
                out[d..d + bpp].copy_from_slice(&bytes[s..s + bpp]);
            }
        }
        out
    }

    #[test]
    fn slide_matches_the_dense_reference() {
        let (w, h) = (600u32, 600u32); // 3x3 tiles, seams at 256 and 512
        let mut dense = vec![0u8; (w * h * 4) as usize];
        // A square straddling both the x and the y seam, so each copied run
        // crosses a source tile edge and lands in the next tile.
        for y in 230..290u32 {
            for x in 200..340u32 {
                let i = ((y * w + x) * 4) as usize;
                dense[i..i + 4].copy_from_slice(&[(x % 251) as u8, (y % 251) as u8, 255, 200]);
            }
        }
        let mut r = Raster::from_dense(w, h, 4, &dense);
        assert_eq!(r.allocated_tiles().count(), 4, "the square spans 2x2 tiles");
        r.slide_in_place(7, -3);
        assert_eq!(r.to_dense(), dense_slide(&dense, w, h, 4, 7, -3));
        assert_eq!(
            r.allocated_tiles().count(),
            4,
            "exactly the tiles the square moved into"
        );
        assert!(r.tile(2, 0).is_none(), "blank tiles stay None");
        assert!(r.tile(0, 2).is_none());
        assert!(r.tile(2, 2).is_none());

        // A blank raster slides without allocating anything.
        let mut blank = Raster::rgba(w, h);
        let before = blank.allocated_tiles().count();
        blank.slide_in_place(7, -3);
        assert_eq!(blank.allocated_tiles().count(), before);
        assert_eq!(before, 0);
    }

    #[test]
    fn slide_is_a_no_op_for_zero_shift() {
        let mut r = Raster::rgba(600, 300);
        r.set4(((10 * 600 + 10) * 4) as usize, [1, 2, 3, 4]);
        r.set4(((200 * 600 + 400) * 4) as usize, [5, 6, 7, 8]);
        let before = r.to_dense();
        let tiles = r
            .allocated_tiles()
            .map(|(tx, ty, _)| (tx, ty))
            .collect::<Vec<_>>();
        r.slide_in_place(0, 0);
        assert_eq!(r.to_dense(), before);
        assert_eq!(
            r.allocated_tiles()
                .map(|(tx, ty, _)| (tx, ty))
                .collect::<Vec<_>>(),
            tiles
        );
    }

    #[test]
    fn slide_works_on_masks() {
        let (w, h) = (300u32, 5u32);
        let mut m = Raster::mask(w, h);
        for y in 0..h {
            for x in 0..w {
                m.set((y * w + x) as usize, 255);
            }
        }
        assert_eq!(m.bpp(), 1);
        let dense = m.to_dense();
        m.slide_in_place(-256, 0);
        assert_eq!(m.bpp(), 1);
        assert_eq!(m.to_dense(), dense_slide(&dense, w, h, 1, -256, 0));
        // Destination x reads source x + 256: the first 44 columns keep
        // content and the rest of the row is zero fill.
        assert_eq!(m.get((4 * w) as usize), 255);
        assert_eq!(m.get((4 * w + 43) as usize), 255);
        assert_eq!(m.get((4 * w + 44) as usize), 0);
        assert_eq!(m.get((4 * w + 299) as usize), 0);
        assert_eq!(
            m.allocated_tiles().count(),
            1,
            "only the left tile column holds content; the right one stays None"
        );
    }

    #[test]
    fn slide_trims_content_that_leaves_the_canvas() {
        let (w, h) = (300u32, 4u32);
        let mut r = Raster::rgba(w, h);
        r.set4(0, [1, 2, 3, 4]); // (0, 0)
        r.set4((299 * 4) as usize, [5, 6, 7, 8]); // (299, 0)
        r.set4(((3 * w + 10) * 4) as usize, [9, 10, 11, 12]); // (10, 3)
        let dense = r.to_dense();
        r.slide_in_place(-3, 2);
        assert_eq!(r.to_dense(), dense_slide(&dense, w, h, 4, -3, 2));
        // The left column and the bottom row fall off the canvas: dropped,
        // never wrapped onto the opposite edge.
        assert_eq!(r.get4(0), [0; 4], "nothing wrapped onto (0, 0)");
        assert_eq!(
            r.get4(((2 * w + 296) * 4) as usize),
            [5, 6, 7, 8],
            "the survivor moved by the shift"
        );
        assert_eq!(r.get4(((2 * w) * 4) as usize), [0; 4]);
        assert_eq!(
            r.get4(((3 * w + 10) * 4) as usize),
            [0; 4],
            "the bottom row is not wrapped to the top"
        );
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
