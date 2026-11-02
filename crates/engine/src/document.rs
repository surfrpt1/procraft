//! Layers, documents, compositing and undo.

use crate::brush::{slot_span, slot_x};
use crate::raster::{try_copy, try_vec, try_zeroed, Raster, TILE};
use liquify::LiquifyStroke;

/// How a layer's pixels combine with what's beneath.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    Difference,
    Exclusion,
    SoftLight,
    HardLight,
}

impl BlendMode {
    pub const ALL: [BlendMode; 10] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::SoftLight,
        BlendMode::HardLight,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::SoftLight => "Soft light",
            BlendMode::HardLight => "Hard light",
        }
    }

    pub(crate) fn apply(self, src: f32, dst: f32) -> f32 {
        match self {
            BlendMode::Normal => src,
            BlendMode::Multiply => src * dst / 255.0,
            BlendMode::Screen => 255.0 - (255.0 - src) * (255.0 - dst) / 255.0,
            BlendMode::Overlay => {
                if dst < 128.0 {
                    2.0 * src * dst / 255.0
                } else {
                    255.0 - 2.0 * (255.0 - src) * (255.0 - dst) / 255.0
                }
            }
            BlendMode::Darken => src.min(dst),
            BlendMode::Lighten => src.max(dst),
            BlendMode::Difference => (src - dst).abs(),
            BlendMode::Exclusion => src + dst - 2.0 * src * dst / 255.0,
            BlendMode::SoftLight => {
                // Pegtop approximation: gentle contrast around mid-gray.
                let (s, d) = (src / 255.0, dst / 255.0);
                let out = if s <= 0.5 {
                    d - (1.0 - 2.0 * s) * d * (1.0 - d)
                } else {
                    d + (2.0 * s - 1.0) * (d.sqrt() - d)
                };
                out.clamp(0.0, 1.0) * 255.0
            }
            BlendMode::HardLight => {
                // Overlay with source and destination swapped.
                if src < 128.0 {
                    2.0 * src * dst / 255.0
                } else {
                    255.0 - 2.0 * (255.0 - src) * (255.0 - dst) / 255.0
                }
            }
        }
    }
}

/// Mirror painting mode: strokes are stamped on both sides of the canvas
/// center line(s). Horizontal mirrors left-right, Vertical mirrors top-bottom,
/// Both mirrors on both axes, Kaleidoscope reflects around N axes through
/// the canvas center (Segments slider, 2..=16).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SymmetryMode {
    #[default]
    Off,
    Horizontal,
    Vertical,
    Both,
    Kaleidoscope,
}

