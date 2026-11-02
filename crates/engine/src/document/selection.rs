//! Selection editing: rectangular/lasso selection and its morphology.

use super::*;

impl Document {
    /// Clear both the rectangular selection and the lasso mask.
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.selection_mask = None;
    }

    /// Selection coverage 0.0..=1.0 at a canvas pixel (1.0 with no mask):
    /// how strongly paint and filters apply through graduated feathered
    /// masks. Binary masks behave exactly as before.
    pub(crate) fn sel_k(mask: Option<&[u8]>, w: u32, x: u32, y: u32) -> f32 {
        match mask {
            Some(m) => m.get((y * w + x) as usize).copied().unwrap_or(0) as f32 / 255.0,
            None => 1.0,
        }
    }

    /// True when `(px, py)` is inside the active selection (rectangular
    /// and/or lasso). With no selection active every pixel counts.
    pub fn pixel_selected(&self, px: u32, py: u32) -> bool {
        if px >= self.w || py >= self.h {
            return false;
        }
        if let Some((sx0, sy0, sx1, sy1)) = self.selection {
            if px < sx0 || px > sx1 || py < sy0 || py > sy1 {
                return false;
            }
        }
        if let Some(mask) = self.selection_mask.as_deref() {
            if mask.get((py * self.w + px) as usize).copied().unwrap_or(0) == 0 {
                return false;
            }
        }
        true
    }

    /// Build a lasso mask from a freehand loop (canvas float coordinates).
    /// Sets the rectangular selection to the loop's bounding box and returns
    /// it, or clears the mask and returns `None` for degenerate loops.
    pub fn set_lasso(
        &mut self,
        points: &[(f32, f32)],
    ) -> Result<Option<(u32, u32, u32, u32)>, String> {
        if points.len() < 3 || self.w == 0 || self.h == 0 {
            self.selection_mask = None;
            return Ok(None);
        }
        let (w, h) = (self.w, self.h);
        let mut fx0 = f32::INFINITY;
        let mut fy0 = f32::INFINITY;
        let mut fx1 = f32::NEG_INFINITY;
        let mut fy1 = f32::NEG_INFINITY;
        for &(x, y) in points {
            fx0 = fx0.min(x);
            fy0 = fy0.min(y);
            fx1 = fx1.max(x);
            fy1 = fy1.max(y);
        }
        let (x0, y0) = (fx0.floor().max(0.0) as u32, fy0.floor().max(0.0) as u32);
        let (x1, y1) = (
            fx1.ceil().min(w as f32 - 1.0).max(0.0) as u32,
            fy1.ceil().min(h as f32 - 1.0).max(0.0) as u32,
        );
        if x1 <= x0 || y1 <= y0 {
            self.selection_mask = None;
            return Ok(None);
        }
        // Even-odd scanline fill of the polygon.
        let n = points.len();
        let mut mask = try_zeroed((w * h) as usize)?;
        for y in y0..=y1 {
            let yc = y as f32 + 0.5;
            let mut xs = Vec::new();
            for i in 0..n {
                let (ax, ay) = points[i];
                let (bx, by) = points[(i + 1) % n];
                if (ay > yc) != (by > yc) {
                    xs.push(ax + (yc - ay) / (by - ay) * (bx - ax));
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            for pair in xs.as_chunks::<2>().0 {
                let xa = (pair[0] - 0.5).ceil() as i32;
                let xb = (pair[1] - 0.5 - 1e-6).floor() as i32;
                for x in xa.max(x0 as i32)..=xb.min(x1 as i32) {
                    if x >= 0 {
                        mask[(y * w + x as u32) as usize] = 255;
                    }
                }
            }
        }
        if mask.iter().all(|&b| b == 0) {
            self.selection_mask = None;
            return Ok(None);
        }
        self.selection = Some((x0, y0, x1, y1));
        self.selection_mask = Some(mask);
        Ok(Some((x0, y0, x1, y1)))
    }

    /// Pixels covered by the active selection (mask coverage for graduated
    /// feathered masks, rectangle area, else 0 when nothing is selected).
    pub fn selection_pixel_count(&self) -> usize {
        if let Some(mask) = self.selection_mask.as_deref() {
            ((mask.iter().map(|&b| b as u32).sum::<u32>() + 127) / 255) as usize
        } else if let Some((x0, y0, x1, y1)) = self.selection {
            ((x1 - x0 + 1) * (y1 - y0 + 1)) as usize
        } else {
            0
        }
    }

    /// Honored radius bound for feather/grow (the UI offers a subset).
    pub const MAX_SELECT_RADIUS: i32 = 64;

    /// Bounding box of nonzero mask pixels (tight marching-ants bounds).
    fn mask_bbox(mask: &[u8], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if mask[(y * w + x) as usize] != 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if x0 == u32::MAX {
            None
        } else {
            Some((x0, y0, x1, y1))
        }
    }

    /// Rasterize the rectangular selection into the mask (255 inside),
    /// ANDed with any existing mask. Returns false when nothing is
    /// selected (mask-only selections pass through untouched).
    fn bake_rect_to_mask(&mut self) -> Result<bool, String> {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 {
            return Ok(false);
        }
        let Some((rx0, ry0, rx1, ry1)) = self.selection else {
            return Ok(self
                .selection_mask
                .as_ref()
                .is_some_and(|m| m.iter().any(|&b| b != 0)));
        };
        let n = (w * h) as usize;
        let mut mask = try_zeroed(n)?;
        for y in ry0..=ry1.min(h - 1) {
            for x in rx0..=rx1.min(w - 1) {
                mask[(y * w + x) as usize] = 255;
            }
        }
        if let Some(old) = self.selection_mask.as_deref() {
            if old.len() == n {
                for (d, s) in mask.iter_mut().zip(old.iter()) {
                    *d = (*d).min(*s);
                }
            }
        }
        if mask.iter().all(|&b| b == 0) {
            self.clear_selection();
            return Ok(false);
        }
        self.selection_mask = Some(mask);
        Ok(true)
    }

    /// Re-tighten the bounds rect to the mask (clears both when empty).
    fn retighten_selection(&mut self) {
        match self.selection_mask.as_deref() {
            Some(m) => {
                self.selection = Self::mask_bbox(m, self.w, self.h);
                if self.selection.is_none() {
                    self.selection_mask = None;
                }
            }
            None => self.selection = None,
        }
    }

    /// Separable sliding-window blur over one gray channel (edge-replicated).
    fn blur_gray(src: &[u8], w: u32, h: u32, r: i32) -> Result<Vec<u8>, String> {
        if r <= 0 || w == 0 || h == 0 {
            return try_copy(src);
        }
        let (w, h, r) = (w as usize, h as usize, r as isize);
        let win = (2 * r + 1) as u32;
        let at = |buf: &[u8], i: isize, n: isize| buf[i.clamp(0, n - 1) as usize] as u32;
        let mut tmp = try_zeroed(w * h)?;
        for y in 0..h {
            let mut sum = 0u32;
            for x in -r..=r {
                sum += at(&src[y * w..(y + 1) * w], x, w as isize);
            }
            for x in 0..w {
                tmp[y * w + x] = ((sum + win / 2) / win) as u8;
                sum = sum - at(&src[y * w..(y + 1) * w], x as isize - r, w as isize)
                    + at(&src[y * w..(y + 1) * w], x as isize + r + 1, w as isize);
            }
        }
        let mut out = try_zeroed(w * h)?;
        for x in 0..w {
            let mut sum = 0u32;
            for y in -r..=r {
                let yy = y.clamp(0, h as isize - 1) as usize;
                sum += tmp[yy * w + x] as u32;
            }
            for y in 0..h {
                out[y * w + x] = ((sum + win / 2) / win) as u8;
                let yo = (y as isize - r).clamp(0, h as isize - 1) as usize;
                let yi = (y as isize + r + 1).clamp(0, h as isize - 1) as usize;
                sum = sum - tmp[yo * w + x] as u32 + tmp[yi * w + x] as u32;
            }
        }
        Ok(out)
    }

    /// Sliding-window extremum over a replicated-edge line.
    fn slide_extreme(line: &[u8], r: usize, dilate: bool) -> Result<Vec<u8>, String> {
        use std::collections::VecDeque;
        let n = line.len();
        if n == 0 || r == 0 {
            return try_copy(line);
        }
        let mut pad = try_vec(n + 2 * r, 0u8)?;
        for slot in pad.iter_mut().take(r) {
            *slot = line[0];
        }
        pad[r..r + n].copy_from_slice(line);
        for slot in pad.iter_mut().skip(r + n) {
            *slot = line[n - 1];
        }
        let better = |a: u8, b: u8| if dilate { a >= b } else { a <= b };
        let mut out = try_vec(n, 0u8)?;
        let mut dq: VecDeque<usize> = VecDeque::new();
        for i in 0..pad.len() {
            while let Some(&back) = dq.back() {
                if better(pad[i], pad[back]) {
                    dq.pop_back();
                } else {
                    break;
                }
            }
            dq.push_back(i);
            if i >= 2 * r {
                let j = i - 2 * r;
                while let Some(&front) = dq.front() {
                    if front < j {
                        dq.pop_front();
                    } else {
                        break;
                    }
                }
                if j < n {
                    out[j] = pad[*dq.front().unwrap()];
                }
            }
        }
        Ok(out)
    }

    /// Separable box dilate (max) / erode (min) over one gray channel.
    fn morph_gray(src: &[u8], w: u32, h: u32, r: i32, dilate: bool) -> Result<Vec<u8>, String> {
        if r <= 0 || w == 0 || h == 0 {
            return try_copy(src);
        }
        let (w, h, r) = (w as usize, h as usize, r as usize);
        let mut tmp = try_zeroed(w * h)?;
        for y in 0..h {
            tmp[y * w..(y + 1) * w].copy_from_slice(&Self::slide_extreme(
                &src[y * w..(y + 1) * w],
                r,
                dilate,
            )?);
        }
        let mut out = try_zeroed(w * h)?;
        let mut col = try_vec(h, 0u8)?;
        for x in 0..w {
            for y in 0..h {
                col[y] = tmp[y * w + x];
            }
            let done = Self::slide_extreme(&col, r, dilate)?;
            for y in 0..h {
                out[y * w + x] = done[y];
            }
        }
        Ok(out)
    }

    /// Feather (soften) the selection edge by `radius` px: the mask ramps
    /// instead of stepping, so paint and filters fade across the edge.
    /// The bounds rect grows to contain the falloff. Returns false when
    /// there was nothing to feather. Selection-only: not undoable (like
    /// Clear selection).
    pub fn feather_selection(&mut self, radius: u32) -> Result<bool, String> {
        let r = radius.min(Self::MAX_SELECT_RADIUS as u32) as i32;
        if !self.bake_rect_to_mask()? {
            return Ok(false);
        }
        if r > 0 {
            if let Some(m) = self.selection_mask.as_deref() {
                let blurred = Self::blur_gray(m, self.w, self.h, r)?;
                self.selection_mask = Some(blurred);
            }
            self.retighten_selection();
        }
        Ok(self.selection_mask.is_some())
    }

    /// Grow (positive) or shrink (negative) the selection by `px`,
    /// clamped to ±[`Document::MAX_SELECT_RADIUS`]. Shrinking to nothing
    /// clears it. Returns false when nothing remains. Not undoable.
    pub fn grow_selection(&mut self, px: i32) -> Result<bool, String> {
        let px = px.clamp(-Self::MAX_SELECT_RADIUS, Self::MAX_SELECT_RADIUS);
        if !self.bake_rect_to_mask()? {
            return Ok(false);
        }
        if px == 0 {
            return Ok(true);
        }
        if let Some(m) = self.selection_mask.as_deref() {
            let morphed = Self::morph_gray(m, self.w, self.h, px.abs(), px > 0)?;
            self.selection_mask = Some(morphed);
        }
        self.retighten_selection();
        Ok(self.selection_mask.is_some())
    }

    /// De-jag the selection edge (blur + re-threshold, edge position kept).
    /// Returns false when there was nothing to smooth. Not undoable.
    pub fn smooth_selection(&mut self) -> Result<bool, String> {
        if !self.bake_rect_to_mask()? {
            return Ok(false);
        }
        if let Some(m) = self.selection_mask.as_deref() {
            let soft = Self::blur_gray(m, self.w, self.h, 2)?;
            let mut out = try_zeroed(soft.len())?;
            for (d, &v) in out.iter_mut().zip(&soft) {
                *d = if v >= 128 { 255 } else { 0 };
            }
            self.selection_mask = Some(out);
        }
        self.retighten_selection();
        Ok(self.selection_mask.is_some())
    }

    /// Flip the selection (unbounded complements become full-canvas masks).
    /// Returns false when there was nothing to invert. Not undoable.
    pub fn invert_selection(&mut self) -> Result<bool, String> {
        if !self.bake_rect_to_mask()? {
            return Ok(false);
        }
        if let Some(m) = self.selection_mask.as_deref() {
            let mut out = try_zeroed(m.len())?;
            for (d, &v) in out.iter_mut().zip(m) {
                *d = 255 - v;
            }
            self.selection_mask = Some(out);
        }
        self.retighten_selection();
        Ok(self.selection_mask.is_some())
    }

    /// Nudge the selection by `(dx, dy)` canvas px (rect corners clamp to
    /// the canvas; masks shift with zero-fill, so content pushed off the
    /// edge is lost). Shrinking to nothing clears it. Returns false when
    /// nothing remains. Not undoable.
    pub fn translate_selection(&mut self, dx: i32, dy: i32) -> Result<bool, String> {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 || !self.bake_rect_to_mask()? {
            return Ok(false);
        }
        if dx == 0 && dy == 0 {
            return Ok(true);
        }
        let Some(m) = self.selection_mask.as_deref() else {
            return Ok(false);
        };
        let n = (w * h) as usize;
        let mut out = try_zeroed(n)?;
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x as i32 - dx, y as i32 - dy);
                if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                    out[(y * w + x) as usize] = m[(sy as u32 * w + sx as u32) as usize];
                }
            }
        }
        if out.iter().all(|&b| b == 0) {
            self.clear_selection();
            return Ok(false);
        }
        self.selection_mask = Some(out);
        self.retighten_selection();
        Ok(true)
    }

    /// Mirror the selection left-right (rect corners remap, masks flip).
    /// Returns false when there was no selection. Not undoable.
    pub fn flip_selection_h(&mut self) -> Result<bool, String> {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 || !self.bake_rect_to_mask()? {
            return Ok(false);
        }
        if let Some(m) = self.selection_mask.as_deref() {
            let mut out = try_zeroed((w * h) as usize)?;
            for y in 0..h {
                for x in 0..w {
                    out[(y * w + x) as usize] = m[(y * w + (w - 1 - x)) as usize];
                }
            }
            self.selection_mask = Some(out);
        }
        self.retighten_selection();
        Ok(self.selection_mask.is_some())
    }

    /// Mirror the selection top-bottom. Returns false when there was no
    /// selection. Not undoable.
    pub fn flip_selection_v(&mut self) -> Result<bool, String> {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 || !self.bake_rect_to_mask()? {
            return Ok(false);
        }
        if let Some(m) = self.selection_mask.as_deref() {
            let mut out = try_zeroed((w * h) as usize)?;
            for y in 0..h {
                for x in 0..w {
                    out[(y * w + x) as usize] = m[((h - 1 - y) * w + x) as usize];
                }
            }
            self.selection_mask = Some(out);
        }
        self.retighten_selection();
        Ok(self.selection_mask.is_some())
    }

    /// Select the active layer's pixels more opaque than `threshold`
    /// (offset-aware; uncovered canvas counts as transparent). Replaces any
    /// selection; empty layers clear it. Returns false when nothing is
    /// selected. Not undoable.
    pub fn select_opaque(&mut self, threshold: u8) -> Result<bool, String> {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 {
            self.clear_selection();
            return Ok(false);
        }
        let Some(layer) = self.layers.get(self.active) else {
            self.clear_selection();
            return Ok(false);
        };
        let (ox, oy) = (layer.ox, layer.oy);
        let src = layer.pixels.try_to_dense()?;
        let mut mask = try_zeroed((w * h) as usize)?;
        for y in 0..h {
            for x in 0..w {
                let (lx, ly) = (x as i32 - ox, y as i32 - oy);
                let a = if lx < 0 || ly < 0 || lx >= w as i32 || ly >= h as i32 {
                    0
                } else {
                    let o = ((ly as u32 * w + lx as u32) * 4) as usize;
                    if o + 3 < src.len() {
                        src[o + 3]
                    } else {
                        0
                    }
                };
                if a > threshold {
                    mask[(y * w + x) as usize] = 255;
                }
            }
        }
        self.selection_mask = if mask.iter().any(|&b| b != 0) {
            Some(mask)
        } else {
            None
        };
        self.retighten_selection();
        Ok(self.selection_mask.is_some())
    }

    /// Paint the selection outline `width` px wide (clamped 1..=32) with
    /// `brush` at `color` through `put` (symmetry- and target-aware). The
    /// edge comes from the selection itself. Caller checkpoints for undo.
    /// Returns false without a selection.
    pub fn stroke_selection(
        &mut self,
        brush: &crate::brush::Brush,
        color: crate::brush::Color,
        width: u32,
    ) -> Result<bool, String> {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 {
            return Ok(false);
        }
        let inside = |m: &[u8], x: i32, y: i32| -> bool {
            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                return false;
            }
            m[(y as u32 * w + x as u32) as usize] > 127
        };
        let mut edge = try_vec((w * h) as usize, false)?;
        let mut any = false;
        match self.selection_mask.as_deref() {
            Some(m) if m.len() == (w * h) as usize => {
                for y in 0..h as i32 {
                    for x in 0..w as i32 {
                        if !inside(m, x, y) {
                            continue;
                        }
                        if !(inside(m, x - 1, y)
                            && inside(m, x + 1, y)
                            && inside(m, x, y - 1)
                            && inside(m, x, y + 1))
                        {
                            edge[(y as u32 * w + x as u32) as usize] = true;
                            any = true;
                        }
                    }
                }
            }
            _ => {
                if let Some((x0, y0, x1, y1)) = self.selection {
                    for x in x0..=x1.min(w - 1) {
                        for &y in &[y0, y1] {
                            if y < h {
                                edge[(y * w + x) as usize] = true;
                                any = true;
                            }
                        }
                    }
                    for y in y0..=y1.min(h - 1) {
                        for &x in &[x0, x1] {
                            if x < w {
                                edge[(y * w + x) as usize] = true;
                                any = true;
                            }
                        }
                    }
                }
            }
        }
        if !any {
            return Ok(false);
        }
        let r = width.clamp(1, 32) as i32 - 1;
        if r > 0 {
            let mut gray = try_zeroed(edge.len())?;
            for (g, &e) in gray.iter_mut().zip(&edge) {
                *g = if e { 255 } else { 0 };
            }
            let fat = Self::morph_gray(&gray, w, h, r, true)?;
            for (e, &v) in edge.iter_mut().zip(&fat) {
                *e = v != 0;
            }
        }
        if self.symmetry == SymmetryMode::Off {
            // Dense row spans: one read/write per row instead of a tile
            // lookup per edge pixel.
            let mut row = Vec::new();
            for y in 0..h {
                let base = (y * w) as usize;
                let Some(first) = (0..w as usize).find(|&i| edge[base + i]) else {
                    continue;
                };
                let last = (0..w as usize)
                    .rev()
                    .find(|&i| edge[base + i])
                    .unwrap_or(first);
                self.put_row_span(brush, color, y, first as u32, last as u32, &mut row, |x| {
                    edge[base + x as usize]
                });
            }
        } else {
            for y in 0..h {
                for x in 0..w {
                    if edge[(y * w + x) as usize] {
                        self.put(brush, color, x, y);
                    }
                }
            }
        }
        self.mark_full_dirty();
        Ok(true)
    }
}
