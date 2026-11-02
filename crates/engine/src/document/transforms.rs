//! Layer transforms: offsets, crop/resize, affine, quad, warp, tris and flips.

use super::*;

impl Document {
    /// Shift a layer by whole pixels. Lossless and O(1): only the layer
    /// offset changes, so content pushed off-canvas stays in the buffer
    /// and returns when moved back (this used to crop destructively).
    /// Offsets are clamped to +/-16384 (the dimension cap). Caller
    /// checkpoints for undo first (offsets ride the history entries).
    pub fn translate_layer(&mut self, index: usize, dx: i32, dy: i32) {
        if (dx == 0 && dy == 0) || index >= self.layers.len() {
            return;
        }
        const LIM: i32 = 16384;
        let layer = &mut self.layers[index];
        layer.ox = (layer.ox + dx).clamp(-LIM, LIM);
        layer.oy = (layer.oy + dy).clamp(-LIM, LIM);
        self.mark_full_dirty();
    }

    /// Shift to fit a canvas bbox into a w×h buffer at offset (`ox`,`oy`):
    /// minimal slide so the bbox maps inside (`(0, 0)` when already
    /// inside). Always safe: the slide preserves every on-canvas-visible
    /// pixel (only off-canvas tails trim — provable from clamping, since
    /// the lost window never overlaps `[0, w)`), so even fully-displaced
    /// bboxes absorb instead of refusing. Pure math; [`Self::absorb_offset`]
    /// applies it to both buffers.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fit_shift(
        w: u32,
        h: u32,
        ox: i32,
        oy: i32,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    ) -> (i32, i32) {
        if w == 0 || h == 0 {
            return (0, 0);
        }
        let (b0x, b1x) = (x0 as i32 - ox, x1 as i32 - ox);
        let (b0y, b1y) = (y0 as i32 - oy, y1 as i32 - oy);
        let sx = if b0x < 0 {
            -b0x
        } else if b1x >= w as i32 {
            -(b1x - w as i32 + 1)
        } else {
            0
        };
        let sy = if b0y < 0 {
            -b0y
        } else if b1y >= h as i32 {
            -(b1y - h as i32 + 1)
        } else {
            0
        };
        (sx, sy)
    }

    /// Make canvas bbox [`x0`..=`x1`]×[`y0`..=`y1`] paintable on a moved
    /// layer: slide pixels (and mask) so the bbox maps inside the buffer,
    /// adjusting the offset equally — on-screen content does not move and
    /// no visible pixel is ever trimmed (see [`Self::fit_shift`]). No-op
    /// when already inside. Returns true when the buffer slid (the caller
    /// then marks the whole layer dirty — content moved outside the dab
    /// rect too). Caller checkpoints first (paint strokes do, so undo
    /// restores buffer and offset together).
    ///
    /// This runs per dab, and a stroke that keeps moving outward slides on
    /// every dab, so the slide itself is tile-granular
    /// ([`Raster::slide_in_place`]): one tile-grid copy, no dense buffer
    /// and no whole-canvas temporary.
    pub fn absorb_offset(&mut self, index: usize, x0: u32, y0: u32, x1: u32, y1: u32) -> bool {
        if index >= self.layers.len() {
            return false;
        }
        let (ox, oy) = (self.layers[index].ox, self.layers[index].oy);
        let (sx, sy) = Self::fit_shift(self.w, self.h, ox, oy, x0, y0, x1, y1);
        if sx == 0 && sy == 0 {
            return false;
        }
        self.layers[index].pixels.slide_in_place(sx, sy);
        if let Some(m) = self.layers[index].mask.as_mut() {
            m.slide_in_place(sx, sy);
        }
        // Content slid +s in the buffer reads at the same screen cell
        // exactly when the offset drops by s.
        self.layers[index].ox = ox - sx;
        self.layers[index].oy = oy - sy;
        true
    }

    /// Re-seat the canvas: the old canvas lands at `(dx, dy)` in the new
    /// `new_w` x `new_h` canvas. Buffers are reallocated and copied with
    /// the shift baked in — each layer's buffer moves by the canvas shift
    /// minus its own offset, and the offset re-bases to 0 — so every canvas
    /// pixel keeps its old content and no visible pixel is lost. (Keeping
    /// buffer cells at identical indices while shifting the offset used to
    /// blank the trailing columns/rows of a leading-edge trim: the new
    /// canvas reads a window shifted by the crop, which the copy never
    /// touched.) New areas start transparent; anything outside the new
    /// canvas is trimmed. Crop is this with `dx = -x0`, `dy = -y0`. Clears
    /// undo/redo: old pixel snapshots no longer match the buffer size
    /// (same precedent as [`Self::load_background`]).
    pub fn rehome_canvas(
        &mut self,
        new_w: u32,
        new_h: u32,
        dx: i32,
        dy: i32,
    ) -> Result<(), String> {
        if new_w == 0 || new_h == 0 || new_w > MAX_CANVAS_SIDE || new_h > MAX_CANVAS_SIDE {
            return Err(format!(
                "bad canvas size: {new_w}x{new_h} (want 1-{MAX_CANVAS_SIDE} each)"
            ));
        }
        if new_w == self.w && new_h == self.h && dx == 0 && dy == 0 {
            return Ok(());
        }
        // Overlapping copy with a content shift: new cell `(x, y)` reads old
        // cell `(x + sx, y + sy)`, so the pixels move by the canvas re-seat
        // minus the layer's own offset (which then re-bases to 0). Cells the
        // shifted window never covers stay transparent; short/corrupt
        // sources read as transparent instead of panicking (the invariant is
        // w*h*bpp, but a crop must never be the place that finds out
        // otherwise).
        #[allow(clippy::too_many_arguments)]
        fn carry(
            buf: &[u8],
            old_w: u32,
            old_h: u32,
            new_w: u32,
            new_h: u32,
            bpp: u32,
            sx: i32,
            sy: i32,
        ) -> Result<Vec<u8>, String> {
            let mut out = try_zeroed((new_w * new_h * bpp) as usize)?;
            let x0 = (-sx).max(0) as u32;
            let x1 = new_w.min((old_w as i32 - sx).max(0) as u32);
            if x1 <= x0 {
                return Ok(out);
            }
            let n = ((x1 - x0) * bpp) as usize;
            let src_x = (x0 as i32 + sx) as u32;
            for y in 0..new_h {
                let src_y = y as i32 + sy;
                if src_y < 0 || src_y as u32 >= old_h {
                    continue;
                }
                let src = ((src_y as u32 * old_w + src_x) * bpp) as usize;
                let dst = ((y * new_w + x0) * bpp) as usize;
                let chunk = buf.get(src..src + n).unwrap_or(&[]);
                if let Some(slot) = out.get_mut(dst..dst + chunk.len()) {
                    slot.copy_from_slice(chunk);
                }
            }
            Ok(out)
        }
        let (old_w, old_h) = (self.w, self.h);
        // Every buffer is allocated before the canvas is touched: a crop or
        // resize that cannot get its memory leaves the document as it was.
        let mut next_layers = Vec::with_capacity(self.layers.len());
        for layer in self.layers.iter() {
            let (sx, sy) = (-dx - layer.ox, -dy - layer.oy);
            let dense = layer.pixels.try_to_dense()?;
            let px = carry(&dense, old_w, old_h, new_w, new_h, 4, sx, sy)?;
            let mask = match layer.mask.as_ref() {
                Some(m) => {
                    let md = m.try_to_dense()?;
                    let carried = carry(&md, old_w, old_h, new_w, new_h, 1, sx, sy)?;
                    Some(Raster::try_from_dense(new_w, new_h, 1, &carried)?)
                }
                None => None,
            };
            let pixels = Raster::try_from_dense(new_w, new_h, 4, &px)?;
            next_layers.push((pixels, mask));
        }
        let next_selection = match self.selection_mask.as_ref() {
            Some(sm) => Some(carry(sm, old_w, old_h, new_w, new_h, 1, -dx, -dy)?),
            None => None,
        };
        for (layer, (pixels, mask)) in self.layers.iter_mut().zip(next_layers) {
            layer.pixels = pixels;
            layer.mask = mask;
            layer.ox = 0;
            layer.oy = 0;
            // A text block's anchor is in canvas pixels: it rides the same
            // shift (pixels need no rerender — the buffer cell and the
            // anchor both move by the same delta).
            if let Some(b) = &mut layer.text {
                b.x += dx as f32;
                b.y += dy as f32;
            }
        }
        if let Some(sm) = next_selection {
            self.selection_mask = Some(sm);
        }
        // Rect selection rides along; drops when fully outside.
        if let Some((x0, y0, x1, y1)) = self.selection {
            let (sx0, sy0) = (x0 as i32 + dx, y0 as i32 + dy);
            let (sx1, sy1) = (x1 as i32 + dx, y1 as i32 + dy);
            let (nw, nh) = (new_w as i32, new_h as i32);
            if sx1 < 0 || sy1 < 0 || sx0 >= nw || sy0 >= nh {
                self.selection = None;
            } else {
                self.selection = Some((
                    sx0.clamp(0, nw - 1) as u32,
                    sy0.clamp(0, nh - 1) as u32,
                    sx1.clamp(0, nw - 1) as u32,
                    sy1.clamp(0, nh - 1) as u32,
                ));
            }
        }
        if self.symmetry_center.is_some() {
            let (cx, cy) = self.symmetry_center_px();
            self.set_symmetry_center(cx + dx as f32, cy + dy as f32);
        }
        self.w = new_w;
        self.h = new_h;
        self.undo.clear();
        self.redo.clear();
        self.undo_bytes = 0;
        self.redo_bytes = 0;
        self.mark_full_dirty();
        Ok(())
    }

    /// Crop to the inclusive rect: content inside keeps its picture, only
    /// re-based at the new top-left (offsets re-base to 0; no visible pixel
    /// is dropped). Validates before touching anything; clears undo/redo
    /// like [`Self::rehome_canvas`].
    pub fn crop(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) -> Result<(), String> {
        if x0 > x1 || y0 > y1 || x1 >= self.w || y1 >= self.h {
            return Err("crop rect is outside the canvas".to_string());
        }
        self.rehome_canvas(x1 - x0 + 1, y1 - y0 + 1, -(x0 as i32), -(y0 as i32))
    }

    /// Bilinear sample in premultiplied-alpha space. Straight-RGBA
    /// interpolation drags transparent black into every edge pixel and
    /// darkens the rim of rotations, scales and warps; weighting the
    /// colour by its own alpha (then unpremultiplying) keeps edges
    /// saturated while the coverage fades.
    pub(super) fn sample_bilinear(pixels: &[u8], w: u32, h: u32, x: f32, y: f32) -> [u8; 4] {
        let x0 = x.floor().clamp(0.0, w as f32 - 1.0) as u32;
        let y0 = y.floor().clamp(0.0, h as f32 - 1.0) as u32;
        let x1 = (x0 + 1).min(w - 1);
        let y1 = (y0 + 1).min(h - 1);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        if fx.abs() < 1e-6 && fy.abs() < 1e-6 {
            let o = ((y0 * w + x0) * 4) as usize;
            return [pixels[o], pixels[o + 1], pixels[o + 2], pixels[o + 3]];
        }
        let at = |x: u32, y: u32| -> [f32; 4] {
            let o = ((y * w + x) * 4) as usize;
            let a = pixels[o + 3] as f32;
            let k = a / 255.0;
            [
                pixels[o] as f32 * k,
                pixels[o + 1] as f32 * k,
                pixels[o + 2] as f32 * k,
                a,
            ]
        };
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let mut pm = [0.0f32; 4];
        for i in 0..4 {
            pm[i] = a[i] * (1.0 - fx) * (1.0 - fy)
                + b[i] * fx * (1.0 - fy)
                + c[i] * (1.0 - fx) * fy
                + d[i] * fx * fy;
        }
        let alpha = pm[3];
        let mut out = [0u8; 4];
        for i in 0..3 {
            let v = if alpha > 0.0 {
                pm[i] * 255.0 / alpha
            } else {
                0.0
            };
            out[i] = v.round().clamp(0.0, 255.0) as u8;
        }
        out[3] = alpha.round().clamp(0.0, 255.0) as u8;
        out
    }

    /// Widen a 1-byte mask to RGBA `(v, 0, 0, 255)` for the RGBA resamplers,
    /// allocating fallibly: a mask pass at 8K would otherwise reserve 256 MiB
    /// with an infallible `collect`.
    fn mask_to_rgba(md: &[u8]) -> Result<Vec<u8>, String> {
        let mut rgba = try_zeroed(md.len() * 4)?;
        for (i, &v) in md.iter().enumerate() {
            rgba[i * 4] = v;
            rgba[i * 4 + 3] = 255;
        }
        Ok(rgba)
    }

    /// Narrow resampled RGBA back to a 1-byte mask (first channel), fallibly.
    fn rgba_to_mask(rgba: &[u8]) -> Result<Vec<u8>, String> {
        let mut gray = try_zeroed(rgba.len() / 4)?;
        for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
            gray[i] = px[0];
        }
        Ok(gray)
    }

    /// Scale then rotate (degrees clockwise) a layer about the paint's
    /// own center (opaque-pixel bbox; canvas center for empty layers),
    /// so the paint spins/grows in place instead of orbiting the canvas.
    /// Multiples of 90° resample exactly (nearest), so repeated
    /// quarter-turns never blur; other angles/scales resample once per
    /// call (repeated fractional transforms are inherently lossy — fewer,
    /// deliberate clicks keep paint crisp). Masks travel with their layer.
    /// Caller checkpoints for undo first.
    pub fn affine_layer(&mut self, index: usize, scale: f32, degrees: f32) -> Result<(), String> {
        if index >= self.layers.len() || (scale - 1.0).abs() < 1e-6 && degrees.abs() < 1e-6 {
            return Ok(());
        }
        let (w, h) = (self.w, self.h);
        let src = self.layers[index].pixels.try_to_dense()?;
        // Pivot: content bbox center keeps paint in place; empty layers
        // keep the old canvas-center behavior (invisible either way).
        let center = Self::content_center(&src, w, h);
        let pivot = center.unwrap_or((w as f32 * 0.5, h as f32 * 0.5));
        let quarters = (degrees / 90.0).round();
        let exact = (scale - 1.0).abs() < 1e-6 && (degrees - quarters * 90.0).abs() < 1e-3;
        let out = Self::affine_buffer(&src, w, h, scale, degrees, pivot.0, pivot.1, exact)?;
        // Mask work is allocated before the paint lands so an out-of-memory
        // failure leaves the layer untouched.
        let mask = match self.layers[index].mask.as_ref() {
            Some(m) => {
                let md = m.try_to_dense()?;
                let rgba = Self::mask_to_rgba(&md)?;
                let out =
                    Self::affine_buffer(&rgba, w, h, scale, degrees, pivot.0, pivot.1, exact)?;
                Some(Self::rgba_to_mask(&out)?)
            }
            None => None,
        };
        let pixels = Raster::try_from_dense(w, h, 4, &out)?;
        let mask = match mask {
            Some(gray) => Some(Raster::try_from_dense(w, h, 1, &gray)?),
            None => None,
        };
        self.layers[index].pixels = pixels;
        if let Some(m) = mask {
            self.layers[index].mask = Some(m);
        }
        if center.is_none() {
            // Empty layer: carry the offset through the forward map about
            // the canvas center, exactly as before (invisible, but stable).
            let theta = degrees.to_radians();
            let (sin, cos) = theta.sin_cos();
            let s = scale.max(0.01);
            let (ox, oy) = (self.layers[index].ox as f32, self.layers[index].oy as f32);
            self.layers[index].ox = (s * (ox * cos - oy * sin)).round() as i32;
            self.layers[index].oy = (s * (ox * sin + oy * cos)).round() as i32;
        }
        // Content path: the buffer rotates about its own center, so the
        // layer offset stays valid untouched (no rounding damage).
        self.mark_full_dirty();
        Ok(())
    }

    /// Center of the opaque-pixel bbox as the mean of indices (so the
    /// discrete set maps onto itself under quarter-turns; the continuous
    /// midpoint would drift a pixel per turn). None when nothing is
    /// painted.
    fn content_center(pixels: &[u8], w: u32, h: u32) -> Option<(f32, f32)> {
        let (x0, y0, x1, y1) = Self::pixel_bounds(pixels, w, h)?;
        Some(((x0 + x1) * 0.5, (y0 + y1) * 0.5))
    }

    /// Inclusive opaque-pixel bbox (min x, min y, max x, max y) for a
    /// layer, so on-canvas transforms can cover the drawing instead of
    /// the whole canvas. None when the layer is empty or cannot be
    /// measured (an OOM fails the transform rather than aborting it).
    pub fn content_bounds(&self, index: usize) -> Option<(f32, f32, f32, f32)> {
        let (w, h) = (self.w, self.h);
        let layer = self.layers.get(index)?;
        Self::pixel_bounds(&layer.pixels.try_to_dense().ok()?, w, h)
    }

    fn pixel_bounds(pixels: &[u8], w: u32, h: u32) -> Option<(f32, f32, f32, f32)> {
        if w == 0 || h == 0 || pixels.len() != (w * h * 4) as usize {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if pixels[((y * w + x) * 4 + 3) as usize] != 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if x0 == u32::MAX {
            return None;
        }
        Some((x0 as f32, y0 as f32, x1 as f32, y1 as f32))
    }

    /// Shared scale-then-rotate resampler over RGBA bytes about pivot
    /// (`cx`, `cy`). Exact quarter-turns sample nearest (lossless
    /// permutation); everything else blends bilinearly.
    #[allow(clippy::too_many_arguments)]
    fn affine_buffer(
        src: &[u8],
        w: u32,
        h: u32,
        scale: f32,
        degrees: f32,
        cx: f32,
        cy: f32,
        exact: bool,
    ) -> Result<Vec<u8>, String> {
        let theta = degrees.to_radians();
        let (sin, cos) = theta.sin_cos();
        let s = scale.max(0.01);
        let mut next = try_zeroed((w * h * 4) as usize)?;
        for y in 0..h {
            for x in 0..w {
                // Inverse map: destination -> source.
                let px = x as f32 - cx;
                let py = y as f32 - cy;
                let sx = (px * cos + py * sin) / s + cx;
                let sy = (-px * sin + py * cos) / s + cy;
                let o = ((y * w + x) * 4) as usize;
                if exact {
                    // Round first: float noise (±1e-6) around integer
                    // sources must not drop pixels at the bounds.
                    let xi = sx.round() as i32;
                    let yi = sy.round() as i32;
                    if xi >= 0 && xi < w as i32 && yi >= 0 && yi < h as i32 {
                        let so = ((yi as u32 * w + xi as u32) * 4) as usize;
                        next[o..o + 4].copy_from_slice(&src[so..so + 4]);
                    }
                } else if sx >= 0.0 && sx < w as f32 && sy >= 0.0 && sy < h as f32 {
                    next[o..o + 4].copy_from_slice(&Self::sample_bilinear(src, w, h, sx, sy));
                }
            }
        }
        Ok(next)
    }

    /// Corner-pin distort (M43): remap the layer through `quad`, the
    /// source coords sampled at destination corners [TL, TR, BR, BL],
    /// bilinearly interpolated across the canvas and resampled with the
    /// shared bilinear sampler (identity quad is an exact no-op). Like
    /// `affine_layer`, the remap is canvas-space so layer offsets stay
    /// valid untouched, and masks travel with their layer. Fractional
    /// pins are lossy by nature — one deliberate gesture per distort
    /// keeps paint crisp. Caller checkpoints for undo first.
    pub fn distort_layer(&mut self, index: usize, quad: [(f32, f32); 4]) -> Result<(), String> {
        if index >= self.layers.len() {
            return Ok(());
        }
        let (w, h) = (self.w, self.h);
        if w < 2 || h < 2 {
            return Ok(());
        }
        let src = self.layers[index].pixels.try_to_dense()?;
        let out = Self::quad_buffer(&src, w, h, &quad)?;
        // Mask work lands before the paint write, so a failed allocation
        // leaves the layer untouched.
        let mask = match self.layers[index].mask.as_ref() {
            Some(m) => {
                let md = m.try_to_dense()?;
                let rgba = Self::mask_to_rgba(&md)?;
                let out = Self::quad_buffer(&rgba, w, h, &quad)?;
                Some(Self::rgba_to_mask(&out)?)
            }
            None => None,
        };
        let pixels = Raster::try_from_dense(w, h, 4, &out)?;
        let mask = match mask {
            Some(gray) => Some(Raster::try_from_dense(w, h, 1, &gray)?),
            None => None,
        };
        self.layers[index].pixels = pixels;
        if let Some(m) = mask {
            self.layers[index].mask = Some(m);
        }
        self.mark_full_dirty();
        Ok(())
    }

    /// Shared corner-pin resampler: destination (x, y) samples the
    /// bilinear blend of `quad` at (x/(w-1), y/(h-1)).
    fn quad_buffer(src: &[u8], w: u32, h: u32, quad: &[(f32, f32); 4]) -> Result<Vec<u8>, String> {
        let (w1, h1) = (w as f32 - 1.0, h as f32 - 1.0);
        let mut next = try_zeroed((w * h * 4) as usize)?;
        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / w1;
                let v = y as f32 / h1;
                let (ax, ay) = quad[0];
                let (bx, by) = quad[1];
                let (cx, cy) = quad[2];
                let (dx, dy) = quad[3];
                let iu = 1.0 - u;
                let iv = 1.0 - v;
                let sx = ax * iu * iv + bx * u * iv + cx * u * v + dx * iu * v;
                let sy = ay * iu * iv + by * u * iv + cy * u * v + dy * iu * v;
                let o = ((y * w + x) * 4) as usize;
                if sx >= 0.0 && sx < w as f32 && sy >= 0.0 && sy < h as f32 {
                    next[o..o + 4].copy_from_slice(&Self::sample_bilinear(src, w, h, sx, sy));
                }
            }
        }
        Ok(next)
    }

    /// Warp mesh (M44): remap the layer through a `cols` x `rows` cell
    /// lattice, `grid` holding `(cols + 1) * (rows + 1)` source coords in
    /// row-major order (identity = regular canvas positions, exact
    /// no-op). Each destination pixel lands in one cell and samples the
    /// bilinear blend of its four lattice points, so dragging one point
    /// pulls paint locally — distort's freeform sibling. Same canvas-space
    /// conventions as `distort_layer`: offsets untouched, masks travel.
    /// Caller checkpoints for undo first.
    pub fn warp_layer(
        &mut self,
        index: usize,
        cols: u32,
        rows: u32,
        grid: &[(f32, f32)],
    ) -> Result<(), String> {
        if index >= self.layers.len() || cols < 1 || rows < 1 {
            return Ok(());
        }
        let (w, h) = (self.w, self.h);
        if w < 2 || h < 2 {
            return Ok(());
        }
        if grid.len() != ((cols + 1) * (rows + 1)) as usize {
            return Ok(());
        }
        let src = self.layers[index].pixels.try_to_dense()?;
        let out = Self::warp_buffer(&src, w, h, cols, rows, grid)?;
        let mask = match self.layers[index].mask.as_ref() {
            Some(m) => {
                let md = m.try_to_dense()?;
                let rgba = Self::mask_to_rgba(&md)?;
                let out = Self::warp_buffer(&rgba, w, h, cols, rows, grid)?;
                Some(Self::rgba_to_mask(&out)?)
            }
            None => None,
        };
        let pixels = Raster::try_from_dense(w, h, 4, &out)?;
        let mask = match mask {
            Some(gray) => Some(Raster::try_from_dense(w, h, 1, &gray)?),
            None => None,
        };
        self.layers[index].pixels = pixels;
        if let Some(m) = mask {
            self.layers[index].mask = Some(m);
        }
        self.mark_full_dirty();
        Ok(())
    }

    /// Shared warp resampler: destination (x, y) maps into lattice space,
    /// then bilinearly blends the four surrounding lattice points.
    fn warp_buffer(
        src: &[u8],
        w: u32,
        h: u32,
        cols: u32,
        rows: u32,
        grid: &[(f32, f32)],
    ) -> Result<Vec<u8>, String> {
        let stride = (cols + 1) as usize;
        let mut next = try_zeroed((w * h * 4) as usize)?;
        for y in 0..h {
            for x in 0..w {
                let gx = (x as f32 / w as f32 * cols as f32).clamp(0.0, cols as f32 - 1e-6);
                let gy = (y as f32 / h as f32 * rows as f32).clamp(0.0, rows as f32 - 1e-6);
                let i = gx.floor() as usize;
                let j = gy.floor() as usize;
                let (fu, fv) = (gx - i as f32, gy - j as f32);
                let p00 = grid[j * stride + i];
                let p10 = grid[j * stride + i + 1];
                let p01 = grid[(j + 1) * stride + i];
                let p11 = grid[(j + 1) * stride + i + 1];
                let iu = 1.0 - fu;
                let iv = 1.0 - fv;
                let sx = p00.0 * iu * iv + p10.0 * fu * iv + p11.0 * fu * fv + p01.0 * iu * fv;
                let sy = p00.1 * iu * iv + p10.1 * fu * iv + p11.1 * fu * fv + p01.1 * iu * fv;
                let o = ((y * w + x) * 4) as usize;
                if sx >= 0.0 && sx < w as f32 && sy >= 0.0 && sy < h as f32 {
                    next[o..o + 4].copy_from_slice(&Self::sample_bilinear(src, w, h, sx, sy));
                }
            }
        }
        Ok(next)
    }

    /// Pin warp for a triangle mesh: `src` and `dst` hold matching pin
    /// positions (row-major by index) and `tris` indexes them. Every
    /// destination pixel falling in a `dst` triangle samples the affine
    /// blend of the same triangle's `src` positions, so content follows
    /// a pin exactly and untouched pins hold their pixels in place
    /// (identity pins are an exact no-op). Pins plus a fixed outer ring
    /// cover the whole canvas, so vacated areas turn transparent like
    /// the other remaps. Masks travel with their layer. Degenerate
    /// (collapsed) triangles are skipped; an all-degenerate mesh leaves
    /// the layer untouched. Caller checkpoints for undo first.
    pub fn warp_layer_tris(
        &mut self,
        index: usize,
        src: &[(f32, f32)],
        dst: &[(f32, f32)],
        tris: &[[u32; 3]],
    ) -> Result<(), String> {
        if index >= self.layers.len() || src.len() != dst.len() || src.len() < 3 {
            return Ok(());
        }
        let (w, h) = (self.w, self.h);
        if w < 2 || h < 2 {
            return Ok(());
        }
        let n = src.len() as u32;
        if tris.is_empty() || tris.iter().any(|t| t.iter().any(|&i| i >= n)) {
            return Ok(());
        }
        if src == dst {
            return Ok(());
        }
        let pixels = self.layers[index].pixels.try_to_dense()?;
        let out = Self::tri_buffer(&pixels, w, h, src, dst, tris)?;
        let mask = match self.layers[index].mask.as_ref() {
            Some(m) => {
                let md = m.try_to_dense()?;
                let rgba = Self::mask_to_rgba(&md)?;
                let out = Self::tri_buffer(&rgba, w, h, src, dst, tris)?;
                Some(Self::rgba_to_mask(&out)?)
            }
            None => None,
        };
        let pixels = Raster::try_from_dense(w, h, 4, &out)?;
        let mask = match mask {
            Some(gray) => Some(Raster::try_from_dense(w, h, 1, &gray)?),
            None => None,
        };
        self.layers[index].pixels = pixels;
        if let Some(m) = mask {
            self.layers[index].mask = Some(m);
        }
        self.mark_full_dirty();
        Ok(())
    }

    /// Shared pin-warp resampler: rasterize each destination triangle,
    /// map pixel centers through its barycentric weights onto the source
    /// triangle and sample bilinearly. Inclusive edge tolerance (1e-3)
    /// keeps shared edges from cracking; later triangles win overlaps.
    fn tri_buffer(
        src_px: &[u8],
        w: u32,
        h: u32,
        src: &[(f32, f32)],
        dst: &[(f32, f32)],
        tris: &[[u32; 3]],
    ) -> Result<Vec<u8>, String> {
        let mut next = try_zeroed((w * h * 4) as usize)?;
        for t in tris {
            let (i0, i1, i2) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let (ax, ay) = dst[i0];
            let (bx, by) = dst[i1];
            let (cx, cy) = dst[i2];
            let area = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
            if area.abs() < 1e-6 {
                continue;
            }
            let (lo_x, hi_x) = (ax.min(bx).min(cx), ax.max(bx).max(cx));
            let (lo_y, hi_y) = (ay.min(by).min(cy), ay.max(by).max(cy));
            if hi_x < 0.0 || hi_y < 0.0 || lo_x > w as f32 - 1.0 || lo_y > h as f32 - 1.0 {
                continue;
            }
            let x0 = lo_x.floor().max(0.0) as u32;
            let y0 = lo_y.floor().max(0.0) as u32;
            let x1 = (hi_x.ceil().min(w as f32 - 1.0)) as u32;
            let y1 = (hi_y.ceil().min(h as f32 - 1.0)) as u32;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (px, py) = (x as f32, y as f32);
                    let w0 = ((bx - px) * (cy - py) - (cx - px) * (by - py)) / area;
                    let w1 = ((cx - px) * (ay - py) - (ax - px) * (cy - py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-3 || w1 < -1e-3 || w2 < -1e-3 {
                        continue;
                    }
                    let sx = w0 * src[i0].0 + w1 * src[i1].0 + w2 * src[i2].0;
                    let sy = w0 * src[i0].1 + w1 * src[i1].1 + w2 * src[i2].1;
                    if sx >= 0.0 && sx < w as f32 && sy >= 0.0 && sy < h as f32 {
                        let o = ((y * w + x) * 4) as usize;
                        next[o..o + 4]
                            .copy_from_slice(&Self::sample_bilinear(src_px, w, h, sx, sy));
                    }
                }
            }
        }
        Ok(next)
    }

    /// Mirror a layer left-right in place. Masks travel with their layer.
    /// Caller checkpoints for undo first.
    pub fn flip_layer_horizontal(&mut self, index: usize) -> Result<(), String> {
        if index >= self.layers.len() {
            return Ok(());
        }
        let (w, h) = (self.w, self.h);
        let src = self.layers[index].pixels.try_to_dense()?;
        let mut next = try_zeroed((w * h * 4) as usize)?;
        for y in 0..h {
            for x in 0..w {
                let s = ((y * w + (w - 1 - x)) * 4) as usize;
                let d = ((y * w + x) * 4) as usize;
                next[d..d + 4].copy_from_slice(&src[s..s + 4]);
            }
        }
        let mask = match self.layers[index].mask.as_ref() {
            Some(m) => {
                let m = m.try_to_dense()?;
                let mut nm = try_zeroed((w * h) as usize)?;
                for y in 0..h {
                    for x in 0..w {
                        nm[(y * w + x) as usize] = m[(y * w + (w - 1 - x)) as usize];
                    }
                }
                Some(nm)
            }
            None => None,
        };
        let pixels = Raster::try_from_dense(w, h, 4, &next)?;
        let mask = match mask {
            Some(nm) => Some(Raster::try_from_dense(w, h, 1, &nm)?),
            None => None,
        };
        self.layers[index].pixels = pixels;
        // Mirror the offset too so the content flips about the canvas
        // center, exactly as before offsets existed.
        self.layers[index].ox = -self.layers[index].ox;
        if let Some(m) = mask {
            self.layers[index].mask = Some(m);
        }
        self.mark_full_dirty();
        Ok(())
    }

    /// Flip a layer top-bottom in place. Masks travel with their layer.
    /// Caller checkpoints for undo first.
    pub fn flip_layer_vertical(&mut self, index: usize) -> Result<(), String> {
        if index >= self.layers.len() {
            return Ok(());
        }
        let (w, h) = (self.w, self.h);
        let src = self.layers[index].pixels.try_to_dense()?;
        let mut next = try_zeroed((w * h * 4) as usize)?;
        for y in 0..h {
            for x in 0..w {
                let s = (((h - 1 - y) * w + x) * 4) as usize;
                let d = ((y * w + x) * 4) as usize;
                next[d..d + 4].copy_from_slice(&src[s..s + 4]);
            }
        }
        let mask = match self.layers[index].mask.as_ref() {
            Some(m) => {
                let m = m.try_to_dense()?;
                let mut nm = try_zeroed((w * h) as usize)?;
                for y in 0..h {
                    for x in 0..w {
                        nm[(y * w + x) as usize] = m[((h - 1 - y) * w + x) as usize];
                    }
                }
                Some(nm)
            }
            None => None,
        };
        let pixels = Raster::try_from_dense(w, h, 4, &next)?;
        let mask = match mask {
            Some(nm) => Some(Raster::try_from_dense(w, h, 1, &nm)?),
            None => None,
        };
        self.layers[index].pixels = pixels;
        // Mirror the offset too so the content flips about the canvas
        // center, exactly as before offsets existed.
        self.layers[index].oy = -self.layers[index].oy;
        if let Some(m) = mask {
            self.layers[index].mask = Some(m);
        }
        self.mark_full_dirty();
        Ok(())
    }

    /// Copy the active layer's sibling: insert a clone right above `index`
    /// and make it active. Caller checkpoints for undo first.
    pub fn duplicate_layer(&mut self, index: usize) {
        if index >= self.layers.len() {
            return;
        }
        let mut copy = self.layers[index].clone();
        copy.name = format!("{} copy", copy.name);
        self.layers.insert(index + 1, copy);
        self.active = index + 1;
        self.mark_full_dirty();
    }

    /// Composite layer `index` onto the layer below with its blend and
    /// opacity (invisible layers contribute nothing), then remove it.
    /// The source mask is baked in; a clipped source is intersected with
    /// the destination layer's own content (its mask included).
    /// Returns false on the bottom layer or a bad index.
    /// Caller checkpoints for undo first.
    pub fn merge_down(&mut self, index: usize) -> Result<bool, String> {
        if index == 0 || index >= self.layers.len() {
            return Ok(false);
        }
        let src = self.layers[index].clone();
        let src_px = src.pixels.try_to_dense()?;
        let src_mask = match src.mask.as_ref() {
            Some(m) => Some(m.try_to_dense()?),
            None => None,
        };
        let sa_base = if self.is_layer_shown(index) {
            src.opacity.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (w, h) = (self.w, self.h);
        // Every buffer is allocated before the document changes, so a
        // failed merge leaves both layers exactly as they were.
        let dst = &self.layers[index - 1];
        let mut dst_px = dst.pixels.try_to_dense()?;
        let dst_mask = match dst.mask.as_ref() {
            Some(m) => Some(m.try_to_dense()?),
            None => None,
        };
        let (dox, doy) = (dst.ox, dst.oy);
        for y in 0..h {
            for x in 0..w {
                // Destination slot -> canvas -> source slot. The merge bakes
                // both offsets; the result keeps dst's offset.
                let (dlx, dly) = (x as i32 - dox, y as i32 - doy);
                if dlx < 0 || dly < 0 || dlx >= w as i32 || dly >= h as i32 {
                    continue;
                }
                let o = ((dly as u32 * w + dlx as u32) * 4) as usize;
                if o + 3 >= dst_px.len() {
                    continue;
                }
                let (slx, sly) = (x as i32 - src.ox, y as i32 - src.oy);
                let (sr, sg, sb, mut sa) =
                    if slx < 0 || sly < 0 || slx >= w as i32 || sly >= h as i32 {
                        (0, 0, 0, 0.0)
                    } else {
                        let so = ((sly as u32 * w + slx as u32) * 4) as usize;
                        if so + 3 >= src_px.len() {
                            (0, 0, 0, 0.0)
                        } else {
                            let mut a = sa_base * src_px[so + 3] as f32 / 255.0;
                            if let Some(m) = src_mask.as_deref() {
                                a *= m.get(so / 4).copied().unwrap_or(255) as f32 / 255.0;
                            }
                            (src_px[so], src_px[so + 1], src_px[so + 2], a)
                        }
                    };
                if src.clip {
                    let dst_a = dst_px[o + 3] as f32 / 255.0;
                    let dst_m = dst_mask
                        .as_deref()
                        .and_then(|m| m.get(o / 4).copied())
                        .unwrap_or(255) as f32
                        / 255.0;
                    sa *= dst_a * dst_m;
                }
                let da = dst_px[o + 3] as f32 / 255.0;
                let out_a = sa + da * (1.0 - sa);
                if out_a > 0.0 {
                    for (ch, sc) in [sr, sg, sb].iter().enumerate() {
                        let blended = src.blend.apply(*sc as f32, dst_px[o + ch] as f32);
                        dst_px[o + ch] = ((blended * sa + dst_px[o + ch] as f32 * da * (1.0 - sa))
                            / out_a)
                            .round() as u8;
                    }
                    dst_px[o + 3] = (out_a * 255.0).round() as u8;
                }
            }
        }
        // The composite bakes the destination's rendered text: the result is
        // a plain pixel layer, never editable text (a text source above
        // vanishes with `src`). Pixels already match, so only the block drops.
        self.layers[index - 1].text = None;
        self.layers[index - 1].pixels.try_set_dense(&dst_px)?;
        self.layers.remove(index);
        self.active = index - 1;
        self.mark_full_dirty();
        Ok(true)
    }

    /// Drop text layer `index`'s vector source, leaving its rendered pixels
    /// as an ordinary layer ("rasterize"). Pixels are untouched, so the
    /// composite is unchanged; returns false when the layer is not text.
    /// Undoable like any pixel edit (snapshots carry the block).
    pub fn rasterize_text_layer(&mut self, index: usize) -> bool {
        match self.layers.get_mut(index) {
            Some(layer) if layer.text.is_some() => {
                layer.text = None;
                true
            }
            _ => false,
        }
    }

    /// Topmost visible text layer whose block bounds contain canvas point
    /// `(x, y)` — bounds are `[x, x+w) x [y, y+h)` in canvas pixels. Used by
    /// the text tool to load an existing layer for editing.
    pub fn text_layer_at(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.layers.len()).rev().find(|&i| {
            if !self.is_layer_shown(i) {
                return false;
            }
            match &self.layers[i].text {
                Some(b) => x >= b.x && y >= b.y && x < b.x + b.w as f32 && y < b.y + b.h as f32,
                None => false,
            }
        })
    }

    /// Composited RGB at `(x, y)` for the eyedropper, if inside the canvas.
    pub fn sample(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if x >= self.w || y >= self.h {
            return None;
        }
        let px = self.composite_pixel(x, y);
        Some([px[0], px[1], px[2]])
    }
}