impl SymmetryMode {
    pub const ALL: [SymmetryMode; 5] = [
        SymmetryMode::Off,
        SymmetryMode::Horizontal,
        SymmetryMode::Vertical,
        SymmetryMode::Both,
        SymmetryMode::Kaleidoscope,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SymmetryMode::Off => "Off",
            SymmetryMode::Horizontal => "Horizontal",
            SymmetryMode::Vertical => "Vertical",
            SymmetryMode::Both => "Both",
            SymmetryMode::Kaleidoscope => "Kaleidoscope",
        }
    }

    pub(crate) fn mirrored_points(
        self,
        segments: u8,
        cx: f32,
        cy: f32,
        x: f32,
        y: f32,
    ) -> Vec<(f32, f32)> {
        let mx = 2.0 * cx - x;
        let my = 2.0 * cy - y;
        match self {
            SymmetryMode::Off => vec![(x, y)],
            SymmetryMode::Horizontal => vec![(x, y), (mx, y)],
            SymmetryMode::Vertical => vec![(x, y), (x, my)],
            SymmetryMode::Both => vec![(x, y), (mx, y), (x, my), (mx, my)],
            SymmetryMode::Kaleidoscope => {
                // Dihedral symmetry: rotate by k*step, mirroring on odd k.
                let n = segments.clamp(2, 16) as usize;
                let (dx, dy) = (x - cx, y - cy);
                let step = std::f32::consts::TAU / n as f32;
                (0..n)
                    .map(|k| {
                        let (px, mut py) = (dx, dy);
                        if k % 2 == 1 {
                            py = -py;
                        }
                        let (s, c) = (k as f32 * step).sin_cos();
                        (cx + px * c - py * s, cy + px * s + py * c)
                    })
                    .collect()
            }
        }
    }

    /// Like `mirrored_points`, but carries a push vector (vx, vy) along:
    /// mirrors negate the mirrored component, kaleidoscope rotates both.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn mirrored_pushes(
        self,
        segments: u8,
        cx: f32,
        cy: f32,
        x: f32,
        y: f32,
        vx: f32,
        vy: f32,
    ) -> Vec<(f32, f32, f32, f32)> {
        let (mx, mvx) = (2.0 * cx - x, -vx);
        let (my, mvy) = (2.0 * cy - y, -vy);
        match self {
            SymmetryMode::Off => vec![(x, y, vx, vy)],
            SymmetryMode::Horizontal => vec![(x, y, vx, vy), (mx, y, mvx, vy)],
            SymmetryMode::Vertical => vec![(x, y, vx, vy), (x, my, vx, mvy)],
            SymmetryMode::Both => vec![
                (x, y, vx, vy),
                (mx, y, mvx, vy),
                (x, my, vx, mvy),
                (mx, my, mvx, mvy),
            ],
            SymmetryMode::Kaleidoscope => {
                // Dihedral symmetry: rotate by k*step, mirroring on odd k.
                let n = segments.clamp(2, 16) as usize;
                let (dx, dy) = (x - cx, y - cy);
                let step = std::f32::consts::TAU / n as f32;
                (0..n)
                    .map(|k| {
                        let (px, mut py) = (dx, dy);
                        let (ux, mut uy) = (vx, vy);
                        if k % 2 == 1 {
                            py = -py;
                            uy = -uy;
                        }
                        let (s, c) = (k as f32 * step).sin_cos();
                        (
                            cx + px * c - py * s,
                            cy + px * s + py * c,
                            ux * c - uy * s,
                            ux * s + uy * c,
                        )
                    })
                    .collect()
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn mirrored_pixels(
        self,
        w: u32,
        h: u32,
        segments: u8,
        cx: f32,
        cy: f32,
        px: u32,
        py: u32,
    ) -> Vec<(u32, u32)> {
        // Mirror across the center, clamped into the canvas.
        let mirror = |v: f32, max: u32| {
            if max == 0 {
                0
            } else {
                v.round().clamp(0.0, max as f32 - 1.0) as u32
            }
        };
        match self {
            SymmetryMode::Kaleidoscope => {
                let n = segments.clamp(2, 16) as usize;
                let (dx, dy) = (px as f32 - cx, py as f32 - cy);
                let step = std::f32::consts::TAU / n as f32;
                (0..n)
                    .map(|k| {
                        let (qx, mut qy) = (dx, dy);
                        if k % 2 == 1 {
                            qy = -qy;
                        }
                        let (s, c) = (k as f32 * step).sin_cos();
                        let fx = cx + qx * c - qy * s;
                        let fy = cy + qx * s + qy * c;
                        (
                            fx.round().clamp(0.0, w as f32 - 1.0) as u32,
                            fy.round().clamp(0.0, h as f32 - 1.0) as u32,
                        )
                    })
                    .collect()
            }
            _ => {
                let mx = mirror(2.0 * cx - px as f32, w);
                let my = mirror(2.0 * cy - py as f32, h);
                match self {
                    SymmetryMode::Off => vec![(px, py)],
                    SymmetryMode::Horizontal => vec![(px, py), (mx, py)],
                    SymmetryMode::Vertical => vec![(px, py), (px, my)],
                    SymmetryMode::Both => vec![(px, py), (mx, py), (px, my), (mx, my)],
                    SymmetryMode::Kaleidoscope => unreachable!(),
                }
            }
        }
    }
}

/// One baked layer: canvas-sized RGBA bytes plus an optional reveal mask.
pub type BakedLayer = (Vec<u8>, Option<Vec<u8>>);

/// One raster layer: row-major RGBA bytes plus presentation state.
#[derive(Clone, Debug)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    /// 0.0 (transparent) ..= 1.0 (opaque).
    pub opacity: f32,
    pub blend: BlendMode,
    /// Tiled RGBA pixels: dense-index reads/writes, allocated per 256² tile.
    pub pixels: Raster,
    /// Optional grayscale reveal mask (tiled, 255 = fully shown).
    /// Multiplies the layer's alpha at composite time. Session-painted;
    /// saved to v5 project files.
    pub mask: Option<Raster>,
    /// Clipping mask: confine this layer to the opaque content beneath it.
    pub clip: bool,
    /// Canvas offset of the buffer's top-left in pixels. The Move tool only
    /// ever changes these (lossless, O(1)): content pushed off-canvas stays
    /// in the buffer and returns when moved back. Paint paths map canvas
    /// coordinates through the offset; compositing, merge and export bake
    /// it. Saved to v6 project files (older versions read 0).
    pub ox: i32,
    pub oy: i32,
    /// Frame hold in animation ticks (M32): how many `1/fps` steps this
    /// frame shows during playback and GIF export. Saved to v10 project
    /// files (older files read 1); clamped to 1..=8 at every use.
    pub hold: u32,
    /// Owning layer group (id into [`Document::groups`); `None` lives at
    /// the top level. Saved to v8 project files (older versions read none).
    pub group: Option<u32>,
    /// Vector text source for text layers: `Some` makes this layer editable
    /// as text, with `pixels` always holding its rendered result. Saved to
    /// v11 project files (older versions read none).
    pub text: Option<crate::text::TextBlock>,
}

/// A named layer container: visibility (and collapse) apply to the whole
/// subtree. Ids are monotonic per document and never reused while a group
/// with that id exists; removal leaves gaps, which open/ops tolerate.
#[derive(Clone, Debug)]
pub struct LayerGroup {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    pub collapsed: bool,
    /// Group opacity 0.0..=1.0: multiplies every member's alpha at
    /// composite time (pass-through semantics — exact for opaque and
    /// non-overlapping content). Saved to v9 project files (v8 files
    /// read 1.0); structural like visibility, not undoable.
    pub opacity: f32,
    /// Parent group id for nesting; `None` lives at the top level.
    pub parent: Option<u32>,
}

impl Layer {
    pub fn blank(name: String, w: u32, h: u32) -> Self {
        Self {
            name,
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            pixels: Raster::rgba(w, h),
            mask: None,
            clip: false,
            ox: 0,
            oy: 0,
            hold: 1,
            group: None,
            text: None,
        }
    }

    /// Map a canvas pixel to this layer's buffer index, if it holds content
    /// there. `w`/`h` are the (shared) buffer dimensions.
    fn local_offset(&self, px: u32, py: u32, w: u32, h: u32) -> Option<usize> {
        let lx = px as i32 - self.ox;
        let ly = py as i32 - self.oy;
        if lx < 0 || ly < 0 || lx >= w as i32 || ly >= h as i32 {
            return None;
        }
        Some(((ly as u32 * w + lx as u32) * 4) as usize)
    }

    pub fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// Is this layer an editable text layer? Its rendered `pixels` are the
    /// text; only [`Document::rerender_text_layer`] rewrites them.
    pub fn is_text(&self) -> bool {
        self.text.is_some()
    }
}

/// Freehand painting target: layer color or its reveal mask. On masks the
/// brush color's luminance is the mask value (dark conceals, white reveals);
/// the eraser always reveals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaintTarget {
    #[default]
    Color,
    Mask,
}

