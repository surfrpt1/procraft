//! Liquify: sparse displacement field and stroke rendering.

use super::*;

/// Sparse per-canvas-pixel sample offsets for one liquify stroke, in
/// lazily allocated `TILE`-side tiles of `[f32; 2]` (x, y). A pixel with no
/// entry has never been pushed and is never resampled, so an untouched 8K
/// layer costs nothing and untouched pixels keep their exact bytes. Tiles
/// are copy-on-write like the layer rasters, so an autosave snapshot of the
/// open document shares the field instead of copying it.
#[derive(Clone, Debug)]
struct LiquifyField {
    tiles_x: u32,
    tiles: std::collections::HashMap<u32, std::sync::Arc<Vec<f32>>>,
}

impl LiquifyField {
    fn new(w: u32) -> Self {
        Self {
            tiles_x: w.div_ceil(TILE),
            tiles: std::collections::HashMap::new(),
        }
    }

    /// Dense copy of `rect` (inclusive) into `out`, row-major with stride =
    /// rect width; returns that stride. Copies whole tile rows, so a
    /// disc-sized read does not pay a hash lookup per pixel.
    fn try_snapshot_into(
        &self,
        rect: (u32, u32, u32, u32),
        out: &mut Vec<[f32; 2]>,
    ) -> Result<u32, String> {
        let (x0, y0, x1, y1) = rect;
        let rw = x1 - x0 + 1;
        try_fill(out, (rw * (y1 - y0 + 1)) as usize, [0.0f32; 2])?;
        for ty in (y0 / TILE)..=(y1 / TILE) {
            for tx in (x0 / TILE)..=(x1 / TILE) {
                let Some(t) = self.tiles.get(&(ty * self.tiles_x + tx)) else {
                    continue;
                };
                let gx0 = x0.max(tx * TILE);
                let gx1 = x1.min(tx * TILE + TILE - 1);
                let gy0 = y0.max(ty * TILE);
                let gy1 = y1.min(ty * TILE + TILE - 1);
                for gy in gy0..=gy1 {
                    let src = (((gy % TILE) * TILE + gx0 % TILE) * 2) as usize;
                    let dst = ((gy - y0) * rw + (gx0 - x0)) as usize;
                    for k in 0..(gx1 - gx0 + 1) as usize {
                        out[dst + k] = [t[src + k * 2], t[src + k * 2 + 1]];
                    }
                }
            }
        }
        Ok(rw)
    }

    /// Write a dense `rect` (row-major, `stride` pixels per row) back into
    /// the tiles, materialising each touched tile once instead of once per
    /// pixel.
    fn try_commit(
        &mut self,
        rect: (u32, u32, u32, u32),
        dense: &[[f32; 2]],
        stride: u32,
    ) -> Result<(), String> {
        let (x0, y0, x1, y1) = rect;
        let tile_len = (TILE * TILE * 2) as usize;
        for ty in (y0 / TILE)..=(y1 / TILE) {
            for tx in (x0 / TILE)..=(x1 / TILE) {
                let gx0 = x0.max(tx * TILE);
                let gx1 = x1.min(tx * TILE + TILE - 1);
                let gy0 = y0.max(ty * TILE);
                let gy1 = y1.min(ty * TILE + TILE - 1);
                let key = ty * self.tiles_x + tx;
                if !self.tiles.contains_key(&key) {
                    // Reserve the slot before filling it: the insert itself
                    // must not be the allocation that aborts.
                    self.tiles
                        .try_reserve(1)
                        .map_err(|e| format!("out of memory for the liquify field: {e}"))?;
                    self.tiles
                        .insert(key, std::sync::Arc::new(try_vec(tile_len, 0.0f32)?));
                }
                let tile = self
                    .tiles
                    .get_mut(&key)
                    .expect("field tile was just materialised");
                if std::sync::Arc::strong_count(tile) > 1 {
                    let mut copy = try_vec(tile_len, 0.0f32)?;
                    copy.copy_from_slice(tile);
                    *tile = std::sync::Arc::new(copy);
                }
                let t = std::sync::Arc::get_mut(tile).expect("field tile is unshared");
                for gy in gy0..=gy1 {
                    let dst = (((gy % TILE) * TILE + gx0 % TILE) * 2) as usize;
                    let src = ((gy - y0) * stride + (gx0 - x0)) as usize;
                    for k in 0..(gx1 - gx0 + 1) as usize {
                        t[dst + k * 2] = dense[src + k][0];
                        t[dst + k * 2 + 1] = dense[src + k][1];
                    }
                }
            }
        }
        Ok(())
    }
}

