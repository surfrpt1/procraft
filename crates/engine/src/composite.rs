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
        StackPlan { layers }
    }

    /// One tile of the composite, `TILE * TILE * 4` bytes, row-major.
    /// Pixels outside the canvas are zero.
    pub fn composite_tile(&self, plan: &StackPlan, tx: u32, ty: u32) -> Vec<u8> {
        let mut out = vec![0u8; (TILE * TILE * 4) as usize];
        let x0 = tx.saturating_mul(TILE);
        let y0 = ty.saturating_mul(TILE);
        let cw = self.w.saturating_sub(x0).min(TILE);
        let ch = self.h.saturating_sub(y0).min(TILE);
        if cw == 0 || ch == 0 {
            return out;
        }
        let (br, bg, bb) = (
            self.background[0] as f32,
            self.background[1] as f32,
            self.background[2] as f32,
        );
        for row in 0..ch {
            let y = y0 + row;
            for col in 0..cw {
                let x = x0 + col;
                let (mut r, mut g, mut b) = (br, bg, bb);
                // Opaque content composited so far (layers only, not the
                // page): clipping masks intersect against it.
                let mut below_a = 0.0f32;
                for pl in &plan.layers {
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
                        let (tx, ty) = (lx % TILE, ly % TILE);
                        let off = ((ty * TILE + tx) * 4) as usize;
                        // The mask shares the layer's geometry, so its
                        // tile-local byte is the reference's dense index.
                        let mask = layer.mask.as_ref().map(|m| {
                            m.tile(lx / TILE, ly / TILE)
                                .map_or(0, |t| t[(ty * TILE + tx) as usize])
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
        out
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
                doc.layers[0].pixels.set4(
                    ((y * w + x) * 4) as usize,
                    [(x * 2) as u8, (y * 3) as u8, (x + y) as u8, 255],
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
    fn edge_tiles_cover_canvas_edges() {
        // 513x300 spans three tile columns and two tile rows, so every
        // edge tile is partial in one or both axes.
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
        for doc in [doc_with_everything(), doc_with_groups()] {
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
}
