//! Layers, documents, compositing and undo.

use crate::raster::{Raster, TILE};

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

    pub fn add_layer(&mut self, name: String) {
        self.layers.push(Layer::blank(name, self.w, self.h));
        self.active = self.layers.len() - 1;
        self.mark_full_dirty();
    }

    pub fn remove_layer(&mut self, index: usize) {
        if self.layers.len() <= 1 || index >= self.layers.len() {
            return;
        }
        self.layers.remove(index);
        self.active = self.active.min(self.layers.len() - 1);
        self.mark_full_dirty();
    }

    pub fn move_layer(&mut self, from: usize, to: usize) {
        if from >= self.layers.len() || to >= self.layers.len() || from == to {
            return;
        }
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        self.active = to;
        self.mark_full_dirty();
    }

    /// Look up a group by id.
    pub fn group(&self, id: u32) -> Option<&LayerGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub(crate) fn group_mut(&mut self, id: u32) -> Option<&mut LayerGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// Product of a layer's ancestor group opacities (1.0 when ungrouped):
    /// what compositing, frames and export multiply into the layer alpha.
    /// Corrupt parent cycles break the same bounded walk as visibility, and
    /// non-finite stored values read as opaque (writers sanitize).
    pub fn group_opacity(&self, index: usize) -> f32 {
        let Some(layer) = self.layers.get(index) else {
            return 1.0;
        };
        let mut op = 1.0f32;
        let mut parent = layer.group;
        let mut guard = self.groups.len().saturating_add(1);
        while let Some(id) = parent {
            if guard == 0 {
                break;
            }
            guard -= 1;
            let Some(g) = self.group(id) else {
                break;
            };
            let o = g.opacity;
            op *= if o.is_finite() {
                o.clamp(0.0, 1.0)
            } else {
                1.0
            };
            if op <= 0.0 {
                return 0.0;
            }
            parent = g.parent;
        }
        op
    }

    /// True when the layer and every ancestor group are visible (what
    /// compositing, playback, export and onion paths must honor instead of
    /// the bare layer flag).
    pub fn is_layer_shown(&self, index: usize) -> bool {
        let Some(layer) = self.layers.get(index) else {
            return false;
        };
        if !layer.visible {
            return false;
        }
        let mut parent = layer.group;
        // Bounded walk: a corrupt parent cycle can never hang compositing.
        let mut guard = self.groups.len().saturating_add(1);
        while let Some(id) = parent {
            if guard == 0 {
                break;
            }
            guard -= 1;
            let Some(g) = self.group(id) else {
                break;
            };
            if !g.visible {
                return false;
            }
            parent = g.parent;
        }
        true
    }

    /// Create an empty top-level group and return its id. Structural like
    /// add/remove layer: not undoable, marks the view dirty.
    pub fn new_group(&mut self, name: String) -> u32 {
        // Ids start at 1; 0 always reads as "no group".
        let id = self.next_group_id.max(1);
        self.next_group_id = id.saturating_add(1).max(1);
        self.groups.push(LayerGroup {
            id,
            name,
            visible: true,
            collapsed: false,
            opacity: 1.0,
            parent: None,
        });
        self.mark_full_dirty();
        id
    }

    pub fn rename_group(&mut self, id: u32, name: String) {
        if let Some(g) = self.group_mut(id) {
            g.name = name;
        }
    }

    pub fn set_group_visible(&mut self, id: u32, visible: bool) {
        if let Some(g) = self.group_mut(id) {
            g.visible = visible;
        }
        self.mark_full_dirty();
    }

    /// Set a group's opacity (non-finite reads as opaque). Structural like
    /// visibility: not undoable, marks the view dirty.
    pub fn set_group_opacity(&mut self, id: u32, opacity: f32) {
        if let Some(g) = self.group_mut(id) {
            g.opacity = if opacity.is_finite() {
                opacity.clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        self.mark_full_dirty();
    }

    pub fn set_group_collapsed(&mut self, id: u32, collapsed: bool) {
        if let Some(g) = self.group_mut(id) {
            g.collapsed = collapsed;
        }
    }

    /// Move a layer into a group (or to the top level with `None`),
    /// keeping group members contiguous: the layer lands next to the
    /// group's current members (topmost side).
    pub fn move_layer_into_group(&mut self, index: usize, group: Option<u32>) {
        if index >= self.layers.len() {
            return;
        }
        if let Some(id) = group {
            if self.group(id).is_none() {
                return;
            }
        }
        let mut layer = self.layers.remove(index);
        layer.group = group;
        let at = match group {
            None => self.layers.len(),
            Some(id) => self
                .layers
                .iter()
                .rposition(|l| l.group == Some(id))
                .map_or(self.layers.len(), |p| p + 1),
        };
        // `active` tracked the removed layer: follow it to its new home.
        self.layers.insert(at, layer);
        self.active = at;
        self.mark_full_dirty();
    }

    /// Disband a group: member layers (and subgroups) move up one level,
    /// the group row disappears. Contents always survive (no destructive
    /// group delete in v1).
    pub fn ungroup(&mut self, id: u32) {
        let Some(pos) = self.groups.iter().position(|g| g.id == id) else {
            return;
        };
        let parent = self.groups[pos].parent;
        self.groups.remove(pos);
        for layer in self.layers.iter_mut() {
            if layer.group == Some(id) {
                layer.group = parent;
            }
        }
        for g in self.groups.iter_mut() {
            if g.parent == Some(id) {
                g.parent = parent;
            }
        }
        self.active = self.active.min(self.layers.len().saturating_sub(1));
        self.mark_full_dirty();
    }

    /// Snapshot the active layer for one undo step (call when a stroke starts).
    /// A new checkpoint discards the redo history.
    pub fn checkpoint(&mut self) {
        if let Some(entry) = self.snapshot() {
            self.push_undo(entry);
            self.redo.clear();
            self.redo_bytes = 0;
        }
    }

    /// Push onto the undo stack, evicting oldest-first while over either cap.
    /// The newest step is always kept, even if it alone exceeds the budget.
    fn push_undo(&mut self, entry: UndoEntry) {
        self.undo_bytes += entry.byte_size();
        self.undo.push(entry);
        while self.undo.len() > 1
            && (self.undo.len() > MAX_UNDO_ENTRIES || self.undo_bytes > MAX_UNDO_BYTES)
        {
            self.undo_bytes -= self.undo[0].byte_size();
            self.undo.remove(0);
        }
    }

    /// Push onto the redo stack, same dual-cap eviction as [`Self::push_undo`].
    fn push_redo(&mut self, entry: UndoEntry) {
        self.redo_bytes += entry.byte_size();
        self.redo.push(entry);
        while self.redo.len() > 1
            && (self.redo.len() > MAX_UNDO_ENTRIES || self.redo_bytes > MAX_UNDO_BYTES)
        {
            self.redo_bytes -= self.redo[0].byte_size();
            self.redo.remove(0);
        }
    }

    /// Copy of the active layer for the history stacks, if there is one.
    fn snapshot(&self) -> Option<UndoEntry> {
        self.layers.get(self.active).map(|layer| UndoEntry {
            layer: self.active,
            pixels: layer.pixels.clone(),
            mask: layer.mask.clone(),
            ox: layer.ox,
            oy: layer.oy,
            text: layer.text.clone(),
        })
    }

    /// Rewind the active layer to the newest checkpoint without consuming it.
    /// The stroke replay path uses this to redraw the stroke with taper.
    pub fn restore_last_checkpoint(&mut self) -> bool {
        let Some(entry) = self.undo.last() else {
            return false;
        };
        let index = entry.layer;
        {
            let Some(layer) = self.layers.get_mut(index) else {
                return false;
            };
            layer.pixels.clone_from(&entry.pixels);
            layer.mask.clone_from(&entry.mask);
            layer.ox = entry.ox;
            layer.oy = entry.oy;
            layer.text.clone_from(&entry.text);
        }
        self.active = index.min(self.layers.len().saturating_sub(1));
        self.mark_full_dirty();
        true
    }

    /// Restore the most recent snapshot. Returns false when history is empty.
    pub fn undo(&mut self) -> bool {
        if let Some(entry) = self.undo.pop() {
            self.undo_bytes -= entry.byte_size();
            if let Some(cur) = self.snapshot() {
                self.push_redo(cur);
                if let Some(layer) = self.layers.get_mut(entry.layer) {
                    layer.pixels = entry.pixels;
                    layer.mask = entry.mask;
                    layer.ox = entry.ox;
                    layer.oy = entry.oy;
                    layer.text = entry.text;
                    self.active = entry.layer.min(self.layers.len().saturating_sub(1));
                    self.mark_full_dirty();
                    return true;
                }
            }
        }
        false
    }

    /// Re-apply the most recently undone snapshot. Returns false when empty.
    pub fn redo(&mut self) -> bool {
        if let Some(entry) = self.redo.pop() {
            self.redo_bytes -= entry.byte_size();
            if let Some(cur) = self.snapshot() {
                self.push_undo(cur);
                if let Some(layer) = self.layers.get_mut(entry.layer) {
                    layer.pixels = entry.pixels;
                    layer.mask = entry.mask;
                    layer.ox = entry.ox;
                    layer.oy = entry.oy;
                    layer.text = entry.text;
                    self.active = entry.layer.min(self.layers.len().saturating_sub(1));
                    self.mark_full_dirty();
                    return true;
                }
            }
        }
        false
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
        let plan = self.stack_plan();
        let mut out = vec![0u8; (self.w as usize) * (self.h as usize) * 4];
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
        out
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
    fn fit_shift(
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
        ) -> Vec<u8> {
            let mut out = vec![0u8; (new_w * new_h * bpp) as usize];
            let x0 = (-sx).max(0) as u32;
            let x1 = new_w.min((old_w as i32 - sx).max(0) as u32);
            if x1 <= x0 {
                return out;
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
            out
        }
        let (old_w, old_h) = (self.w, self.h);
        for layer in self.layers.iter_mut() {
            let (sx, sy) = (-dx - layer.ox, -dy - layer.oy);
            let dense = layer.pixels.to_dense();
            let next = carry(&dense, old_w, old_h, new_w, new_h, 4, sx, sy);
            layer.pixels = Raster::from_dense(new_w, new_h, 4, &next);
            if let Some(m) = layer.mask.take() {
                let md = m.to_dense();
                layer.mask = Some(Raster::from_dense(
                    new_w,
                    new_h,
                    1,
                    &carry(&md, old_w, old_h, new_w, new_h, 1, sx, sy),
                ));
            }
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
        if let Some(sm) = self.selection_mask.take() {
            self.selection_mask = Some(carry(&sm, old_w, old_h, new_w, new_h, 1, -dx, -dy));
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
    fn sample_bilinear(pixels: &[u8], w: u32, h: u32, x: f32, y: f32) -> [u8; 4] {
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

    /// Scale then rotate (degrees clockwise) a layer about the paint's
    /// own center (opaque-pixel bbox; canvas center for empty layers),
    /// so the paint spins/grows in place instead of orbiting the canvas.
    /// Multiples of 90° resample exactly (nearest), so repeated
    /// quarter-turns never blur; other angles/scales resample once per
    /// call (repeated fractional transforms are inherently lossy — fewer,
    /// deliberate clicks keep paint crisp). Masks travel with their layer.
    /// Caller checkpoints for undo first.
    pub fn affine_layer(&mut self, index: usize, scale: f32, degrees: f32) {
        if index >= self.layers.len() || (scale - 1.0).abs() < 1e-6 && degrees.abs() < 1e-6 {
            return;
        }
        let (w, h) = (self.w, self.h);
        let src = self.layers[index].pixels.to_dense();
        // Pivot: content bbox center keeps paint in place; empty layers
        // keep the old canvas-center behavior (invisible either way).
        let center = Self::content_center(&src, w, h);
        let pivot = center.unwrap_or((w as f32 * 0.5, h as f32 * 0.5));
        let quarters = (degrees / 90.0).round();
        let exact = (scale - 1.0).abs() < 1e-6 && (degrees - quarters * 90.0).abs() < 1e-3;
        let out = Self::affine_buffer(&src, w, h, scale, degrees, pivot.0, pivot.1, exact);
        self.layers[index].pixels = Raster::from_dense(w, h, 4, &out);
        if let Some(m) = self.layers[index].mask.take() {
            let md = m.to_dense();
            let rgba: Vec<u8> = md.iter().flat_map(|&v| [v, 0, 0, 255]).collect();
            let out = Self::affine_buffer(&rgba, w, h, scale, degrees, pivot.0, pivot.1, exact);
            let gray: Vec<u8> = out.as_chunks::<4>().0.iter().map(|p| p[0]).collect();
            self.layers[index].mask = Some(Raster::from_dense(w, h, 1, &gray));
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
    /// the whole canvas. None when the layer is empty.
    pub fn content_bounds(&self, index: usize) -> Option<(f32, f32, f32, f32)> {
        let (w, h) = (self.w, self.h);
        let layer = self.layers.get(index)?;
        Self::pixel_bounds(&layer.pixels.to_dense(), w, h)
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
    ) -> Vec<u8> {
        let theta = degrees.to_radians();
        let (sin, cos) = theta.sin_cos();
        let s = scale.max(0.01);
        let mut next = vec![0u8; (w * h * 4) as usize];
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
        next
    }

    /// Corner-pin distort (M43): remap the layer through `quad`, the
    /// source coords sampled at destination corners [TL, TR, BR, BL],
    /// bilinearly interpolated across the canvas and resampled with the
    /// shared bilinear sampler (identity quad is an exact no-op). Like
    /// `affine_layer`, the remap is canvas-space so layer offsets stay
    /// valid untouched, and masks travel with their layer. Fractional
    /// pins are lossy by nature — one deliberate gesture per distort
    /// keeps paint crisp. Caller checkpoints for undo first.
    pub fn distort_layer(&mut self, index: usize, quad: [(f32, f32); 4]) {
        if index >= self.layers.len() {
            return;
        }
        let (w, h) = (self.w, self.h);
        if w < 2 || h < 2 {
            return;
        }
        let src = self.layers[index].pixels.to_dense();
        let out = Self::quad_buffer(&src, w, h, &quad);
        self.layers[index].pixels = Raster::from_dense(w, h, 4, &out);
        if let Some(m) = self.layers[index].mask.take() {
            let md = m.to_dense();
            let rgba: Vec<u8> = md.iter().flat_map(|&v| [v, 0, 0, 255]).collect();
            let out = Self::quad_buffer(&rgba, w, h, &quad);
            let gray: Vec<u8> = out.as_chunks::<4>().0.iter().map(|p| p[0]).collect();
            self.layers[index].mask = Some(Raster::from_dense(w, h, 1, &gray));
        }
        self.mark_full_dirty();
    }

    /// Shared corner-pin resampler: destination (x, y) samples the
    /// bilinear blend of `quad` at (x/(w-1), y/(h-1)).
    fn quad_buffer(src: &[u8], w: u32, h: u32, quad: &[(f32, f32); 4]) -> Vec<u8> {
        let (w1, h1) = (w as f32 - 1.0, h as f32 - 1.0);
        let mut next = vec![0u8; (w * h * 4) as usize];
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
        next
    }

    /// Warp mesh (M44): remap the layer through a `cols` x `rows` cell
    /// lattice, `grid` holding `(cols + 1) * (rows + 1)` source coords in
    /// row-major order (identity = regular canvas positions, exact
    /// no-op). Each destination pixel lands in one cell and samples the
    /// bilinear blend of its four lattice points, so dragging one point
    /// pulls paint locally — distort's freeform sibling. Same canvas-space
    /// conventions as `distort_layer`: offsets untouched, masks travel.
    /// Caller checkpoints for undo first.
    pub fn warp_layer(&mut self, index: usize, cols: u32, rows: u32, grid: &[(f32, f32)]) {
        if index >= self.layers.len() || cols < 1 || rows < 1 {
            return;
        }
        let (w, h) = (self.w, self.h);
        if w < 2 || h < 2 {
            return;
        }
        if grid.len() != ((cols + 1) * (rows + 1)) as usize {
            return;
        }
        let src = self.layers[index].pixels.to_dense();
        let out = Self::warp_buffer(&src, w, h, cols, rows, grid);
        self.layers[index].pixels = Raster::from_dense(w, h, 4, &out);
        if let Some(m) = self.layers[index].mask.take() {
            let md = m.to_dense();
            let rgba: Vec<u8> = md.iter().flat_map(|&v| [v, 0, 0, 255]).collect();
            let out = Self::warp_buffer(&rgba, w, h, cols, rows, grid);
            let gray: Vec<u8> = out.as_chunks::<4>().0.iter().map(|p| p[0]).collect();
            self.layers[index].mask = Some(Raster::from_dense(w, h, 1, &gray));
        }
        self.mark_full_dirty();
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
    ) -> Vec<u8> {
        let stride = (cols + 1) as usize;
        let mut next = vec![0u8; (w * h * 4) as usize];
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
        next
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
    ) {
        if index >= self.layers.len() || src.len() != dst.len() || src.len() < 3 {
            return;
        }
        let (w, h) = (self.w, self.h);
        if w < 2 || h < 2 {
            return;
        }
        let n = src.len() as u32;
        if tris.is_empty() || tris.iter().any(|t| t.iter().any(|&i| i >= n)) {
            return;
        }
        if src == dst {
            return;
        }
        let pixels = self.layers[index].pixels.to_dense();
        let out = Self::tri_buffer(&pixels, w, h, src, dst, tris);
        self.layers[index].pixels = Raster::from_dense(w, h, 4, &out);
        if let Some(m) = self.layers[index].mask.take() {
            let md = m.to_dense();
            let rgba: Vec<u8> = md.iter().flat_map(|&v| [v, 0, 0, 255]).collect();
            let out = Self::tri_buffer(&rgba, w, h, src, dst, tris);
            let gray: Vec<u8> = out.as_chunks::<4>().0.iter().map(|p| p[0]).collect();
            self.layers[index].mask = Some(Raster::from_dense(w, h, 1, &gray));
        }
        self.mark_full_dirty();
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
    ) -> Vec<u8> {
        let mut next = vec![0u8; (w * h * 4) as usize];
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
        next
    }

    /// Mirror a layer left-right in place. Masks travel with their layer.
    /// Caller checkpoints for undo first.
    pub fn flip_layer_horizontal(&mut self, index: usize) {
        if index >= self.layers.len() {
            return;
        }
        let (w, h) = (self.w, self.h);
        let src = self.layers[index].pixels.to_dense();
        let mut next = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let s = ((y * w + (w - 1 - x)) * 4) as usize;
                let d = ((y * w + x) * 4) as usize;
                next[d..d + 4].copy_from_slice(&src[s..s + 4]);
            }
        }
        self.layers[index].pixels = Raster::from_dense(w, h, 4, &next);
        // Mirror the offset too so the content flips about the canvas
        // center, exactly as before offsets existed.
        self.layers[index].ox = -self.layers[index].ox;
        if let Some(m) = self.layers[index].mask.take() {
            let m = m.to_dense();
            let mut nm = vec![0u8; (w * h) as usize];
            for y in 0..h {
                for x in 0..w {
                    nm[(y * w + x) as usize] = m[(y * w + (w - 1 - x)) as usize];
                }
            }
            self.layers[index].mask = Some(Raster::from_dense(w, h, 1, &nm));
        }
        self.mark_full_dirty();
    }

    /// Flip a layer top-bottom in place. Masks travel with their layer.
    /// Caller checkpoints for undo first.
    pub fn flip_layer_vertical(&mut self, index: usize) {
        if index >= self.layers.len() {
            return;
        }
        let (w, h) = (self.w, self.h);
        let src = self.layers[index].pixels.to_dense();
        let mut next = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let s = (((h - 1 - y) * w + x) * 4) as usize;
                let d = ((y * w + x) * 4) as usize;
                next[d..d + 4].copy_from_slice(&src[s..s + 4]);
            }
        }
        self.layers[index].pixels = Raster::from_dense(w, h, 4, &next);
        // Mirror the offset too so the content flips about the canvas
        // center, exactly as before offsets existed.
        self.layers[index].oy = -self.layers[index].oy;
        if let Some(m) = self.layers[index].mask.take() {
            let m = m.to_dense();
            let mut nm = vec![0u8; (w * h) as usize];
            for y in 0..h {
                for x in 0..w {
                    nm[(y * w + x) as usize] = m[((h - 1 - y) * w + x) as usize];
                }
            }
            self.layers[index].mask = Some(Raster::from_dense(w, h, 1, &nm));
        }
        self.mark_full_dirty();
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
    pub fn merge_down(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.layers.len() {
            return false;
        }
        let src = self.layers[index].clone();
        let src_px = src.pixels.to_dense();
        let src_mask = src.mask.as_ref().map(|m| m.to_dense());
        let sa_base = if self.is_layer_shown(index) {
            src.opacity.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (w, h) = (self.w, self.h);
        // The composite bakes the destination's rendered text: the result is
        // a plain pixel layer, never editable text (a text source above
        // vanishes with `src`). Pixels already match, so only the block drops.
        self.layers[index - 1].text = None;
        let dst = &mut self.layers[index - 1];
        let mut dst_px = dst.pixels.to_dense();
        let dst_mask = dst.mask.as_ref().map(|m| m.to_dense());
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
        dst.pixels.replace_dense(&dst_px);
        self.layers.remove(index);
        self.active = index - 1;
        self.mark_full_dirty();
        true
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
        let dense = m.to_dense();
        let mut out = Vec::with_capacity(dense.len() * 4);
        for y in 0..self.h {
            for x in 0..self.w {
                let v = match layer.local_offset(x, y, self.w, self.h) {
                    Some(o) => dense.get(o / 4).copied().unwrap_or(0),
                    None => 0,
                };
                out.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Some(out)
    }

    /// Bake one layer to canvas-size buffers with its offset applied
    /// (cropped to the canvas): what interchange formats store. Returns
    /// the shifted pixels plus the shifted mask (or `None` when the layer
    /// has no valid mask, matching the exporter's existing rule).
    pub fn baked_layer(&self, index: usize) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let layer = self.layers.get(index)?;
        let n = (self.w * self.h) as usize;
        if layer.pixels.w() != self.w || layer.pixels.h() != self.h || layer.pixels.len() != n * 4 {
            return None;
        }
        let src = layer.pixels.to_dense();
        let src_mask = layer.mask.as_ref().map(|m| m.to_dense());
        let mut px = vec![0u8; n * 4];
        let mut mk: Option<Vec<u8>> = match src_mask.as_deref() {
            Some(m) if m.len() == n => Some(vec![0u8; n]),
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
        Some((px, mk))
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
    pub fn set_lasso(&mut self, points: &[(f32, f32)]) -> Option<(u32, u32, u32, u32)> {
        if points.len() < 3 || self.w == 0 || self.h == 0 {
            self.selection_mask = None;
            return None;
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
            return None;
        }
        // Even-odd scanline fill of the polygon.
        let n = points.len();
        let mut mask = vec![0u8; (w * h) as usize];
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
            return None;
        }
        self.selection = Some((x0, y0, x1, y1));
        self.selection_mask = Some(mask);
        Some((x0, y0, x1, y1))
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
    fn bake_rect_to_mask(&mut self) -> bool {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 {
            return false;
        }
        let Some((rx0, ry0, rx1, ry1)) = self.selection else {
            return self
                .selection_mask
                .as_ref()
                .is_some_and(|m| m.iter().any(|&b| b != 0));
        };
        let n = (w * h) as usize;
        let mut mask = vec![0u8; n];
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
            return false;
        }
        self.selection_mask = Some(mask);
        true
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
    fn blur_gray(src: &[u8], w: u32, h: u32, r: i32) -> Vec<u8> {
        if r <= 0 || w == 0 || h == 0 {
            return src.to_vec();
        }
        let (w, h, r) = (w as usize, h as usize, r as isize);
        let win = (2 * r + 1) as u32;
        let at = |buf: &[u8], i: isize, n: isize| buf[i.clamp(0, n - 1) as usize] as u32;
        let mut tmp = vec![0u8; w * h];
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
        let mut out = vec![0u8; w * h];
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
        out
    }

    /// Sliding-window extremum over a replicated-edge line.
    fn slide_extreme(line: &[u8], r: usize, dilate: bool) -> Vec<u8> {
        use std::collections::VecDeque;
        let n = line.len();
        if n == 0 || r == 0 {
            return line.to_vec();
        }
        let mut pad = Vec::with_capacity(n + 2 * r);
        for _ in 0..r {
            pad.push(line[0]);
        }
        pad.extend_from_slice(line);
        for _ in 0..r {
            pad.push(line[n - 1]);
        }
        let better = |a: u8, b: u8| if dilate { a >= b } else { a <= b };
        let mut out = vec![0u8; n];
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
        out
    }

    /// Separable box dilate (max) / erode (min) over one gray channel.
    fn morph_gray(src: &[u8], w: u32, h: u32, r: i32, dilate: bool) -> Vec<u8> {
        if r <= 0 || w == 0 || h == 0 {
            return src.to_vec();
        }
        let (w, h, r) = (w as usize, h as usize, r as usize);
        let mut tmp = vec![0u8; w * h];
        for y in 0..h {
            tmp[y * w..(y + 1) * w].copy_from_slice(&Self::slide_extreme(
                &src[y * w..(y + 1) * w],
                r,
                dilate,
            ));
        }
        let mut out = vec![0u8; w * h];
        let mut col = vec![0u8; h];
        for x in 0..w {
            for y in 0..h {
                col[y] = tmp[y * w + x];
            }
            let done = Self::slide_extreme(&col, r, dilate);
            for y in 0..h {
                out[y * w + x] = done[y];
            }
        }
        out
    }

    /// Feather (soften) the selection edge by `radius` px: the mask ramps
    /// instead of stepping, so paint and filters fade across the edge.
    /// The bounds rect grows to contain the falloff. Returns false when
    /// there was nothing to feather. Selection-only: not undoable (like
    /// Clear selection).
    pub fn feather_selection(&mut self, radius: u32) -> bool {
        let r = radius.min(Self::MAX_SELECT_RADIUS as u32) as i32;
        if !self.bake_rect_to_mask() {
            return false;
        }
        if r > 0 {
            if let Some(m) = self.selection_mask.take() {
                self.selection_mask = Some(Self::blur_gray(&m, self.w, self.h, r));
            }
            self.retighten_selection();
        }
        self.selection_mask.is_some()
    }

    /// Grow (positive) or shrink (negative) the selection by `px`,
    /// clamped to ±[`Document::MAX_SELECT_RADIUS`]. Shrinking to nothing
    /// clears it. Returns false when nothing remains. Not undoable.
    pub fn grow_selection(&mut self, px: i32) -> bool {
        let px = px.clamp(-Self::MAX_SELECT_RADIUS, Self::MAX_SELECT_RADIUS);
        if !self.bake_rect_to_mask() {
            return false;
        }
        if px == 0 {
            return true;
        }
        if let Some(m) = self.selection_mask.take() {
            self.selection_mask = Some(Self::morph_gray(&m, self.w, self.h, px.abs(), px > 0));
        }
        self.retighten_selection();
        self.selection_mask.is_some()
    }

    /// De-jag the selection edge (blur + re-threshold, edge position kept).
    /// Returns false when there was nothing to smooth. Not undoable.
    pub fn smooth_selection(&mut self) -> bool {
        if !self.bake_rect_to_mask() {
            return false;
        }
        if let Some(m) = self.selection_mask.take() {
            let soft = Self::blur_gray(&m, self.w, self.h, 2);
            self.selection_mask = Some(
                soft.into_iter()
                    .map(|v| if v >= 128 { 255 } else { 0 })
                    .collect(),
            );
        }
        self.retighten_selection();
        self.selection_mask.is_some()
    }

    /// Flip the selection (unbounded complements become full-canvas masks).
    /// Returns false when there was nothing to invert. Not undoable.
    pub fn invert_selection(&mut self) -> bool {
        if !self.bake_rect_to_mask() {
            return false;
        }
        if let Some(m) = self.selection_mask.take() {
            self.selection_mask = Some(m.into_iter().map(|v| 255 - v).collect());
        }
        self.retighten_selection();
        self.selection_mask.is_some()
    }

    /// Nudge the selection by `(dx, dy)` canvas px (rect corners clamp to
    /// the canvas; masks shift with zero-fill, so content pushed off the
    /// edge is lost). Shrinking to nothing clears it. Returns false when
    /// nothing remains. Not undoable.
    pub fn translate_selection(&mut self, dx: i32, dy: i32) -> bool {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 || !self.bake_rect_to_mask() {
            return false;
        }
        if dx == 0 && dy == 0 {
            return true;
        }
        let Some(m) = self.selection_mask.take() else {
            return false;
        };
        let n = (w * h) as usize;
        let mut out = vec![0u8; n];
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
            return false;
        }
        self.selection_mask = Some(out);
        self.retighten_selection();
        true
    }

    /// Mirror the selection left-right (rect corners remap, masks flip).
    /// Returns false when there was no selection. Not undoable.
    pub fn flip_selection_h(&mut self) -> bool {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 || !self.bake_rect_to_mask() {
            return false;
        }
        if let Some(m) = self.selection_mask.take() {
            let mut out = vec![0u8; (w * h) as usize];
            for y in 0..h {
                for x in 0..w {
                    out[(y * w + x) as usize] = m[(y * w + (w - 1 - x)) as usize];
                }
            }
            self.selection_mask = Some(out);
        }
        self.retighten_selection();
        self.selection_mask.is_some()
    }

    /// Mirror the selection top-bottom. Returns false when there was no
    /// selection. Not undoable.
    pub fn flip_selection_v(&mut self) -> bool {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 || !self.bake_rect_to_mask() {
            return false;
        }
        if let Some(m) = self.selection_mask.take() {
            let mut out = vec![0u8; (w * h) as usize];
            for y in 0..h {
                for x in 0..w {
                    out[(y * w + x) as usize] = m[((h - 1 - y) * w + x) as usize];
                }
            }
            self.selection_mask = Some(out);
        }
        self.retighten_selection();
        self.selection_mask.is_some()
    }

    /// Select the active layer's pixels more opaque than `threshold`
    /// (offset-aware; uncovered canvas counts as transparent). Replaces any
    /// selection; empty layers clear it. Returns false when nothing is
    /// selected. Not undoable.
    pub fn select_opaque(&mut self, threshold: u8) -> bool {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 {
            self.clear_selection();
            return false;
        }
        let Some(layer) = self.layers.get(self.active) else {
            self.clear_selection();
            return false;
        };
        let (ox, oy) = (layer.ox, layer.oy);
        let src = layer.pixels.to_dense();
        let mut mask = vec![0u8; (w * h) as usize];
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
        self.selection_mask.is_some()
    }

    /// Paint the selection outline `width` px wide (clamped 1..=32) with
    /// `brush` at `color` through `put` (symmetry- and target-aware). The
    /// edge comes from the selection itself. Caller checkpoints for undo.
    /// Returns false without a selection.
    pub fn stroke_selection(
        &mut self,
        brush: &super::brush::Brush,
        color: super::brush::Color,
        width: u32,
    ) -> bool {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 {
            return false;
        }
        let inside = |m: &[u8], x: i32, y: i32| -> bool {
            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                return false;
            }
            m[(y as u32 * w + x as u32) as usize] > 127
        };
        let mut edge = vec![false; (w * h) as usize];
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
            return false;
        }
        let r = width.clamp(1, 32) as i32 - 1;
        if r > 0 {
            let gray: Vec<u8> = edge.iter().map(|&b| if b { 255 } else { 0 }).collect();
            let fat = Self::morph_gray(&gray, w, h, r, true);
            edge = fat.into_iter().map(|v| v != 0).collect();
        }
        for y in 0..h {
            for x in 0..w {
                if edge[(y * w + x) as usize] {
                    self.put(brush, color, x, y);
                }
            }
        }
        self.mark_full_dirty();
        true
    }

    /// Separable sliding-window box blur (all channels), O(w*h).
    fn box_blur(src: &[u8], w: u32, h: u32, r: i32) -> Vec<u8> {
        let (w, h) = (w as i32, h as i32);
        let wu = w as u32;
        let window = (2 * r + 1) as u32;
        let px = |buf: &[u8], x: i32, y: i32, ch: usize| -> u32 {
            let x = x.clamp(0, w - 1) as u32;
            let y = y.clamp(0, h - 1) as u32;
            buf[((y * wu + x) * 4) as usize + ch] as u32
        };
        // Horizontal pass: src -> tmp.
        let mut tmp = vec![0u8; src.len()];
        for y in 0..h {
            let mut sum = [0u32; 4];
            for dx in -r..=r {
                for (ch, s) in sum.iter_mut().enumerate() {
                    *s += px(src, dx, y, ch);
                }
            }
            for x in 0..w {
                for ch in 0..4 {
                    tmp[((y as u32 * wu + x as u32) * 4) as usize + ch] = (sum[ch] / window) as u8;
                }
                for (ch, s) in sum.iter_mut().enumerate() {
                    *s += px(src, x + r + 1, y, ch);
                    *s -= px(src, x - r, y, ch);
                }
            }
        }
        // Vertical pass: tmp -> out.
        let mut out = vec![0u8; src.len()];
        for x in 0..w {
            let mut sum = [0u32; 4];
            for dy in -r..=r {
                for (ch, s) in sum.iter_mut().enumerate() {
                    *s += px(&tmp, x, dy, ch);
                }
            }
            for y in 0..h {
                for ch in 0..4 {
                    out[((y as u32 * wu + x as u32) * 4) as usize + ch] = (sum[ch] / window) as u8;
                }
                for (ch, s) in sum.iter_mut().enumerate() {
                    *s += px(&tmp, x, y + r + 1, ch);
                    *s -= px(&tmp, x, y - r, ch);
                }
            }
        }
        out
    }

    /// Brightness/contrast on a layer: `brightness` -255..=255 is added,
    /// `contrast` 0.0..=3.0 scales around mid-gray (1.0 keeps contrast).
    /// RGB only; alpha is untouched. Honors the active selection.
    /// Caller checkpoints for undo first.
    pub fn adjust_tone(&mut self, index: usize, brightness: i32, contrast: f32) {
        if index >= self.layers.len() || (brightness == 0 && (contrast - 1.0).abs() < f32::EPSILON)
        {
            return;
        }
        let (w, h) = (self.w, self.h);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.to_dense();
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
        self.layers[index].pixels.replace_dense(&pixels);
        self.mark_full_dirty();
    }

    /// Box blur on a layer, `radius` 1..=25 (0 is a no-op). Honors the
    /// active selection for writes. Caller checkpoints for undo first.
    pub fn blur_layer(&mut self, index: usize, radius: u32) {
        if index >= self.layers.len() || radius == 0 {
            return;
        }
        let (w, h) = (self.w, self.h);
        let blurred = Self::box_blur(
            &self.layers[index].pixels.to_dense(),
            w,
            h,
            radius.min(25) as i32,
        );
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.to_dense();
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
        self.layers[index].pixels.replace_dense(&pixels);
        self.mark_full_dirty();
    }

    /// Unsharp mask on a layer: `amount` 0.0..=2.0 mixes the high-frequency
    /// detail back in (0 is a no-op). Honors the active selection.
    /// Caller checkpoints for undo first.
    pub fn sharpen_layer(&mut self, index: usize, amount: f32) {
        if index >= self.layers.len() || amount <= 0.0 {
            return;
        }
        let amount = amount.min(2.0);
        let (w, h) = (self.w, self.h);
        let src = self.layers[index].pixels.to_dense();
        let blurred = Self::box_blur(&src, w, h, 2);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.to_dense();
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
        self.layers[index].pixels.replace_dense(&pixels);
        self.mark_full_dirty();
    }

    /// Remap the RGB channels through `lut` (alpha untouched). Honors the
    /// active selection with graduated-mask mixing. Caller checkpoints
    /// for undo first. Powers Curves + Levels (M36) and the M37 pack.
    pub fn apply_lut(&mut self, index: usize, lut: &[u8; 256]) {
        if index >= self.layers.len() {
            return;
        }
        let (w, h) = (self.w, self.h);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.to_dense();
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
        self.layers[index].pixels.replace_dense(&pixels);
        self.mark_full_dirty();
    }

    /// M37 HSL adjust: rotate hue by `hue_deg` degrees, scale saturation
    /// by `sat_mult`, shift lightness by `light` (-1..=1). Grays have no
    /// hue, so they only answer to lightness (same honesty as the palette
    /// harmonies). Honors the active selection. Caller checkpoints first.
    pub fn adjust_hsl(&mut self, index: usize, hue_deg: f32, sat_mult: f32, light: f32) {
        if index >= self.layers.len() {
            return;
        }
        let sat_mult = sat_mult.clamp(0.0, 4.0);
        let light = light.clamp(-1.0, 1.0);
        if hue_deg == 0.0 && sat_mult == 1.0 && light == 0.0 {
            return;
        }
        let (w, h) = (self.w, self.h);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.to_dense();
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
                    super::palette::rgb_to_hsl([pixels[o], pixels[o + 1], pixels[o + 2]]);
                let v = if ss <= 0.0 {
                    let g = ((ll + light).clamp(0.0, 1.0) * 255.0).round();
                    [g as u8, g as u8, g as u8]
                } else {
                    super::palette::hsl_to_rgb(
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
        self.layers[index].pixels.replace_dense(&pixels);
        self.mark_full_dirty();
    }

    /// M37 Color balance: channel offsets in -255..=255 (positive warms
    /// toward red/green/blue, negative toward cyan/magenta/yellow).
    /// Honors the active selection. Caller checkpoints first.
    pub fn color_balance(&mut self, index: usize, r: i32, g: i32, b: i32) {
        if index >= self.layers.len() || (r == 0 && g == 0 && b == 0) {
            return;
        }
        let off = [r as f32, g as f32, b as f32];
        let (w, h) = (self.w, self.h);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.to_dense();
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
        self.layers[index].pixels.replace_dense(&pixels);
        self.mark_full_dirty();
    }

    /// M37 Recolor (colorize): push every pixel's hue/saturation toward
    /// `target` while keeping its own luminance, mixed by `strength`
    /// 0..=1 (0 is a no-op). White stays white, black stays black —
    /// that is what colorize means. Honors the active selection.
    /// Caller checkpoints first.
    pub fn colorize(&mut self, index: usize, target: [u8; 3], strength: f32) {
        if index >= self.layers.len() || strength <= 0.0 {
            return;
        }
        let strength = strength.clamp(0.0, 1.0);
        let (th, ts, _) = super::palette::rgb_to_hsl(target);
        let (w, h) = (self.w, self.h);
        let sel = self.selection;
        let mask = self.selection_mask.as_deref();
        let mut pixels = self.layers[index].pixels.to_dense();
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
                    super::palette::rgb_to_hsl([pixels[o], pixels[o + 1], pixels[o + 2]]);
                let tint = super::palette::hsl_to_rgb(th, ts, ll);
                let k = Self::sel_k(mask, w, x, y) * strength;
                for ch in 0..3 {
                    let mixed = pixels[o + ch] as f32 * (1.0 - k) + tint[ch] as f32 * k;
                    pixels[o + ch] = mixed.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        self.layers[index].pixels.replace_dense(&pixels);
        self.mark_full_dirty();
    }

    /// M40 Liquify push: displace layer pixels along the drag vector
    /// `(dx, dy)` within `size` px of `(x, y)`, reverse-mapped with
    /// bilinear sampling so no holes tear open. `strength` 0..=1 scales
    /// the push; falloff is smooth to the rim. Honors the active
    /// selection for writes (graduated-mask mixed, like blur). Caller
    /// checkpoints once per stroke; zero push is a no-op.
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
    ) {
        if index >= self.layers.len() || size < 1.0 || strength <= 0.0 {
            return;
        }
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        if self.w == 0 || self.h == 0 {
            return;
        }
        // One push per symmetry mirror, deduped when a mirror lands on an
        // identical (point, vector) pair (e.g. kaleidoscope center).
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
            self.liquify_push(index, px, py, pdx, pdy, size, strength);
        }
    }

    /// One warp centered at (x, y) pushing pixels by (dx, dy). Reads from
    /// a bounded region copy (disc plus sample reach), not the full layer.
    #[allow(clippy::too_many_arguments)]
    fn liquify_push(
        &mut self,
        index: usize,
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        size: f32,
        strength: f32,
    ) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let (w, h) = (self.w, self.h);
        let radius = size * 0.5;
        let bx0 = ((x - radius).floor().max(0.0)) as u32;
        let by0 = ((y - radius).floor().max(0.0)) as u32;
        let bx1 = ((x + radius).ceil().min(w as f32 - 1.0).max(0.0)) as u32;
        let by1 = ((y + radius).ceil().min(h as f32 - 1.0).max(0.0)) as u32;
        if bx0 > bx1 || by0 > by1 {
            return;
        }
        // Reverse samples reach up to |d| * strength px back along the
        // push; copy that hull so the whole warp reads from the region.
        let reach = (dx.hypot(dy) * strength).ceil() as u32 + 1;
        let rx0 = bx0.saturating_sub(reach);
        let ry0 = by0.saturating_sub(reach);
        let rx1 = (bx1 + reach).min(w - 1);
        let ry1 = (by1 + reach).min(h - 1);
        let rw = rx1 - rx0 + 1;
        let sel = self.selection;
        let Document {
            layers,
            selection_mask,
            ..
        } = self;
        let mask = selection_mask.as_deref();
        // Reads come from a bounded region copy (disc plus sample reach):
        // sampling the tiles directly keeps an 8K push O(region) instead of
        // materialising the whole layer twice per pointer move.
        let pixels = &layers[index].pixels;
        let mut src = Vec::with_capacity((rw * (ry1 - ry0 + 1) * 4) as usize);
        for ry in ry0..=ry1 {
            for rx in rx0..=rx1 {
                src.extend_from_slice(&pixels.get4(((ry * w + rx) * 4) as usize));
            }
        }
        let sample = |sx: f32, sy: f32, ch: usize| -> f32 {
            let sx = sx.clamp(0.0, w as f32 - 1.0);
            let sy = sy.clamp(0.0, h as f32 - 1.0);
            let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let at = |x: u32, y: u32| src[(((y - ry0) * rw + (x - rx0)) * 4) as usize + ch] as f32;
            at(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + at(x1, y0) * fx * (1.0 - fy)
                + at(x0, y1) * (1.0 - fx) * fy
                + at(x1, y1) * fx * fy
        };
        let layer = &mut layers[index];
        for py in by0..=by1 {
            for px in bx0..=bx1 {
                let dist = (((px as f32 + 0.5 - x).powi(2) + (py as f32 + 0.5 - y).powi(2)).sqrt()
                    / radius)
                    .min(1.0);
                // Smooth falloff: full push at the center, zero at the rim.
                let f = 0.5 + 0.5 * (std::f32::consts::PI * dist).cos();
                let (ox, oy) = (px as f32 - dx * strength * f, py as f32 - dy * strength * f);
                if (ox - px as f32).abs() < f32::EPSILON && (oy - py as f32).abs() < f32::EPSILON {
                    continue;
                }
                let selected = !matches!(sel, Some((a, b, c, d)) if px < a || px > c || py < b || py > d)
                    && match mask {
                        Some(m) => m[(py * w + px) as usize] != 0,
                        None => true,
                    };
                if !selected {
                    continue;
                }
                let o = ((py * w + px) * 4) as usize;
                if o + 3 >= layer.pixels.len() {
                    continue;
                }
                let k = Self::sel_k(mask, w, px, py);
                let mut dst = layer.pixels.get4(o);
                for (ch, d) in dst.iter_mut().enumerate() {
                    let v = sample(ox, oy, ch);
                    *d = (*d as f32 * (1.0 - k) + v * k).round() as u8;
                }
                layer.pixels.set4(o, dst);
            }
        }
        self.mark_dirty(bx0, by0, bx1, by1);
    }

    /// Save the composite as PNG.
    pub fn save_png(&self, path: &std::path::Path) -> Result<(), String> {
        image::save_buffer(
            path,
            &self.flatten(),
            self.w,
            self.h,
            image::ColorType::Rgba8,
        )
        .map_err(|e| e.to_string())
    }

    /// Save the composite as JPEG at `quality` (1-100, higher keeps more
    /// detail). JPEG stores no alpha; the composite is already opaque
    /// over the background, so this only strips the alpha channel.
    pub fn save_jpeg(&self, path: &std::path::Path, quality: u8) -> Result<(), String> {
        if !(1..=100).contains(&quality) {
            return Err(format!("bad JPEG quality: {quality} (want 1-100)"));
        }
        let flat = self.flatten();
        let mut rgb = Vec::with_capacity((self.w * self.h) as usize * 3);
        for i in 0..flat.len() / 4 {
            let o = i * 4;
            rgb.extend_from_slice(&flat[o..o + 3]);
        }
        let mut buf = Vec::new();
        {
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
            enc.encode(&rgb, self.w, self.h, image::ExtendedColorType::Rgb8)
                .map_err(|e| e.to_string())?;
        }
        std::fs::write(path, &buf).map_err(|e| e.to_string())
    }

    /// Save the composite as a plain single-image TIFF (lossless RGBA,
    /// no layers or extras — the layered original stays in `.procraft`).
    pub fn save_tiff(&self, path: &std::path::Path) -> Result<(), String> {
        use image::ImageEncoder;
        let flat = self.flatten();
        let mut buf = Vec::new();
        {
            let enc = image::codecs::tiff::TiffEncoder::new(std::io::Cursor::new(&mut buf));
            enc.write_image(&flat, self.w, self.h, image::ExtendedColorType::Rgba8)
                .map_err(|e| e.to_string())?;
        }
        std::fs::write(path, &buf).map_err(|e| e.to_string())
    }

    /// Load an image file flattened onto a fresh layer.
    /// M45 hardening: files over 64MB fail closed on metadata, and
    /// dimensions are checked from the header before the full decode
    /// so a phone panorama can't OOM the session (cap matches
    /// [`MAX_CANVAS_SIDE`] and `rehome_canvas`).
    pub fn load_background(&mut self, path: &std::path::Path) -> Result<(), String> {
        const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > MAX_FILE_BYTES {
                return Err("image file is too large (max 64MB)".to_string());
            }
        }
        let (hw, hh) = image::ImageReader::open(path)
            .map_err(|e| e.to_string())?
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .into_dimensions()
            .map_err(|e| e.to_string())?;
        if hw == 0 || hh == 0 || hw > MAX_CANVAS_SIDE || hh > MAX_CANVAS_SIDE {
            return Err(format!(
                "image too large: {hw}x{hh} (want 1-{MAX_CANVAS_SIDE} each)"
            ));
        }
        let img = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
        let (w, h) = (img.width(), img.height());
        if w == 0 || h == 0 || w > MAX_CANVAS_SIDE || h > MAX_CANVAS_SIDE {
            return Err(format!(
                "image too large: {w}x{h} (want 1-{MAX_CANVAS_SIDE} each)"
            ));
        }
        self.w = w;
        self.h = h;
        let mut layer = Layer::blank("Imported".to_string(), w, h);
        layer.pixels = Raster::from_dense(w, h, 4, img.as_raw());
        self.layers = vec![layer];
        self.active = 0;
        self.selection = None;
        self.selection_mask = None;
        self.undo.clear();
        self.redo.clear();
        self.undo_bytes = 0;
        self.redo_bytes = 0;
        Ok(())
    }

    /// Save the full project (layers, opacity, blend, background, symmetry)
    /// to a `.procraft` file: magic + little-endian records, zlib-compressed
    /// pixels. Version 11 adds per-layer text blocks (v10 and older files
    /// still open text-less; old app versions cleanly refuse v11 files at
    /// the magic).
    pub fn save_project(&self, path: &std::path::Path) -> Result<(), String> {
        use std::io::Write;
        let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
        f.write_all(b"PCRAFT11").map_err(|e| e.to_string())?;
        f.write_all(&self.w.to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.write_all(&self.h.to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.write_all(&self.background).map_err(|e| e.to_string())?;
        f.write_all(&(self.layers.len() as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.write_all(&(self.active as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        let symmetry = match self.symmetry {
            SymmetryMode::Off => 0u8,
            SymmetryMode::Horizontal => 1u8,
            SymmetryMode::Vertical => 2u8,
            SymmetryMode::Both => 3u8,
            SymmetryMode::Kaleidoscope => 4u8,
        };
        f.write_all(&[symmetry]).map_err(|e| e.to_string())?;
        f.write_all(&[self.symmetry_segments.clamp(2, 16)])
            .map_err(|e| e.to_string())?;
        // v7 stores the mirror center (flag + two f32s); older files read
        // canvas middle.
        match self.symmetry_center {
            Some((cx, cy)) => {
                f.write_all(&[1u8]).map_err(|e| e.to_string())?;
                f.write_all(&cx.to_le_bytes()).map_err(|e| e.to_string())?;
                f.write_all(&cy.to_le_bytes()).map_err(|e| e.to_string())?;
            }
            None => {
                f.write_all(&[0u8]).map_err(|e| e.to_string())?;
            }
        }
        // v8 stores the layer-group table (count + records); older files
        // read an empty table.
        f.write_all(&(self.groups.len() as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        for g in &self.groups {
            f.write_all(&g.id.to_le_bytes())
                .map_err(|e| e.to_string())?;
            match g.parent {
                Some(p) => {
                    f.write_all(&[1u8]).map_err(|e| e.to_string())?;
                    f.write_all(&p.to_le_bytes()).map_err(|e| e.to_string())?;
                }
                None => {
                    f.write_all(&[0u8]).map_err(|e| e.to_string())?;
                }
            }
            let name = g.name.as_bytes();
            f.write_all(&(name.len() as u32).to_le_bytes())
                .map_err(|e| e.to_string())?;
            f.write_all(name).map_err(|e| e.to_string())?;
            f.write_all(&[u8::from(g.visible)])
                .map_err(|e| e.to_string())?;
            f.write_all(&[u8::from(g.collapsed)])
                .map_err(|e| e.to_string())?;
            // v9 stores the group opacity; older files end the record here.
            f.write_all(&g.opacity.to_le_bytes())
                .map_err(|e| e.to_string())?;
        }
        for layer in &self.layers {
            let name = layer.name.as_bytes();
            f.write_all(&(name.len() as u32).to_le_bytes())
                .map_err(|e| e.to_string())?;
            f.write_all(name).map_err(|e| e.to_string())?;
            f.write_all(&[u8::from(layer.visible)])
                .map_err(|e| e.to_string())?;
            f.write_all(&layer.opacity.to_le_bytes())
                .map_err(|e| e.to_string())?;
            let blend = match layer.blend {
                BlendMode::Normal => 0u8,
                BlendMode::Multiply => 1u8,
                BlendMode::Screen => 2u8,
                BlendMode::Overlay => 3u8,
                BlendMode::Darken => 4u8,
                BlendMode::Lighten => 5u8,
                BlendMode::Difference => 6u8,
                BlendMode::Exclusion => 7u8,
                BlendMode::SoftLight => 8u8,
                BlendMode::HardLight => 9u8,
            };
            f.write_all(&[blend]).map_err(|e| e.to_string())?;
            f.write_all(&[u8::from(layer.clip)])
                .map_err(|e| e.to_string())?;
            // v6 stores the canvas offset (Move tool) as two i32s.
            f.write_all(&layer.ox.to_le_bytes())
                .map_err(|e| e.to_string())?;
            f.write_all(&layer.oy.to_le_bytes())
                .map_err(|e| e.to_string())?;
            // v8 stores the owning group (flag + id); older files read none.
            match layer.group {
                Some(id) => {
                    f.write_all(&[1u8]).map_err(|e| e.to_string())?;
                    f.write_all(&id.to_le_bytes()).map_err(|e| e.to_string())?;
                }
                None => {
                    f.write_all(&[0u8]).map_err(|e| e.to_string())?;
                }
            }
            let expect = (self.w * self.h * 4) as usize;
            let pixels = layer.pixels.to_dense();
            if pixels.len() != expect {
                return Err("layer has wrong pixel size".to_string());
            }
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(&pixels).map_err(|e| e.to_string())?;
            let packed = enc.finish().map_err(|e| e.to_string())?;
            f.write_all(&(packed.len() as u32).to_le_bytes())
                .map_err(|e| e.to_string())?;
            f.write_all(&packed).map_err(|e| e.to_string())?;
            // Optional reveal mask, zlib-compressed like the pixels.
            match layer.mask.as_ref().map(|m| m.to_dense()) {
                Some(m) if m.len() == (self.w * self.h) as usize => {
                    f.write_all(&[1u8]).map_err(|e| e.to_string())?;
                    let mut enc =
                        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                    enc.write_all(&m).map_err(|e| e.to_string())?;
                    let packed = enc.finish().map_err(|e| e.to_string())?;
                    f.write_all(&(packed.len() as u32).to_le_bytes())
                        .map_err(|e| e.to_string())?;
                    f.write_all(&packed).map_err(|e| e.to_string())?;
                }
                _ => {
                    f.write_all(&[0u8]).map_err(|e| e.to_string())?;
                }
            }
            // v10 stores the animation frame hold (ticks); older layer
            // records end at the mask.
            f.write_all(&layer.hold.to_le_bytes())
                .map_err(|e| e.to_string())?;
            // v11 appends the text-layer definition; pixel-only layers
            // write a zero flag so every record stays self-describing.
            match &layer.text {
                Some(t) => {
                    f.write_all(&[1u8]).map_err(|e| e.to_string())?;
                    f.write_all(&t.x.to_le_bytes()).map_err(|e| e.to_string())?;
                    f.write_all(&t.y.to_le_bytes()).map_err(|e| e.to_string())?;
                    f.write_all(&t.size_px.to_le_bytes())
                        .map_err(|e| e.to_string())?;
                    f.write_all(&t.color).map_err(|e| e.to_string())?;
                    f.write_all(&t.w.to_le_bytes()).map_err(|e| e.to_string())?;
                    f.write_all(&t.h.to_le_bytes()).map_err(|e| e.to_string())?;
                    f.write_all(&t.style.leading.to_le_bytes())
                        .map_err(|e| e.to_string())?;
                    f.write_all(&t.style.tracking.to_le_bytes())
                        .map_err(|e| e.to_string())?;
                    match t.style.outline {
                        Some((c, px)) => {
                            f.write_all(&[1u8]).map_err(|e| e.to_string())?;
                            f.write_all(&c.0).map_err(|e| e.to_string())?;
                            f.write_all(&px.to_le_bytes()).map_err(|e| e.to_string())?;
                        }
                        None => {
                            f.write_all(&[0u8]).map_err(|e| e.to_string())?;
                        }
                    }
                    match &t.style.font_bytes {
                        Some(font) => {
                            f.write_all(&[1u8]).map_err(|e| e.to_string())?;
                            f.write_all(&(font.len() as u32).to_le_bytes())
                                .map_err(|e| e.to_string())?;
                            f.write_all(font).map_err(|e| e.to_string())?;
                        }
                        None => {
                            f.write_all(&[0u8]).map_err(|e| e.to_string())?;
                        }
                    }
                    let text = t.text.as_bytes();
                    f.write_all(&(text.len() as u32).to_le_bytes())
                        .map_err(|e| e.to_string())?;
                    f.write_all(text).map_err(|e| e.to_string())?;
                }
                None => {
                    f.write_all(&[0u8]).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }

    /// Load a `.procraft` file, replacing the current document.
    /// M45 hardening: files over 200MB fail closed on metadata alone
    /// (same cap as PSD/brush imports) — never `read_to_end` into memory.
    pub fn open_project(path: &std::path::Path) -> Result<Document, String> {
        use std::io::Read;
        const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > MAX_FILE_BYTES {
                return Err("project file is too large (max 200MB)".to_string());
            }
        }
        let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        let mut cur = &buf[..];
        let mut take = |n: usize| -> Result<&[u8], String> {
            if cur.len() < n {
                return Err("project file is truncated".to_string());
            }
            let (head, tail) = cur.split_at(n);
            cur = tail;
            Ok(head)
        };
        let magic = take(8)?.to_vec();
        let v11 = magic == b"PCRAFT11";
        let v10 = v11 || magic == b"PCRAFT10";
        let v9 = v10 || magic == b"PCRAFT09";
        let v8 = v9 || magic == b"PCRAFT08";
        let v7 = v8 || magic == b"PCRAFT07";
        let v6 = v7 || magic == b"PCRAFT06";
        let v5 = v6 || magic == b"PCRAFT05";
        let v4 = v5 || magic == b"PCRAFT04";
        let v3 = v4 || magic == b"PCRAFT03";
        let v2 = v3 || magic == b"PCRAFT02";
        if magic != b"PCRAFT01" && !v2 {
            return Err("not a ProCraft project file".to_string());
        }
        let w = u32::from_le_bytes(
            take(4)?[..4]
                .try_into()
                .map_err(|_| "bad width".to_string())?,
        );
        let h = u32::from_le_bytes(
            take(4)?[..4]
                .try_into()
                .map_err(|_| "bad height".to_string())?,
        );
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            return Err("project has bad dimensions".to_string());
        }
        let bg: [u8; 4] = take(4)?[..4]
            .try_into()
            .map_err(|_| "bad background".to_string())?;
        let count = u32::from_le_bytes(
            take(4)?[..4]
                .try_into()
                .map_err(|_| "bad layer count".to_string())?,
        );
        if count == 0 || count > 256 {
            return Err("project has bad layer count".to_string());
        }
        let active = u32::from_le_bytes(
            take(4)?[..4]
                .try_into()
                .map_err(|_| "bad active layer".to_string())?,
        ) as usize;
        let symmetry = if v2 {
            match take(1)?[0] {
                1 => SymmetryMode::Horizontal,
                2 => SymmetryMode::Vertical,
                3 => SymmetryMode::Both,
                4 if v3 => SymmetryMode::Kaleidoscope,
                _ => SymmetryMode::Off,
            }
        } else {
            SymmetryMode::Off
        };
        let symmetry_segments = if v3 {
            let s = take(1)?[0];
            if (2..=16).contains(&s) {
                s
            } else {
                6
            }
        } else {
            6
        };
        // v7 appends the mirror center (flag + two f32s); older files read
        // canvas middle. Non-finite values fall back to the default.
        let symmetry_center = if v7 {
            let flag = take(1)?[0];
            if flag == 0 {
                None
            } else {
                let cx = f32::from_le_bytes(
                    take(4)?[..4]
                        .try_into()
                        .map_err(|_| "bad symmetry center".to_string())?,
                );
                let cy = f32::from_le_bytes(
                    take(4)?[..4]
                        .try_into()
                        .map_err(|_| "bad symmetry center".to_string())?,
                );
                if cx.is_finite() && cy.is_finite() {
                    Some((cx.clamp(-16384.0, 16384.0), cy.clamp(-16384.0, 16384.0)))
                } else {
                    None
                }
            }
        } else {
            None
        };
        // v8 appends the layer-group table after the mirror center;
        // older files read an empty table. Unknown parents sanitize to
        // top-level so corrupt ids can never orphan a group.
        let mut groups = Vec::new();
        if v8 {
            let gcount = u32::from_le_bytes(
                take(4)?[..4]
                    .try_into()
                    .map_err(|_| "bad group count".to_string())?,
            ) as usize;
            if gcount > 256 {
                return Err("project has bad group count".to_string());
            }
            let mut seen = Vec::new();
            for _ in 0..gcount {
                let id = u32::from_le_bytes(
                    take(4)?[..4]
                        .try_into()
                        .map_err(|_| "bad group id".to_string())?,
                );
                if id == 0 || seen.contains(&id) {
                    return Err("project has bad group id".to_string());
                }
                seen.push(id);
                let parent = if take(1)?[0] == 0 {
                    None
                } else {
                    Some(u32::from_le_bytes(
                        take(4)?[..4]
                            .try_into()
                            .map_err(|_| "bad group parent".to_string())?,
                    ))
                };
                let name_len = u32::from_le_bytes(
                    take(4)?[..4]
                        .try_into()
                        .map_err(|_| "bad group name".to_string())?,
                ) as usize;
                if name_len > 512 {
                    return Err("project has bad group name".to_string());
                }
                let name = String::from_utf8(take(name_len)?.to_vec())
                    .map_err(|_| "bad group name".to_string())?;
                let visible = take(1)?[0] != 0;
                let collapsed = take(1)?[0] != 0;
                // v9 appends the group opacity; v8 files end here and read
                // full opacity. Non-finite values fall back to opaque.
                let opacity = if v9 {
                    let o = f32::from_le_bytes(
                        take(4)?[..4]
                            .try_into()
                            .map_err(|_| "bad group opacity".to_string())?,
                    );
                    if o.is_finite() {
                        o.clamp(0.0, 1.0)
                    } else {
                        1.0
                    }
                } else {
                    1.0
                };
                groups.push(LayerGroup {
                    id,
                    name,
                    visible,
                    collapsed,
                    opacity,
                    parent,
                });
            }
            for g in groups.iter_mut() {
                if let Some(p) = g.parent {
                    if !seen.contains(&p) {
                        g.parent = None;
                    }
                }
            }
        }
        let mut layers = Vec::new();
        for _ in 0..count {
            let name_len = u32::from_le_bytes(
                take(4)?[..4]
                    .try_into()
                    .map_err(|_| "bad name".to_string())?,
            ) as usize;
            if name_len > 512 {
                return Err("project has bad layer name".to_string());
            }
            let name = String::from_utf8(take(name_len)?.to_vec())
                .map_err(|_| "bad layer name".to_string())?;
            let visible = take(1)?[0] != 0;
            let opacity = f32::from_le_bytes(
                take(4)?[..4]
                    .try_into()
                    .map_err(|_| "bad opacity".to_string())?,
            )
            .clamp(0.0, 1.0);
            let blend = match take(1)?[0] {
                1 => BlendMode::Multiply,
                2 => BlendMode::Screen,
                3 => BlendMode::Overlay,
                4 if v5 => BlendMode::Darken,
                5 if v5 => BlendMode::Lighten,
                6 if v5 => BlendMode::Difference,
                7 if v5 => BlendMode::Exclusion,
                8 if v5 => BlendMode::SoftLight,
                9 if v5 => BlendMode::HardLight,
                _ => BlendMode::Normal,
            };
            // v5 stores the clipping flag right after the blend byte.
            let clip = if v5 { take(1)?[0] != 0 } else { false };
            // v6 appends the canvas offset (two i32s); older files read 0.
            let (ox, oy) = if v6 {
                let ox = i32::from_le_bytes(
                    take(4)?[..4]
                        .try_into()
                        .map_err(|_| "bad layer offset".to_string())?,
                )
                .clamp(-16384, 16384);
                let oy = i32::from_le_bytes(
                    take(4)?[..4]
                        .try_into()
                        .map_err(|_| "bad layer offset".to_string())?,
                )
                .clamp(-16384, 16384);
                (ox, oy)
            } else {
                (0, 0)
            };
            // v8 appends the owning group (flag + id) after the offset;
            // older files read top-level. Unknown ids sanitize to none.
            let group = if v8 {
                if take(1)?[0] == 0 {
                    None
                } else {
                    Some(u32::from_le_bytes(
                        take(4)?[..4]
                            .try_into()
                            .map_err(|_| "bad layer group".to_string())?,
                    ))
                }
            } else {
                None
            };
            let expect = (w * h * 4) as usize;
            let pixels = if v4 {
                let comp_len = u32::from_le_bytes(
                    take(4)?[..4]
                        .try_into()
                        .map_err(|_| "bad compressed size".to_string())?,
                ) as usize;
                if comp_len == 0 || comp_len > expect.saturating_add(10 * 1024 * 1024) {
                    return Err("project has bad compressed size".to_string());
                }
                let packed = take(comp_len)?.to_vec();
                let mut dec = flate2::read::ZlibDecoder::new(&packed[..]);
                let mut raw = Vec::with_capacity(expect);
                use std::io::Read;
                dec.read_to_end(&mut raw)
                    .map_err(|_| "project has corrupt pixels".to_string())?;
                if raw.len() != expect {
                    return Err("project has corrupt pixels".to_string());
                }
                raw
            } else {
                take(expect)?.to_vec()
            };
            // v5 appends an optional zlib-compressed reveal mask per layer.
            let mask = if v5 {
                let flag = take(1)?[0];
                if flag == 0 {
                    None
                } else {
                    let want = (w * h) as usize;
                    let comp_len = u32::from_le_bytes(
                        take(4)?[..4]
                            .try_into()
                            .map_err(|_| "bad mask size".to_string())?,
                    ) as usize;
                    if comp_len == 0 || comp_len > want.saturating_add(10 * 1024 * 1024) {
                        return Err("project has bad mask size".to_string());
                    }
                    let packed = take(comp_len)?.to_vec();
                    let mut dec = flate2::read::ZlibDecoder::new(&packed[..]);
                    let mut raw = Vec::with_capacity(want);
                    use std::io::Read;
                    dec.read_to_end(&mut raw)
                        .map_err(|_| "project has corrupt mask".to_string())?;
                    if raw.len() != want {
                        return Err("project has corrupt mask".to_string());
                    }
                    Some(raw)
                }
            } else {
                None
            };
            // Unknown group ids sanitize to top-level so a corrupt byte
            // can never orphan a layer.
            let known = groups.iter().map(|g| g.id).collect::<Vec<_>>();
            let group = group.filter(|id| known.contains(id));
            // v10 appends the animation frame hold after the mask; older
            // files hold every frame once. Absurd values clamp into range.
            let hold = if v10 {
                u32::from_le_bytes(
                    take(4)?[..4]
                        .try_into()
                        .map_err(|_| "bad frame hold".to_string())?,
                )
                .clamp(crate::MIN_ANIM_HOLD, crate::MAX_ANIM_HOLD)
            } else {
                1
            };
            // v11 appends the text-layer definition; older files end the
            // layer record at the hold. Hostile values fail closed (caps
            // and finite floats), in-range extremes clamp to editor limits.
            let text = if v11 {
                use crate::text::{
                    check_font, TextBlock, TextStyle, MAX_FONT_BYTES, MAX_LAYOUT_SIDE,
                    MAX_OUTLINE_PX, MAX_TEXT_BYTES, MAX_TEXT_CHARS, MAX_TEXT_LINES, MAX_TEXT_PX,
                    MAX_TEXT_SPACING, MIN_TEXT_PX,
                };
                if take(1)?[0] == 0 {
                    None
                } else {
                    let f32_at = |b: Result<&[u8], String>| -> Result<f32, String> {
                        let b = b?;
                        Ok(f32::from_le_bytes(
                            b[..4]
                                .try_into()
                                .map_err(|_| "bad text value".to_string())?,
                        ))
                    };
                    let u32_at = |b: Result<&[u8], String>| -> Result<u32, String> {
                        let b = b?;
                        Ok(u32::from_le_bytes(
                            b[..4]
                                .try_into()
                                .map_err(|_| "bad text value".to_string())?,
                        ))
                    };
                    let x = f32_at(take(4))?;
                    let y = f32_at(take(4))?;
                    let size_px = f32_at(take(4))?;
                    let color: [u8; 4] = take(4)?
                        .try_into()
                        .map_err(|_| "bad text color".to_string())?;
                    let w = u32_at(take(4))?;
                    let h = u32_at(take(4))?;
                    let leading = f32_at(take(4))?;
                    let tracking = f32_at(take(4))?;
                    if ![x, y, size_px, leading, tracking]
                        .iter()
                        .all(|v| v.is_finite())
                    {
                        return Err("project has non-finite text values".to_string());
                    }
                    let outline = if take(1)?[0] == 0 {
                        None
                    } else {
                        let c: [u8; 4] = take(4)?
                            .try_into()
                            .map_err(|_| "bad text outline".to_string())?;
                        let px = u32_at(take(4))?;
                        Some((crate::brush::Color(c), px.clamp(1, MAX_OUTLINE_PX)))
                    };
                    let font_bytes = if take(1)?[0] == 0 {
                        None
                    } else {
                        let len = u32_at(take(4))? as usize;
                        if len > MAX_FONT_BYTES {
                            return Err("project text font is too large".to_string());
                        }
                        let bytes = take(len)?.to_vec();
                        check_font(&bytes)
                            .map_err(|e| format!("project text font is invalid: {e}"))?;
                        Some(bytes)
                    };
                    let len = u32_at(take(4))? as usize;
                    if len > MAX_TEXT_BYTES {
                        return Err("project text is too large".to_string());
                    }
                    let text = String::from_utf8(take(len)?.to_vec())
                        .map_err(|_| "project text is not utf-8".to_string())?;
                    // Re-validate the editor's caps so a loaded file can
                    // always be re-rendered by the dialog (the layout
                    // rejects over-cap text, which would strand it).
                    if text.chars().count() > MAX_TEXT_CHARS {
                        return Err("project text has too many characters".to_string());
                    }
                    if text.split('\n').count() > MAX_TEXT_LINES {
                        return Err("project text has too many lines".to_string());
                    }
                    Some(TextBlock {
                        text,
                        size_px: size_px.clamp(MIN_TEXT_PX, MAX_TEXT_PX),
                        color,
                        style: TextStyle {
                            font_bytes,
                            leading: leading.clamp(-MAX_TEXT_SPACING, MAX_TEXT_SPACING),
                            tracking: tracking.clamp(-MAX_TEXT_SPACING, MAX_TEXT_SPACING),
                            outline,
                        },
                        x,
                        y,
                        w: w.min(MAX_LAYOUT_SIDE),
                        h: h.min(MAX_LAYOUT_SIDE),
                    })
                }
            } else {
                None
            };
            layers.push(Layer {
                name,
                visible,
                opacity,
                blend,
                pixels: Raster::from_dense(w, h, 4, &pixels),
                mask: mask.map(|m| Raster::from_dense(w, h, 1, &m)),
                clip,
                ox,
                oy,
                hold,
                group,
                text,
            });
        }
        if !cur.is_empty() {
            return Err("project has trailing data".to_string());
        }
        let next_group_id = groups
            .iter()
            .map(|g| g.id)
            .max()
            .map_or(1, |m| m.saturating_add(1).max(1));
        Ok(Document {
            w,
            h,
            layers,
            active: active.min(count as usize - 1),
            background: bg,
            selection: None,
            selection_mask: None,
            symmetry,
            symmetry_segments,
            symmetry_center,
            paint_target: PaintTarget::Color,
            dirty: None,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_bytes: 0,
            redo_bytes: 0,
            groups,
            next_group_id,
            smudge_finger: None,
            rev: 0,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only: an RGBA raster sized to the document from dense bytes.
    fn px_rgba(doc: &Document, bytes: &[u8]) -> Raster {
        Raster::from_dense(doc.w, doc.h, 4, bytes)
    }

    /// Test-only: a 1-bpp mask sized to the document from dense bytes.
    fn px_mask(doc: &Document, bytes: &[u8]) -> Raster {
        Raster::from_dense(doc.w, doc.h, 1, bytes)
    }

    #[test]
    fn revision_advances_on_dirty_marks() {
        let mut doc = Document::new(8, 8);
        let r0 = doc.revision();
        doc.mark_full_dirty();
        assert_eq!(doc.revision(), r0 + 1);
        doc.mark_dirty(1, 1, 2, 2);
        assert_eq!(doc.revision(), r0 + 2);
        // A dab fully outside the canvas paints nothing: no bump.
        doc.mark_dirty(5, 5, 1, 1);
        assert_eq!(doc.revision(), r0 + 2);
    }

    #[test]
    fn opaque_top_layer_wins() {
        let mut doc = Document::new(2, 1);
        // Bottom layer: solid red.
        {
            let l = &mut doc.layers[0];
            l.pixels = Raster::from_dense(2, 1, 4, &[255, 0, 0, 255, 255, 0, 0, 255]);
        }
        doc.add_layer("top".to_string());
        // Top layer, first pixel green, second transparent.
        {
            let l = &mut doc.layers[1];
            l.pixels = Raster::from_dense(2, 1, 4, &[0, 255, 0, 255, 0, 0, 0, 0]);
        }
        let flat = doc.flatten();
        assert_eq!(&flat[0..4], &[0, 255, 0, 255]);
        assert_eq!(&flat[4..8], &[255, 0, 0, 255]);
    }

    #[test]
    fn undo_restores_pixels() {
        let mut doc = Document::new(1, 1);
        doc.checkpoint();
        doc.layers[0].pixels = px_rgba(&doc, &(vec![9, 9, 9, 9]));
        assert!(doc.undo());
        assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
        assert!(!doc.undo());
    }

    #[test]
    fn restore_last_checkpoint_rewinds_without_dropping_undo() {
        let mut doc = Document::new(1, 1);
        assert!(!doc.restore_last_checkpoint());
        doc.checkpoint();
        doc.layers[0].pixels = px_rgba(&doc, &(vec![9, 9, 9, 9]));
        assert!(doc.restore_last_checkpoint());
        assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
        // The checkpoint survives the rewind and still undoes normally.
        assert!(doc.undo());
        assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
    }

    #[test]
    fn redo_restores_undone_work() {
        let mut doc = Document::new(1, 1);
        assert!(!doc.redo());
        doc.checkpoint();
        doc.layers[0].pixels = px_rgba(&doc, &(vec![9, 9, 9, 9]));
        assert!(doc.undo());
        assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
        assert!(doc.redo());
        assert_eq!(doc.layers[0].pixels, vec![9, 9, 9, 9]);
        assert!(!doc.redo());
        // Undo still works after a redo.
        assert!(doc.undo());
        assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
    }

    #[test]
    fn group_hide_conceals_members_in_flatten() {
        let mut doc = Document::new(2, 1);
        doc.layers[0].pixels = px_rgba(&doc, &(vec![255, 0, 0, 255, 255, 0, 0, 255]));
        doc.add_layer("top".to_string());
        doc.layers[1].pixels = px_rgba(&doc, &(vec![0, 0, 255, 255, 0, 0, 255, 255]));
        let g = doc.new_group("G".to_string());
        doc.move_layer_into_group(1, Some(g));
        assert!(doc.is_layer_shown(1));
        doc.set_group_visible(g, false);
        assert!(!doc.is_layer_shown(1));
        assert!(doc.is_layer_shown(0));
        // Top pixel falls through to the red layer below.
        assert_eq!(&doc.flatten()[0..4], &[255, 0, 0, 255]);
        doc.set_group_visible(g, true);
        assert_eq!(&doc.flatten()[0..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn nested_groups_need_every_level_visible() {
        let mut doc = Document::new(1, 1);
        let outer = doc.new_group("outer".to_string());
        let inner = doc.new_group("inner".to_string());
        doc.groups
            .iter_mut()
            .find(|g| g.id == inner)
            .unwrap()
            .parent = Some(outer);
        doc.move_layer_into_group(0, Some(inner));
        assert!(doc.is_layer_shown(0));
        doc.set_group_visible(inner, false);
        assert!(!doc.is_layer_shown(0));
        doc.set_group_visible(inner, true);
        doc.set_group_visible(outer, false);
        assert!(!doc.is_layer_shown(0));
    }

    #[test]
    fn ungroup_spills_members_up_one_level() {
        let mut doc = Document::new(1, 1);
        doc.add_layer("b".to_string());
        let outer = doc.new_group("outer".to_string());
        let inner = doc.new_group("inner".to_string());
        doc.groups
            .iter_mut()
            .find(|g| g.id == inner)
            .unwrap()
            .parent = Some(outer);
        doc.move_layer_into_group(0, Some(inner));
        doc.move_layer_into_group(0, Some(outer));
        doc.ungroup(inner);
        assert!(doc.group(inner).is_none());
        assert_eq!(doc.layers[0].group, Some(outer));
        assert_eq!(doc.layers[1].group, Some(outer));
        doc.ungroup(outer);
        assert!(doc.groups.is_empty());
        assert!(doc.layers.iter().all(|l| l.group.is_none()));
    }

    #[test]
    fn move_into_group_lands_adjacent() {
        let mut doc = Document::new(1, 1);
        doc.add_layer("b".to_string());
        doc.add_layer("c".to_string());
        let g = doc.new_group("G".to_string());
        // Bottom layer joins the group: inserted at the top end.
        doc.move_layer_into_group(0, Some(g));
        assert_eq!(doc.layers[2].name, "Layer 1");
        assert_eq!(doc.layers[2].group, Some(g));
        assert_eq!(doc.active, 2);
        // Unknown groups and indices are ignored.
        doc.move_layer_into_group(0, Some(999));
        assert!(doc.layers[0].group.is_none());
        doc.move_layer_into_group(99, Some(g));
        assert_eq!(doc.layers.len(), 3);
    }

    #[test]
    fn v8_roundtrip_keeps_groups() {
        let mut doc = Document::new(2, 1);
        let g = doc.new_group("Sketch".to_string());
        doc.set_group_visible(g, false);
        doc.move_layer_into_group(0, Some(g));
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v8.pcraft");
        doc.save_project(&path).unwrap();
        let head = std::fs::read(&path).unwrap();
        assert_eq!(&head[..8], b"PCRAFT11");
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.groups.len(), 1);
        assert_eq!(back.groups[0].name, "Sketch");
        assert!(!back.groups[0].visible);
        assert_eq!(back.layers[0].group, Some(g));
        assert!(!back.is_layer_shown(0));
        // Ids keep increasing after a reload (no reuse).
        let mut back = back;
        let g2 = back.new_group("More".to_string());
        assert!(g2 > g);
    }

    #[test]
    fn group_opacity_fades_members() {
        let mut doc = Document::new(2, 1);
        doc.layers[0].pixels = px_rgba(&doc, &([255, 0, 0, 255].repeat(2)));
        let g = doc.new_group("Fade".to_string());
        doc.move_layer_into_group(0, Some(g));
        assert_eq!(doc.group_opacity(0), 1.0);
        doc.set_group_opacity(g, 0.5);
        assert_eq!(doc.group_opacity(0), 0.5);
        // Red at half alpha over the white page.
        assert_eq!(&doc.flatten()[0..4], &[255, 128, 128, 255]);
        // Nested products multiply: 0.5 x 0.5.
        let outer = doc.new_group("Outer".to_string());
        doc.groups.iter_mut().find(|x| x.id == g).unwrap().parent = Some(outer);
        doc.set_group_opacity(outer, 0.5);
        assert_eq!(doc.group_opacity(0), 0.25);
        assert_eq!(&doc.flatten()[0..4], &[255, 191, 191, 255]);
        // Zero-opacity groups conceal like hidden ones.
        doc.set_group_opacity(g, 0.0);
        assert_eq!(&doc.flatten()[0..4], &[255, 255, 255, 255]);
        // Frames fade with their group too (playback, onion, GIF):
        // fully faded means no ghost pixels at all.
        let ghost = crate::onion_ghost(&doc, 0, 1.0);
        assert_eq!(&ghost[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn v9_roundtrip_keeps_group_opacity() {
        let mut doc = Document::new(2, 1);
        doc.layers[0].pixels = px_rgba(&doc, &([255, 0, 0, 255].repeat(2)));
        let g = doc.new_group("Fade".to_string());
        doc.set_group_opacity(g, 0.5);
        doc.move_layer_into_group(0, Some(g));
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v9.pcraft");
        doc.save_project(&path).unwrap();
        let head = std::fs::read(&path).unwrap();
        assert_eq!(&head[..8], b"PCRAFT11");
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.groups.len(), 1);
        assert_eq!(back.groups[0].opacity, 0.5);
        assert_eq!(back.layers[0].group, Some(g));
        // Opacity composes after reload too.
        assert_eq!(&back.flatten()[0..4], &[255, 128, 128, 255]);
    }

    #[test]
    fn v8_files_open_with_opaque_groups() {
        // Hand-build a minimal version-8 file (group record without the
        // v9 opacity tail).
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v8-compat.pcraft");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT08");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[255, 255, 255, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[0]); // Symmetry off.
        buf.extend_from_slice(&[6]);
        buf.extend_from_slice(&[0]); // No mirror center.
        buf.extend_from_slice(&1u32.to_le_bytes()); // One group.
        buf.extend_from_slice(&7u32.to_le_bytes()); // id
        buf.extend_from_slice(&[0]); // No parent.
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"G");
        buf.extend_from_slice(&[1]); // Visible.
        buf.extend_from_slice(&[0]); // Expanded.
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]); // Blend: Normal.
        buf.extend_from_slice(&[0]); // Clip: off.
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&[1]); // In group 7.
        buf.extend_from_slice(&7u32.to_le_bytes());
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&[9u8; 2 * 4]).unwrap();
        let packed = enc.finish().unwrap();
        buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        buf.extend_from_slice(&packed);
        buf.extend_from_slice(&[0]); // No mask.
        std::fs::write(&path, &buf).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.groups.len(), 1);
        assert_eq!(back.groups[0].opacity, 1.0);
        assert_eq!(back.layers[0].group, Some(7));
    }

    #[test]
    fn v10_roundtrip_keeps_frame_holds() {
        let mut doc = Document::new(2, 1);
        doc.add_layer("F2".to_string());
        doc.layers[0].hold = 3;
        doc.layers[1].hold = 8;
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v10.pcraft");
        doc.save_project(&path).unwrap();
        let head = std::fs::read(&path).unwrap();
        assert_eq!(&head[..8], b"PCRAFT11");
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.layers[0].hold, 3);
        assert_eq!(back.layers[1].hold, 8);
    }

    #[test]
    fn v9_files_open_with_unit_holds() {
        // Hand-build a minimal version-9 file (group opacity present, no
        // layer hold tails).
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v9-compat.pcraft");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT09");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[255, 255, 255, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[0]); // Symmetry off.
        buf.extend_from_slice(&[6]);
        buf.extend_from_slice(&[0]); // No mirror center.
        buf.extend_from_slice(&1u32.to_le_bytes()); // One group.
        buf.extend_from_slice(&7u32.to_le_bytes()); // id
        buf.extend_from_slice(&[0]); // No parent.
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"G");
        buf.extend_from_slice(&[1]); // Visible.
        buf.extend_from_slice(&[0]); // Expanded.
        buf.extend_from_slice(&0.5f32.to_le_bytes()); // Group opacity.
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]); // Blend: Normal.
        buf.extend_from_slice(&[0]); // Clip: off.
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&[1]); // In group 7.
        buf.extend_from_slice(&7u32.to_le_bytes());
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&[9u8; 2 * 4]).unwrap();
        let packed = enc.finish().unwrap();
        buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        buf.extend_from_slice(&packed);
        buf.extend_from_slice(&[0]); // No mask.
        std::fs::write(&path, &buf).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.layers[0].hold, 1);
        assert_eq!(back.groups[0].opacity, 0.5);
        assert_eq!(back.layers[0].group, Some(7));
    }

    #[test]
    fn v10_project_opens_text_less() {
        // Hand-build a minimal version-10 file (frame hold tail, no v11
        // text record).
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v10-compat.pcraft");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT10");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[255, 255, 255, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[0]); // Symmetry off.
        buf.extend_from_slice(&[6]);
        buf.extend_from_slice(&[0]); // No mirror center.
        buf.extend_from_slice(&0u32.to_le_bytes()); // No groups.
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]); // Blend: Normal.
        buf.extend_from_slice(&[0]); // Clip: off.
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&[0]); // No group.
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&[9u8; 2 * 4]).unwrap();
        let packed = enc.finish().unwrap();
        buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        buf.extend_from_slice(&packed);
        buf.extend_from_slice(&[0]); // No mask.
        buf.extend_from_slice(&1u32.to_le_bytes()); // Frame hold.
        std::fs::write(&path, &buf).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(!back.layers[0].is_text());
    }

    #[test]
    fn save_emits_v11_magic() {
        let doc = Document::new(2, 1);
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v11-magic.pcraft");
        doc.save_project(&path).unwrap();
        let head = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(&head[..8], b"PCRAFT11");
    }

    #[test]
    fn v11_load_rejects_hostile_text_records() {
        use crate::text::{MAX_FONT_BYTES, MAX_TEXT_BYTES, MAX_TEXT_CHARS, MAX_TEXT_LINES};
        use std::io::Write;
        let build = |font: Option<(u32, Vec<u8>)>, x: f32, text: &str| -> Vec<u8> {
            let mut buf = Vec::new();
            buf.extend_from_slice(b"PCRAFT11");
            buf.extend_from_slice(&2u32.to_le_bytes());
            buf.extend_from_slice(&1u32.to_le_bytes());
            buf.extend_from_slice(&[255, 255, 255, 255]);
            buf.extend_from_slice(&1u32.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes());
            buf.extend_from_slice(&[0]); // Symmetry off.
            buf.extend_from_slice(&[6]);
            buf.extend_from_slice(&[0]); // No mirror center.
            buf.extend_from_slice(&0u32.to_le_bytes()); // No groups.
            buf.extend_from_slice(&1u32.to_le_bytes());
            buf.extend_from_slice(b"L");
            buf.extend_from_slice(&[1]);
            buf.extend_from_slice(&1.0f32.to_le_bytes());
            buf.extend_from_slice(&[0]); // Blend: Normal.
            buf.extend_from_slice(&[0]); // Clip: off.
            buf.extend_from_slice(&0i32.to_le_bytes());
            buf.extend_from_slice(&0i32.to_le_bytes());
            buf.extend_from_slice(&[0]); // No group.
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(&[9u8; 2 * 4]).unwrap();
            let packed = enc.finish().unwrap();
            buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            buf.extend_from_slice(&packed);
            buf.extend_from_slice(&[0]); // No mask.
            buf.extend_from_slice(&1u32.to_le_bytes()); // Frame hold.
                                                        // v11 text record.
            buf.extend_from_slice(&[1]); // Has text.
            buf.extend_from_slice(&x.to_le_bytes());
            buf.extend_from_slice(&8.0f32.to_le_bytes());
            buf.extend_from_slice(&24.0f32.to_le_bytes());
            buf.extend_from_slice(&[0, 0, 0, 255]);
            buf.extend_from_slice(&5u32.to_le_bytes());
            buf.extend_from_slice(&7u32.to_le_bytes());
            buf.extend_from_slice(&0.0f32.to_le_bytes());
            buf.extend_from_slice(&0.0f32.to_le_bytes());
            buf.extend_from_slice(&[0]); // No outline.
            match &font {
                Some((len, bytes)) => {
                    buf.extend_from_slice(&[1]);
                    buf.extend_from_slice(&len.to_le_bytes());
                    buf.extend_from_slice(bytes);
                }
                None => buf.extend_from_slice(&[0]),
            }
            let t = text.as_bytes();
            buf.extend_from_slice(&(t.len() as u32).to_le_bytes());
            buf.extend_from_slice(t);
            buf
        };
        let dir = std::env::temp_dir();
        // Font declared over the 2 MiB cap.
        let path = dir.join("procraft-v11-hostile-font.pcraft");
        std::fs::write(
            &path,
            build(Some((MAX_FONT_BYTES as u32 + 1, Vec::new())), 4.0, "hi"),
        )
        .unwrap();
        assert!(Document::open_project(&path).is_err());
        let _ = std::fs::remove_file(&path);
        // Non-finite anchor.
        let path = dir.join("procraft-v11-hostile-nan.pcraft");
        std::fs::write(&path, build(None, f32::NAN, "hi")).unwrap();
        assert!(Document::open_project(&path).is_err());
        let _ = std::fs::remove_file(&path);
        // Text over the 16 KiB cap.
        let path = dir.join("procraft-v11-hostile-text.pcraft");
        std::fs::write(&path, build(None, 4.0, &"x".repeat(MAX_TEXT_BYTES + 1))).unwrap();
        assert!(Document::open_project(&path).is_err());
        let _ = std::fs::remove_file(&path);
        // Within the byte cap but past the editor's char cap.
        let path = dir.join("procraft-v11-hostile-chars.pcraft");
        std::fs::write(&path, build(None, 4.0, &"x".repeat(MAX_TEXT_CHARS + 1))).unwrap();
        assert!(Document::open_project(&path).is_err());
        let _ = std::fs::remove_file(&path);
        // Within both byte and char caps but past the line cap.
        let path = dir.join("procraft-v11-hostile-lines.pcraft");
        std::fs::write(&path, build(None, 4.0, &"x\n".repeat(MAX_TEXT_LINES + 1))).unwrap();
        assert!(Document::open_project(&path).is_err());
        let _ = std::fs::remove_file(&path);
        // At the line cap with a trailing newline: split('\n') — the
        // layout's own count — sees one segment more than lines() does,
        // so the editor would refuse this file without the load check.
        let path = dir.join("procraft-v11-hostile-trailnl.pcraft");
        std::fs::write(&path, build(None, 4.0, &"x\n".repeat(MAX_TEXT_LINES))).unwrap();
        assert!(Document::open_project(&path).is_err());
        let _ = std::fs::remove_file(&path);
        // Structurally valid record but unparseable font bytes.
        let path = dir.join("procraft-v11-hostile-badfont.pcraft");
        std::fs::write(&path, build(Some((100, vec![0xAA; 100])), 4.0, "hi")).unwrap();
        assert!(Document::open_project(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn jpeg_export_decodes_at_canvas_size() {
        let mut doc = Document::new(16, 12);
        // Solid red: lossy error stays one-sided and easy to bound.
        let px = &mut doc.layers[0].pixels;
        for i in 0..px.len() / 4 {
            px.set4(i * 4, [255, 0, 0, 255]);
        }
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-m28.jpg");
        doc.save_jpeg(&path, 90).unwrap();
        let raw = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(&raw[..2], &[0xFF, 0xD8]); // JPEG SOI marker
        let img = image::load_from_memory(&raw).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (16, 12));
        let p = img.get_pixel(8, 6);
        assert!(p[0] > 200 && p[1] < 60 && p[2] < 60);
    }

    #[test]
    fn jpeg_rejects_bad_quality() {
        let doc = Document::new(4, 4);
        let dir = std::env::temp_dir();
        assert!(doc.save_jpeg(&dir.join("procraft-m28-q0.jpg"), 0).is_err());
        assert!(doc
            .save_jpeg(&dir.join("procraft-m28-q101.jpg"), 101)
            .is_err());
    }

    #[test]
    fn tiff_export_roundtrips_lossless() {
        let mut doc = Document::new(8, 6);
        let px = &mut doc.layers[0].pixels;
        for i in 0..px.len() / 4 {
            px.set4(i * 4, [(i % 251) as u8, 17, 200, 255]);
        }
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-m28.tiff");
        doc.save_tiff(&path).unwrap();
        let raw = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(raw.starts_with(b"II*\0") || raw.starts_with(b"MM\0*"));
        let img = image::load_from_memory(&raw).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (8, 6));
        assert_eq!(img.as_raw(), &doc.flatten());
    }

    #[test]
    fn crop_keeps_content_and_rebases_offsets() {
        // 6x4 with a red block in buffer x=0..=1, layer shifted ox=2 so it
        // shows at canvas x=2..=3. Crop x=1..=4: the block lands at x=1..=2
        // and the layer re-bases to ox=0 (buffer == canvas after a crop).
        let mut doc = Document::new(6, 4);
        let px = &mut doc.layers[0].pixels;
        for y in 0..4u32 {
            for x in 0..2u32 {
                let o = ((y * 6 + x) * 4) as usize;
                px.set(o, 255);
                px.set(o + 3, 255);
            }
        }
        doc.layers[0].ox = 2;
        assert_eq!(&doc.flatten()[8..12], &[255, 0, 0, 255]);
        doc.selection = Some((0, 0, 0, 0));
        doc.crop(1, 0, 4, 3).unwrap();
        assert_eq!((doc.w, doc.h), (4, 4));
        let flat = doc.flatten();
        assert_eq!(&flat[0..4], &[255, 255, 255, 255]);
        assert_eq!(&flat[4..8], &[255, 0, 0, 255]);
        assert_eq!(&flat[8..12], &[255, 0, 0, 255]);
        assert_eq!(doc.layers[0].ox, 0);
        // The corner selection shifted fully outside, so it drops.
        assert!(doc.selection.is_none());
    }

    #[test]
    fn rehome_pad_and_trim_carry_content() {
        let mut doc = Document::new(4, 4);
        let o = (5 * 4) as usize;
        let px = &mut doc.layers[0].pixels;
        px.set(o + 2, 255);
        px.set(o + 3, 255);
        doc.selection = Some((1, 1, 2, 2));
        doc.set_symmetry_center(1.0, 1.0);
        // Pad to 8x8: old canvas sits at (2, 2).
        doc.rehome_canvas(8, 8, 2, 2).unwrap();
        assert_eq!((doc.w, doc.h), (8, 8));
        let flat = doc.flatten();
        assert_eq!(&flat[((3 * 8 + 3) * 4) as usize..][..4], &[0, 0, 255, 255]);
        assert_eq!(&flat[0..4], &[255, 255, 255, 255]);
        // A re-home re-bases every layer: buffer == canvas, ox/oy back to 0.
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
        assert_eq!(doc.selection, Some((3, 3, 4, 4)));
        assert_eq!(doc.symmetry_center_px(), (3.0, 3.0));
        // Trim back: content returns home.
        doc.rehome_canvas(4, 4, -2, -2).unwrap();
        let flat = doc.flatten();
        assert_eq!(&flat[(5 * 4)..][..4], &[0, 0, 255, 255]);
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
        assert_eq!(doc.selection, Some((1, 1, 2, 2)));
    }

    #[test]
    fn crop_leading_trim_keeps_trailing_corner_content() {
        // A leading-edge trim used to blank the rightmost x0 cols / bottom
        // y0 rows: the carry kept buffer cells [0,new) but the new canvas
        // reads cells shifted by x0. The bottom-right corner must ride along.
        let mut doc = Document::new(6, 4);
        let at = |x: u32, y: u32| ((y * 6 + x) * 4) as usize;
        doc.layers[0].pixels.set4(at(5, 3), [255, 0, 0, 255]);
        doc.layers[0].pixels.set4(at(4, 2), [0, 255, 0, 255]);
        doc.crop(1, 1, 5, 3).unwrap();
        assert_eq!((doc.w, doc.h), (5, 3));
        let flat = doc.flatten();
        let n_at = |x: u32, y: u32| ((y * 5 + x) * 4) as usize;
        // Old (5,3) lands at (4,2); old (4,2) at (3,1); (0,0) is page.
        assert_eq!(&flat[n_at(4, 2)..n_at(4, 2) + 4], &[255, 0, 0, 255]);
        assert_eq!(&flat[n_at(3, 1)..n_at(3, 1) + 4], &[0, 255, 0, 255]);
        assert_eq!(&flat[0..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn resize_center_shrink_keeps_middle_content() {
        // Center-anchor shrink 8->6 trims one column per side (dx=-1): old
        // x=6 must land at new x=5, and old x=7 (trimmed) must be gone.
        let mut doc = Document::new(8, 4);
        let at = |x: u32, y: u32| ((y * 8 + x) * 4) as usize;
        doc.layers[0].pixels.set4(at(6, 1), [0, 255, 0, 255]);
        doc.layers[0].pixels.set4(at(7, 1), [0, 0, 255, 255]);
        doc.rehome_canvas(6, 4, -1, 0).unwrap();
        let flat = doc.flatten();
        let n_at = |x: u32, y: u32| ((y * 6 + x) * 4) as usize;
        assert_eq!(&flat[n_at(5, 1)..n_at(5, 1) + 4], &[0, 255, 0, 255]);
        assert!(flat
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p != &[0, 0, 255, 255]));
    }

    #[test]
    fn crop_with_offset_layer_rebases_to_zero() {
        // Layer shifted ox=1: buffer cell 1 shows at canvas 2, cell 4 at
        // canvas 5, cell 0 at canvas 1 (trimmed away). Crop x=2..=5 -> 4x4:
        // old canvas shifts by dx=-2 and the layer re-bases to ox=0.
        let mut doc = Document::new(6, 4);
        let at = |x: u32, y: u32| ((y * 6 + x) * 4) as usize;
        doc.layers[0].pixels.set4(at(0, 0), [255, 0, 0, 255]);
        doc.layers[0].pixels.set4(at(1, 0), [0, 255, 0, 255]);
        doc.layers[0].pixels.set4(at(4, 0), [0, 0, 255, 255]);
        doc.layers[0].ox = 1;
        doc.crop(2, 0, 5, 3).unwrap();
        assert_eq!((doc.w, doc.h), (4, 4));
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
        let flat = doc.flatten();
        assert_eq!(&flat[0..4], &[0, 255, 0, 255]);
        assert_eq!(&flat[12..16], &[0, 0, 255, 255]);
        assert!(flat
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p != &[255, 0, 0, 255]));
    }

    #[test]
    fn canvas_ops_reject_bad_sizes_and_clear_history() {
        let mut doc = Document::new(6, 4);
        doc.checkpoint();
        assert!(doc.undo());
        doc.checkpoint();
        doc.rehome_canvas(8, 8, 0, 0).unwrap();
        // Old snapshots mismatch the new buffers, so history is gone.
        assert!(!doc.undo());
        assert!(!doc.redo());
        assert!(doc.crop(5, 0, 9, 3).is_err());
        assert!(doc.crop(3, 0, 1, 3).is_err());
        assert!(doc.rehome_canvas(0, 8, 0, 0).is_err());
        assert!(doc.rehome_canvas(8193, 8, 0, 0).is_err());
        // Failed ops touch nothing.
        assert_eq!((doc.w, doc.h), (8, 8));
    }

    #[test]
    fn canvas_cap_allows_8k_but_not_more() {
        assert_eq!(MAX_CANVAS_SIDE, 8192);
        assert!(Document::new(8192, 8192)
            .rehome_canvas(8192, 8192, 0, 0)
            .is_ok());
        let err = Document::new(64, 64)
            .rehome_canvas(8193, 64, 0, 0)
            .unwrap_err();
        assert!(err.contains("want 1-8192 each"), "{err}");
    }

    #[test]
    fn v7_project_opens_groupless() {
        // Hand-build a minimal version-7 file (center flag, no groups).
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v7-compat.pcraft");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT07");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[255, 255, 255, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[0]); // Symmetry off.
        buf.extend_from_slice(&[6]);
        buf.extend_from_slice(&[0]); // No mirror center.
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&0i32.to_le_bytes());
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&[9u8; 2 * 4]).unwrap();
        let packed = enc.finish().unwrap();
        buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        buf.extend_from_slice(&packed);
        buf.extend_from_slice(&[0]); // No mask.
        std::fs::write(&path, &buf).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(back.groups.is_empty());
        assert!(back.layers[0].group.is_none());
        assert!(back.is_layer_shown(0));
        assert_eq!(back.layers[0].pixels, vec![9u8; 8]);
    }

    #[test]
    fn undo_keeps_count_cap_on_small_canvas() {
        let mut doc = Document::new(16, 16);
        for _ in 0..40 {
            doc.checkpoint();
        }
        assert_eq!(doc.undo.len(), 32);
        assert!(doc.undo_bytes <= MAX_UNDO_BYTES);
    }

    #[test]
    fn undo_byte_budget_binds_on_large_canvas() {
        // 2048x2048 RGBA of allocated tiles = 16 MiB per step: the 256 MiB
        // budget caps at 16, well under the 32-count backstop.
        let mut doc = Document::new(2048, 2048);
        // Touch every tile first: a blank layer holds no tiles now, so only
        // real storage can make the byte budget bind.
        doc.layers[0].pixels.fill(255);
        for _ in 0..40 {
            doc.checkpoint();
        }
        assert_eq!(doc.undo.len(), 16);
        assert!(doc.undo_bytes <= MAX_UNDO_BYTES);
        // The newest step is still undoable after budget eviction.
        assert!(doc.undo());
    }

    #[test]
    fn new_blend_modes_do_the_math() {
        use BlendMode::*;
        let b = BlendMode::Normal;
        assert_eq!(b.apply(10.0, 20.0), 10.0);
        assert_eq!(Darken.apply(10.0, 20.0), 10.0);
        assert_eq!(Lighten.apply(10.0, 20.0), 20.0);
        assert_eq!(Difference.apply(10.0, 20.0), 10.0);
        assert_eq!(Difference.apply(200.0, 50.0), 150.0);
        // Exclusion of identical mid-grays dips below either input.
        let e = Exclusion.apply(128.0, 128.0);
        assert!(e < 128.0 && e > 100.0, "{e}");
        // Soft light keeps mid-gray near itself, hard light pushes contrast.
        let s = SoftLight.apply(128.0, 100.0);
        assert!((s - 100.0).abs() < 12.0, "{s}");
        assert_eq!(HardLight.apply(0.0, 200.0), 0.0);
        assert_eq!(HardLight.apply(255.0, 200.0), 255.0);
        assert_eq!(BlendMode::ALL.len(), 10);
    }

    #[test]
    fn mask_multiplies_layer_alpha() {
        let mut doc = Document::new(4, 1);
        doc.background = [255, 255, 255, 255];
        doc.layers[0].pixels = px_rgba(
            &doc,
            &(vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ]),
        );
        doc.add_mask(0);
        // Conceal the left half.
        if let Some(m) = doc.layers[0].mask.as_mut() {
            m.set(0, 0);
            m.set(1, 0);
        }
        let flat = doc.flatten();
        assert_eq!(&flat[0..4], &[255, 255, 255, 255]);
        assert_eq!(&flat[8..12], &[255, 0, 0, 255]);
        // Clearing the mask reveals everything again.
        doc.clear_mask(0);
        assert!(doc.layers[0].mask.is_none());
        let flat = doc.flatten();
        assert_eq!(&flat[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn clip_confines_layer_to_content_below() {
        let mut doc = Document::new(4, 1);
        doc.background = [255, 255, 255, 255];
        // Base: single red dot at x=1.
        doc.layers[0].pixels.set(7, 255);
        doc.layers[0].pixels.set(4, 255);
        doc.add_layer("top".to_string());
        doc.layers[1].pixels = px_rgba(&doc, &([0, 255, 0, 255].repeat(4)));
        doc.layers[1].clip = true;
        let flat = doc.flatten();
        assert_eq!(&flat[0..4], &[255, 255, 255, 255]);
        assert_eq!(&flat[4..8], &[0, 255, 0, 255]);
        assert_eq!(&flat[8..12], &[255, 255, 255, 255]);
        // Unclipped, the green covers everything.
        doc.layers[1].clip = false;
        let flat = doc.flatten();
        assert_eq!(&flat[0..4], &[0, 255, 0, 255]);
    }

    #[test]
    fn mask_painting_uses_luminance_and_undoes() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(16, 16);
        doc.paint_target = PaintTarget::Mask;
        let brush = Brush {
            size: 8.0,
            softness: 0.0,
            ..Default::default()
        };
        assert!(doc.layers[0].mask.is_none());
        doc.checkpoint();
        doc.dab(&brush, Color::rgb(0, 0, 0), 8.0, 8.0);
        // Black paint concealed the middle; mask auto-materialized.
        let m = doc.layers[0].mask.as_ref().expect("mask created");
        assert!(m.to_dense().iter().any(|&b| b < 255));
        // Eraser reveals back toward white.
        let before: Vec<u8> = m.to_dense();
        let eraser = Brush {
            eraser: true,
            ..brush
        };
        doc.dab(&eraser, Color::rgb(0, 0, 0), 8.0, 8.0);
        let after = doc.layers[0].mask.as_ref().unwrap().to_dense();
        assert!(after.iter().zip(before.iter()).all(|(&a, &b)| a >= b));
        assert!(after.iter().any(|&b| b > 0));
        // Undo restores the pre-stroke state (no mask at all).
        assert!(doc.undo());
        assert!(doc.layers[0].mask.is_none());
        assert!(doc.redo());
        assert!(doc.layers[0].mask.is_some());
    }

    #[test]
    fn v6_roundtrip_keeps_masks_clips_and_blends() {
        let mut doc = Document::new(4, 2);
        doc.layers[0].blend = BlendMode::Difference;
        doc.layers[0].pixels = px_rgba(&doc, &(vec![200; 4 * 2 * 4]));
        doc.add_mask(0);
        doc.layers[0].mask.as_mut().unwrap().set(3, 0);
        doc.add_layer("clipped".to_string());
        doc.layers[1].blend = BlendMode::SoftLight;
        doc.layers[1].clip = true;
        doc.layers[1].pixels = px_rgba(&doc, &(vec![7; 4 * 2 * 4]));
        doc.layers[1].ox = -3;
        doc.layers[1].oy = 2;
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v6.pcraft");
        doc.save_project(&path).unwrap();
        let head = std::fs::read(&path).unwrap();
        assert_eq!(&head[..8], b"PCRAFT11");
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.layers[0].blend, BlendMode::Difference);
        assert_eq!(back.layers[0].mask.as_ref().unwrap().get(3), 0);
        assert_eq!(back.layers[1].blend, BlendMode::SoftLight);
        assert!(back.layers[1].clip);
        assert_eq!(back.layers[1].pixels, vec![7; 4 * 2 * 4]);
        assert_eq!((back.layers[1].ox, back.layers[1].oy), (-3, 2));
    }

    #[test]
    fn v5_files_open_with_zero_offsets() {
        // Minimal v5 file: like v4 plus clip byte, no ox/oy fields.
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v5-compat.pcraft");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT05");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[255, 255, 255, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&[6]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]); // blend: Normal
        buf.extend_from_slice(&[0]); // clip: off
        let raw = vec![9u8; 2 * 4];
        let packed = {
            use std::io::Write;
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap()
        };
        buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        buf.extend_from_slice(&packed);
        buf.extend_from_slice(&[0]); // mask flag: none
        std::fs::write(&path, &buf).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.layers[0].pixels, raw);
        assert_eq!((back.layers[0].ox, back.layers[0].oy), (0, 0));
    }

    #[test]
    fn v4_files_open_without_masks_or_clips() {
        // Minimal v4 file: zlib pixels, blend byte, no clip/mask fields.
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v4-nomask.pcraft");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT04");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[255, 255, 255, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&[6]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]);
        let raw = vec![9u8; 2 * 4];
        let packed = {
            use std::io::Write;
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(&raw).unwrap();
            enc.finish().unwrap()
        };
        buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        buf.extend_from_slice(&packed);
        std::fs::write(&path, &buf).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.layers[0].pixels, raw);
        assert!(back.layers[0].mask.is_none());
        assert!(!back.layers[0].clip);
    }

    #[test]
    fn merge_bakes_mask_and_clip() {
        let mut doc = Document::new(2, 1);
        // Below: blue, left pixel only.
        doc.layers[0].pixels = px_rgba(&doc, &(vec![0, 0, 255, 255, 0, 0, 0, 0]));
        doc.add_layer("top".to_string());
        doc.layers[1].pixels = px_rgba(&doc, &(vec![255, 0, 0, 255, 255, 0, 0, 255]));
        doc.add_mask(1);
        // Mask hides the right pixel of the top layer.
        doc.layers[1].mask.as_mut().unwrap().set(1, 0);
        doc.layers[1].clip = true;
        assert!(doc.merge_down(1));
        // Left: red over blue = red. Right: top hidden by mask AND outside
        // the blue content, so it stays transparent.
        assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
        assert_eq!(doc.layers[0].pixels.get4(4), [0, 0, 0, 0]);
    }

    #[test]
    fn transforms_carry_the_mask() {
        let mut doc = Document::new(4, 1);
        doc.add_mask(0);
        doc.layers[0].mask.as_mut().unwrap().set(0, 0);
        doc.translate_layer(0, 1, 0);
        // With offsets, mask data doesn't move; offset changes instead.
        assert_eq!(doc.layers[0].ox, 1);
        assert_eq!(doc.layers[0].mask.as_ref().unwrap().get(0), 0);
        doc.flip_layer_horizontal(0);
        // 4-pixel wide: horizontal flip mirrors mask indices 0<->3, 1<->2.
        assert_eq!(doc.layers[0].mask.as_ref().unwrap().get(3), 0);
        assert_eq!(doc.layers[0].ox, -1);
        // Affine keeps dimensions and mask presence.
        doc.affine_layer(0, 1.0, 90.0);
        assert!(doc.layers[0].mask.is_some());
        assert_eq!(doc.layers[0].mask.as_ref().unwrap().len(), 4);
    }

    #[test]
    fn new_stroke_clears_redo() {
        let mut doc = Document::new(1, 1);
        doc.checkpoint();
        doc.layers[0].pixels = px_rgba(&doc, &(vec![1, 1, 1, 1]));
        assert!(doc.undo());
        doc.checkpoint();
        doc.layers[0].pixels = px_rgba(&doc, &(vec![2, 2, 2, 2]));
        assert!(!doc.redo());
    }

    #[test]
    fn multiply_blends_channels() {
        let mut doc = Document::new(1, 1);
        doc.background = [255, 255, 255, 255];
        doc.layers[0].pixels = px_rgba(&doc, &(vec![128, 200, 64, 255]));
        doc.layers[0].blend = BlendMode::Multiply;
        let flat = doc.flatten();
        assert_eq!(&flat[0..4], &[128, 200, 64, 255]);
        // Screen over black returns the source unchanged.
        doc.background = [0, 0, 0, 255];
        doc.layers[0].blend = BlendMode::Screen;
        let flat = doc.flatten();
        assert_eq!(&flat[0..4], &[128, 200, 64, 255]);
        // Screen over mid-gray lightens past the source.
        doc.background = [128, 128, 128, 255];
        let flat = doc.flatten();
        assert!(flat[0] > 128 && flat[1] > 200 && flat[2] > 64, "{flat:?}");
    }

    #[test]
    fn project_roundtrip_keeps_everything() {
        let mut doc = Document::new(4, 3);
        doc.background = [10, 20, 30, 255];
        doc.layers[0].pixels = px_rgba(&doc, &(vec![7; 4 * 3 * 4]));
        doc.layers[0].opacity = 0.5;
        doc.layers[0].blend = BlendMode::Overlay;
        doc.symmetry = SymmetryMode::Both;
        doc.set_symmetry_center(1.5, 1.0);
        doc.add_layer("second".to_string());
        doc.layers[1].visible = false;
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-roundtrip.pcraft");
        doc.save_project(&path).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((back.w, back.h), (4, 3));
        assert_eq!(back.background, [10, 20, 30, 255]);
        assert_eq!(back.layers.len(), 2);
        assert_eq!(back.layers[0].opacity, 0.5);
        assert_eq!(back.layers[0].blend, BlendMode::Overlay);
        assert_eq!(back.layers[0].pixels, vec![7; 4 * 3 * 4]);
        assert_eq!(back.layers[1].name, "second");
        assert!(!back.layers[1].visible);
        assert_eq!(back.active, 1);
        assert_eq!(back.symmetry, SymmetryMode::Both);
        assert_eq!(back.symmetry_center, Some((1.5, 1.0)));
        assert!(Document::open_project(&dir.join("missing.pcraft")).is_err());
    }

    #[test]
    fn v1_project_still_opens_with_symmetry_off() {
        // Hand-build a minimal version-1 file (no symmetry byte).
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v1-compat.pcraft");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT01");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[255, 255, 255, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&[9u8; 2 * 4]);
        std::fs::write(&path, &buf).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((back.w, back.h), (2, 1));
        assert_eq!(back.symmetry, SymmetryMode::Off);
        assert_eq!(back.layers[0].pixels, vec![9u8; 8]);
    }

    #[test]
    fn moved_mirror_center_shifts_paint() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(21, 21);
        doc.symmetry = SymmetryMode::Horizontal;
        // Default middle: x=2 mirrors to x=18.
        assert!(SymmetryMode::Horizontal
            .mirrored_pixels(21, 21, 6, 10.0, 10.0, 2, 5)
            .contains(&(18, 5)));
        // Moved center x=5: x=2 mirrors to x=8 instead.
        doc.set_symmetry_center(5.0, 10.0);
        assert_eq!(doc.symmetry_center_px(), (5.0, 10.0));
        let brush = Brush {
            size: 3.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(255, 0, 0), 2.5, 5.5);
        let at =
            |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 21 + x) * 4 + 3) as usize);
        assert!(at(&doc, 2, 5) > 0);
        assert!(at(&doc, 8, 5) > 0);
        assert_eq!(at(&doc, 18, 5), 0);
        // Reset restores the middle; out-of-range values clamp.
        doc.reset_symmetry_center();
        assert_eq!(doc.symmetry_center, None);
        doc.set_symmetry_center(1e9, -1e9);
        assert_eq!(doc.symmetry_center, Some((16384.0, -16384.0)));
    }

    #[test]
    fn v6_project_opens_with_centered_symmetry() {
        // Hand-build a minimal version-6 file (no center bytes).
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v6-compat.pcraft");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT06");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[255, 255, 255, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[1]); // Horizontal.
        buf.extend_from_slice(&[6]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&0i32.to_le_bytes());
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&[9u8; 2 * 4]).unwrap();
        let packed = enc.finish().unwrap();
        buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        buf.extend_from_slice(&packed);
        buf.extend_from_slice(&[0]); // No mask.
        std::fs::write(&path, &buf).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.symmetry, SymmetryMode::Horizontal);
        assert_eq!(back.symmetry_center, None);
        assert_eq!(back.symmetry_center_px(), (0.5, 0.0));
        assert_eq!(back.layers[0].pixels, vec![9u8; 8]);
    }

    #[test]
    fn flip_horizontal_mirrors_pixels() {
        let mut doc = Document::new(4, 1);
        doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
        doc.flip_layer_horizontal(0);
        assert_eq!(doc.layers[0].pixels.get4(12), [255, 0, 0, 255]);
        assert_eq!(doc.layers[0].pixels.get4(0), [0, 0, 0, 0]);
        // Double flip restores.
        doc.flip_layer_horizontal(0);
        assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
    }

    #[test]
    fn flip_vertical_mirrors_pixels() {
        let mut doc = Document::new(1, 4);
        doc.layers[0].pixels.set4(0, [0, 255, 0, 255]);
        doc.flip_layer_vertical(0);
        assert_eq!(doc.layers[0].pixels.get4(12), [0, 255, 0, 255]);
        doc.flip_layer_vertical(0);
        assert_eq!(doc.layers[0].pixels.get4(0), [0, 255, 0, 255]);
    }

    #[test]
    fn translate_shifts_pixels() {
        let mut doc = Document::new(4, 1);
        doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
        doc.translate_layer(0, 2, 0);
        // With offsets, pixels don't move; offset changes instead.
        assert_eq!(doc.layers[0].ox, 2);
        assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
    }

    #[test]
    fn move_off_canvas_and_back_restores() {
        // The reported Move bug: content pushed fully off-canvas vanished
        // on return because translate cropped destructively. Offsets keep
        // every pixel, so the round trip is exact.
        let mut doc = Document::new(8, 4);
        doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
        doc.layers[0].pixels.set4(4, [0, 255, 0, 255]);
        let want = doc.layers[0].pixels.clone();
        // Paint shows at the origin.
        assert_eq!(&doc.flatten()[0..4], &[255, 0, 0, 255]);
        // Fully off-canvas: composite shows only the page, buffer intact.
        doc.translate_layer(0, 100, 0);
        assert!(doc
            .flatten()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p == &[255, 255, 255, 255]));
        assert_eq!(doc.layers[0].pixels, want);
        // Back home: pixel-perfect, buffer untouched throughout.
        doc.translate_layer(0, -100, 0);
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
        assert_eq!(doc.layers[0].pixels, want);
        assert_eq!(&doc.flatten()[0..4], &[255, 0, 0, 255]);
        assert_eq!(&doc.flatten()[4..8], &[0, 255, 0, 255]);
        // Partially off-canvas: visible part composites, rest survives.
        doc.translate_layer(0, -1, 0);
        assert_eq!(&doc.flatten()[0..4], &[0, 255, 0, 255]);
        doc.translate_layer(0, 1, 0);
        assert_eq!(&doc.flatten()[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn move_undo_redo_restores_offset() {
        let mut doc = Document::new(8, 8);
        doc.checkpoint();
        doc.translate_layer(0, 30, -12);
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (30, -12));
        assert!(doc.undo());
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
        assert!(doc.redo());
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (30, -12));
    }

    #[test]
    fn paint_after_move_lands_shifted() {
        // Brush input is canvas-space; storage follows the layer offset,
        // so paint appears under the cursor on moved layers.
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(32, 32);
        doc.translate_layer(0, 5, 0);
        let brush = Brush {
            size: 3.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(255, 0, 0), 10.5, 10.5);
        // Composite shows paint at the canvas dab point...
        let at = |d: &Document, x: u32, y: u32| {
            let o = ((y * 32 + x) * 4) as usize;
            [
                d.flatten()[o],
                d.flatten()[o + 1],
                d.flatten()[o + 2],
                d.flatten()[o + 3],
            ]
        };
        assert_eq!(at(&doc, 10, 10), [255, 0, 0, 255]);
        // ...stored shifted in the buffer (local 5, 10).
        let o = ((10 * 32 + 5) * 4) as usize;
        assert_eq!(doc.layers[0].pixels.get4(o), [255, 0, 0, 255]);
        // A far-moved layer still paints the page: the buffer slides
        // home (only invisible tails trim) instead of refusing the dab.
        doc.translate_layer(0, 1000, 0);
        doc.dab(&brush, Color::rgb(0, 255, 0), 10.5, 10.5);
        assert_eq!(at(&doc, 10, 10), [0, 255, 0, 255]);
    }

    #[test]
    fn merge_with_offsets_bakes_positions() {
        // dst blue at canvas 0; src red lives at buffer 0 but ox=1.
        let mut doc = Document::new(3, 1);
        doc.layers[0].pixels.set4(0, [0, 0, 255, 255]);
        doc.add_layer("top".to_string());
        doc.layers[1].pixels.set4(0, [255, 0, 0, 255]);
        doc.layers[1].ox = 1;
        assert!(doc.merge_down(1));
        // Red baked at canvas 1 (buffer 1), blue kept at canvas 0.
        assert_eq!(doc.layers[0].pixels.get4(0), [0, 0, 255, 255]);
        assert_eq!(doc.layers[0].pixels.get4(4), [255, 0, 0, 255]);
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
    }

    #[test]
    fn flip_mirrors_offset_about_center() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].ox = 3;
        doc.layers[0].oy = -2;
        doc.flip_layer_horizontal(0);
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (-3, -2));
        doc.flip_layer_vertical(0);
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (-3, 2));
    }

    #[test]
    fn mask_view_follows_offset() {
        let mut doc = Document::new(4, 1);
        doc.add_mask(0);
        doc.layers[0].mask.as_mut().unwrap().set(0, 200);
        // Unshifted: viewer shows the value at canvas 0.
        assert_eq!(&doc.mask_view().unwrap()[0..4], &[200, 200, 200, 255]);
        // Shifted right: canvas 0 reads concealed, canvas 1 shows it.
        doc.translate_layer(0, 1, 0);
        let view = doc.mask_view().unwrap();
        assert_eq!(&view[0..4], &[0, 0, 0, 255]);
        assert_eq!(&view[4..8], &[200, 200, 200, 255]);
    }

    #[test]
    fn bilinear_edges_keep_their_colour_over_transparency() {
        // One opaque red pixel beside transparent black. A straight-RGBA
        // blend would give dark red (128, 0, 0, 128) at the midpoint; the
        // premultiplied blend keeps the hue and only fades coverage.
        let mut px = vec![0u8; 2 * 2 * 4];
        px[0..4].copy_from_slice(&[255, 0, 0, 255]);
        let s = Document::sample_bilinear(&px, 2, 2, 0.5, 0.0);
        assert_eq!(&s[..3], &[255, 0, 0], "sample was {s:?}");
        assert!((s[3] as i32 - 128).abs() <= 1, "alpha was {}", s[3]);
        // An exact hit still returns the pixel bit-for-bit.
        assert_eq!(
            Document::sample_bilinear(&px, 2, 2, 0.0, 0.0),
            [255, 0, 0, 255]
        );
        // Transparent neighbourhoods stay fully transparent, never grey.
        assert_eq!(Document::sample_bilinear(&px, 2, 2, 1.0, 1.0), [0, 0, 0, 0]);
    }

    #[test]
    fn affine_identity_keeps_pixels() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.fill(100);
        let before = doc.layers[0].pixels.clone();
        doc.affine_layer(0, 1.0, 0.0);
        assert_eq!(doc.layers[0].pixels, before);
        // 2x zoom of solid color stays solid.
        doc.affine_layer(0, 2.0, 0.0);
        assert!(doc.layers[0]
            .pixels
            .to_dense()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[0] == 100 && p[3] == 100));
    }

    fn opaque_bbox_center(doc: &Document) -> (f32, f32) {
        let (w, h) = (doc.w, doc.h);
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if doc.layers[0].pixels.get(((y * w + x) * 4 + 3) as usize) != 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        ((x0 + x1 + 1) as f32 * 0.5, (y0 + y1 + 1) as f32 * 0.5)
    }

    #[test]
    fn affine_spins_and_grows_about_paint_center() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(64, 64);
        let brush = Brush {
            size: 30.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        // Off-center hard rect: the old canvas-center pivot would orbit
        // it ~6px per 15° click; rotating in place only turns corners.
        doc.fill_rect(&brush, Color::rgb(255, 255, 255), 12, 12, 19, 19);
        let before = opaque_bbox_center(&doc);
        assert!((before.0 - 16.0).abs() < 1.0 && (before.1 - 16.0).abs() < 1.0);
        let painted = doc.layers[0].pixels.clone();
        doc.affine_layer(0, 1.0, 15.0);
        // Rotation applied (corners moved) but the paint stayed put.
        assert_ne!(doc.layers[0].pixels, painted);
        let after = opaque_bbox_center(&doc);
        assert!(
            (after.0 - before.0).abs() <= 2.0 && (after.1 - before.1).abs() <= 2.0,
            "before={before:?} after={after:?}"
        );
        // Scale grows about the same center.
        doc.affine_layer(0, 1.25, 0.0);
        let grown = opaque_bbox_center(&doc);
        assert!(
            (grown.0 - before.0).abs() <= 3.0 && (grown.1 - before.1).abs() <= 3.0,
            "before={before:?} grown={grown:?}"
        );
        // Offsets ride along untouched (no rounding damage).
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
    }

    #[test]
    fn affine_quarter_turns_are_lossless() {
        // Pivot-stable block: distinct values per pixel catch any smear,
        // and the centered bbox keeps the pivot fixed across turns.
        let mut doc = Document::new(16, 16);
        for y in 4..12u32 {
            for x in 4..12u32 {
                let o = ((y * 16 + x) * 4) as usize;
                doc.layers[0].pixels.set(o, (x * 16 + y) as u8);
                doc.layers[0].pixels.set(o + 1, 200);
                doc.layers[0].pixels.set(o + 2, 100);
                doc.layers[0].pixels.set(o + 3, 255);
            }
        }
        let before = doc.layers[0].pixels.clone();
        let count: usize = before.to_dense().chunks(4).filter(|p| p[3] != 0).count();
        doc.affine_layer(0, 1.0, 90.0);
        assert_eq!(
            doc.layers[0]
                .pixels
                .to_dense()
                .chunks(4)
                .filter(|p| p[3] != 0)
                .count(),
            count
        );
        // Four quarter-turns return every byte exactly.
        for _ in 0..3 {
            doc.affine_layer(0, 1.0, 90.0);
        }
        assert_eq!(doc.layers[0].pixels, before);
    }

    #[test]
    fn distort_identity_quad_is_noop() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.fill(100);
        let before = doc.layers[0].pixels.clone();
        doc.distort_layer(0, [(0.0, 0.0), (7.0, 0.0), (7.0, 7.0), (0.0, 7.0)]);
        assert_eq!(doc.layers[0].pixels, before);
    }

    #[test]
    fn distort_pin_moves_paint_and_mask() {
        // One opaque pixel at the top-left; pin that corner to the
        // canvas center. Dest (0,0) now samples the empty center, and
        // the mask travels with the paint.
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
        let mut m = vec![0u8; 64];
        m[0] = 255;
        doc.layers[0].mask = Some(px_mask(&doc, &(m)));
        doc.distort_layer(0, [(4.0, 4.0), (7.0, 0.0), (7.0, 7.0), (0.0, 7.0)]);
        assert_eq!(doc.layers[0].pixels.get4(0), [0, 0, 0, 0]);
        assert_eq!(doc.layers[0].mask.as_ref().unwrap().get(0), 0);
        // Bad index is a silent no-op, never a panic.
        let before = doc.layers[0].pixels.clone();
        doc.distort_layer(9, [(0.0, 0.0), (7.0, 0.0), (7.0, 7.0), (0.0, 7.0)]);
        assert_eq!(doc.layers[0].pixels, before);
    }

    #[test]
    fn distort_degenerate_canvas_is_noop() {
        let mut doc = Document::new(1, 1);
        doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
        doc.distort_layer(0, [(0.0, 0.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)]);
        assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
    }

    fn identity_lattice(w: f32, h: f32, cols: u32, rows: u32) -> Vec<(f32, f32)> {
        let mut grid = Vec::new();
        for j in 0..=rows {
            for i in 0..=cols {
                grid.push((i as f32 * w / cols as f32, j as f32 * h / rows as f32));
            }
        }
        grid
    }

    #[test]
    fn warp_identity_lattice_is_noop() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.fill(100);
        let before = doc.layers[0].pixels.clone();
        let grid = identity_lattice(8.0, 8.0, 2, 2);
        doc.warp_layer(0, 2, 2, &grid);
        assert_eq!(doc.layers[0].pixels, before);
    }

    #[test]
    fn warp_pull_moves_paint_and_mask() {
        // One opaque pixel at the top-left; drag the lattice center to
        // the origin. Dest (4,4) sits exactly on lattice point (1,1),
        // so it samples the painted texel; the mask travels with it.
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
        let mut m = vec![0u8; 64];
        m[0] = 255;
        doc.layers[0].mask = Some(px_mask(&doc, &(m)));
        let mut grid = identity_lattice(8.0, 8.0, 2, 2);
        // Row-major (row 1, col 1) of the 3-wide lattice.
        grid[4] = (0.0, 0.0);
        doc.warp_layer(0, 2, 2, &grid);
        let o = ((4 * 8 + 4) * 4) as usize;
        assert_eq!(doc.layers[0].pixels.get4(o), [255, 0, 0, 255]);
        assert_eq!(
            doc.layers[0]
                .mask
                .as_ref()
                .unwrap()
                .get((4 * 8 + 4) as usize),
            255
        );
        // Bad grid (wrong point count) is a silent no-op.
        let before = doc.layers[0].pixels.clone();
        doc.warp_layer(0, 2, 2, &[(0.0, 0.0)]);
        assert_eq!(doc.layers[0].pixels, before);
    }

    #[test]
    fn warp_degenerate_canvas_is_noop() {
        let mut doc = Document::new(1, 1);
        doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
        doc.warp_layer(0, 1, 1, &[(0.0, 0.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)]);
        assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
    }

    fn tri_pins(w: f32, h: f32, center: (f32, f32)) -> Vec<(f32, f32)> {
        vec![
            (0.0, 0.0),
            (w - 1.0, 0.0),
            (w - 1.0, h - 1.0),
            (0.0, h - 1.0),
            center,
        ]
    }

    const TRI_FAN: [[u32; 3]; 4] = [[0, 1, 4], [1, 2, 4], [2, 3, 4], [3, 0, 4]];

    #[test]
    fn tri_identity_pins_are_noop() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.fill(100);
        let before = doc.layers[0].pixels.clone();
        let src = tri_pins(8.0, 8.0, (4.0, 4.0));
        doc.warp_layer_tris(0, &src, &src, &TRI_FAN);
        assert_eq!(doc.layers[0].pixels, before);
    }

    #[test]
    fn tri_pin_pull_moves_paint_and_mask() {
        // Paint sits exactly under the center pin; drag the pin to (2,2).
        // The destination point (2,2) is the moved pin itself, so it
        // samples the paint's original spot and the mask travels with it.
        let mut doc = Document::new(8, 8);
        let o = ((4 * 8 + 4) * 4) as usize;
        doc.layers[0].pixels.set4(o, [255, 0, 0, 255]);
        let mut m = vec![0u8; 64];
        m[(4 * 8 + 4) as usize] = 255;
        doc.layers[0].mask = Some(px_mask(&doc, &(m)));
        let src = tri_pins(8.0, 8.0, (4.0, 4.0));
        let dst = tri_pins(8.0, 8.0, (2.0, 2.0));
        doc.warp_layer_tris(0, &src, &dst, &TRI_FAN);
        let d = ((2 * 8 + 2) * 4) as usize;
        assert_eq!(doc.layers[0].pixels.get4(d), [255, 0, 0, 255]);
        assert_eq!(
            doc.layers[0]
                .mask
                .as_ref()
                .unwrap()
                .get((2 * 8 + 2) as usize),
            255
        );
        assert_eq!(doc.layers[0].pixels.get(o + 3), 0);
    }

    #[test]
    fn tri_corner_pins_stay_fixed_while_center_moves() {
        // The canvas corners are pins too: content near an unmoved
        // corner keeps sampling itself, so a center pull does not drag
        // the whole layer.
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
        let src = tri_pins(8.0, 8.0, (4.0, 4.0));
        let dst = tri_pins(8.0, 8.0, (2.0, 2.0));
        doc.warp_layer_tris(0, &src, &dst, &TRI_FAN);
        assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
    }

    #[test]
    fn tri_bad_input_is_noop() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.fill(100);
        let before = doc.layers[0].pixels.clone();
        let src = tri_pins(8.0, 8.0, (4.0, 4.0));
        doc.warp_layer_tris(9, &src, &src, &TRI_FAN);
        doc.warp_layer_tris(0, &src, &src[..3], &TRI_FAN);
        doc.warp_layer_tris(0, &src, &src, &[[0, 1, 99]]);
        doc.warp_layer_tris(0, &src, &src, &[]);
        let mut flat = Document::new(1, 1);
        flat.layers[0].pixels.set4(0, [255, 0, 0, 255]);
        flat.warp_layer_tris(0, &src, &src, &TRI_FAN);
        assert_eq!(doc.layers[0].pixels, before);
        assert_eq!(flat.layers[0].pixels.get4(0), [255, 0, 0, 255]);
    }

    #[test]
    fn content_bounds_covers_the_painted_pixels() {
        let mut doc = Document::new(8, 8);
        assert_eq!(doc.content_bounds(0), None);
        doc.layers[0]
            .pixels
            .set(((3 * 8 + 2) * 4 + 3) as usize, 255);
        doc.layers[0]
            .pixels
            .set(((5 * 8 + 6) * 4 + 3) as usize, 255);
        assert_eq!(doc.content_bounds(0), Some((2.0, 3.0, 6.0, 5.0)));
        assert_eq!(doc.content_bounds(9), None);
    }

    #[test]
    fn tone_brightness_contrast_and_alpha() {
        let mut doc = Document::new(2, 1);
        doc.layers[0].pixels = px_rgba(&doc, &(vec![100, 100, 100, 200, 100, 100, 100, 200]));
        doc.adjust_tone(0, 10, 1.0);
        assert_eq!(doc.layers[0].pixels.get4(0), [110, 110, 110, 200]);
        // Zero contrast flattens to mid-gray; alpha still untouched.
        doc.adjust_tone(0, 0, 0.0);
        assert_eq!(doc.layers[0].pixels.get4(0), [128, 128, 128, 200]);
        // Identity is a no-op.
        let before = doc.layers[0].pixels.clone();
        doc.adjust_tone(0, 0, 1.0);
        assert_eq!(doc.layers[0].pixels, before);
    }

    #[test]
    fn blur_spreads_dot_keeps_flat() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.fill(77);
        let before = doc.layers[0].pixels.clone();
        doc.blur_layer(0, 3);
        assert_eq!(doc.layers[0].pixels, before);
        // Single opaque dot on transparency spreads out.
        let mut doc = Document::new(9, 9);
        doc.layers[0]
            .pixels
            .set(((4 * 9 + 4) * 4 + 3) as usize, 255);
        doc.layers[0].pixels.set(((4 * 9 + 4) * 4) as usize, 255);
        doc.blur_layer(0, 2);
        let at =
            |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 9 + x) * 4 + 3) as usize);
        assert!(at(&doc, 4, 4) < 255);
        assert!(at(&doc, 5, 4) > 0);
        assert_eq!(at(&doc, 0, 0), 0);
    }

    #[test]
    fn sharpen_boosts_edge_contrast() {
        // Left half 100, right half 200.
        let mut doc = Document::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                let v = if x < 4 { 100u8 } else { 200u8 };
                let o = ((y * 8 + x) * 4) as usize;
                doc.layers[0].pixels.set(o, v);
                doc.layers[0].pixels.set(o + 1, v);
                doc.layers[0].pixels.set(o + 2, v);
                doc.layers[0].pixels.set(o + 3, 255);
            }
        }
        doc.sharpen_layer(0, 1.0);
        let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 8 + x) * 4) as usize);
        assert!(at(&doc, 3, 4) < 100, "dark edge pixel dips");
        assert!(
            at(&doc, 4, 4) > 200 || at(&doc, 4, 4) == 255,
            "bright edge pixel rises"
        );
        // Flat area is untouched by sharpening.
        assert_eq!(at(&doc, 0, 0), 100);
        assert_eq!(at(&doc, 7, 7), 200);
    }

    #[test]
    fn filters_respect_selection() {
        let mut doc = Document::new(6, 6);
        doc.layers[0].pixels.fill(255);
        doc.selection = Some((0, 0, 2, 2));
        doc.adjust_tone(0, -50, 1.0);
        assert_eq!(doc.layers[0].pixels.get(0), 205);
        let o = ((5 * 6 + 5) * 4) as usize;
        assert_eq!(doc.layers[0].pixels.get(o), 255);
        doc.blur_layer(0, 2);
        assert_eq!(doc.layers[0].pixels.get(o), 255);
        doc.clear_selection();
        assert!(doc.selection.is_none() && doc.selection_mask.is_none());
    }

    #[test]
    fn kaleidoscope_segments_clamp() {
        let at = |m: SymmetryMode, seg: u8| m.mirrored_pixels(21, 21, seg, 10.0, 10.0, 5, 5).len();
        assert_eq!(at(SymmetryMode::Kaleidoscope, 6), 6);
        assert_eq!(at(SymmetryMode::Kaleidoscope, 1), 2);
        assert_eq!(at(SymmetryMode::Kaleidoscope, 99), 16);
        assert_eq!(at(SymmetryMode::Both, 9), 4);
    }

    #[test]
    fn kaleidoscope_paints_n_copies() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(21, 21);
        doc.symmetry = SymmetryMode::Kaleidoscope;
        doc.symmetry_segments = 4;
        let brush = Brush {
            size: 3.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(255, 0, 0), 14.5, 10.5);
        let at =
            |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 21 + x) * 4 + 3) as usize);
        for (x, y) in [(14, 10), (10, 14), (6, 10), (10, 6)] {
            assert!(at(&doc, x, y) > 0, "missing dab at {x},{y}");
        }
        assert_eq!(at(&doc, 0, 0), 0);
    }

    #[test]
    fn roundtrip_keeps_kaleidoscope() {
        let mut doc = Document::new(4, 3);
        doc.symmetry = SymmetryMode::Kaleidoscope;
        doc.symmetry_segments = 9;
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-kaleido.pcraft");
        doc.save_project(&path).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.symmetry, SymmetryMode::Kaleidoscope);
        assert_eq!(back.symmetry_segments, 9);
    }

    #[test]
    fn v2_project_opens_with_default_segments() {
        // Minimal version-2 file: one symmetry byte, no segments byte.
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v2-compat.pcraft");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT02");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[255, 255, 255, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[1]); // Horizontal.
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&[5u8; 2 * 4]);
        std::fs::write(&path, &buf).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.symmetry, SymmetryMode::Horizontal);
        assert_eq!(back.symmetry_segments, 6);
    }

    #[test]
    fn merge_down_composites_and_removes() {
        let mut doc = Document::new(2, 1);
        doc.layers[0].pixels = px_rgba(&doc, &(vec![0, 0, 255, 255, 0, 0, 255, 255]));
        doc.add_layer("top".to_string());
        doc.layers[1].pixels = px_rgba(&doc, &(vec![255, 0, 0, 255, 0, 0, 0, 0]));
        assert!(doc.merge_down(1));
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.active, 0);
        assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
        assert_eq!(doc.layers[0].pixels.get4(4), [0, 0, 255, 255]);
        // Bottom layer refuses.
        assert!(!doc.merge_down(0));
        // Invisible top contributes nothing but still merges away.
        doc.add_layer("ghost".to_string());
        doc.layers[1].visible = false;
        doc.layers[1].pixels = px_rgba(&doc, &(vec![0, 255, 0, 255, 0, 255, 0, 255]));
        assert!(doc.merge_down(1));
        assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
    }

    #[test]
    fn duplicate_copies_layer() {
        let mut doc = Document::new(2, 1);
        doc.layers[0].pixels = px_rgba(&doc, &(vec![7; 8]));
        doc.duplicate_layer(0);
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(doc.active, 1);
        assert_eq!(doc.layers[1].name, "Layer 1 copy");
        assert_eq!(doc.layers[1].pixels, vec![7; 8]);
        doc.duplicate_layer(9);
        assert_eq!(doc.layers.len(), 2);
    }

    #[test]
    fn sample_reads_composite() {
        let mut doc = Document::new(2, 1);
        doc.layers[0].pixels = px_rgba(&doc, &(vec![10, 20, 30, 255, 0, 0, 0, 0]));
        assert_eq!(doc.sample(0, 0), Some([10, 20, 30]));
        assert_eq!(doc.sample(9, 9), None);
    }

    #[test]
    fn lasso_selects_interior_only() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(20, 20);
        let pts = vec![(0.0, 0.0), (12.0, 0.0), (0.0, 12.0), (0.0, 0.0)];
        let bbox = doc.set_lasso(&pts).expect("lasso builds");
        assert!(doc.pixel_selected(2, 2));
        assert!(!doc.pixel_selected(15, 15));
        assert_eq!(doc.selection, Some(bbox));
        assert!(doc.selection_pixel_count() > 0);
        // Painting is constrained to the lasso.
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        doc.fill_rect(&brush, Color::rgb(9, 9, 9), 0, 0, 19, 19);
        let inside = ((2 * 20 + 2) * 4) as usize;
        assert_eq!(doc.layers[0].pixels.get(inside + 3), 255);
        let outside = ((15 * 20 + 15) * 4) as usize;
        assert_eq!(doc.layers[0].pixels.get(outside + 3), 0);
        // Degenerate loops clear.
        assert!(doc.set_lasso(&[]).is_none());
        assert!(doc.selection_mask.is_none());
        doc.clear_selection();
        assert!(doc.selection.is_none() && doc.selection_mask.is_none());
        assert_eq!(doc.selection_pixel_count(), 0);
    }

    #[test]
    fn shape_fill_respects_selection() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(10, 10);
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        doc.selection = Some((2, 2, 5, 5));
        doc.fill_rect(&brush, Color::rgb(1, 2, 3), 0, 0, 9, 9);
        assert_eq!(doc.layers[0].pixels.get4(0), [0, 0, 0, 0]);
        let o = ((3 * 10 + 3) * 4) as usize;
        assert_eq!(doc.layers[0].pixels.get(o + 3), 255);
    }

    #[test]
    fn v3_project_still_opens_raw() {
        // Hand-build a minimal version-3 file (raw pixels, symmetry + segments).
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v3-compat.pcraft");
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT03");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[1, 2, 3, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[3]); // Both.
        buf.extend_from_slice(&[9]); // Segments.
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&[6u8; 2 * 4]);
        std::fs::write(&path, &buf).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.symmetry, SymmetryMode::Both);
        assert_eq!(back.symmetry_segments, 9);
        assert_eq!(back.layers[0].pixels, vec![6u8; 8]);
    }

    #[test]
    fn v4_roundtrip_compresses_sparse_canvas() {
        // Mostly-transparent canvas must shrink far below raw size.
        let doc = Document::new(128, 128);
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v4-size.pcraft");
        doc.save_project(&path).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let raw = 128usize * 128 * 4;
        assert!(
            meta.len() as usize * 4 < raw,
            "compressed {} vs raw {raw}",
            meta.len()
        );
        let head = std::fs::read(&path).unwrap();
        assert_eq!(&head[..8], b"PCRAFT11");
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.layers[0].pixels, doc.layers[0].pixels);
    }

    #[test]
    fn v4_noisy_pixels_still_roundtrip() {
        // Incompressible noise must survive the zlib round trip exactly.
        let mut doc = Document::new(32, 32);
        let mut x: u32 = 0x12345678;
        let bytes: Vec<u8> = (0..32 * 32 * 4)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                (x >> 24) as u8
            })
            .collect();
        doc.layers[0].pixels = Raster::from_dense(32, 32, 4, &bytes);
        let want = doc.layers[0].pixels.clone();
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v4-noise.pcraft");
        doc.save_project(&path).unwrap();
        let back = Document::open_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.layers[0].pixels, want);
    }

    #[test]
    fn v4_corrupt_pixels_rejected() {
        let doc = Document::new(8, 8);
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-v4-corrupt.pcraft");
        doc.save_project(&path).unwrap();
        let mut buf = std::fs::read(&path).unwrap();
        // Flip bytes inside the compressed payload (last 8 bytes).
        let n = buf.len();
        for b in buf[n - 8..].iter_mut() {
            *b ^= 0xFF;
        }
        std::fs::write(&path, &buf).unwrap();
        assert!(Document::open_project(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn m45_import_rejects_oversize_dimensions() {
        // M45: a phone panorama wider than the canvas cap must fail
        // closed instead of adopting an oversize layer (OOM guard).
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-m45-oversize.png");
        let wide = MAX_CANVAS_SIDE + 8;
        let tall = 100u32;
        let raw = vec![200u8; (wide * tall * 4) as usize];
        let mut out = Vec::new();
        {
            use image::ImageEncoder;
            let enc = image::codecs::png::PngEncoder::new(&mut out);
            enc.write_image(&raw, wide, tall, image::ExtendedColorType::Rgba8)
                .unwrap();
        }
        std::fs::write(&path, &out).unwrap();
        let mut doc = Document::new(8, 8);
        let err = doc.load_background(&path).unwrap_err();
        assert!(err.contains("too large"), "unexpected: {err}");
        // Failed import touches nothing.
        assert_eq!((doc.w, doc.h), (8, 8));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn m45_open_project_rejects_huge_file() {
        // M45: a 200MB+ file must fail closed on metadata alone,
        // never read_to_end into memory.
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-m45-huge.pcraft");
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(200 * 1024 * 1024 + 1).unwrap();
        drop(f);
        let err = Document::open_project(&path).unwrap_err();
        assert!(err.contains("too large"), "unexpected: {err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn m45_project_fuzz_truncations_and_flips_never_panic() {
        // M45: truncations fail closed (Err); sampled bit-flips never
        // panic (Ok allowed — flipping a background byte is still a
        // valid file, just a different color).
        let doc = Document::new(8, 8);
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-m45-fuzz.pcraft");
        doc.save_project(&path).unwrap();
        let full = std::fs::read(&path).unwrap();
        // Truncations: empty, 1 byte, mid, and len-1 all fail closed.
        for n in [0usize, 1, 8, full.len() / 2, full.len() - 1] {
            std::fs::write(&path, &full[..n]).unwrap();
            assert!(Document::open_project(&path).is_err(), "trunc {n} opened");
        }
        // Bit-flips: magic flips must fail closed; every flip must not
        // panic (the return value is dropped — a panic fails the test).
        for i in [0usize, 4, 8, 12, 32, 64] {
            if i >= full.len() {
                continue;
            }
            let mut bad = full.clone();
            bad[i] ^= 0xFF;
            std::fs::write(&path, &bad).unwrap();
            let _ = Document::open_project(&path);
        }
        // Magic-byte flips specifically are never valid files.
        for i in [0usize, 4] {
            let mut bad = full.clone();
            bad[i] ^= 0xFF;
            std::fs::write(&path, &bad).unwrap();
            assert!(Document::open_project(&path).is_err(), "flip {i} opened");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn strokes_fully_outside_canvas_are_safe() {
        use crate::brush::{Brush, BrushTip, Color};
        // Regression: a dab fully right/below the canvas used to record an
        // inverted dirty rect, and the texture upload's `x1 - x0 + 1` then
        // underflowed and quit the app.
        let mut doc = Document::new(32, 32);
        let brush = Brush {
            size: 8.0,
            ..Default::default()
        };
        let clean = doc.layers[0].pixels.clone();
        for (from, to) in [
            (Some((100.0, 100.0)), (400.0, 400.0)),   // right-bottom
            (Some((-400.0, 100.0)), (-100.0, 100.0)), // left
            (Some((100.0, -400.0)), (100.0, -100.0)), // above
            (None, (1000.0, 1000.0)),                 // single dab far outside
        ] {
            doc.stroke_to(&brush, Color::rgb(255, 0, 0), from, to, 0.0);
        }
        assert_eq!(doc.layers[0].pixels, clean);
        // No poisoned region may survive: consumers subtract safely.
        if let Some((x0, y0, x1, y1)) = doc.take_dirty() {
            assert!(x0 <= x1 && y0 <= y1);
            let bytes = doc.flatten_region(x0, y0, x1, y1);
            let w = (x1 - x0 + 1) as usize;
            let h = (y1 - y0 + 1) as usize;
            assert_eq!(bytes.len(), w * h * 4);
        }
        // Same for stamp tips and mask painting.
        let tip = BrushTip::from_mask(2, 2, vec![255; 4]).unwrap();
        let stamp = Brush {
            size: 16.0,
            tip: Some(tip),
            ..Brush::default()
        };
        doc.stroke_to(
            &stamp,
            Color::rgb(1, 2, 3),
            Some((200.0, 200.0)),
            (500.0, 500.0),
            0.0,
        );
        assert_eq!(doc.layers[0].pixels, clean);
        doc.paint_target = PaintTarget::Mask;
        doc.stroke_to(
            &brush,
            Color::rgb(0, 0, 0),
            Some((200.0, 200.0)),
            (500.0, 500.0),
            0.0,
        );
        assert!(doc.layers[0].mask.is_none());
        doc.paint_target = PaintTarget::Color;
    }

    fn selected_rect(w: u32, h: u32, r: (u32, u32, u32, u32)) -> Document {
        let mut doc = Document::new(w, h);
        doc.selection = Some(r);
        doc
    }

    #[test]
    fn lasso_writes_full_strength_masks() {
        let mut doc = Document::new(10, 10);
        doc.set_lasso(&[(2.0, 2.0), (2.0, 5.0), (5.0, 5.0), (5.0, 2.0)]);
        let mask = doc.selection_mask.clone().unwrap();
        assert!(mask.iter().all(|&b| b == 0 || b == 255));
        assert_eq!(
            doc.selection_pixel_count(),
            mask.iter().filter(|&&b| b != 0).count()
        );
    }

    #[test]
    fn feather_ramps_and_expands_bounds() {
        let mut doc = selected_rect(20, 20, (5, 5, 14, 14));
        assert!(doc.feather_selection(3));
        let m = doc.selection_mask.clone().unwrap();
        let at = |x: u32, y: u32| m[(y * 20 + x) as usize];
        // Deep interior stays full, far exterior stays empty.
        assert_eq!(at(10, 10), 255);
        assert_eq!(at(0, 0), 0);
        // The old hard edge is now a ramp (partial coverage just outside).
        let ramp = at(3, 10);
        assert!(ramp > 0 && ramp < 255, "{ramp}");
        // Bounds grew to contain the falloff; coverage is conserved by blur.
        assert_eq!(doc.selection, Some((2, 2, 17, 17)));
        let count = doc.selection_pixel_count() as i32;
        assert!((count - 100).abs() <= 2, "{count}");
        // Radius 0 only normalizes, and empty selections report false.
        let mut doc2 = selected_rect(20, 20, (5, 5, 14, 14));
        assert!(doc2.feather_selection(0));
        assert_eq!(doc2.selection_pixel_count(), 100);
        let mut doc3 = Document::new(8, 8);
        assert!(!doc3.feather_selection(4));
    }

    #[test]
    fn grow_and_shrink_track_bounds() {
        let mut doc = selected_rect(20, 20, (5, 5, 14, 14));
        assert!(doc.grow_selection(2));
        assert_eq!(doc.selection, Some((3, 3, 16, 16)));
        assert!(doc.selection_pixel_count() > 100);
        assert!(doc.grow_selection(-2));
        assert_eq!(doc.selection, Some((5, 5, 14, 14)));
        assert_eq!(doc.selection_pixel_count(), 100);
        // Shrinking to nothing clears everything.
        assert!(!doc.grow_selection(-50));
        assert!(doc.selection.is_none() && doc.selection_mask.is_none());
        // Zero on nothing stays nothing.
        assert!(!doc.grow_selection(0));
        let mut doc2 = Document::new(8, 8);
        assert!(!doc2.grow_selection(3));
    }

    #[test]
    fn smooth_fills_pinholes_keeps_solids() {
        let mut doc = Document::new(12, 12);
        doc.selection_mask = Some(vec![255u8; 144]);
        doc.selection_mask.as_mut().unwrap()[5 * 12 + 5] = 0;
        assert!(doc.smooth_selection());
        // Single-pixel hole heals (blur r2 keeps it over threshold).
        assert_eq!(doc.selection_mask.clone().unwrap()[5 * 12 + 5], 255);
        // Solid rects survive smoothing byte-identical in the interior.
        let mut doc2 = selected_rect(20, 20, (5, 5, 14, 14));
        assert!(doc2.smooth_selection());
        assert_eq!(
            doc2.selection_mask.clone().unwrap()[(10 * 20 + 10) as usize],
            255
        );
        let mut doc3 = Document::new(8, 8);
        assert!(!doc3.smooth_selection());
    }

    #[test]
    fn invert_flips_to_full_canvas_complement() {
        let mut doc = selected_rect(10, 10, (2, 2, 5, 5));
        assert!(doc.invert_selection());
        let m = doc.selection_mask.clone().unwrap();
        assert_eq!(m[(3 * 10 + 3) as usize], 0);
        assert_eq!(m[0], 255);
        assert_eq!(doc.selection, Some((0, 0, 9, 9)));
        assert_eq!(doc.selection_pixel_count(), 100 - 16);
        let mut doc2 = Document::new(8, 8);
        assert!(!doc2.invert_selection());
    }

    #[test]
    fn opaque_selects_painted_pixels() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(10, 10);
        let brush = Brush {
            size: 3.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(255, 0, 0), 2.5, 2.5);
        assert!(doc.select_opaque(8));
        let n = doc.selection_pixel_count();
        assert!(n > 0 && n < 100, "{n}");
        // Bounds hug the dab.
        let (x0, y0, x1, y1) = doc.selection.unwrap();
        assert!(x1 - x0 <= 4 && y1 - y0 <= 4);
        // Untouched layers clear the selection.
        let mut doc2 = Document::new(8, 8);
        assert!(!doc2.select_opaque(8));
        assert!(doc2.selection.is_none());
    }

    #[test]
    fn stroke_paints_only_the_edge() {
        use crate::brush::{Brush, Color};
        let mut doc = selected_rect(12, 12, (2, 2, 7, 7));
        let brush = Brush {
            size: 1.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.checkpoint();
        assert!(doc.stroke_selection(&brush, Color::rgb(0, 255, 0), 1));
        let at =
            |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 12 + x) * 4) as usize + 3);
        // Edge pixels painted, deep interior untouched.
        assert!(at(&doc, 2, 4) > 0);
        assert!(at(&doc, 7, 7) > 0);
        assert_eq!(at(&doc, 4, 4), 0);
        assert_eq!(at(&doc, 0, 0), 0);
        // One undo step removes the whole outline.
        assert!(doc.undo());
        assert_eq!(at(&doc, 2, 4), 0);
        // No selection, no paint.
        let mut doc2 = Document::new(8, 8);
        assert!(!doc2.stroke_selection(&brush, Color::rgb(0, 255, 0), 1));
    }

    #[test]
    fn feathered_masks_partially_apply_paint_and_tone() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(20, 20);
        doc.selection = Some((5, 5, 14, 14));
        assert!(doc.feather_selection(3));
        // A ramp pixel just outside the old edge carries partial coverage.
        let m = doc.selection_mask.clone().unwrap();
        let ramp = m[(4 * 20 + 10) as usize];
        // Tone applies proportionally through the ramp.
        doc.checkpoint();
        doc.adjust_tone(0, 100, 1.0);
        let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 20 + x) * 4) as usize);
        assert_eq!(at(&doc, 10, 10), 100);
        let edge = at(&doc, 10, 4);
        if ramp > 0 {
            assert!(edge > 0 && edge < 100, "{edge} ramp {ramp}");
        } else {
            assert_eq!(edge, 0);
        }
        assert_eq!(at(&doc, 0, 0), 0);
        // Dabs fade through the same ramp.
        let mut doc2 = Document::new(20, 20);
        doc2.selection = Some((5, 5, 14, 14));
        assert!(doc2.feather_selection(3));
        let brush = Brush {
            size: 1.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc2.dab(&brush, Color::rgb(255, 255, 255), 10.5, 4.5);
        let a = doc2.layers[0].pixels.get(((4 * 20 + 10) * 4) as usize + 3);
        let ramp2 = doc2.selection_mask.clone().unwrap()[(4 * 20 + 10) as usize];
        if ramp2 > 0 {
            assert!(a > 0 && a < 255, "{a} ramp {ramp2}");
        } else {
            assert_eq!(a, 0);
        }
    }

    #[test]
    fn gallery_thumb_downscales_square() {
        // 4x4 where pixel (x,y) holds red=x*64+y*16: each 2x2 block's
        // top-left must win the single output pixel (nearest-neighbor).
        let mut src = vec![0u8; 4 * 4 * 4];
        for y in 0..4 {
            for x in 0..4 {
                let i = ((y * 4 + x) * 4) as usize;
                src[i] = (x * 64 + y * 16) as u8;
                src[i + 1] = 1;
                src[i + 2] = 2;
                src[i + 3] = 255;
            }
        }
        let (out, tw, th) = gallery_thumb(&src, 4, 4, 2);
        assert_eq!((tw, th), (2, 2));
        let at = |x: usize, y: usize| out[(y * 2 + x) * 4];
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(1, 0), 128);
        assert_eq!(at(0, 1), 32);
        assert_eq!(at(1, 1), 160);
    }

    #[test]
    fn gallery_thumb_preserves_aspect_and_skips_upscale() {
        let src = vec![7u8; 8 * 4 * 4];
        let (out, tw, th) = gallery_thumb(&src, 8, 4, 4);
        assert_eq!((tw, th), (4, 2));
        assert_eq!(out.len(), 4 * 2 * 4);
        let small = vec![9u8; 2 * 2 * 4];
        let (same, w, h) = gallery_thumb(&small, 2, 2, 96);
        assert_eq!((w, h), (2, 2));
        assert_eq!(same, small);
    }

    #[test]
    fn gallery_thumb_rejects_bad_input() {
        let (out, w, h) = gallery_thumb(&[1u8; 10], 4, 4, 96);
        assert!(out.is_empty() && w == 0 && h == 0);
        let (out, w, h) = gallery_thumb(&[], 0, 0, 96);
        assert!(out.is_empty() && w == 0 && h == 0);
    }

    #[test]
    fn translate_selection_moves_rect() {
        let mut doc = Document::new(10, 10);
        doc.selection = Some((2, 2, 5, 5));
        assert!(doc.translate_selection(3, 1));
        assert_eq!(doc.selection, Some((5, 3, 8, 6)));
        // Weights survive a mask nudge (bake materializes the rect first).
        let m = doc.selection_mask.clone().unwrap();
        assert!(m.iter().all(|&b| b == 0 || b == 255));
    }

    #[test]
    fn translate_selection_off_canvas_clears() {
        let mut doc = Document::new(4, 4);
        doc.selection = Some((0, 0, 1, 1));
        assert!(!doc.translate_selection(10, 0));
        assert!(doc.selection.is_none() && doc.selection_mask.is_none());
        // Partial push keeps the on-canvas tail.
        let mut doc = Document::new(4, 4);
        doc.selection = Some((0, 0, 1, 1));
        assert!(doc.translate_selection(3, 0));
        assert_eq!(doc.selection, Some((3, 0, 3, 1)));
    }

    #[test]
    fn flip_selection_mirrors_rect_and_mask() {
        let mut doc = Document::new(10, 10);
        doc.selection = Some((1, 2, 4, 6));
        assert!(doc.flip_selection_h());
        assert_eq!(doc.selection, Some((5, 2, 8, 6)));
        let mut doc = Document::new(10, 10);
        doc.selection = Some((1, 2, 4, 6));
        assert!(doc.flip_selection_v());
        assert_eq!(doc.selection, Some((1, 3, 4, 7)));
        // Weighted mask pixels land mirrored (feather fringe included).
        let mut doc = Document::new(4, 4);
        let mut mask = vec![0u8; 16];
        mask[0] = 128;
        doc.selection_mask = Some(mask);
        assert!(doc.flip_selection_h());
        let m = doc.selection_mask.clone().unwrap();
        assert_eq!(m[3], 128);
        assert!(doc.flip_selection_v());
        let m = doc.selection_mask.clone().unwrap();
        assert_eq!(m[15], 128);
    }

    #[test]
    fn levels_lut_identity_and_crush() {
        let id = levels_lut(0, 1.0, 255);
        for (i, v) in id.iter().enumerate() {
            assert_eq!(*v, i as u8);
        }
        // Degenerate range refuses to clip: identity, not black.
        assert_eq!(levels_lut(200, 1.0, 100)[64], 64);
        let crush = levels_lut(128, 1.0, 255);
        assert_eq!(crush[64], 0);
        assert_eq!(crush[255], 255);
        // Gamma > 1 darkens mids; < 1 lifts them.
        assert!(levels_lut(0, 2.0, 255)[128] < 128);
        assert!(levels_lut(0, 0.5, 255)[128] > 128);
    }

    #[test]
    fn curves_lut_identity_pins_ends() {
        let id = curves_lut(0, 0, 0);
        for (i, v) in id.iter().enumerate() {
            assert_eq!(*v, i as u8);
        }
        let lift = curves_lut(100, 0, 0);
        assert_eq!(lift[0], 0);
        assert_eq!(lift[255], 255);
        assert_eq!(lift[64], 164);
        // Between-point interpolation is linear.
        assert_eq!(lift[32], 82);
    }

    #[test]
    fn apply_lut_honors_selection() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(4, 4);
        let brush = Brush {
            size: 20.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(100, 100, 100), 1.5, 1.5);
        let lut = levels_lut(128, 1.0, 255);
        // Only pixel (0,0) selected: it crushes, the rest is untouched.
        doc.selection = Some((0, 0, 0, 0));
        doc.apply_lut(0, &lut);
        assert_eq!(doc.layers[0].pixels.get(0), 0);
        // Pixel (1,1) offset 20 in the 4x4 RGBA buffer.
        let o = 20usize;
        assert_eq!(doc.layers[0].pixels.get(o), 100);
        // Whole canvas: mid gray crushes to 0, alpha stays 255.
        doc.clear_selection();
        doc.apply_lut(0, &lut);
        assert_eq!(doc.layers[0].pixels.get(o), 0);
        assert_eq!(doc.layers[0].pixels.get(o + 3), 255);
    }

    #[test]
    fn hsl_adjust_invariants() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(4, 4);
        let brush = Brush {
            size: 20.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        // Gray has no hue: hue spins leave it gray, lightness lifts it.
        doc.dab(&brush, Color::rgb(128, 128, 128), 1.5, 1.5);
        doc.adjust_hsl(0, 180.0, 2.0, 0.0);
        let o = 20usize;
        assert_eq!(doc.layers[0].pixels.get(o), doc.layers[0].pixels.get(o + 1));
        assert_eq!(
            doc.layers[0].pixels.get(o + 1),
            doc.layers[0].pixels.get(o + 2)
        );
        doc.adjust_hsl(0, 0.0, 1.0, 1.0);
        assert_eq!(doc.layers[0].pixels.get(o), 255);
        // Pure red rotated +120 lands on green.
        let mut doc = Document::new(2, 2);
        doc.dab(&brush, Color::rgb(255, 0, 0), 0.5, 0.5);
        doc.adjust_hsl(0, 120.0, 1.0, 0.0);
        let dense = doc.layers[0].pixels.to_dense();
        let px = &dense[0..3];
        assert!(px[1] > 200 && px[0] < 55 && px[2] < 55, "{px:?}");
    }

    #[test]
    fn balance_and_colorize_behave() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(2, 2);
        let brush = Brush {
            size: 20.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(100, 100, 100), 0.5, 0.5);
        doc.color_balance(0, 51, 0, 0);
        assert_eq!(doc.layers[0].pixels.get(0), 151);
        assert_eq!(doc.layers[0].pixels.get(1), 100);
        // No-op guards leave pixels alone.
        let before = doc.layers[0].pixels.clone();
        doc.color_balance(0, 0, 0, 0);
        doc.colorize(0, [255, 0, 0], 0.0);
        assert_eq!(doc.layers[0].pixels, before);
        // Colorize mid-gray to red: keeps luminance, takes the hue.
        let mut doc = Document::new(2, 2);
        doc.dab(&brush, Color::rgb(128, 128, 128), 0.5, 0.5);
        doc.colorize(0, [255, 0, 0], 1.0);
        let dense = doc.layers[0].pixels.to_dense();
        let px = &dense[0..3];
        assert!(px[0] > 200 && px[1] < 80 && px[2] < 80, "{px:?}");
        // Selection gate: excluded pixels keep their color.
        let mut doc = Document::new(4, 4);
        doc.dab(&brush, Color::rgb(100, 100, 100), 1.5, 1.5);
        doc.selection = Some((0, 0, 0, 0));
        doc.color_balance(0, 51, 51, 51);
        assert_eq!(doc.layers[0].pixels.get(0), 151);
        assert_eq!(doc.layers[0].pixels.get(20), 100);
    }

    #[test]
    fn liquify_pushes_pixels_along_the_drag() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(32, 32);
        let brush = Brush {
            size: 30.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        // Solid white bar on the left half.
        doc.fill_rect(&brush, Color::rgb(255, 255, 255), 4, 4, 11, 27);
        // Push right at the bar's edge: the bar's rim must advance.
        doc.liquify(0, 11.5, 15.5, 6.0, 0.0, 16.0, 1.0);
        let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 32 + x) * 4) as usize);
        assert!(at(&doc, 14, 15) > 200, "rim advanced");
        assert_eq!(at(&doc, 24, 15), 0, "far field untouched");
        // Zero push and zero strength are no-ops.
        let before = doc.layers[0].pixels.clone();
        doc.liquify(0, 11.5, 15.5, 0.0, 0.0, 16.0, 1.0);
        doc.liquify(0, 11.5, 15.5, 6.0, 0.0, 16.0, 0.0);
        assert_eq!(doc.layers[0].pixels, before);
    }

    #[test]
    fn liquify_pushes_across_a_tile_seam() {
        use crate::brush::{Brush, Color};
        // x = 256 is a tile seam; the push pulls red from the left tile
        // into the right one, so the region read must cross the seam.
        let mut doc = Document::new(520, 40);
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        doc.fill_rect(&brush, Color::rgb(255, 0, 0), 0, 0, 255, 39);
        doc.fill_rect(&brush, Color::rgb(0, 0, 255), 256, 0, 519, 39);
        doc.liquify(0, 256.0, 20.0, 10.0, 0.0, 24.0, 1.0);
        let at = |x: u32| doc.layers[0].pixels.get4(((20 * 520 + x) * 4) as usize);
        assert_eq!(
            at(257),
            [255, 0, 0, 255],
            "red is pulled across the seam from the left tile"
        );
        assert_eq!(at(400), [0, 0, 255, 255], "far field untouched");
    }

    #[test]
    fn liquify_honors_selection() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(32, 32);
        let brush = Brush {
            size: 30.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.fill_rect(&brush, Color::rgb(255, 255, 255), 4, 4, 27, 27);
        // Selection far from the push keeps its pixels (warp still runs).
        doc.selection = Some((24, 24, 31, 31));
        let before = doc.layers[0].pixels.clone();
        doc.liquify(0, 8.5, 8.5, 6.0, 0.0, 12.0, 1.0);
        let o = ((25 * 32 + 25) * 4) as usize;
        assert_eq!(doc.layers[0].pixels.get(o), before.get(o));
    }

    #[test]
    fn liquify_matches_naive_reference_with_long_reach() {
        // Characterization: the region-bounded copy must reproduce the
        // original whole-buffer warp bit-for-bit, including long pushes
        // whose reverse samples read far outside the disc (and off-canvas).
        #[allow(clippy::too_many_arguments)]
        fn naive_liquify(
            src: &[u8],
            w: u32,
            h: u32,
            x: f32,
            y: f32,
            dx: f32,
            dy: f32,
            size: f32,
            strength: f32,
        ) -> Vec<u8> {
            let radius = size * 0.5;
            let bx0 = ((x - radius).floor().max(0.0)) as u32;
            let by0 = ((y - radius).floor().max(0.0)) as u32;
            let bx1 = ((x + radius).ceil().min(w as f32 - 1.0).max(0.0)) as u32;
            let by1 = ((y + radius).ceil().min(h as f32 - 1.0).max(0.0)) as u32;
            let sample = |sx: f32, sy: f32, ch: usize| -> f32 {
                let sx = sx.clamp(0.0, w as f32 - 1.0);
                let sy = sy.clamp(0.0, h as f32 - 1.0);
                let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
                let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
                let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
                let at = |x: u32, y: u32| src[((y * w + x) as usize * 4) + ch] as f32;
                at(x0, y0) * (1.0 - fx) * (1.0 - fy)
                    + at(x1, y0) * fx * (1.0 - fy)
                    + at(x0, y1) * (1.0 - fx) * fy
                    + at(x1, y1) * fx * fy
            };
            let mut pixels = src.to_vec();
            for py in by0..=by1 {
                for px in bx0..=bx1 {
                    let dist = (((px as f32 + 0.5 - x).powi(2) + (py as f32 + 0.5 - y).powi(2))
                        .sqrt()
                        / radius)
                        .min(1.0);
                    let f = 0.5 + 0.5 * (std::f32::consts::PI * dist).cos();
                    let (ox, oy) = (px as f32 - dx * strength * f, py as f32 - dy * strength * f);
                    if (ox - px as f32).abs() < f32::EPSILON
                        && (oy - py as f32).abs() < f32::EPSILON
                    {
                        continue;
                    }
                    let o = ((py * w + px) * 4) as usize;
                    for ch in 0..4 {
                        // No selection: k = 1, write the sampled value.
                        pixels[o + ch] = sample(ox, oy, ch).round() as u8;
                    }
                }
            }
            pixels
        }
        let mut doc = Document::new(32, 32);
        // Deterministic pseudo-random paint so any region-copy slip shows.
        let bytes: Vec<u8> = (0..32 * 32 * 4)
            .map(|i| ((i * 37 + 11) % 256) as u8)
            .collect();
        doc.layers[0].pixels = Raster::from_dense(32, 32, 4, &bytes);
        // Reach (|d| = 11.4) far beyond the disc: samples come from the
        // ring outside the write bbox.
        let src = doc.layers[0].pixels.clone();
        doc.liquify(0, 20.0, 12.0, 9.0, -7.0, 14.0, 1.0);
        assert_eq!(
            doc.layers[0].pixels,
            naive_liquify(&src.to_dense(), 32, 32, 20.0, 12.0, 9.0, -7.0, 14.0, 1.0)
        );
        // Corner warp: reads clamp against the canvas edge.
        let src = doc.layers[0].pixels.clone();
        doc.liquify(0, 2.0, 2.0, 5.0, 4.0, 14.0, 0.7);
        assert_eq!(
            doc.layers[0].pixels,
            naive_liquify(&src.to_dense(), 32, 32, 2.0, 2.0, 5.0, 4.0, 14.0, 0.7)
        );
    }

    #[test]
    fn liquify_mirrors_with_symmetry() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(32, 32);
        let brush = Brush {
            size: 30.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        // Two bars mirrored about the canvas middle: [3,7] and [24,28].
        doc.fill_rect(&brush, Color::rgb(255, 255, 255), 3, 8, 7, 24);
        doc.fill_rect(&brush, Color::rgb(255, 255, 255), 24, 8, 28, 24);
        doc.symmetry = SymmetryMode::Horizontal;
        // Push right at the left bar's rim: the mirrored push runs left
        // at the mirrored point with a mirrored direction.
        doc.liquify(0, 7.5, 15.5, 6.0, 0.0, 16.0, 1.0);
        let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 32 + x) * 4) as usize);
        assert!(at(&doc, 10, 15) > 200, "primary rim advanced");
        assert!(at(&doc, 21, 15) > 200, "mirrored rim advanced");
        assert_eq!(at(&doc, 15, 15), 0, "between the discs untouched");
    }

    #[test]
    fn liquify_mirrors_kaleidoscope_push_vectors() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(32, 32);
        let brush = Brush {
            size: 30.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        // East bar and its k=1 (90 deg) image, origin at the middle.
        doc.fill_rect(&brush, Color::rgb(255, 255, 255), 21, 13, 23, 19);
        doc.fill_rect(&brush, Color::rgb(255, 255, 255), 13, 21, 19, 23);
        doc.set_symmetry_center(16.0, 16.0);
        doc.symmetry = SymmetryMode::Kaleidoscope;
        doc.symmetry_segments = 4;
        // Push up at the east bar's middle: k=1 mirrors the point 90
        // degrees CCW (south) and rotates the push vector to point west.
        doc.liquify(0, 23.0, 16.0, 0.0, -6.0, 16.0, 1.0);
        let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 32 + x) * 4) as usize);
        assert!(at(&doc, 23, 13) > 200, "primary push advanced the east rim");
        assert!(at(&doc, 12, 23) > 200, "k=1 push pulled the south bar west");
    }

    #[test]
    fn fit_shift_math() {
        // Inside: no slide.
        assert_eq!(Document::fit_shift(100, 100, 30, 0, 40, 10, 50, 20), (0, 0));
        // Left overhang slides right; right overhang slides left.
        assert_eq!(
            Document::fit_shift(100, 100, 60, 0, 10, 10, 20, 20),
            (50, 0)
        );
        assert_eq!(
            Document::fit_shift(100, 100, -60, 0, 80, 10, 90, 20),
            (-51, 0)
        );
        // Fully displaced bboxes still absorb (only invisible tails trim).
        assert_eq!(
            Document::fit_shift(100, 100, 5000, 0, 10, 10, 20, 20),
            (4990, 0)
        );
        assert_eq!(Document::fit_shift(0, 0, 0, 0, 0, 0, 0, 0), (0, 0));
    }

    #[test]
    fn absorb_keeps_content_on_screen() {
        let mut doc = Document::new(32, 32);
        // Paint a marker, move the layer, absorb where the marker shows.
        doc.layers[0].pixels.set(((4 * 32 + 4) * 4) as usize, 255);
        doc.layers[0]
            .pixels
            .set(((4 * 32 + 4) * 4 + 3) as usize, 255);
        doc.translate_layer(0, 10, 0);
        // Canvas (0,4) maps outside the buffer: slide home, marker rides
        // along and still shows at canvas (14,4).
        assert!(doc.absorb_offset(0, 0, 4, 0, 4));
        let flat = doc.flatten();
        assert_eq!(flat[((4 * 32 + 14) * 4) as usize], 255);
        // Offset shrank by the slide; second absorb is a no-op.
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
        assert!(!doc.absorb_offset(0, 14, 4, 14, 4));
    }

    #[test]
    fn paint_works_full_page_after_move() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(32, 32);
        doc.translate_layer(0, 20, 0);
        // The strip the offset pushed out of the buffer paints again.
        let brush = Brush {
            size: 4.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(255, 255, 255), 2.0, 16.0);
        let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 32 + x) * 4) as usize);
        // Composite shows the dab where the user put it.
        let flat = doc.flatten();
        assert!(flat[((16 * 32 + 2) * 4) as usize] > 200);
        // And pre-move content survived the slide (moved, not wiped).
        assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
        let _ = at;
    }

    #[test]
    fn rasterize_keeps_pixels_drops_block_and_undoes() {
        use crate::text::{TextBlock, TextStyle};
        let mut doc = Document::new(200, 120);
        let block = TextBlock {
            text: "Hi".into(),
            size_px: 32.0,
            color: [0, 0, 0, 255],
            style: TextStyle::default(),
            x: 10.0,
            y: 10.0,
            w: 0,
            h: 0,
        };
        let (idx, ..) = doc.add_text_layer("T", block).unwrap();
        let pixels = doc.layers[idx].pixels.clone();
        doc.checkpoint();
        assert!(doc.rasterize_text_layer(idx));
        assert!(!doc.layers[idx].is_text());
        assert_eq!(
            doc.layers[idx].pixels, pixels,
            "rasterize must not touch pixels"
        );
        assert!(!doc.rasterize_text_layer(idx)); // idempotent
        assert!(doc.undo()); // rasterize is undoable
        assert!(doc.layers[idx].is_text());
        assert_eq!(doc.layers[idx].pixels, pixels);
    }

    #[test]
    fn merge_down_onto_text_layer_rasterizes_target() {
        use crate::text::{TextBlock, TextStyle};
        let mut doc = Document::new(64, 64);
        let block = TextBlock {
            text: "Hi".into(),
            size_px: 24.0,
            color: [0, 0, 0, 255],
            style: TextStyle::default(),
            x: 2.0,
            y: 2.0,
            w: 0,
            h: 0,
        };
        let (idx, ..) = doc.add_text_layer("T", block).unwrap();
        doc.add_layer("Layer over".into()); // becomes index idx+1
        assert!(doc.merge_down(idx + 1));
        // The baked result must not pretend to still be editable text.
        assert!(!doc.layers[idx].is_text());
    }

    #[test]
    fn text_layer_at_hits_bounds_topmost_visible() {
        use crate::text::{TextBlock, TextStyle};
        let mut doc = Document::new(200, 120);
        let a = TextBlock {
            text: "A".into(),
            size_px: 32.0,
            color: [0, 0, 0, 255],
            style: TextStyle::default(),
            x: 10.0,
            y: 10.0,
            w: 0,
            h: 0,
        };
        let (i, w, h) = doc.add_text_layer("A", a).unwrap();
        assert_eq!(doc.text_layer_at(10.0, 10.0), Some(i));
        // Inside near-edge (block bounds are [x, x+w) x [y, y+h)).
        assert_eq!(doc.text_layer_at(w as f32 + 9.9, h as f32 + 9.9), Some(i));
        assert_eq!(doc.text_layer_at(w as f32 + 10.1, 10.0), None);
        assert_eq!(doc.text_layer_at(-1.0, -1.0), None);
        doc.layers[i].visible = false;
        assert_eq!(doc.text_layer_at(10.0, 10.0), None);
        doc.layers[i].visible = true;
        // Topmost visible text layer wins.
        let b = TextBlock {
            text: "B".into(),
            size_px: 32.0,
            color: [0, 0, 0, 255],
            style: TextStyle::default(),
            x: 10.0,
            y: 10.0,
            w: 0,
            h: 0,
        };
        let (j, ..) = doc.add_text_layer("B", b).unwrap();
        assert_eq!(doc.text_layer_at(12.0, 12.0), Some(j));
    }

    #[test]
    fn crop_shifts_text_block_anchor_with_content() {
        use crate::text::{TextBlock, TextStyle};
        let mut doc = Document::new(240, 160);
        let block = TextBlock {
            text: "Hi".into(),
            size_px: 32.0,
            color: [255, 0, 0, 255],
            style: TextStyle::default(),
            x: 60.0,
            y: 40.0,
            w: 0,
            h: 0,
        };
        let (idx, w, h) = doc.add_text_layer("T", block).unwrap();
        let sample = |d: &Document, x0: u32, y0: u32| {
            let l = &d.layers[idx];
            let mut out = Vec::new();
            for y in y0..y0 + h {
                for x in x0..x0 + w {
                    let bx = x as i32 - l.ox;
                    let by = y as i32 - l.oy;
                    if bx < 0 || by < 0 || bx as u32 >= d.w || by as u32 >= d.h {
                        out.extend_from_slice(&[0, 0, 0, 0]);
                        continue;
                    }
                    let o = ((by as u32 * d.w + bx as u32) * 4) as usize;
                    out.extend_from_slice(&l.pixels.get4(o));
                }
            }
            out
        };
        let before = sample(&doc, 60, 40);
        assert!(before.iter().any(|&b| b != 0), "text must render pixels");
        doc.crop(10, 10, 149, 149).unwrap(); // dx=-10, dy=-10
                                             // The anchor moved with its content.
        let t = doc.layers[idx].text.as_ref().unwrap();
        assert_eq!((t.x, t.y), (50.0, 30.0));
        // Pixels read back identically at the shifted canvas coords, and a
        // rerender at the new anchor reproduces them exactly.
        assert_eq!(sample(&doc, 50, 30), before);
        doc.rerender_text_layer(idx).unwrap();
        assert_eq!(sample(&doc, 50, 30), before);
        // One hit, at the new location; the old one is gone.
        assert_eq!(doc.text_layer_at(51.0, 31.0), Some(idx));
        assert_eq!(doc.text_layer_at(40.0, 20.0), None);
    }

    #[test]
    fn blank_layers_allocate_no_tiles() {
        let mut doc = Document::new(2048, 2048);
        for i in 0..100 {
            doc.add_layer(format!("L{i}"));
        }
        assert_eq!(doc.layers.len(), 101);
        assert_eq!(doc.layer_alloc_bytes(), 0);
    }

    #[test]
    fn undo_checkpoint_costs_touched_tiles_only() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(600, 600); // 3x3 tiles
        let brush = Brush::default();
        doc.dab(&brush, Color([255, 0, 0, 255]), 10.0, 10.0);
        doc.checkpoint();
        doc.dab(&brush, Color([0, 0, 255, 255]), 550.0, 550.0);
        assert!(
            doc.undo_bytes <= 2 * 256 * 1024,
            "one checkpoint of a two-tile layer: {} bytes",
            doc.undo_bytes
        );
        assert!(doc.undo());
        assert_eq!(
            doc.layers[0].pixels.get4(((550 * 600 + 550) * 4) as usize),
            [0; 4]
        );
        assert_eq!(
            doc.layers[0].pixels.get(((10 * 600 + 10) * 4 + 3) as usize),
            255
        );
    }

    #[test]
    fn duplicate_layer_shares_tiles_until_painted() {
        use crate::brush::{Brush, Color};
        let mut doc = Document::new(600, 600);
        doc.dab(&Brush::default(), Color([0, 255, 0, 255]), 12.0, 12.0);
        doc.duplicate_layer(0);
        // Per-layer accounting counts the shared slot twice; the process holds
        // one tile because the Arc is shared.
        assert_eq!(doc.layer_alloc_bytes(), 2 * 256 * 1024);
        doc.active = 1;
        doc.dab(&Brush::default(), Color([1, 2, 3, 255]), 300.0, 300.0);
        assert_eq!(doc.layer_alloc_bytes(), 3 * 256 * 1024);
    }
}