impl PaintTarget {
    pub fn name(self) -> &'static str {
        match self {
            PaintTarget::Color => "Color",
            PaintTarget::Mask => "Mask",
        }
    }
}

/// A layered document. Layer 0 is the bottom of the stack.
#[derive(Clone, Debug)]
pub struct Document {
    pub w: u32,
    pub h: u32,
    pub layers: Vec<Layer>,
    pub active: usize,
    /// Backdrop shown under transparent pixels (the page color).
    pub background: [u8; 4],
    /// Optional rectangular selection (inclusive canvas pixels): painting
    /// outside it is ignored.
    pub selection: Option<(u32, u32, u32, u32)>,
    /// Optional freehand lasso mask (row-major, 1 = inside): painting outside
    /// it is ignored. Session-only, never saved to project files.
    pub selection_mask: Option<Vec<u8>>,
    /// Mirror painting mode (not all versions of the file format store it).
    pub symmetry: SymmetryMode,
    /// Kaleidoscope reflection axes, 2..=16 (only used in that mode).
    pub symmetry_segments: u8,
    /// Mirror center in canvas pixels (`None` = canvas center). Draggable
    /// on-canvas like guide vanishing points; saved to v7 project files.
    pub symmetry_center: Option<(f32, f32)>,
    /// Freehand painting target (layer color or its reveal mask).
    pub paint_target: PaintTarget,
    dirty: Option<(u32, u32, u32, u32)>,
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    undo_bytes: usize,
    redo_bytes: usize,
    /// Layer groups (presentation only: paint/masks still address layers).
    /// Saved to v8 project files.
    pub groups: Vec<LayerGroup>,
    next_group_id: u32,
    /// Running pickup color for the smudge stroke (straight RGBA f32s);
    /// reset on every stroke start, never saved.
    pub(crate) smudge_finger: Option<[f32; 4]>,
    /// Content revision, bumped by every dirty mark: lets callers detect
    /// mutations made behind their back (e.g. a transform session holding
    /// a pre-transform pixel base). Session-only, never saved.
    rev: u64,
    /// Per-stroke peak coverage (0..=255 per canvas pixel) for the active
    /// stroke: a pixel keeps the strongest single dab instead of summing
    /// every dab that crosses it, so one stroke stops at the brush opacity
    /// and keeps its soft rim. Session-only, never saved.
    pub(crate) stroke_peak: Option<Raster>,
    /// Canvas bbox touched by the active stroke (inclusive), for cheap
    /// stroke-local restores. Session-only.
    pub(crate) stroke_rect: Option<(u32, u32, u32, u32)>,
    /// Active liquify stroke (snapshot + displacement field). Session-only.
    liquify: Option<LiquifyStroke>,
}

