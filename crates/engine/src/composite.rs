//! Tile compositing: the engine side of lazy display.
//!
//! `Document::composite_pixel` stays the reference implementation; a
//! [`StackPlan`] freezes the per-layer blend inputs (visibility, effective
//! opacity, clip, offsets) once per document revision, and `composite_tile`
//! blends one 256² tile by reading each layer's tile directly.
//!
//! The plan does not own pixels: applying it needs the document it came
//! from. Rebuild it after any structural change (visibility, groups,
//! opacity, offsets, layer order).

use crate::document::{BlendMode, Document};
use crate::raster::TILE;

/// One layer's frozen blend inputs.
#[derive(Clone, Debug)]
struct PlannedLayer {
    index: usize,
    ox: i32,
    oy: i32,
    /// Layer opacity clamped to 0..=1 (before group opacity).
    opacity: f32,
    /// Product of ancestor group opacities.
    group: f32,
    blend: BlendMode,
    clip: bool,
    /// Pixels (and mask, when present) are canvas-shaped RGBA/mask rasters,
    /// so a tile-local read hits the same byte as the reference dense read.
    /// Foreign geometry (the type allows it; file readers reject it) takes
    /// the dense fallback and stays byte-identical.
    tiled: bool,
}

/// Per-layer blend inputs for the current visibility and groups.
#[derive(Clone, Debug, Default)]
pub struct StackPlan {
    layers: Vec<PlannedLayer>,
    /// `Document::revision()` at snapshot time. A mismatch means the plan
    /// outlived the state it describes and would blend stale inputs.
    rev: u64,
    /// Layer count at snapshot time: structural edits that reorder or drop
    /// layers bump `rev` too, but checking both makes a stale index obvious.
    layer_count: usize,
}

impl Document {
    /// Snapshot the per-layer blend inputs: visible layers, effective
    /// opacity (layer × groups), clip flags and offsets. The per-pixel
    /// loop never consults visibility or group opacity again.
    pub fn stack_plan(&self) -> StackPlan {
        let mut layers = Vec::with_capacity(self.layers.len());
        for (index, layer) in self.layers.iter().enumerate() {
            if !self.is_layer_shown(index) {
                continue;
            }
            // Kept as two factors, not one product: the reference blends
            // `alpha * layer_opacity * group_opacity` left to right, and
            // preserving that order keeps rounding identical.
            let opacity = layer.opacity.clamp(0.0, 1.0);
            let group = self.group_opacity(index);
            if opacity <= 0.0 || group <= 0.0 {
                continue;
            }
            let mask_tiled = match layer.mask.as_ref() {
                Some(m) => m.w() == self.w && m.h() == self.h && m.bpp() == 1,
                None => true,
            };
            let tiled = layer.pixels.w() == self.w
                && layer.pixels.h() == self.h
                && layer.pixels.bpp() == 4
                && mask_tiled;
            layers.push(PlannedLayer {
                index,
                ox: layer.ox,
                oy: layer.oy,
                opacity,
                group,
                blend: layer.blend,
                clip: layer.clip,
                tiled,
            });
        }
        StackPlan {
            layers,
            rev: self.revision(),
            layer_count: self.layers.len(),
        }
    }

    /// One tile of the composite, `TILE * TILE * 4` bytes, row-major.
    /// Pixels outside the canvas are zero.
    pub fn composite_tile(&self, plan: &StackPlan, tx: u32, ty: u32) -> Vec<u8> {
        let mut out = vec![0u8; (TILE * TILE * 4) as usize];
        self.composite_tile_into(plan, tx, ty, None, &mut out);
        out
    }

