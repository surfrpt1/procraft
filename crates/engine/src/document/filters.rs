//! Layer filters: tone, blur/sharpen, LUTs, HSL, balance and colorize.

use super::*;

impl Document {
    /// Separable sliding-window box blur (all channels), O(w*h) and
    /// row-major in both passes.
    ///
    /// The vertical pass carries one running sum per column instead of
    /// walking each column's samples: the old column-strided walk cost a
    /// cache miss per sample (2.3 s per 8K frame) because one row is 30 KB.
    /// Sums are clamped-window sums of at most `255 * window`, so a
    /// per-radius divide table gives the same integer truncation as
    /// `sum / window` without a runtime division per channel.
    pub(super) fn box_blur(src: &[u8], w: u32, h: u32, r: i32) -> Result<Vec<u8>, String> {
        let (wu, hu) = (w as usize, h as usize);
        if r <= 0 || wu == 0 || hu == 0 {
            return try_copy(src);
        }
        let r = r as usize;
        let window = (2 * r + 1) as u32;
        let mut div = try_zeroed((255 * window) as usize + 1)?;
        for (s, d) in div.iter_mut().enumerate() {
            *d = (s as u32 / window) as u8;
        }
        let stride = wu * 4;
        // Window sample offsets per coordinate (scaled by `step`), clamped
        // to the edge: the entering sample and the leaving one.
        let edge_offsets = |n: usize, step: usize| -> Result<Vec<(usize, usize)>, String> {
            let mut out = try_vec(n, (0usize, 0usize))?;
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = ((i + r + 1).min(n - 1) * step, i.saturating_sub(r) * step);
            }
            Ok(out)
        };
        // Horizontal pass: src -> tmp, one sliding sum per row.
        let x_offsets = edge_offsets(wu, 4)?;
        let mut tmp = try_zeroed(src.len())?;
        for y in 0..hu {
            let row = &src[y * stride..y * stride + stride];
            let dst = &mut tmp[y * stride..y * stride + stride];
            let mut sum = [0u32; 4];
            for dx in 0..=2 * r {
                let o = dx.saturating_sub(r).min(wu - 1) * 4;
                for (s, &v) in sum.iter_mut().zip(&row[o..o + 4]) {
                    *s += v as u32;
                }
            }
            for (x, &(add, sub)) in x_offsets.iter().enumerate() {
                // Four-byte slices let the bounds checks hoist out of the
                // channel loop; per-byte indexing cost ~25% here.
                let d = &mut dst[x * 4..x * 4 + 4];
                let a = &row[add..add + 4];
                let s_in = &row[sub..sub + 4];
                let pairs = a.iter().zip(s_in.iter());
                for (ch, (s, (&a, b))) in sum.iter_mut().zip(pairs).enumerate() {
                    let q = *s;
                    d[ch] = div[q as usize];
                    *s = q + a as u32 - *b as u32;
                }
            }
        }
        // Vertical pass: tmp -> out, one sliding sum per column row.
        let mut out = try_zeroed(src.len())?;
        let mut acc = try_vec(stride, 0u32)?;
        for dy in 0..=2 * r {
            let o = dy.saturating_sub(r).min(hu - 1) * stride;
            for (a, &b) in acc.iter_mut().zip(&tmp[o..o + stride]) {
                *a += b as u32;
            }
        }
        let y_offsets = edge_offsets(hu, stride)?;
        for y in 0..hu {
            let dst = &mut out[y * stride..y * stride + stride];
            for (d, &s) in dst.iter_mut().zip(&acc) {
                *d = div[s as usize];
            }
            let (add, sub) = y_offsets[y];
            let add = &tmp[add..add + stride];
            let sub = &tmp[sub..sub + stride];
            for ((a, &b), &c) in acc.iter_mut().zip(add).zip(sub) {
                *a = *a + b as u32 - c as u32;
            }
        }
        Ok(out)
    }

    /// Brightness/contrast on a layer: `brightness` -255..=255 is added,
    /// `contrast` 0.0..=3.0 scales around mid-gray (1.0 keeps contrast).
    /// RGB only; alpha is untouched. Honors the active selection.
    /// Caller checkpoints for undo first.
    pub fn adjust_tone(
        &mut self,
        index: usize,
        brightness: i32,
        contrast: f32,
    ) -> Result<(), String> {
        if index >= self.layers.len() || (brightness == 0 && (contrast - 1.0).abs() < f32::EPSILON)
        {
            return Ok(());
        }
        let (w, h) = (self.w, self.h);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.try_to_dense()?;
        for y in 0..h {
            for x in 0..w {
                if let Some((a, b, c, d)) = sel {
                    if x < a || x > c || y < b || y > d {
                        continue;
                    }
                }
                if let Some(m) = mask {
                    if m[(y * w + x) as usize] == 0 {
                        continue;
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                if o + 2 >= pixels.len() {
                    continue;
                }
                // Graduated masks partially apply (binary masks: full strength).
                let k = Self::sel_k(mask, w, x, y);
                for ch in 0..3 {
                    let v = (pixels[o + ch] as f32 - 128.0) * contrast + 128.0 + brightness as f32;
                    let mixed = pixels[o + ch] as f32 * (1.0 - k) + v * k;
                    pixels[o + ch] = mixed.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        self.layers[index].pixels.try_set_dense(&pixels)?;
        self.mark_full_dirty();
        Ok(())
    }

    /// Box blur on a layer, `radius` 1..=25 (0 is a no-op). Honors the
    /// active selection for writes. Caller checkpoints for undo first.
    pub fn blur_layer(&mut self, index: usize, radius: u32) -> Result<(), String> {
        if index >= self.layers.len() || radius == 0 {
            return Ok(());
        }
        let (w, h) = (self.w, self.h);
        let dense = self.layers[index].pixels.try_to_dense()?;
        let blurred = Self::box_blur(&dense, w, h, radius.min(25) as i32)?;
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        // Without a selection or mask every pixel takes the blurred value;
        // the blend below would be the identity, so skip it (and the second
        // dense copy of the layer) entirely.
        if sel.is_none() && mask.is_none() {
            self.layers[index].pixels.try_set_dense(&blurred)?;
            self.mark_full_dirty();
            return Ok(());
        }
        let mut pixels = dense;
        for y in 0..h {
            for x in 0..w {
                let selected = !matches!(sel, Some((a, b, c, d)) if x < a || x > c || y < b || y > d)
                    && match mask {
                        Some(m) => m[(y * w + x) as usize] != 0,
                        None => true,
                    };
                if selected {
                    let o = ((y * w + x) * 4) as usize;
                    let k = Self::sel_k(mask, w, x, y);
                    for ch in 0..4 {
                        pixels[o + ch] = (pixels[o + ch] as f32 * (1.0 - k)
                            + blurred[o + ch] as f32 * k)
                            .round() as u8;
                    }
                }
            }
        }
        self.layers[index].pixels.try_set_dense(&pixels)?;
        self.mark_full_dirty();
        Ok(())
    }

    /// Unsharp mask on a layer: `amount` 0.0..=2.0 mixes the high-frequency
    /// detail back in (0 is a no-op). Honors the active selection.
    /// Caller checkpoints for undo first.
    pub fn sharpen_layer(&mut self, index: usize, amount: f32) -> Result<(), String> {
        if index >= self.layers.len() || amount <= 0.0 {
            return Ok(());
        }
        let amount = amount.min(2.0);
        let (w, h) = (self.w, self.h);
        let src = self.layers[index].pixels.try_to_dense()?;
        let blurred = Self::box_blur(&src, w, h, 2)?;
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.try_to_dense()?;
        for y in 0..h {
            for x in 0..w {
                let selected = !matches!(sel, Some((a, b, c, d)) if x < a || x > c || y < b || y > d)
                    && match mask {
                        Some(m) => m[(y * w + x) as usize] != 0,
                        None => true,
                    };
                if !selected {
                    continue;
                }
                let o = ((y * w + x) * 4) as usize;
                let k = Self::sel_k(mask, w, x, y);
                for ch in 0..3 {
                    let v = pixels[o + ch] as f32
                        + (pixels[o + ch] as f32 - blurred[o + ch] as f32) * amount;
                    let mixed = pixels[o + ch] as f32 * (1.0 - k) + v * k;
                    pixels[o + ch] = mixed.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        self.layers[index].pixels.try_set_dense(&pixels)?;
        self.mark_full_dirty();
        Ok(())
    }

    /// Remap the RGB channels through `lut` (alpha untouched). Honors the
    /// active selection with graduated-mask mixing. Caller checkpoints
    /// for undo first. Powers Curves + Levels (M36) and the M37 pack.
    pub fn apply_lut(&mut self, index: usize, lut: &[u8; 256]) -> Result<(), String> {
        if index >= self.layers.len() {
            return Ok(());
        }
        let (w, h) = (self.w, self.h);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.try_to_dense()?;
        for y in 0..h {
            for x in 0..w {
                if let Some((a, b, c, d)) = sel {
                    if x < a || x > c || y < b || y > d {
                        continue;
                    }
                }
                if let Some(m) = mask {
                    if m[(y * w + x) as usize] == 0 {
                        continue;
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                if o + 2 >= pixels.len() {
                    continue;
                }
                let k = Self::sel_k(mask, w, x, y);
                for ch in 0..3 {
                    let v = lut[pixels[o + ch] as usize] as f32;
                    let mixed = pixels[o + ch] as f32 * (1.0 - k) + v * k;
                    pixels[o + ch] = mixed.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        self.layers[index].pixels.try_set_dense(&pixels)?;
        self.mark_full_dirty();
        Ok(())
    }

    /// M37 HSL adjust: rotate hue by `hue_deg` degrees, scale saturation
    /// by `sat_mult`, shift lightness by `light` (-1..=1). Grays have no
    /// hue, so they only answer to lightness (same honesty as the palette
    /// harmonies). Honors the active selection. Caller checkpoints first.
    pub fn adjust_hsl(
        &mut self,
        index: usize,
        hue_deg: f32,
        sat_mult: f32,
        light: f32,
    ) -> Result<(), String> {
        if index >= self.layers.len() {
            return Ok(());
        }
        let sat_mult = sat_mult.clamp(0.0, 4.0);
        let light = light.clamp(-1.0, 1.0);
        if hue_deg == 0.0 && sat_mult == 1.0 && light == 0.0 {
            return Ok(());
        }
        let (w, h) = (self.w, self.h);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.try_to_dense()?;
        for y in 0..h {
            for x in 0..w {
                if let Some((a, b, c, d)) = sel {
                    if x < a || x > c || y < b || y > d {
                        continue;
                    }
                }
                if let Some(m) = mask {
                    if m[(y * w + x) as usize] == 0 {
                        continue;
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                if o + 2 >= pixels.len() {
                    continue;
                }
                let (hh, ss, ll) =
                    crate::palette::rgb_to_hsl([pixels[o], pixels[o + 1], pixels[o + 2]]);
                let v = if ss <= 0.0 {
                    let g = ((ll + light).clamp(0.0, 1.0) * 255.0).round();
                    [g as u8, g as u8, g as u8]
                } else {
                    crate::palette::hsl_to_rgb(
                        hh + hue_deg,
                        (ss * sat_mult).clamp(0.0, 1.0),
                        (ll + light).clamp(0.0, 1.0),
                    )
                };
                let k = Self::sel_k(mask, w, x, y);
                for ch in 0..3 {
                    let mixed = pixels[o + ch] as f32 * (1.0 - k) + v[ch] as f32 * k;
                    pixels[o + ch] = mixed.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        self.layers[index].pixels.try_set_dense(&pixels)?;
        self.mark_full_dirty();
        Ok(())
    }

    /// M37 Color balance: channel offsets in -255..=255 (positive warms
    /// toward red/green/blue, negative toward cyan/magenta/yellow).
    /// Honors the active selection. Caller checkpoints first.
    pub fn color_balance(&mut self, index: usize, r: i32, g: i32, b: i32) -> Result<(), String> {
        if index >= self.layers.len() || (r == 0 && g == 0 && b == 0) {
            return Ok(());
        }
        let off = [r as f32, g as f32, b as f32];
        let (w, h) = (self.w, self.h);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.try_to_dense()?;
        for y in 0..h {
            for x in 0..w {
                if let Some((a, b, c, d)) = sel {
                    if x < a || x > c || y < b || y > d {
                        continue;
                    }
                }
                if let Some(m) = mask {
                    if m[(y * w + x) as usize] == 0 {
                        continue;
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                if o + 2 >= pixels.len() {
                    continue;
                }
                let k = Self::sel_k(mask, w, x, y);
                for ch in 0..3 {
                    let v = pixels[o + ch] as f32 + off[ch];
                    let mixed = pixels[o + ch] as f32 * (1.0 - k) + v * k;
                    pixels[o + ch] = mixed.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        self.layers[index].pixels.try_set_dense(&pixels)?;
        self.mark_full_dirty();
        Ok(())
    }

    /// M37 Recolor (colorize): push every pixel's hue/saturation toward
    /// `target` while keeping its own luminance, mixed by `strength`
    /// 0..=1 (0 is a no-op). White stays white, black stays black —
    /// that is what colorize means. Honors the active selection.
    /// Caller checkpoints first.
    pub fn colorize(&mut self, index: usize, target: [u8; 3], strength: f32) -> Result<(), String> {
        if index >= self.layers.len() || strength <= 0.0 {
            return Ok(());
        }
        let strength = strength.clamp(0.0, 1.0);
        let (th, ts, _) = crate::palette::rgb_to_hsl(target);
        let (w, h) = (self.w, self.h);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.try_to_dense()?;
        for y in 0..h {
            for x in 0..w {
                if let Some((a, b, c, d)) = sel {
                    if x < a || x > c || y < b || y > d {
                        continue;
                    }
                }
                if let Some(m) = mask {
                    if m[(y * w + x) as usize] == 0 {
                        continue;
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                if o + 2 >= pixels.len() {
                    continue;
                }
                let (_, _, ll) =
                    crate::palette::rgb_to_hsl([pixels[o], pixels[o + 1], pixels[o + 2]]);
                let tint = crate::palette::hsl_to_rgb(th, ts, ll);
                let k = Self::sel_k(mask, w, x, y) * strength;
                for ch in 0..3 {
                    let mixed = pixels[o + ch] as f32 * (1.0 - k) + tint[ch] as f32 * k;
                    pixels[o + ch] = mixed.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        self.layers[index].pixels.try_set_dense(&pixels)?;
        self.mark_full_dirty();
        Ok(())
    }
}

/// M36 Levels: stretch `[shadow, highlight]` to `[0, 255]` with `gamma`
/// (1.0 = linear; >1 darkens mids, <1 lifts them). Degenerate ranges
/// (`highlight <= shadow`) yield identity; gamma clamps to 0.1..=10.
pub fn levels_lut(shadow: u8, gamma: f32, highlight: u8) -> [u8; 256] {
    if highlight <= shadow {
        return std::array::from_fn(|i| i as u8);
    }
    let gamma = gamma.clamp(0.1, 10.0);
    let (lo, hi) = (shadow as f32, highlight as f32);
    std::array::from_fn(|i| {
        let t = ((i as f32 - lo) / (hi - lo)).clamp(0.0, 1.0);
        (t.powf(gamma) * 255.0).round().clamp(0.0, 255.0) as u8
    })
}

/// M36 Curves (parametric 3-point): `shadow`/`mid`/`high` lift (+) or
/// crush (-) the 64/128/192 control points; endpoints stay pinned, so
/// pure black/white never clip. Piecewise-linear between the five
/// points; all-zero is identity.
pub fn curves_lut(shadow: i32, mid: i32, high: i32) -> [u8; 256] {
    let pts = [
        (0u8, 0u8),
        (64, (64 + shadow).clamp(0, 255) as u8),
        (128, (128 + mid).clamp(0, 255) as u8),
        (192, (192 + high).clamp(0, 255) as u8),
        (255, 255),
    ];
    let mut lut = [0u8; 256];
    for (i, slot) in lut.iter_mut().enumerate() {
        let v = i as u8;
        let mut y = 255u8;
        for w in pts.windows(2) {
            let ((x0, y0), (x1, y1)) = ((w[0].0, w[0].1), (w[1].0, w[1].1));
            if v >= x0 && v <= x1 {
                if x1 == x0 {
                    y = y1;
                } else {
                    let t = (v - x0) as f32 / (x1 - x0) as f32;
                    y = (y0 as f32 + (y1 as f32 - y0 as f32) * t).round() as u8;
                }
                break;
            }
        }
        *slot = y;
    }
    lut
}