/// One liquify stroke: the layer as it was when the stroke opened plus the
/// displacement field, which is carried along with the content it moves (a
/// push shifts the offsets of the pixels it touches, like dragging paint).
/// Rendering happens once per pointer segment over the swept region, so a
/// pixel is resampled once per stroke instead of once per push: a drag can
/// no longer soften what its own earlier pushes already warped.
#[derive(Clone, Debug)]
pub(super) struct LiquifyStroke {
    layer: usize,
    src: Raster,
    field: LiquifyField,
    /// Dense scratch reused by the pushes and the segment render so a drag
    /// frame does not allocate per push. Cleared after use; only the
    /// capacity (never the bytes) persists across frames.
    scratch_off: Vec<[f32; 2]>,
    scratch_disc: Vec<[f32; 2]>,
    scratch_cov: Vec<u8>,
    scratch_src: Vec<u8>,
    scratch_row: Vec<u8>,
    /// `Document::rev` after the last render. Any other write invalidates
    /// the snapshot, so the next push starts a fresh stroke from the
    /// current pixels instead of resurrecting stale ones.
    rev: u64,
}

impl LiquifyStroke {
    fn new(index: usize, pixels: &Raster, canvas_w: u32, rev: u64) -> Result<Self, String> {
        Ok(Self {
            layer: index,
            src: pixels.try_clone()?,
            field: LiquifyField::new(canvas_w),
            scratch_off: Vec::new(),
            scratch_disc: Vec::new(),
            scratch_cov: Vec::new(),
            scratch_src: Vec::new(),
            scratch_row: Vec::new(),
            rev,
        })
    }
}

/// Group pushed rects that overlap or touch, so a deferred render covers
/// the swept path once while mirror copies far apart stay separate (their
/// joint bounding box would span the canvas between them).
fn merge_rects(rects: Vec<(u32, u32, u32, u32)>) -> Vec<Vec<(u32, u32, u32, u32)>> {
    fn touch(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> bool {
        a.0 <= b.2.saturating_add(1)
            && b.0 <= a.2.saturating_add(1)
            && a.1 <= b.3.saturating_add(1)
            && b.1 <= a.3.saturating_add(1)
    }
    let mut groups: Vec<Vec<(u32, u32, u32, u32)>> = Vec::new();
    for r in rects {
        let hits: Vec<usize> = groups
            .iter()
            .enumerate()
            .filter(|(_, parts)| parts.iter().any(|&p| touch(p, r)))
            .map(|(i, _)| i)
            .collect();
        match hits.split_first() {
            None => groups.push(vec![r]),
            Some((&first, rest)) => {
                for &i in rest.iter().rev() {
                    let merged = groups.remove(i);
                    groups[first].extend(merged);
                }
                groups[first].push(r);
            }
        }
    }
    groups
}

/// Split one pointer segment into liquify pushes no longer than `max_step`,
/// so a fast drag warps the whole segment in this frame instead of trailing
/// the pointer. The 64-push ceiling keeps a pathological one-frame jump
/// from stalling the frame.
pub(super) fn liquify_steps(
    from: (f32, f32),
    to: (f32, f32),
    max_step: f32,
) -> Vec<((f32, f32), (f32, f32))> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return Vec::new();
    }
    let steps = (len / max_step.max(1.0)).ceil().clamp(1.0, 64.0);
    let mut pushes = Vec::with_capacity(steps as usize);
    for i in 1..=steps as u32 {
        let t = i as f32 / steps;
        let t0 = (i - 1) as f32 / steps;
        let (sx, sy) = (dx * (t - t0), dy * (t - t0));
        pushes.push(((from.0 + dx * t, from.1 + dy * t), (sx, sy)));
    }
    pushes
}