/// History cap: at most 32 steps per stack (backstop)…
const MAX_UNDO_ENTRIES: usize = 32;
/// …and at most 256 MiB per stack. A 4K masked layer is ~40 MiB per step,
/// so the byte budget binds first on big canvases (6 steps) while the count
/// cap binds on small ones (32 steps x ~5 MiB at 1280x800).
const MAX_UNDO_BYTES: usize = 256 * 1024 * 1024;

/// Canvas side cap for crop/resize: bounds reallocations (tiles are
/// allocated on demand, so an 8192² layer that is never painted costs
/// nothing; a painted one is 1024 tiles, 256 MiB) and matches the 8K
/// export target.
pub const MAX_CANVAS_SIDE: u32 = 8192;

#[derive(Clone, Debug)]
struct UndoEntry {
    layer: usize,
    pixels: Raster,
    mask: Option<Raster>,
    ox: i32,
    oy: i32,
    /// Text-layer vector source at snapshot time, so undo/redo restore the
    /// block and its pixels together.
    text: Option<crate::text::TextBlock>,
}

impl UndoEntry {
    fn byte_size(&self) -> usize {
        self.pixels.alloc_bytes()
            + self.mask.as_ref().map_or(0, Raster::alloc_bytes)
            + self.text.as_ref().map_or(0, |t| {
                t.text.len() + t.style.font_bytes.as_ref().map_or(0, Vec::len)
            })
    }
}