    /// Recompose `rect` (canvas pixels, inclusive) of one tile into `out`,
    /// a full-tile buffer. `None` covers the whole canvas part of the tile.
    ///
    /// Pixels outside `rect` are left untouched, so a cache can patch the
    /// tile it already holds instead of paying for the whole 256² square —
    /// a stroke event only changes a dab-wide band.
    pub fn composite_tile_into(
        &self,
        plan: &StackPlan,
        tx: u32,
        ty: u32,
        rect: Option<(u32, u32, u32, u32)>,
        out: &mut [u8],
    ) {
        // A stale plan would read layers by index after a reorder/removal.
        debug_assert_eq!(
            plan.rev,
            self.revision(),
            "stack plan is stale: rebuild it after document mutations"
        );
        debug_assert_eq!(
            plan.layer_count,
            self.layers.len(),
            "stack plan is stale: layer count changed since the snapshot"
        );
        debug_assert_eq!(
            out.len(),
            (TILE * TILE * 4) as usize,
            "tile buffer must hold one full tile"
        );
        let x0 = tx.saturating_mul(TILE);
        let y0 = ty.saturating_mul(TILE);
        let cw = self.w.saturating_sub(x0).min(TILE);
        let ch = self.h.saturating_sub(y0).min(TILE);
        if cw == 0 || ch == 0 {
            return;
        }
        // Tile-local, clamped row/column range of `rect` (or the whole tile).
        let (col0, col1, row0, row1) = match rect {
            Some((rx0, ry0, rx1, ry1)) => {
                if rx1 < x0 || ry1 < y0 || rx0 >= x0 + cw || ry0 >= y0 + ch {
                    return;
                }
                (
                    rx0.saturating_sub(x0),
                    (rx1 - x0).min(cw - 1),
                    ry0.saturating_sub(y0),
                    (ry1 - y0).min(ch - 1),
                )
            }
            None => (0, cw - 1, 0, ch - 1),
        };
        let (br, bg, bb) = (
            self.background[0] as f32,
            self.background[1] as f32,
            self.background[2] as f32,
        );
        // A tiled layer with no allocated tiles cannot contribute anywhere:
        // skip it once instead of looking up an absent tile per pixel. Most
        // layers of a large document are empty in most tiles, and those
        // tiles are recomposited constantly (stroke dirt, viewport passes).
        let live: Vec<&PlannedLayer> = plan
            .layers
            .iter()
            .filter(|pl| !pl.tiled || self.layers[pl.index].pixels.alloc_bytes() > 0)
            .collect();
        let fill_row = |out: &mut [u8], row: u32| {
            let start = (row * TILE + col0) as usize * 4;
            let end = (row * TILE + col1 + 1) as usize * 4;
            let px = [
                self.background[0],
                self.background[1],
                self.background[2],
                255,
            ];
            for slot in out[start..end].as_chunks_mut::<4>().0 {
                slot.copy_from_slice(&px);
            }
        };
        if live.is_empty() {
            // The page alone: every pixel is the same opaque background.
            for row in row0..=row1 {
                fill_row(out, row);
            }
            return;
        }
        // Per-row activity: a layer whose covered tile row holds no pixels
        // is skipped for the whole row. Reused across rows.
        let mut active = vec![false; live.len()];
        let w_u32 = self.w;
        let h_u32 = self.h;
        let (xc0, xc1) = (x0 + col0, x0 + col1 + 1);
        for row in row0..=row1 {
            let y = y0 + row;
            for (slot, pl) in active.iter_mut().zip(&live) {
                let lx0 = xc0 as i32 - pl.ox;
                let lx1 = xc1 as i32 - pl.ox;
                let ly = y as i32 - pl.oy;
                *slot = if lx0 >= w_u32 as i32 || lx1 <= 0 || ly < 0 || ly >= h_u32 as i32 {
                    false
                } else if pl.tiled {
                    let layer = &self.layers[pl.index];
                    let (c0, c1) = (
                        (lx0.max(0) as u32) / TILE,
                        ((lx1 - 1).max(0) as u32).min(w_u32 - 1) / TILE,
                    );
                    let ty = ly as u32 / TILE;
                    (c0..=c1).any(|tx| {
                        layer.pixels.tile(tx, ty).is_some()
                            || layer
                                .mask
                                .as_ref()
                                .is_some_and(|m| m.tile(tx, ty).is_some())
                    })
                } else {
                    true
                };
            }
            if !active.iter().any(|&a| a) {
                fill_row(out, row);
                continue;
            }
            for col in col0..=col1 {
                let x = x0 + col;
                let (mut r, mut g, mut b) = (br, bg, bb);
                // Opaque content composited so far (layers only, not the
                // page): clipping masks intersect against it.
                let mut below_a = 0.0f32;
                for (i, pl) in live.iter().enumerate() {
                    if !active[i] {
                        continue;
                    }
                    let lx = x as i32 - pl.ox;
                    let ly = y as i32 - pl.oy;
                    if lx < 0 || ly < 0 || lx >= self.w as i32 || ly >= self.h as i32 {
                        continue;
                    }
                    let (lx, ly) = (lx as u32, ly as u32);
                    let layer = &self.layers[pl.index];
                    let (px, mask) = if pl.tiled {
                        let Some(tile) = layer.pixels.tile(lx / TILE, ly / TILE) else {
                            continue;
                        };
                        let (tix, tiy) = (lx % TILE, ly % TILE);
                        let off = ((tiy * TILE + tix) * 4) as usize;
                        // The mask shares the layer's geometry, so its
                        // tile-local byte is the reference's dense index.
                        let mask = layer.mask.as_ref().map(|m| {
                            m.tile(lx / TILE, ly / TILE)
                                .map_or(0, |t| t[(tiy * TILE + tix) as usize])
                        });
                        (
                            [tile[off], tile[off + 1], tile[off + 2], tile[off + 3]],
                            mask,
                        )
                    } else {
                        // Foreign geometry: exactly the reference read.
                        let o = ((ly as usize) * self.w as usize + lx as usize) * 4;
                        if o + 4 > layer.pixels.len() {
                            continue;
                        }
                        (
                            layer.pixels.get4(o),
                            layer.mask.as_ref().map(|m| m.get(o / 4)),
                        )
                    };
                    let mut a = px[3] as f32 / 255.0 * pl.opacity * pl.group;
                    if let Some(mv) = mask {
                        a *= mv as f32 / 255.0;
                    }
                    if pl.clip {
                        a *= below_a;
                    }
                    if a <= 0.0 {
                        continue;
                    }
                    r = pl.blend.apply(px[0] as f32, r) * a + r * (1.0 - a);
                    g = pl.blend.apply(px[1] as f32, g) * a + g * (1.0 - a);
                    b = pl.blend.apply(px[2] as f32, b) * a + b * (1.0 - a);
                    below_a = a + below_a * (1.0 - a);
                }
                let o = ((row * TILE + col) * 4) as usize;
                out[o] = r.round().clamp(0.0, 255.0) as u8;
                out[o + 1] = g.round().clamp(0.0, 255.0) as u8;
                out[o + 2] = b.round().clamp(0.0, 255.0) as u8;
                out[o + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::document::{BlendMode, Document};
    use crate::raster::{Raster, TILE};

    /// Opaque content plus a fractional multiply wash, an offset masked
    /// layer with fractional opacity, and a two-layer clip chain.
    fn doc_with_everything() -> Document {
        let mut doc = Document::new(300, 200);
        doc.background = [250, 240, 230, 255];
        let w = doc.w;
        doc.layers[0].pixels.fill(0);
        for y in 0..120u32 {
            for x in 0..180u32 {
                doc.layers[0]
                    .pixels
                    .set4(((y * w + x) * 4) as usize, [10, 60, 200, 255]);
            }
        }
        doc.add_layer("mult".into());
        doc.layers[1].blend = BlendMode::Multiply;
        for y in 0..200u32 {
            for x in 0..300u32 {
                if (x + y) % 3 == 0 {
                    doc.layers[1]
                        .pixels
                        .set4(((y * w + x) * 4) as usize, [255, 40, 40, 180]);
                }
            }
        }
        doc.add_layer("masked".into());
        doc.layers[2].ox = 25;
        doc.layers[2].oy = -10;
        doc.layers[2].opacity = 0.73;
        for y in 0..200u32 {
            for x in 0..300u32 {
                let v = ((x * 11 + y * 7) % 256) as u8;
                doc.layers[2]
                    .pixels
                    .set4(((y * w + x) * 4) as usize, [0, v, 255 - v, 255]);
            }
        }
        let mask = doc.layers[2]
            .mask
            .get_or_insert_with(|| Raster::mask(w, doc.h));
        for y in 0..200u32 {
            for x in 0..300u32 {
                mask.set((y * w + x) as usize, ((x * 7 + y * 13) % 256) as u8);
            }
        }
        doc.add_layer("clipped".into());
        doc.layers[3].clip = true;
        for y in 0..200u32 {
            for x in 0..300u32 {
                doc.layers[3]
                    .pixels
                    .set4(((y * w + x) * 4) as usize, [40, 0, 90, 200]);
            }
        }
        doc.add_layer("clipped2".into());
        doc.layers[4].clip = true;
        doc.layers[4].opacity = 0.55;
        doc.layers[4].ox = -7;
        doc.layers[4].oy = 3;
        for y in 0..200u32 {
            for x in 0..300u32 {
                if x % 2 == 0 {
                    doc.layers[4]
                        .pixels
                        .set4(((y * w + x) * 4) as usize, [255, 255, 0, 255]);
                }
            }
        }
        doc
    }

    /// Nested groups with non-power-of-two opacities plus a hidden group.
    fn doc_with_groups() -> Document {
        let mut doc = Document::new(90, 70);
        doc.background = [5, 15, 25, 255];
        let w = doc.w;
        for y in 0..70u32 {
            for x in 0..90u32 {
                // Fractional, holey base: `below_a` stays below 1, so the
                // clipped group member below is a real intersection and not
                // an invisible `a * 1.0`.
                let a = if (x * y) % 7 == 0 { 0 } else { 140 };
                doc.layers[0].pixels.set4(
                    ((y * w + x) * 4) as usize,
                    [(x * 2) as u8, (y * 3) as u8, (x + y) as u8, a],
                );
            }
        }
        doc.add_layer("soft".into());
        doc.layers[1].opacity = 0.37;
        doc.layers[1].blend = BlendMode::Screen;
        for y in 0..70u32 {
            for x in 0..90u32 {
                let a = ((x * 11 + y * 5) % 256) as u8;
                doc.layers[1]
                    .pixels
                    .set4(((y * w + x) * 4) as usize, [200, 30, 90, a]);
            }
        }
        let outer = doc.new_group("outer".into());
        doc.move_layer_into_group(1, Some(outer));
        doc.set_group_opacity(outer, 0.5);
        let inner = doc.new_group("inner".into());
        doc.groups
            .iter_mut()
            .find(|g| g.id == inner)
            .unwrap()
            .parent = Some(outer);
        doc.set_group_opacity(inner, 0.3);
        doc.add_layer("deep".into());
        doc.layers[2].opacity = 0.83;
        doc.layers[2].blend = BlendMode::HardLight;
        for y in 0..70u32 {
            for x in 0..90u32 {
                if (x + y) % 5 != 0 {
                    doc.layers[2]
                        .pixels
                        .set4(((y * w + x) * 4) as usize, [250, 250, 10, 255]);
                }
            }
        }
        doc.move_layer_into_group(2, Some(inner));
        // A hidden group member contributes nothing.
        let hidden = doc.new_group("hidden".into());
        doc.add_layer("concealed".into());
        let lidx = doc.layers.len() - 1;
        doc.layers[lidx].pixels.fill(255);
        doc.move_layer_into_group(lidx, Some(hidden));
        doc.set_group_visible(hidden, false);
        // Clipping *inside* a group: the clip flag and the stacked group
        // opacities meet in one alpha, which the plan must reproduce.
        doc.add_layer("clipped-in-group".into());
        let clip_idx = doc.layers.len() - 1;
        doc.layers[clip_idx].clip = true;
        doc.layers[clip_idx].opacity = 0.66;
        doc.layers[clip_idx].blend = BlendMode::Multiply;
        doc.layers[clip_idx].ox = -5;
        doc.layers[clip_idx].oy = 4;
        for y in 0..70u32 {
            for x in 0..90u32 {
                let a = ((x * 13 + y * 9) % 256) as u8;
                doc.layers[clip_idx]
                    .pixels
                    .set4(((y * w + x) * 4) as usize, [90, 220, 120, a]);
            }
        }
        doc.move_layer_into_group(clip_idx, Some(outer));
        doc
    }

    /// Mask and clip on the *same* layer: the mask scales `a` first, then
    /// the clip multiplies it by the accumulated `below_a`. Both clipped
    /// layers carry non-trivial opacities and offsets.
    fn doc_with_mask_and_clip() -> Document {
        let mut doc = Document::new(120, 90);
        doc.background = [240, 235, 225, 255];
        let w = doc.w;
        // Fractional base alpha: clipping is a real intersection, not 0/1.
        for y in 0..90u32 {
            for x in 0..120u32 {
                doc.layers[0].pixels.set4(
                    ((y * w + x) * 4) as usize,
                    [(x * 2) as u8, (y * 2) as u8, 170, ((x + 2 * y) % 200) as u8],
                );
            }
        }
        doc.add_layer("masked-clip-a".into());
        doc.layers[1].clip = true;
        doc.layers[1].opacity = 0.62;
        doc.layers[1].ox = -9;
        doc.layers[1].oy = 5;
        for y in 0..90u32 {
            for x in 0..120u32 {
                doc.layers[1].pixels.set4(
                    ((y * w + x) * 4) as usize,
                    [250, 40, 70, ((x * 7 + y * 3) % 256) as u8],
                );
            }
        }
        let mask = doc.layers[1]
            .mask
            .get_or_insert_with(|| Raster::mask(w, doc.h));
        for y in 0..90u32 {
            for x in 0..120u32 {
                // Hard 0 and 255 bands plus a mid band: conceal, reveal,
                // and a fractional factor all feed the clipped alpha.
                let v = match (x / 13) % 3 {
                    0 => 0,
                    1 => 255,
                    _ => ((x * 5 + y * 9) % 256) as u8,
                };
                mask.set((y * w + x) as usize, v);
            }
        }
        doc.add_layer("masked-clip-b".into());
        doc.layers[2].clip = true;
        doc.layers[2].opacity = 0.43;
        doc.layers[2].ox = 6;
        doc.layers[2].oy = -4;
        for y in 0..90u32 {
            for x in 0..120u32 {
                doc.layers[2].pixels.set4(
                    ((y * w + x) * 4) as usize,
                    [10, 200, 240, ((x * 3 + y * 5) % 256) as u8],
                );
            }
        }
        let mask = doc.layers[2]
            .mask
            .get_or_insert_with(|| Raster::mask(w, doc.h));
        for y in 0..90u32 {
            for x in 0..120u32 {
                mask.set((y * w + x) as usize, ((y * 11 + x * 3) % 256) as u8);
            }
        }
        doc
    }

    /// 513x300: three tile columns and two tile rows, so tiles are partial in
    /// both axes and tile row 1 starts at `y = 256`.
    fn doc_wide_513_300() -> Document {
        let mut doc = Document::new(513, 300);
        doc.background = [0, 0, 0, 255];
        let w = doc.w;
        for y in 0..300u32 {
            for x in 0..513u32 {
                if (x / 17 + y / 11) % 4 == 0 {
                    doc.layers[0]
                        .pixels
                        .set4(((y * w + x) * 4) as usize, [x as u8, y as u8, 77, 255]);
                }
            }
        }
        doc.add_layer("top".into());
        doc.layers[1].opacity = 0.4;
        doc.layers[1].ox = -200;
        doc.layers[1].oy = 100;
        for y in 0..300u32 {
            for x in 0..513u32 {
                doc.layers[1]
                    .pixels
                    .set4(((y * w + x) * 4) as usize, [9, 200, 31, 255]);
            }
        }
        doc
    }

    fn assert_tile_parity(doc: &Document) {
        let plan = doc.stack_plan();
        for ty in 0..doc.tiles_y() {
            for tx in 0..doc.tiles_x() {
                let tile = doc.composite_tile(&plan, tx, ty);
                assert_eq!(
                    tile.len(),
                    (TILE * TILE * 4) as usize,
                    "tile {tx},{ty} size"
                );
                for row in 0..TILE {
                    for col in 0..TILE {
                        let x = tx * TILE + col;
                        let y = ty * TILE + row;
                        let o = ((row * TILE + col) * 4) as usize;
                        let want = if x < doc.w && y < doc.h {
                            doc.composite_pixel(x, y)
                        } else {
                            [0, 0, 0, 0]
                        };
                        assert_eq!(&tile[o..o + 4], &want, "tile {tx},{ty} pixel {x},{y}");
                    }
                }
            }
        }
    }

    fn assert_flatten_parity(doc: &Document) {
        let out = doc.flatten();
        assert_eq!(out.len(), (doc.w * doc.h * 4) as usize);
        for y in 0..doc.h {
            for x in 0..doc.w {
                let o = ((y * doc.w + x) * 4) as usize;
                assert_eq!(&out[o..o + 4], &doc.composite_pixel(x, y), "pixel {x},{y}");
            }
        }
    }

    fn region_reference(doc: &Document, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<u8> {
        let x1 = x1.min(doc.w.saturating_sub(1));
        let y1 = y1.min(doc.h.saturating_sub(1));
        let (x0, y0) = (x0.min(x1), y0.min(y1));
        let mut out = Vec::with_capacity(((x1 - x0 + 1) * (y1 - y0 + 1) * 4) as usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.extend_from_slice(&doc.composite_pixel(x, y));
            }
        }
        out
    }

    #[test]
    fn tile_composite_matches_per_pixel_reference() {
        assert_tile_parity(&doc_with_everything());
        assert_tile_parity(&doc_with_groups());
    }

    /// A cached tile patched with only the edited rect must equal a full
    /// recomposite, and must not touch bytes outside the rect.
    #[test]
    fn sub_rect_patch_matches_a_full_recomposite() {
        let mut doc = doc_with_everything();
        let w = doc.w;
        let plan = doc.stack_plan();
        let mut cached: Vec<Vec<u8>> = Vec::new();
        for ty in 0..doc.tiles_y() {
            for tx in 0..doc.tiles_x() {
                cached.push(doc.composite_tile(&plan, tx, ty));
            }
        }
        // A stroke-sized edit: opaque green band straddling the tile-0/1 seam.
        let rect = (230, 90, 270, 120);
        for y in rect.1..=rect.3 {
            for x in rect.0..=rect.2 {
                doc.layers[0]
                    .pixels
                    .set4(((y * w + x) * 4) as usize, [0, 255, 0, 255]);
            }
        }
        let plan = doc.stack_plan();
        let mut i = 0;
        for ty in 0..doc.tiles_y() {
            for tx in 0..doc.tiles_x() {
                let before = cached[i].clone();
                let mut tile = cached[i].clone();
                doc.composite_tile_into(&plan, tx, ty, Some(rect), &mut tile);
                let full = doc.composite_tile(&plan, tx, ty);
                let (x0, y0) = (tx * TILE, ty * TILE);
                for row in 0..TILE {
                    for col in 0..TILE {
                        let o = ((row * TILE + col) * 4) as usize;
                        let (x, y) = (x0 + col, y0 + row);
                        let in_rect = x >= rect.0 && x <= rect.2 && y >= rect.1 && y <= rect.3;
                        if in_rect && x < doc.w && y < doc.h {
                            assert_eq!(&tile[o..o + 4], &full[o..o + 4], "patched {x},{y}");
                        } else {
                            assert_eq!(
                                &tile[o..o + 4],
                                &before[o..o + 4],
                                "outside the rect at {x},{y} was rewritten"
                            );
                        }
                    }
                }
                i += 1;
            }
        }
    }

    /// A rect that misses a tile leaves it byte-identical.
    #[test]
    fn patch_rect_outside_a_tile_is_a_no_op() {
        let doc = doc_with_everything();
        let plan = doc.stack_plan();
        let mut tile = vec![0xABu8; (TILE * TILE * 4) as usize];
        let before = tile.clone();
        doc.composite_tile_into(&plan, 1, 0, Some((0, 0, 100, 100)), &mut tile);
        assert_eq!(tile, before, "an untouched tile was rewritten");
    }

    #[test]
    fn tile_outside_the_canvas_is_transparent() {
        let doc = doc_with_everything();
        let plan = doc.stack_plan();
        for (tx, ty) in [(doc.tiles_x(), 0), (0, doc.tiles_y()), (9, 9)] {
            let tile = doc.composite_tile(&plan, tx, ty);
            assert_eq!(tile.len(), (TILE * TILE * 4) as usize);
            assert!(tile.iter().all(|&b| b == 0), "tile {tx},{ty} not empty");
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stack plan is stale")]
    fn stale_plan_trips_the_debug_assert() {
        // A structural edit after the snapshot invalidates the plan; using
        // it anyway would blend the wrong layer inputs.
        let mut doc = doc_with_everything();
        let plan = doc.stack_plan();
        doc.add_layer("late".into());
        let _ = doc.composite_tile(&plan, 0, 0);
    }

    #[test]
    fn edge_tiles_cover_canvas_edges() {
        // 513x300 spans three tile columns and two tile rows, so every
        // edge tile is partial in one or both axes.
        let doc = doc_wide_513_300();
        assert_tile_parity(&doc);
        assert_flatten_parity(&doc);
        // A 1x1 canvas and an exact multiple of the tile side.
        assert_tile_parity(&Document::new(1, 1));
        let mut square = Document::new(256, 256);
        square.layers[0].pixels.fill(9);
        assert_tile_parity(&square);
    }

    #[test]
    fn every_blend_mode_matches_reference() {
        for blend in BlendMode::ALL {
            let mut doc = Document::new(70, 50);
            doc.background = [12, 34, 56, 255];
            let w = doc.w;
            for y in 0..50u32 {
                for x in 0..70u32 {
                    doc.layers[0].pixels.set4(
                        ((y * w + x) * 4) as usize,
                        [(x * 3) as u8, (y * 4) as u8, 180, 255],
                    );
                }
            }
            doc.add_layer("blend".into());
            doc.layers[1].blend = blend;
            doc.layers[1].opacity = 0.61;
            for y in 0..50u32 {
                for x in 0..70u32 {
                    doc.layers[1].pixels.set4(
                        ((y * w + x) * 4) as usize,
                        [250, 20, 130, ((x * 5 + y * 3) % 256) as u8],
                    );
                }
            }
            doc.add_layer("blend2".into());
            doc.layers[2].blend = blend;
            doc.layers[2].opacity = 0.83;
            doc.layers[2].ox = 3;
            doc.layers[2].oy = -2;
            doc.layers[2].pixels.fill(255);
            assert_tile_parity(&doc);
        }
    }

    #[test]
    fn masks_and_clip_chains_match_reference() {
        let mut doc = Document::new(80, 60);
        doc.background = [200, 210, 220, 255];
        let w = doc.w;
        // Fractional base alpha: clipping is a real intersection, not 0/1.
        for y in 0..60u32 {
            for x in 0..80u32 {
                doc.layers[0]
                    .pixels
                    .set4(((y * w + x) * 4) as usize, [255, 0, 0, 160]);
            }
        }
        doc.add_layer("clip-a".into());
        doc.layers[1].clip = true;
        doc.layers[1].blend = BlendMode::Overlay;
        for y in 0..60u32 {
            for x in 0..80u32 {
                doc.layers[1].pixels.set4(
                    ((y * w + x) * 4) as usize,
                    [0, 255, 0, ((x * 9 + y * 17) % 256) as u8],
                );
            }
        }
        doc.add_layer("clip-b".into());
        doc.layers[2].clip = true;
        doc.layers[2].opacity = 0.42;
        doc.layers[2].ox = -11;
        doc.layers[2].oy = 6;
        doc.layers[2].pixels.fill(128);
        doc.add_layer("mask-third".into());
        doc.layers[3].pixels.fill(200);
        doc.layers[3].opacity = 0.9;
        let mask = doc.layers[3]
            .mask
            .get_or_insert_with(|| Raster::mask(w, doc.h));
        for y in 0..60u32 {
            for x in 0..80u32 {
                // Hard 0 and 255 bands plus a mid band.
                let v = match (x / 10) % 3 {
                    0 => 0,
                    1 => 255,
                    _ => (x * 3 + y) as u8,
                };
                mask.set((y * w + x) as usize, v);
            }
        }
        assert_tile_parity(&doc);
        assert_flatten_parity(&doc);
    }

    #[test]
    fn mask_and_clip_on_the_same_layer_match_reference() {
        // The contract calls out mask and clip meeting on one layer: the
        // mask factor and the clip intersection must compose in the same
        // order (and with the same rounding) as the reference.
        let doc = doc_with_mask_and_clip();
        assert_tile_parity(&doc);
        assert_flatten_parity(&doc);
        // Non-vacuity: dropping either factor must move pixels, so a future
        // fixture edit cannot leave the combination silently untested.
        let mut no_mask = doc.clone();
        no_mask.layers[1].mask = None;
        no_mask.layers[2].mask = None;
        let mut no_clip = doc.clone();
        no_clip.layers[1].clip = false;
        no_clip.layers[2].clip = false;
        assert_ne!(doc.flatten(), no_mask.flatten(), "mask has no effect");
        assert_ne!(doc.flatten(), no_clip.flatten(), "clip has no effect");
    }

    #[test]
    fn clip_inside_a_group_changes_the_composite() {
        // Guards the group fixture: an opaque base pins `below_a` at 1 and
        // would turn the clipped group member into a silent no-op, leaving
        // clip-in-group untested even though parity still passes.
        let grouped = doc_with_groups();
        let mut unclipped = grouped.clone();
        for layer in unclipped.layers.iter_mut() {
            layer.clip = false;
        }
        assert_ne!(grouped.flatten(), unclipped.flatten(), "clip is a no-op");
    }

    #[test]
    fn foreign_raster_geometry_matches_reference() {
        // File readers reject these, but the type allows them: the plan must
        // fall back to dense reads and stay byte-identical.
        let mut doc = Document::new(2, 2);
        doc.layers[0].pixels = Raster::rgba(4, 1);
        doc.layers[0].pixels.fill(255);
        assert_tile_parity(&doc);
        let mut doc = Document::new(2, 2);
        doc.layers[0].pixels.fill(90);
        doc.layers[0].mask = Some(Raster::mask(4, 1));
        doc.layers[0].mask.as_mut().unwrap().fill(128);
        assert_tile_parity(&doc);
    }

    #[test]
    fn flatten_uses_tiles_and_matches_pixel_reference() {
        assert_flatten_parity(&doc_with_everything());
        assert_flatten_parity(&doc_with_groups());
    }

    #[test]
    fn flatten_region_matches_reference_in_canvas_order() {
        for doc in [
            doc_with_everything(),
            doc_with_groups(),
            doc_with_mask_and_clip(),
        ] {
            for (x0, y0, x1, y1) in [
                (0, 0, 0, 0),
                (5, 7, 5, 7),
                (250, 0, 270, 5),
                (255, 255, 257, 257),
                (0, 0, 10_000, 10_000),
                (10, 10, 5, 5),
            ] {
                assert_eq!(
                    doc.flatten_region(x0, y0, x1, y1),
                    region_reference(&doc, x0, y0, x1, y1),
                    "region {x0},{y0}..{x1},{y1} on {}x{}",
                    doc.w,
                    doc.h
                );
            }
        }
    }

    #[test]
    fn flatten_region_crosses_a_tile_row() {
        // On the small fixtures every region clamps below y = 200, so the
        // outer `ty` loop in `flatten_region` only ever ran once. Straddle
        // the y = 256 tile-row boundary on the 513x300 canvas.
        let doc = doc_wide_513_300();
        for (x0, y0, x1, y1) in [
            (500, 250, 520, 260),
            (250, 255, 260, 257),
            (0, 250, 600, 262),
        ] {
            let got = doc.flatten_region(x0, y0, x1, y1);
            let want = region_reference(&doc, x0, y0, x1, y1);
            assert_eq!(got.len(), want.len(), "region {x0},{y0}..{x1},{y1} size");
            assert_eq!(got, want, "region {x0},{y0}..{x1},{y1}");
        }
    }
}