impl Document {
    /// M40 Liquify push: displace layer pixels along the drag vector
    /// `(dx, dy)` within `size` px of `(x, y)`. `strength` 0..=1 scales the
    /// push; falloff is smooth to the rim. Honors the active selection.
    /// Repeated calls (or one `liquify_begin`/`liquify_end` stroke) accumulate
    /// into a displacement field carried along with the content, and re-render
    /// the swept region from the stroke's snapshot, so a stroke resamples each
    /// pixel at most once: in place resampling would smear the previous warp
    /// again on every push. Zero push is a no-op.
    ///
    /// An allocation failure aborts the stroke and restores the layer to the
    /// stroke snapshot, so a drag that runs out of memory keeps the pixels it
    /// had before it started.
    #[allow(clippy::too_many_arguments)]
    pub fn liquify(
        &mut self,
        index: usize,
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        size: f32,
        strength: f32,
    ) -> Result<(), String> {
        if index >= self.layers.len() || size < 1.0 || strength <= 0.0 {
            return Ok(());
        }
        if dx == 0.0 && dy == 0.0 {
            return Ok(());
        }
        if self.w == 0 || self.h == 0 {
            return Ok(());
        }
        let mut stroke = self.take_stroke(index)?;
        let mut dirty = Vec::new();
        for (px, py, pdx, pdy) in self.liquify_mirrors(x, y, dx, dy) {
            match self.liquify_field_push(&mut stroke, px, py, pdx, pdy, size, strength) {
                Ok(Some(rect)) => dirty.push(rect),
                Ok(None) => {}
                Err(e) => return Err(self.abort_liquify(stroke, e)),
            }
        }
        if let Err(e) = self.flush_liquify(&mut stroke, dirty) {
            return Err(self.abort_liquify(stroke, e));
        }
        self.liquify = Some(stroke);
        Ok(())
    }

    /// Open a liquify stroke on `index`: the snapshot every push resamples
    /// from. The app calls it once per drag (right after the undo
    /// checkpoint) and [`Self::liquify_end`] on release; pushes also open a
    /// stroke on their own when none is active.
    pub fn liquify_begin(&mut self, index: usize) -> Result<(), String> {
        if let Some(stroke) = self.liquify.as_ref() {
            if stroke.layer == index && stroke.rev == self.rev {
                return Ok(());
            }
        }
        let Some(layer) = self.layers.get(index) else {
            return Ok(());
        };
        match LiquifyStroke::new(index, &layer.pixels, self.w, self.rev) {
            Ok(stroke) => {
                self.liquify = Some(stroke);
                Ok(())
            }
            Err(e) => {
                self.liquify = None;
                Err(e)
            }
        }
    }

    /// Close the active liquify stroke and free its snapshot and field.
    pub fn liquify_end(&mut self) {
        self.liquify = None;
    }

    /// Warp along one pointer segment. The segment is split into pushes no
    /// longer than an eighth of the brush size (min 2 px) so a fast drag
    /// warps all of it this frame instead of trailing the pointer; the last
    /// push ends exactly at the pointer. The field updates of the whole
    /// segment accumulate first and the swept region renders once, so a long
    /// jump costs one resample of the path instead of one full disc render
    /// per sub-push.
    ///
    /// Like [`Self::liquify`], an allocation failure rolls the whole stroke
    /// back to its snapshot and returns the error.
    pub fn liquify_segment(
        &mut self,
        index: usize,
        from: (f32, f32),
        to: (f32, f32),
        size: f32,
        strength: f32,
    ) -> Result<(), String> {
        if index >= self.layers.len() || size < 1.0 || strength <= 0.0 {
            return Ok(());
        }
        if self.w == 0 || self.h == 0 {
            return Ok(());
        }
        let steps = liquify_steps(from, to, (size * 0.125).max(2.0));
        if steps.is_empty() {
            return Ok(());
        }
        let mut stroke = self.take_stroke(index)?;
        let mut dirty = Vec::new();
        for ((x, y), (dx, dy)) in steps {
            for (px, py, pdx, pdy) in self.liquify_mirrors(x, y, dx, dy) {
                match self.liquify_field_push(&mut stroke, px, py, pdx, pdy, size, strength) {
                    Ok(Some(rect)) => dirty.push(rect),
                    Ok(None) => {}
                    Err(e) => return Err(self.abort_liquify(stroke, e)),
                }
            }
        }
        if let Err(e) = self.flush_liquify(&mut stroke, dirty) {
            return Err(self.abort_liquify(stroke, e));
        }
        self.liquify = Some(stroke);
        Ok(())
    }