impl Document {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            layers: vec![Layer::blank("Layer 1".to_string(), w, h)],
            active: 0,
            background: [255, 255, 255, 255],
            selection: None,
            selection_mask: None,
            symmetry: SymmetryMode::Off,
            symmetry_segments: 6,
            symmetry_center: None,
            paint_target: PaintTarget::Color,
            dirty: None,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_bytes: 0,
            redo_bytes: 0,
            groups: Vec::new(),
            next_group_id: 1,
            smudge_finger: None,
            rev: 0,
            stroke_peak: None,
            stroke_rect: None,
            liquify: None,
        }
    }

    /// Assemble a document from imported layers (white page, given active
    /// layer, no selection or history). For format importers in this crate.
    pub(crate) fn from_layers(w: u32, h: u32, layers: Vec<Layer>, active: usize) -> Self {
        let active = active.min(layers.len().saturating_sub(1));
        Self {
            w,
            h,
            layers,
            active,
            background: [255, 255, 255, 255],
            selection: None,
            selection_mask: None,
            symmetry: SymmetryMode::Off,
            symmetry_segments: 6,
            symmetry_center: None,
            paint_target: PaintTarget::Color,
            dirty: None,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_bytes: 0,
            redo_bytes: 0,
            groups: Vec::new(),
            next_group_id: 1,
            smudge_finger: None,
            rev: 0,
            stroke_peak: None,
            stroke_rect: None,
            liquify: None,
        }
    }
    /// Mirror center in canvas pixels (canvas middle when never moved).
    pub fn symmetry_center_px(&self) -> (f32, f32) {
        self.symmetry_center
            .unwrap_or(((self.w as f32 - 1.0) * 0.5, (self.h as f32 - 1.0) * 0.5))
    }

    /// Move the mirror center (clamped, like layer offsets and VPs).
    pub fn set_symmetry_center(&mut self, x: f32, y: f32) {
        self.symmetry_center = Some((x.clamp(-16384.0, 16384.0), y.clamp(-16384.0, 16384.0)));
    }

    /// Forget a moved mirror center (back to canvas middle).
    pub fn reset_symmetry_center(&mut self) {
        self.symmetry_center = None;
    }

    /// Take and clear the dirty region (inclusive bounds) for partial
    /// texture uploads. `None` means nothing changed: skip the upload.
    pub fn take_dirty(&mut self) -> Option<(u32, u32, u32, u32)> {
        self.dirty.take()
    }

    /// Content revision: bumped by every pixel-changing dirty mark. Callers
    /// snapshot it to detect mutations that happen behind their back.
    pub fn revision(&self) -> u64 {
        self.rev
    }

    /// Tiles held by live layers (not history): what the memory readout shows.
    pub fn layer_alloc_bytes(&self) -> usize {
        self.layers
            .iter()
            .map(|l| l.pixels.alloc_bytes() + l.mask.as_ref().map_or(0, Raster::alloc_bytes))
            .sum()
    }

    pub fn tiles_x(&self) -> u32 {
        self.w.div_ceil(TILE)
    }

    pub fn tiles_y(&self) -> u32 {
        self.h.div_ceil(TILE)
    }

    /// Mark the whole canvas dirty (structural changes, undo, background).
    pub fn mark_full_dirty(&mut self) {
        if self.w > 0 && self.h > 0 {
            self.dirty = Some((0, 0, self.w - 1, self.h - 1));
            self.rev = self.rev.wrapping_add(1);
        }
    }

    pub(crate) fn mark_dirty(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        // Dabs fully outside the canvas invert the rect (x0 > x1): they
        // paint nothing, so record nothing instead of poisoning consumers
        // with an underflowing region.
        if x0 > x1 || y0 > y1 {
            return;
        }
        self.rev = self.rev.wrapping_add(1);
        let r = (
            x0.min(self.w.saturating_sub(1)),
            y0.min(self.h.saturating_sub(1)),
            x1.min(self.w.saturating_sub(1)),
            y1.min(self.h.saturating_sub(1)),
        );
        self.dirty = Some(match self.dirty {
            Some((a, b, c, d)) => (a.min(r.0), b.min(r.1), c.max(r.2), d.max(r.3)),
            None => r,
        });
    }

    pub fn active_layer_mut(&mut self) -> Option<&mut Layer> {
        self.layers.get_mut(self.active)
    }

    /// Reference compositor: one pixel, walking every layer through dense
    /// reads. The tile compositor (`composite_tile`) must match it byte for
    /// byte; tests compare the two.
    pub(crate) fn composite_pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let (mut r, mut g, mut b) = (
            self.background[0] as f32,
            self.background[1] as f32,
            self.background[2] as f32,
        );
        // Opaque content composited so far (layers only, not the page):
        // clipping masks intersect against it.
        let mut below_a = 0.0f32;
        for (li, layer) in self.layers.iter().enumerate() {
            if !self.is_layer_shown(li) {
                continue;
            }
            let gop = self.group_opacity(li);
            if layer.opacity <= 0.0 || gop <= 0.0 {
                continue;
            }
            // Buffer content sits at the layer offset; off-canvas parts of
            // the buffer simply don't cover this pixel.
            let Some(o) = layer.local_offset(x, y, self.w, self.h) else {
                continue;
            };
            if o + 4 > layer.pixels.len() {
                continue;
            }
            let px = layer.pixels.get4(o);
            let mut a = px[3] as f32 / 255.0 * layer.opacity.clamp(0.0, 1.0) * gop;
            if let Some(m) = layer.mask.as_ref() {
                a *= m.get(o / 4) as f32 / 255.0;
            }
            if layer.clip {
                a *= below_a;
            }
            if a <= 0.0 {
                continue;
            }
            let blend = layer.blend;
            r = blend.apply(px[0] as f32, r) * a + r * (1.0 - a);
            g = blend.apply(px[1] as f32, g) * a + g * (1.0 - a);
            b = blend.apply(px[2] as f32, b) * a + b * (1.0 - a);
            below_a = a + below_a * (1.0 - a);
        }
        [
            r.round().clamp(0.0, 255.0) as u8,
            g.round().clamp(0.0, 255.0) as u8,
            b.round().clamp(0.0, 255.0) as u8,
            255,
        ]
    }

    /// Composite bottom-to-top over the background into RGBA bytes.
    /// Walks tiles through the stack plan: one composite per 256² tile,
    /// then tile rows are copied into canvas rows.
    pub fn flatten(&self) -> Vec<u8> {
        let mut out = vec![0u8; (self.w as usize) * (self.h as usize) * 4];
        self.fill_flat(&mut out);
        out
    }

    /// [`Self::flatten`] with checked size and a fallible allocation: 8K
    /// RGBA is ~132 MB, so export paths on a tablet must surface OOM as
    /// an error instead of aborting the process.
    pub fn try_flatten(&self) -> Result<Vec<u8>, String> {
        let len = (self.w as usize)
            .checked_mul(self.h as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| "canvas dimensions overflow".to_string())?;
        let mut out = crate::raster::try_zeroed(len)?;
        self.fill_flat(&mut out);
        Ok(out)
    }

    fn fill_flat(&self, out: &mut [u8]) {
        let plan = self.stack_plan();
        for ty in 0..self.tiles_y() {
            for tx in 0..self.tiles_x() {
                let tile = self.composite_tile(&plan, tx, ty);
                let x0 = (tx * TILE) as usize;
                let y0 = (ty * TILE) as usize;
                let tw = (self.w as usize - x0).min(TILE as usize);
                let th = (self.h as usize - y0).min(TILE as usize);
                for row in 0..th {
                    let src = row * TILE as usize * 4;
                    let dst = ((y0 + row) * self.w as usize + x0) * 4;
                    out[dst..dst + tw * 4].copy_from_slice(&tile[src..src + tw * 4]);
                }
            }
        }
    }

    /// Composite only the inclusive region `(x0, y0)..=(x1, y1)` (clamped),
    /// row-major RGBA, for partial texture uploads. Tiles are composited
    /// once per tile row and emitted in canvas row order, so a region
    /// spanning tiles still yields a contiguous row-major buffer.
    pub fn flatten_region(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<u8> {
        if self.w == 0 || self.h == 0 {
            // No tiles exist; the pixel loop used to emit the single
            // background pixel at (0, 0). Keep that result.
            return self.composite_pixel(0, 0).to_vec();
        }
        let x1 = x1.min(self.w.saturating_sub(1));
        let y1 = y1.min(self.h.saturating_sub(1));
        let (x0, y0) = (x0.min(x1), y0.min(y1));
        let plan = self.stack_plan();
        let mut out = Vec::with_capacity(((x1 - x0 + 1) * (y1 - y0 + 1) * 4) as usize);
        let (tx0, tx1) = (x0 / TILE, x1 / TILE);
        for ty in y0 / TILE..=y1 / TILE {
            let row_tiles: Vec<Vec<u8>> = (tx0..=tx1)
                .map(|tx| self.composite_tile(&plan, tx, ty))
                .collect();
            for y in y0.max(ty * TILE)..=y1.min((ty + 1) * TILE - 1) {
                let row_in_tile = (y % TILE) as usize * TILE as usize;
                for (i, tx) in (tx0..=tx1).enumerate() {
                    let cx0 = (tx * TILE).max(x0);
                    let cx1 = ((tx + 1) * TILE - 1).min(x1);
                    for x in cx0..=cx1 {
                        let o = (row_in_tile + (x % TILE) as usize) * 4;
                        out.extend_from_slice(&row_tiles[i][o..o + 4]);
                    }
                }
            }
        }
        out
    }

    /// Attach a fully-revealing (white) mask to a layer. Structural, like
    /// add/remove layer: not undoable itself, but every mask stroke after
    /// it is (snapshots carry the mask).
    pub fn add_mask(&mut self, index: usize) {
        if index >= self.layers.len() {
            return;
        }
        let n = (self.w * self.h) as usize;
        let slot = &mut self.layers[index].mask;
        if slot.as_ref().map(|m| m.len() != n).unwrap_or(true) {
            let mut fresh = Raster::mask(self.w, self.h);
            fresh.fill(255);
            *slot = Some(fresh);
        }
        self.mark_full_dirty();
    }

    /// Remove a layer's mask, revealing the whole layer again.
    pub fn clear_mask(&mut self, index: usize) {
        if index >= self.layers.len() {
            return;
        }
        self.layers[index].mask = None;
        self.mark_full_dirty();
    }

    /// The active layer's mask as opaque grayscale RGBA for the mask viewer.
    /// Rendered at the layer offset (aligned with canvas paint); uncovered
    /// canvas reads concealed (black). `None` when there is no (valid) mask.
    pub fn mask_view(&self) -> Option<Vec<u8>> {
        let layer = self.layers.get(self.active)?;
        let m = layer.mask.as_ref()?;
        if m.len() != (self.w * self.h) as usize {
            return None;
        }
        if self.w == 0 || self.h == 0 {
            return Some(Vec::new());
        }
        self.mask_view_region(0, 0, self.w - 1, self.h - 1)
    }

    /// One inclusive canvas rect of [`mask_view`](Self::mask_view) as row-major
    /// RGBA, `(x1 - x0 + 1) * (y1 - y0 + 1) * 4` bytes, clamped to the canvas:
    /// what a dirty-rect mask upload needs. Reads the mask through the tile
    /// accessors rather than densifying it (a dense 8K mask is 64 MB), so a
    /// dab-sized rect costs a dab-sized read. `None` when there is no (valid)
    /// mask, the canvas is empty, or the rect is empty.
    pub fn mask_view_region(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> Option<Vec<u8>> {
        let layer = self.layers.get(self.active)?;
        let m = layer.mask.as_ref()?;
        if m.len() != (self.w * self.h) as usize || self.w == 0 || self.h == 0 {
            return None;
        }
        let (x0, y0) = (x0.min(self.w - 1), y0.min(self.h - 1));
        let (x1, y1) = (x1.min(self.w - 1), y1.min(self.h - 1));
        if x0 > x1 || y0 > y1 {
            return None;
        }
        let (rw, rh) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
        // Uncovered canvas is black and every pixel is opaque.
        let mut out = vec![0u8; rw * rh * 4];
        for px in out.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        // Dense band read when the mask shares the canvas geometry (the only
        // shape the engine creates); a ragged mask keeps the per-pixel rule.
        if m.w() == self.w && m.h() == self.h {
            let ly0 = (y0 as i64 - layer.oy as i64).max(0);
            let ly1 = (y1 as i64 - layer.oy as i64).min(self.h as i64 - 1);
            if let (true, Some((lo, hi))) = (ly0 <= ly1, slot_span(x0, x1, layer.ox, self.w)) {
                let (cy0, cy1) = (
                    (ly0 + layer.oy as i64) as u32,
                    (ly1 + layer.oy as i64) as u32,
                );
                let bw = (hi - lo + 1) as usize;
                let mut band = Vec::new();
                m.region_into(
                    (
                        slot_x(lo, layer.ox),
                        ly0 as u32,
                        slot_x(hi, layer.ox),
                        ly1 as u32,
                    ),
                    &mut band,
                );
                for (row, y) in (cy0..=cy1).enumerate() {
                    let dst_row = (y - y0) as usize * rw;
                    for x in lo..=hi {
                        let v = band[row * bw + (x - lo) as usize];
                        let d = (dst_row + (x - x0) as usize) * 4;
                        out[d] = v;
                        out[d + 1] = v;
                        out[d + 2] = v;
                    }
                }
            }
            return Some(out);
        }
        for (ry, y) in (y0..=y1).enumerate() {
            for (rx, x) in (x0..=x1).enumerate() {
                let v = match layer.local_offset(x, y, self.w, self.h) {
                    Some(o) => m.get(o / 4),
                    None => 0,
                };
                let d = (ry * rw + rx) * 4;
                out[d] = v;
                out[d + 1] = v;
                out[d + 2] = v;
            }
        }
        Some(out)
    }

    /// Bake one layer to canvas-size buffers with its offset applied
    /// (cropped to the canvas): what interchange formats store. Returns
    /// the shifted pixels plus the shifted mask (or `None` when the layer
    /// has no valid mask, matching the exporter's existing rule).
    pub fn baked_layer(&self, index: usize) -> Result<Option<BakedLayer>, String> {
        let Some(layer) = self.layers.get(index) else {
            return Ok(None);
        };
        let n = (self.w as usize)
            .checked_mul(self.h as usize)
            .ok_or_else(|| "canvas dimensions overflow".to_string())?;
        let n4 = n
            .checked_mul(4)
            .ok_or_else(|| "canvas dimensions overflow".to_string())?;
        if layer.pixels.w() != self.w || layer.pixels.h() != self.h || layer.pixels.len() != n4 {
            return Ok(None);
        }
        let src = layer.pixels.try_to_dense()?;
        let src_mask = match layer.mask.as_ref() {
            Some(m) => Some(m.try_to_dense()?),
            None => None,
        };
        let mut px = crate::raster::try_zeroed(n4)?;
        let mut mk: Option<Vec<u8>> = match src_mask.as_deref() {
            Some(m) if m.len() == n => Some(crate::raster::try_zeroed(n)?),
            _ => None,
        };
        for y in 0..self.h {
            for x in 0..self.w {
                if let Some(o) = layer.local_offset(x, y, self.w, self.h) {
                    if o + 3 < src.len() {
                        let d = ((y * self.w + x) * 4) as usize;
                        px[d..d + 4].copy_from_slice(&src[o..o + 4]);
                        if let (Some(m), Some(dst)) = (src_mask.as_deref(), mk.as_mut()) {
                            dst[(y * self.w + x) as usize] = m.get(o / 4).copied().unwrap_or(0);
                        }
                    }
                }
            }
        }
        Ok(Some((px, mk)))
    }
}

/// Downscale row-major RGBA bytes to a thumbnail whose longest side is
/// `max_side` (nearest-neighbor, aspect preserved). Returns
/// `(bytes, thumb_w, thumb_h)`. Images already at or under `max_side`
/// are returned unchanged (no upscale). Empty or length-mismatched
/// input yields an empty thumbnail — the Gallery shows a placeholder
/// instead of failing.
pub fn gallery_thumb(src: &[u8], w: u32, h: u32, max_side: u32) -> (Vec<u8>, u32, u32) {
    if w == 0 || h == 0 || max_side == 0 {
        return (Vec::new(), 0, 0);
    }
    if src.len() != (w as usize) * (h as usize) * 4 {
        return (Vec::new(), 0, 0);
    }
    let longest = w.max(h);
    if longest <= max_side {
        return (src.to_vec(), w, h);
    }
    let scale = max_side as f64 / longest as f64;
    let tw = ((w as f64 * scale).round() as u32).max(1);
    let th = ((h as f64 * scale).round() as u32).max(1);
    let mut out = vec![0u8; (tw as usize) * (th as usize) * 4];
    for ty in 0..th {
        let sy = ((ty as f64 / scale).floor() as u32).min(h - 1);
        for tx in 0..tw {
            let sx = ((tx as f64 / scale).floor() as u32).min(w - 1);
            let s = ((sy * w + sx) * 4) as usize;
            let d = ((ty * tw + tx) * 4) as usize;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    (out, tw, th)
}

mod filters;
mod layers;
mod selection;
mod undo;
pub use filters::{curves_lut, levels_lut};

#[cfg(test)]
mod tests;

mod io;

mod liquify;
mod transforms;
#[cfg(test)]
use liquify::liquify_steps;