    /// Reuse the open stroke only while its snapshot still matches the
    /// layer: any other write (paint, undo, filter) starts a new one.
    fn take_stroke(&mut self, index: usize) -> Result<LiquifyStroke, String> {
        match self.liquify.take() {
            Some(stroke) if stroke.layer == index && stroke.rev == self.rev => Ok(stroke),
            _ => LiquifyStroke::new(index, &self.layers[index].pixels, self.w, self.rev),
        }
    }

    /// Drop a failed stroke whole and put the layer back to the stroke
    /// snapshot, so a partial field or a half-written region never leaks
    /// into the next frame. Returns the original error for the caller.
    fn abort_liquify(&mut self, stroke: LiquifyStroke, err: String) -> String {
        self.liquify = None;
        let index = stroke.layer;
        if let Some(layer) = self.layers.get_mut(index) {
            layer.pixels = stroke.src;
        }
        self.mark_full_dirty();
        err
    }

    /// One push per symmetry mirror, deduped when a mirror lands on an
    /// identical (point, vector) pair (e.g. kaleidoscope center).
    fn liquify_mirrors(&self, x: f32, y: f32, dx: f32, dy: f32) -> Vec<(f32, f32, f32, f32)> {
        let (ccx, ccy) = self.symmetry_center_px();
        let pushes = self
            .symmetry
            .mirrored_pushes(self.symmetry_segments, ccx, ccy, x, y, dx, dy);
        let mut done: Vec<(f32, f32, f32, f32)> = Vec::with_capacity(pushes.len());
        for (px, py, pdx, pdy) in pushes {
            if done.iter().any(|&(a, b, c, d)| {
                (a - px).abs() < 0.001
                    && (b - py).abs() < 0.001
                    && (c - pdx).abs() < 0.001
                    && (d - pdy).abs() < 0.001
            }) {
                continue;
            }
            done.push((px, py, pdx, pdy));
        }
        done
    }

    /// Render the pushed rects once each (grouped by proximity) and store
    /// the stroke's new revision.
    fn flush_liquify(
        &mut self,
        stroke: &mut LiquifyStroke,
        dirty: Vec<(u32, u32, u32, u32)>,
    ) -> Result<(), String> {
        for group in merge_rects(dirty) {
            self.liquify_render(stroke, &group)?;
        }
        Ok(())
    }

    /// Add one warp to the stroke's field. The disc is read from a dense
    /// snapshot, rewritten dense, then committed tile by tile, so a push
    /// pays one hash lookup per tile rather than one per pixel. Returns the
    /// disc rect to render, or `None` when the push moves nothing.
    #[allow(clippy::too_many_arguments)]
    fn liquify_field_push(
        &mut self,
        stroke: &mut LiquifyStroke,
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        size: f32,
        strength: f32,
    ) -> Result<Option<(u32, u32, u32, u32)>, String> {
        let (w, h) = (self.w, self.h);
        let radius = size * 0.5;
        let bx0 = ((x - radius).floor().max(0.0)) as u32;
        let by0 = ((y - radius).floor().max(0.0)) as u32;
        let bx1 = ((x + radius).ceil().min(w as f32 - 1.0).max(0.0)) as u32;
        let by1 = ((y + radius).ceil().min(h as f32 - 1.0).max(0.0)) as u32;
        if bx0 > bx1 || by0 > by1 {
            return Ok(None);
        }
        let selection = self.selection;
        let mask = self.selection_mask.as_deref();
        let dw = (bx1 - bx0 + 1) as usize;
        let dh = (by1 - by0 + 1) as usize;
        // The displacement this push gives each pixel (reverse sample offset
        // direction: content moves along `(dx, dy)`).
        let mut disp = std::mem::take(&mut stroke.scratch_disc);
        try_fill(&mut disp, dw * dh, [0.0f32; 2])?;
        let mut step = 0.0f32;
        let r2 = radius * radius;
        for gy in 0..dh {
            let py = by0 + gy as u32;
            for gx in 0..dw {
                let px = bx0 + gx as u32;
                let (qx, qy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                let d2 = qx * qx + qy * qy;
                // Smooth falloff: full push at the center, zero at the rim.
                // Past the rim the cosine lands exactly on zero, so the rim
                // test skips the sqrt and the cos for the rect's corners.
                let f = if d2 >= r2 {
                    0.0
                } else {
                    0.5 + 0.5 * (std::f32::consts::PI * (d2.sqrt() / radius).min(1.0)).cos()
                };
                let in_rect = !matches!(
                    selection,
                    Some((a, b, c, d)) if px < a || px > c || py < b || py > d
                );
                let k = if in_rect {
                    Self::sel_k(mask, w, px, py)
                } else {
                    0.0
                };
                // A feathered selection scales the displacement, so a pixel at
                // k = 0 samples itself and stays put.
                let d = [-dx * strength * f * k, -dy * strength * f * k];
                step = step.max(d[0].abs()).max(d[1].abs());
                disp[gy * dw + gx] = d;
            }
        }
        if step == 0.0 {
            stroke.scratch_disc = disp;
            return Ok(None);
        }
        // The offset is carried with the content, like dragging the pixels:
        // `F_new(p) = F_old(p + d) + d`. Summing `d` at a fixed pixel instead
        // only moves content while the disc passes over it, so a drag
        // transfers barely half its distance and the warp reads as a stiff
        // block instead of a pull. Reading the pre-push field needs a dense
        // copy of the disc plus the bilinear support.
        let pad = step.ceil() as u32 + 1;
        let (sx0, sy0) = (bx0.saturating_sub(pad), by0.saturating_sub(pad));
        let (sx1, sy1) = ((bx1 + pad).min(w - 1), (by1 + pad).min(h - 1));
        let sw = stroke
            .field
            .try_snapshot_into((sx0, sy0, sx1, sy1), &mut stroke.scratch_off)?
            as usize;
        let old = &stroke.scratch_off;
        for gy in 0..dh {
            let py = by0 + gy as u32;
            for gx in 0..dw {
                let px = bx0 + gx as u32;
                let i = gy * dw + gx;
                let d = disp[i];
                if d[0] == 0.0 && d[1] == 0.0 {
                    // Untouched by this push: keep the field as it was.
                    disp[i] = old[(py - sy0) as usize * sw + (px - sx0) as usize];
                    continue;
                }
                let sx = (px as f32 + d[0]).clamp(0.0, w as f32 - 1.0);
                let sy = (py as f32 + d[1]).clamp(0.0, h as f32 - 1.0);
                let (ix0, iy0) = (sx.floor() as u32, sy.floor() as u32);
                let (ix1, iy1) = ((ix0 + 1).min(w - 1), (iy0 + 1).min(h - 1));
                let (fx, fy) = (sx - ix0 as f32, sy - iy0 as f32);
                let at = |x: u32, y: u32| old[(y - sy0) as usize * sw + (x - sx0) as usize];
                let (a, b, c, e) = (at(ix0, iy0), at(ix1, iy0), at(ix0, iy1), at(ix1, iy1));
                let prev = [
                    a[0] * (1.0 - fx) * (1.0 - fy)
                        + b[0] * fx * (1.0 - fy)
                        + c[0] * (1.0 - fx) * fy
                        + e[0] * fx * fy,
                    a[1] * (1.0 - fx) * (1.0 - fy)
                        + b[1] * fx * (1.0 - fy)
                        + c[1] * (1.0 - fx) * fy
                        + e[1] * fx * fy,
                ];
                // Clamp to the canvas: a sample past the edge folds back to
                // the same clamped read, and the clamp bounds the region copy
                // a long drag makes.
                disp[i] = [
                    (prev[0] + d[0]).clamp(-(px as f32), (w - 1 - px) as f32),
                    (prev[1] + d[1]).clamp(-(py as f32), (h - 1 - py) as f32),
                ];
            }
        }
        stroke
            .field
            .try_commit((bx0, by0, bx1, by1), &disp, dw as u32)?;
        stroke.scratch_disc = disp;
        Ok(Some((bx0, by0, bx1, by1)))
    }

    /// Re-render the pushed rects of one group from the stroke snapshot
    /// through the displacement field. Offsets and source pixels come from
    /// dense copies (one copy per tile row), and rows go back tile-segment
    /// wise, skipping bytes that did not change. Filtering is premultiplied
    /// and the writes unpremultiplied, so transparent black never bleeds
    /// into a warped edge and darkens its color.
    fn liquify_render(
        &mut self,
        stroke: &mut LiquifyStroke,
        parts: &[(u32, u32, u32, u32)],
    ) -> Result<(), String> {
        let (w, h) = (self.w, self.h);
        let Some(&first) = parts.first() else {
            return Ok(());
        };
        if w == 0 || h == 0 {
            return Ok(());
        }
        let mut rect = first;
        for &p in &parts[1..] {
            rect = (
                rect.0.min(p.0),
                rect.1.min(p.1),
                rect.2.max(p.2),
                rect.3.max(p.3),
            );
        }
        let (x0, y0) = (rect.0.min(w - 1), rect.1.min(h - 1));
        let (x1, y1) = (rect.2.min(w - 1), rect.3.min(h - 1));
        if x0 > x1 || y0 > y1 {
            return Ok(());
        }
        let (bw, bh) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
        // Cover only the pushed rects inside the group's box: the extra
        // pixels a merged box adds are never resampled, so they keep their
        // exact bytes. A single rect covers its own box, so it needs no map.
        let mut cov = std::mem::take(&mut stroke.scratch_cov);
        cov.clear();
        if parts.len() > 1 {
            try_fill(&mut cov, bw * bh, 0u8)?;
            for &(px0, py0, px1, py1) in parts {
                let (px1, py1) = (px1.min(x1), py1.min(y1));
                let (px0, py0) = (px0.min(px1), py0.min(py1));
                if px0 > px1 || py0 > py1 {
                    continue;
                }
                let n = (px1 - px0 + 1) as usize;
                for py in py0..=py1 {
                    let o = (py - y0) as usize * bw + (px0 - x0) as usize;
                    cov[o..o + n].fill(1);
                }
            }
        }
        let fw = stroke
            .field
            .try_snapshot_into((x0, y0, x1, y1), &mut stroke.scratch_off)?;
        // Furthest reverse sample over the pushed pixels: the source region
        // must reach that far outside the rewritten rect. `+ 1` covers the
        // far bilinear corner.
        let mut furthest = 0.0f32;
        for &(px0, py0, px1, py1) in parts {
            let (px1, py1) = (px1.min(x1), py1.min(y1));
            let (px0, py0) = (px0.min(px1), py0.min(py1));
            if px0 > px1 || py0 > py1 {
                continue;
            }
            let n = (px1 - px0 + 1) as usize;
            for py in py0..=py1 {
                let o = (py - y0) as usize * fw as usize + (px0 - x0) as usize;
                for v in &stroke.scratch_off[o..o + n] {
                    furthest = furthest.max(v[0].abs()).max(v[1].abs());
                }
            }
        }
        let reach = furthest.ceil() as u32 + 1;
        let rx0 = x0.saturating_sub(reach);
        let ry0 = y0.saturating_sub(reach);
        let rx1 = (x1 + reach).min(w - 1);
        let ry1 = (y1 + reach).min(h - 1);
        let rw = (rx1 - rx0 + 1) as usize;
        let mut src_buf = std::mem::take(&mut stroke.scratch_src);
        stroke
            .src
            .try_region_into((rx0, ry0, rx1, ry1), &mut src_buf)?;
        let region = &src_buf;
        let sample = |sx: f32, sy: f32| -> [u8; 4] {
            let sx = sx.clamp(0.0, w as f32 - 1.0);
            let sy = sy.clamp(0.0, h as f32 - 1.0);
            let (cx0, cy0) = (sx.floor() as u32, sy.floor() as u32);
            let (cx1, cy1) = ((cx0 + 1).min(w - 1), (cy0 + 1).min(h - 1));
            let (fx, fy) = ((sx - cx0 as f32) as f64, (sy - cy0 as f32) as f64);
            let at = |x: u32, y: u32| -> [u8; 4] {
                let o = ((y - ry0) as usize * rw + (x - rx0) as usize) * 4;
                [region[o], region[o + 1], region[o + 2], region[o + 3]]
            };
            let (mut pr, mut pg, mut pb, mut pa) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for (x, y, weight) in [
                (cx0, cy0, (1.0 - fx) * (1.0 - fy)),
                (cx1, cy0, fx * (1.0 - fy)),
                (cx0, cy1, (1.0 - fx) * fy),
                (cx1, cy1, fx * fy),
            ] {
                let [r, g, b, a] = at(x, y);
                let a = a as f64 * weight;
                pa += a;
                pr += r as f64 * a;
                pg += g as f64 * a;
                pb += b as f64 * a;
            }
            if pa <= 0.0 {
                return [0, 0, 0, 0];
            }
            [
                (pr / pa).round().clamp(0.0, 255.0) as u8,
                (pg / pa).round().clamp(0.0, 255.0) as u8,
                (pb / pa).round().clamp(0.0, 255.0) as u8,
                pa.round().clamp(0.0, 255.0) as u8,
            ]
        };
        let mut row = std::mem::take(&mut stroke.scratch_row);
        try_fill(&mut row, bw * 4, 0u8)?;
        let field = &stroke.scratch_off;
        let layer = &mut self.layers[stroke.layer];
        for py in y0..=y1 {
            let f_row = (py - y0) as usize * fw as usize;
            let cov_row = if cov.is_empty() {
                &[][..]
            } else {
                &cov[(py - y0) as usize * bw..][..bw]
            };
            for gx in 0..bw {
                if !cov_row.is_empty() && cov_row[gx] == 0 {
                    continue;
                }
                let off = field[f_row + gx];
                let out = if off[0] == 0.0 && off[1] == 0.0 {
                    // The pixel samples itself: the filtered value is its own
                    // bytes, except that zero alpha normalises colour away.
                    let o = ((py - ry0) as usize * rw + (x0 + gx as u32 - rx0) as usize) * 4;
                    if region[o + 3] == 0 {
                        [0, 0, 0, 0]
                    } else {
                        [region[o], region[o + 1], region[o + 2], region[o + 3]]
                    }
                } else {
                    sample((x0 + gx as u32) as f32 + off[0], py as f32 + off[1])
                };
                row[gx * 4..gx * 4 + 4].copy_from_slice(&out);
            }
            if cov_row.is_empty() {
                layer.pixels.try_write_row_changed(x0, py, &row)?;
                continue;
            }
            let mut gx = 0;
            while gx < bw {
                if cov_row[gx] == 0 {
                    gx += 1;
                    continue;
                }
                let start = gx;
                while gx < bw && cov_row[gx] != 0 {
                    gx += 1;
                }
                layer.pixels.try_write_row_changed(
                    x0 + start as u32,
                    py,
                    &row[start * 4..gx * 4],
                )?;
            }
        }
        stroke.scratch_cov = cov;
        stroke.scratch_src = src_buf;
        stroke.scratch_row = row;
        self.mark_dirty(x0, y0, x1, y1);
        stroke.rev = self.rev;
        Ok(())
    }
}
