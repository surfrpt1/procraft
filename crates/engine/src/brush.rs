//! Soft-round brush engine: colors, dab stamping and stroke interpolation.

use super::document::Document;

/// RGBA color, 0..=255 per channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self([r, g, b, 255])
    }
}

/// Grayscale brush tip (stamp) from a third-party brush file: alpha mask,
/// row-major, white (255) = full ink. Rendered at `Brush::size` wide.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BrushTip {
    pub w: u32,
    pub h: u32,
    pub mask: Vec<u8>,
}

/// Maximum tip dimension in pixels (larger tips are downscaled on import).
pub const MAX_TIP_SIDE: u32 = 256;

impl BrushTip {
    /// Build a tip from a raw mask, validating dimensions and length.
    pub fn from_mask(w: u32, h: u32, mask: Vec<u8>) -> Result<Self, String> {
        if w == 0 || h == 0 || w > MAX_TIP_SIDE || h > MAX_TIP_SIDE {
            return Err("brush tip has bad dimensions".to_string());
        }
        if mask.len() != (w * h) as usize {
            return Err("brush tip has corrupt pixels".to_string());
        }
        if mask.iter().all(|&b| b == 0) {
            return Err("brush tip is fully transparent".to_string());
        }
        Ok(Self { w, h, mask })
    }
}

/// Polygonal shape families for the shape tools (outline + fill share
/// the same vertices).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapePoly {
    Triangle,
    Diamond,
    Star,
    Polygon,
}

/// Per-dab dynamics sample for [`Document::stroke_to_dyn`]: pen pressure,
/// normalized pointer speed and stroke direction at one stroke endpoint.
/// The engine interpolates these per dab, so a single pointer segment can
/// taper, fade and turn across its dabs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DabDynamics {
    /// Pen pressure, 0.0..=1.0 (1.0 = full pressure / mouse).
    pub pressure: f32,
    /// Pointer speed normalized by 3500 px/s, 0.0..=1.0.
    pub speed_norm: f32,
    /// Stroke direction in radians (`atan2(dy, dx)`).
    pub angle: f32,
    /// Stroke-length taper fraction, 0.0..=1.0 (1.0 = full size).
    pub taper: f32,
}

impl Default for DabDynamics {
    fn default() -> Self {
        Self {
            pressure: 1.0,
            speed_norm: 0.0,
            angle: 0.0,
            taper: 1.0,
        }
    }
}

/// Stroke start/end taper settings, normalized to the whole stroke length.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Taper {
    #[serde(default)]
    pub start: f32,
    #[serde(default)]
    pub end: f32,
    #[serde(default = "def_taper_size")]
    pub size: f32,
    #[serde(default)]
    pub shape: f32,
    #[serde(default)]
    pub opacity: f32,
    #[serde(default)]
    pub pressure: f32,
}

/// Taper span in brush diameters: at 1.0, an end fades over six widths.
pub const TAPER_DIAMETERS: f32 = 6.0;

impl Default for Taper {
    fn default() -> Self {
        Self::NONE
    }
}

impl Taper {
    pub const NONE: Self = Self {
        start: 0.0,
        end: 0.0,
        size: 1.0,
        shape: 0.0,
        opacity: 0.0,
        pressure: 0.0,
    };

    /// Shape-only taper fraction at normalized stroke position `u`
    /// (0 = start, 1 = end). Size and pressure scaling are applied by
    /// [`Taper::size_factor`].
    pub fn factor_at(&self, u: f32) -> f32 {
        let start = clamp01(self.start);
        let end = clamp01(self.end);
        let sum = start + end;
        let (start, end) = if sum > 1.0 {
            (start / sum, end / sum)
        } else {
            (start, end)
        };
        let u = clamp01(u);
        let rise = if start > 0.0 {
            (u / start).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let fall = if end > 0.0 {
            ((1.0 - u) / end).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let f = rise.min(fall);
        self.shape_of(f)
    }

    /// Taper factor from canvas distances: `from_start` travelled so far
    /// and `to_end` still to go, for a brush `size` px wide. Each end fades
    /// over `start`/`end` × [`TAPER_DIAMETERS`] brush diameters, so live
    /// dabs need no final stroke length and the release replay reproduces
    /// them exactly.
    pub fn distance_factor(&self, from_start: f32, to_end: f32, size: f32) -> f32 {
        let span = size.max(1.0) * TAPER_DIAMETERS;
        let rise = ramp(from_start, self.start, span);
        let fall = ramp(to_end, self.end, span);
        self.shape_of(rise.min(fall))
    }

    /// Live taper for a dab at `from_start` px into the stroke. Only the
    /// start ramp is known while the pen is down; the end ramp needs the
    /// final length and is applied once on release ([`distance_factor`]).
    /// Passing 0 for the end here would apply the *full* end ramp to every
    /// live dab, so the stroke would be painted thin while drawing and
    /// repainted at full width on lift.
    pub fn live_factor(&self, from_start: f32, size: f32) -> f32 {
        let span = size.max(1.0) * TAPER_DIAMETERS;
        self.shape_of(ramp(from_start, self.start, span))
    }

    fn shape_of(&self, f: f32) -> f32 {
        let smooth = f * f * (3.0 - 2.0 * f);
        f + (smooth - f) * clamp01(self.shape)
    }

    /// Scale a shape-only taper factor by the Size and Pressure settings.
    /// Size controls how far the taper can narrow; Pressure makes low
    /// pressure narrow further inside the tapered ends.
    pub fn size_factor(&self, taper: f32, pressure: f32) -> f32 {
        let f = clamp01(taper);
        let pressure = clamp01(self.pressure) * (1.0 - clamp01(pressure)) * (1.0 - f);
        let f = f * (1.0 - pressure);
        1.0 - clamp01(self.size) * (1.0 - f)
    }
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// One taper ramp: 0 at the tip, 1 once `dist` reaches `amount` × `span`.
/// `amount` 0 means that end of the stroke has no taper.
fn ramp(dist: f32, amount: f32, span: f32) -> f32 {
    if amount <= 0.0 {
        1.0
    } else {
        (dist / (clamp01(amount) * span)).clamp(0.0, 1.0)
    }
}

// serde defaults for the fields added in M14: old `.pcbrush` files without
// these keys load with the same values as `Brush::default()`.
fn def_pressure_size() -> f32 {
    0.7
}
fn def_pressure_opacity() -> f32 {
    0.6
}
// Speed dynamics are off by default, like Procreate: with a mouse speed is
// the only input, and reacting to it turns every stroke into a thread with
// blobs wherever the hand paused. Brushes that want speed opt in.
fn def_velocity_size() -> f32 {
    0.0
}
fn def_velocity_opacity() -> f32 {
    0.0
}
fn def_grain() -> f32 {
    0.0
}
fn def_grain_scale() -> f32 {
    1.0
}
fn def_taper_size() -> f32 {
    1.0
}
fn def_roundness() -> f32 {
    1.0
}
fn def_angle_follow() -> bool {
    false
}
fn def_scatter() -> f32 {
    0.0
}
fn def_stabilizer() -> f32 {
    0.35
}
fn def_dual_scale() -> f32 {
    0.5
}
fn def_shape_count() -> u8 {
    1
}
fn def_shape_rotation() -> f32 {
    0.0
}
fn def_angle_scatter() -> f32 {
    0.0
}

/// Deterministic 0.0..=1.0 hash for scatter offsets and grain. Position
/// based (not counter based) so strokes replay identically across
/// undo/redo, thumbnails and file round-trips. Original integer-mix math.
fn hash01(x: i32, y: i32) -> f32 {
    let mut h = (x as u32)
        .wrapping_mul(374761393)
        .wrapping_add((y as u32).wrapping_mul(668265263));
    h ^= h >> 13;
    h = h.wrapping_mul(1274126177);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_215.0
}

/// Grain cell in canvas pixels: `3 * scale`, back off to a third of the
/// dab. A cell wider than that gives every pixel of a small brush the same
/// value, so a 2–6 px Chalk mark paints as flat blotches (dashes) instead
/// of grain; large brushes keep the preset's coarse texture.
fn grain_cell(size: f32, scale: f32) -> u32 {
    let base = (3.0 * scale.clamp(0.25, 4.0)).round().max(1.0);
    let per_dab = (size.max(1.0) / 3.0).floor().max(1.0);
    base.min(per_dab) as u32
}

/// Procedural grain sample 0.0..=1.0: one value-noise texel per `cell`
/// pixel block. No assets, clean-room.
fn grain_at(px: u32, py: u32, cell: u32) -> f32 {
    hash01((px / cell) as i32, (py / cell) as i32)
}

/// Bilinear tap of an imported stamp tip at texel-space `(tx, ty)`.
/// Texels outside the tip read as transparent, so scaled tips keep a
/// soft edge instead of blocky nearest-neighbour steps.
fn sample_tip(tip: &BrushTip, tx: f32, ty: f32) -> f32 {
    let x = tx - 0.5;
    let y = ty - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let mut acc = 0.0;
    for (dy, wy) in [(0.0, 1.0 - fy), (1.0, fy)] {
        for (dx, wx) in [(0.0, 1.0 - fx), (1.0, fx)] {
            let (ix, iy) = (x0 + dx, y0 + dy);
            if ix < 0.0 || iy < 0.0 || ix >= tip.w as f32 || iy >= tip.h as f32 {
                continue;
            }
            acc += tip.mask[(iy as u32 * tip.w + ix as u32) as usize] as f32 / 255.0 * wx * wy;
        }
    }
    acc
}

/// Grain sample 0.0..=1.0: an imported grayscale texture tiled across the
/// canvas when present, otherwise the procedural noise above. The texture is
/// magnified by `grain_scale` (larger = coarser), so imported Procreate grain
/// stays deterministic and canvas-anchored instead of following each dab.
/// `dab` is the resolved dab size; it narrows the grain cell so small
/// brushes still get several cells across the mark.
fn grain_sample(brush: &Brush, px: u32, py: u32, dab: f32) -> f32 {
    let cell = grain_cell(dab, brush.grain_scale);
    let scale = brush.grain_scale.clamp(0.25, 4.0).min(cell as f32);
    if let Some(tip) = brush.grain_tip.as_ref() {
        if tip.w > 0 && tip.h > 0 && tip.mask.len() == (tip.w * tip.h) as usize {
            let (sx, sy) = if brush.grain_angle == 0.0 {
                (px as f64, py as f64)
            } else {
                let (sin, cos) = brush.grain_angle.sin_cos();
                let (sin, cos) = (sin as f64, cos as f64);
                (
                    px as f64 * cos - py as f64 * sin,
                    px as f64 * sin + py as f64 * cos,
                )
            };
            let scale = scale as f64;
            let tw = tip.w as usize;
            let th = tip.h as usize;
            let tx = ((sx / scale).rem_euclid(tw as f64) as usize).min(tw - 1);
            let ty = ((sy / scale).rem_euclid(th as f64) as usize).min(th - 1);
            return tip.mask[ty * tw + tx] as f32 / 255.0;
        }
    }
    grain_at(px, py, cell)
}

/// M39 dual-tip multiplier 0.0..=1.0 at canvas offset (`ox`, `oy`) from
/// the dab center, for a secondary tip spanning `size * dual_scale` px
/// (nearest-neighbor, unrotated, like the primary stamp path). Absent or
/// degenerate tips return 1.0 (single-tip behavior, bit-for-bit).
fn dual_factor(dual: &Option<BrushTip>, dual_scale: f32, size: f32, ox: f32, oy: f32) -> f32 {
    let Some(tip) = dual.as_ref() else {
        return 1.0;
    };
    if tip.w == 0 || tip.h == 0 || tip.mask.len() != (tip.w * tip.h) as usize {
        return 1.0;
    }
    let span = (size * dual_scale.clamp(0.1, 4.0)).max(1.0);
    let scale = span / tip.w as f32;
    let (half_w, half_h) = (span * 0.5, tip.h as f32 * scale * 0.5);
    let tx = ((ox + half_w) / scale) as i32;
    let ty = ((oy + half_h) / scale) as i32;
    if tx < 0 || ty < 0 || tx >= tip.w as i32 || ty >= tip.h as i32 {
        return 0.0;
    }
    tip.mask[(ty as u32 * tip.w + tx as u32) as usize] as f32 / 255.0
}

/// M39 wet-edge boost: lifts mid-coverage fringe dabs toward full
/// strength (peaks at 50% coverage, zero at 0/1), so strokes dry with a
/// darker rim. `wet` 0.0 is a no-op.
fn wet_boost(fa: f32, wet: f32) -> f32 {
    (fa + wet.clamp(0.0, 1.0) * fa * (1.0 - fa)).clamp(0.0, 1.0)
}

/// M39 dab composite: the blend mode mixes brush color with the
/// backdrop, then alpha composites source-over. Normal reproduces the
/// classic path bit-for-bit; the eraser keeps destination-out upstream.
fn composite_dab(
    pixels: &mut crate::raster::Raster,
    o: usize,
    color: [u8; 4],
    src_a: f32,
    mode: super::document::BlendMode,
) {
    let dst = pixels.get4(o);
    let dst_a = dst[3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return;
    }
    let mut out = dst;
    for ch in 0..3 {
        let b = mode.apply(color[ch] as f32, dst[ch] as f32);
        out[ch] = ((b * src_a + dst[ch] as f32 * dst_a * (1.0 - src_a)) / out_a).round() as u8;
    }
    out[3] = (out_a * 255.0).round() as u8;
    pixels.set4(o, out);
}

/// Brush shape settings. All original math: a radial falloff stamp laid down
/// along the pointer path at `spacing` intervals. When `tip` is present the
/// parametric falloff is replaced by the grayscale stamp (imported brushes).
/// M14 adds per-dab dynamics: pressure and pointer speed scale size/opacity
/// through [`Brush::resolve_size`]/[`Brush::resolve_opacity`], `grain`
/// breaks the alpha with procedural noise, `roundness` + `angle_follow`
/// squeeze and steer dabs along the stroke, and `scatter` offsets dabs.
/// `stabilizer` is applied app-side (pointer smoothing) and stored here so
/// it travels with the brush preset.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Brush {
    /// Dab diameter in canvas pixels (stamp width when `tip` is present).
    pub size: f32,
    /// Per-dab alpha multiplier, 0.0..=1.0.
    pub opacity: f32,
    /// Edge softness, 0.0 (hard disc) ..= 1.0 (fully soft). Unused by stamps.
    pub softness: f32,
    /// Dab spacing as a fraction of `size` (0.1 = smooth, 1.0 = dotted).
    pub spacing: f32,
    /// Eraser mode: remove alpha instead of painting color.
    pub eraser: bool,
    /// Optional imported stamp tip (absent in `.pcbrush` files from v9).
    #[serde(default)]
    pub tip: Option<BrushTip>,
    /// Pressure size response, -1.0 (light pressure grows) ..= 1.0 (light
    /// pressure shrinks). Effective size is
    /// `size * (1 - pressure_size * (1 - p))`.
    #[serde(default = "def_pressure_size")]
    pub pressure_size: f32,
    /// Pressure opacity response, -1.0 ..= 1.0 (same curve).
    #[serde(default = "def_pressure_opacity")]
    pub pressure_opacity: f32,
    /// Optional transfer curve remapping raw pressure before the size/opacity
    /// responses, as `[[input, output], ..]` points sorted by input.
    #[serde(default)]
    pub pressure_curve: Option<Vec<[f32; 2]>>,
    /// Optional opacity-only pressure transfer curve. When present it
    /// replaces `pressure_curve` for opacity, so size and opacity can follow
    /// different pressure responses. Absent falls back to `pressure_curve`.
    #[serde(default)]
    pub pressure_opacity_curve: Option<Vec<[f32; 2]>>,
    /// How much speed thins the dab, 0.0..=1.0 (weight 0.6, as M7 dynamics).
    #[serde(default = "def_velocity_size")]
    pub velocity_size: f32,
    /// How much speed fades the dab, 0.0..=1.0 (weight 0.4, as M7 dynamics).
    #[serde(default = "def_velocity_opacity")]
    pub velocity_opacity: f32,
    /// Optional size transfer curve remapping normalized speed before the
    /// velocity response. When present it replaces `velocity_size`.
    #[serde(default)]
    pub velocity_size_curve: Option<Vec<[f32; 2]>>,
    /// Optional opacity-only speed transfer curve. When present it replaces
    /// `velocity_opacity`, so size and opacity can follow different speed
    /// responses. Absent falls back to `velocity_opacity`.
    #[serde(default)]
    pub velocity_opacity_curve: Option<Vec<[f32; 2]>>,
    /// Procedural grain strength, 0.0 (off) ..= 1.0 (heavy chalk).
    #[serde(default = "def_grain")]
    pub grain: f32,
    /// Grain cell scale, 0.25 (fine) ..= 4.0 (coarse).
    #[serde(default = "def_grain_scale")]
    pub grain_scale: f32,
    /// Optional imported grain texture (tiled canvas-anchored); None keeps
    /// the procedural grain.
    #[serde(default)]
    pub grain_tip: Option<BrushTip>,
    /// Rotation of the grain texture in radians, canvas-anchored.
    #[serde(default)]
    pub grain_angle: f32,
    /// Dab roundness, 0.1 (flat calligraphy) ..= 1.0 (circle).
    #[serde(default = "def_roundness")]
    pub roundness: f32,
    /// Random vertical shape compression per dab, 0.0 (fixed) ..= 1.0.
    #[serde(default)]
    pub roundness_jitter: f32,
    /// Random dab-size reduction per stamp, 0.0 (fixed) ..= 1.0.
    #[serde(default)]
    pub size_jitter: f32,
    /// Random dab-opacity reduction per stamp, 0.0 (fixed) ..= 1.0.
    #[serde(default)]
    pub opacity_jitter: f32,
    /// Fixed dab rotation in radians, applied before stroke steering.
    #[serde(default)]
    pub angle: f32,
    /// Steer the dab's long axis along the stroke direction.
    #[serde(default = "def_angle_follow")]
    pub angle_follow: bool,
    /// Random dab offset as a fraction of `size`, 0.0 (off) ..= 1.0.
    #[serde(default = "def_scatter")]
    pub scatter: f32,
    /// Pointer smoothing amount, 0.0 (raw) ..= 1.0 (heavy streamline).
    #[serde(default = "def_stabilizer")]
    pub stabilizer: f32,
    /// Per-brush paint blend mode (Normal = classic source-over). The
    /// eraser always uses destination-out regardless of this setting.
    #[serde(default)]
    pub blend: super::document::BlendMode,
    /// Wet edge / glaze: boosts mid-coverage fringe dabs, 0.0 (off) ..= 1.0.
    #[serde(default)]
    pub wet_edge: f32,
    /// Secondary stamp tip multiplied into dab coverage (dual-brush
    /// texture; None = single tip). Imported via "dual tip" in the app.
    #[serde(default)]
    pub dual_tip: Option<BrushTip>,
    /// Secondary tip span as a fraction of `size`, 0.1 ..= 4.0.
    #[serde(default = "def_dual_scale")]
    pub dual_scale: f32,
    /// Shape rotation relative to the stroke direction, -1.0 (inverse) ..
    /// 1.0 (follows the stroke). Only used when `angle_follow` is false.
    #[serde(default = "def_shape_rotation")]
    pub shape_rotation: f32,
    /// Random shape rotation per stamp, 0.0 (upright) ..= 1.0 (full turn).
    #[serde(default = "def_angle_scatter")]
    pub angle_scatter: f32,
    /// Shapes stamped per dab, 1 ..= 16.
    #[serde(default = "def_shape_count")]
    pub shape_count: u8,
    /// Random reduction of `shape_count` per dab, 0.0 (fixed) ..= 1.0.
    #[serde(default)]
    pub shape_count_jitter: f32,
    /// Stroke start/end taper (Procreate Taper panel).
    #[serde(default)]
    pub taper: Taper,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            size: 24.0,
            opacity: 1.0,
            // Hard-ish by convention: a brush you cannot see the edge of
            // reads as out of focus, and every paint app ships its default
            // round around 80% hardness. Softness is one slider away.
            softness: 0.2,
            spacing: 0.15,
            eraser: false,
            tip: None,
            pressure_size: def_pressure_size(),
            pressure_opacity: def_pressure_opacity(),
            pressure_curve: None,
            pressure_opacity_curve: None,
            velocity_size: def_velocity_size(),
            velocity_opacity: def_velocity_opacity(),
            velocity_size_curve: None,
            velocity_opacity_curve: None,
            grain: def_grain(),
            grain_scale: def_grain_scale(),
            grain_tip: None,
            grain_angle: 0.0,
            roundness: def_roundness(),
            roundness_jitter: 0.0,
            size_jitter: 0.0,
            opacity_jitter: 0.0,
            angle: 0.0,
            angle_follow: def_angle_follow(),
            scatter: def_scatter(),
            stabilizer: def_stabilizer(),
            blend: super::document::BlendMode::Normal,
            wet_edge: 0.0,
            dual_tip: None,
            dual_scale: def_dual_scale(),
            shape_rotation: def_shape_rotation(),
            angle_scatter: def_angle_scatter(),
            shape_count: def_shape_count(),
            shape_count_jitter: 0.0,
            taper: Taper::NONE,
        }
    }
}

impl Brush {
    /// Effective dab diameter for a pressure/speed sample. At full pressure
    /// and zero speed this returns exactly `size` (old behavior preserved).
    pub fn resolve_size(&self, pressure: f32, speed_norm: f32) -> f32 {
        let pressure = self.curved_pressure(pressure);
        let p = self.pressure_size.clamp(-1.0, 1.0);
        let sized = self.size * (1.0 - p * (1.0 - pressure));
        (sized * self.curved_velocity_size(speed_norm)).max(0.0)
    }

    /// Effective per-dab opacity for a pressure/speed sample. At full
    /// pressure and zero speed this returns exactly `opacity`.
    pub fn resolve_opacity(&self, pressure: f32, speed_norm: f32) -> f32 {
        let pressure = self.curved_opacity_pressure(pressure);
        let p = self.pressure_opacity.clamp(-1.0, 1.0);
        let op = self.opacity * (1.0 - p * (1.0 - pressure));
        (op * self.curved_velocity_opacity(speed_norm)).clamp(0.0, 1.0)
    }

    fn curved_pressure(&self, pressure: f32) -> f32 {
        let Some(curve) = self.pressure_curve.as_deref() else {
            return clamp01(pressure);
        };
        sample_curve(curve, pressure)
    }

    fn curved_opacity_pressure(&self, pressure: f32) -> f32 {
        let Some(curve) = self.pressure_opacity_curve.as_deref() else {
            return self.curved_pressure(pressure);
        };
        sample_curve(curve, pressure)
    }

    fn curved_velocity_size(&self, speed_norm: f32) -> f32 {
        let speed = clamp01(speed_norm);
        let Some(curve) = self.velocity_size_curve.as_deref() else {
            return 1.0 - 0.6 * speed * clamp01(self.velocity_size);
        };
        sample_curve(curve, speed)
    }

    fn curved_velocity_opacity(&self, speed_norm: f32) -> f32 {
        let speed = clamp01(speed_norm);
        let Some(curve) = self.velocity_opacity_curve.as_deref() else {
            return 1.0 - 0.4 * speed * clamp01(self.velocity_opacity);
        };
        sample_curve(curve, speed)
    }
}

/// Piecewise-linear lookup through a sorted pressure curve.
fn sample_curve(curve: &[[f32; 2]], pressure: f32) -> f32 {
    let pressure = clamp01(pressure);
    let Some(first) = curve.first() else {
        return pressure;
    };
    if pressure <= first[0] {
        return clamp01(first[1]);
    }
    for pair in curve.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if pressure <= b[0] {
            let span = b[0] - a[0];
            let t = if span > f32::EPSILON {
                (pressure - a[0]) / span
            } else {
                0.0
            };
            return clamp01(a[1] + (b[1] - a[1]) * t);
        }
    }
    clamp01(curve[curve.len() - 1][1])
}

impl Document {
    /// Open a stroke scope: the per-pixel coverage peak resets, so the next
    /// dabs measure against a clean slate. Stroke walkers call this at the
    /// first segment; call it directly after an explicit lift.
    pub fn begin_stroke(&mut self) {
        self.stroke_peak = Some(crate::raster::Raster::mask(self.w, self.h));
        self.stroke_rect = None;
    }

    /// Close the active stroke: the next dab opens a fresh one (and the
    /// peak raster is freed).
    pub fn end_stroke(&mut self) {
        self.stroke_peak = None;
        self.stroke_rect = None;
        self.liquify_end();
    }

    /// Canvas bbox (inclusive) painted by the active stroke, if any.
    pub fn stroke_rect(&self) -> Option<(u32, u32, u32, u32)> {
        self.stroke_rect
    }

    /// Increment that lifts a pixel's stroke coverage from `seen` to
    /// `target` (both 0..=255). Source-over is sequential, so the second
    /// pass has to close the *remaining* gap: `(target - seen) / (1 - seen)`.
    /// Straight subtraction leaves the stroke short of its target (a soft
    /// brush plateaus around two thirds of its opacity).
    fn stroke_delta(target: u8, seen: u8) -> u8 {
        if target <= seen {
            return 0;
        }
        let gap = (target - seen) as u32;
        let left = (255 - seen) as u32;
        ((gap * 255 + left / 2) / left) as u8
    }

    /// Grow a stroke bbox (inclusive) from a dab's canvas bounds. A free
    /// function so dab loops can pass the field alone, keeping the layers
    /// borrow disjoint.
    fn grow_stroke_rect(
        rect: &mut Option<(u32, u32, u32, u32)>,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    ) {
        *rect = Some(match *rect {
            Some((ax, ay, bx, by)) => (ax.min(x0), ay.min(y0), bx.max(x1), by.max(y1)),
            None => (x0, y0, x1, y1),
        });
    }

    /// Stamp one dab centered at canvas pixel `(x, y)` on the active layer,
    /// mirrored per [`Document::symmetry`].
    /// Pixels outside [`Document::selection`] (when set) are skipped.
    /// With [`PaintTarget::Mask`] the brush paints the layer's reveal mask
    /// instead (auto-created white on first stroke; color luminance is the
    /// mask value, the eraser reveals). Shapes, fills and flood fill always
    /// paint color.
    pub fn dab(&mut self, brush: &Brush, color: Color, x: f32, y: f32) {
        // A lone dab is its own stroke: without a fresh peak, repeated taps
        // would keep folding into the previous gesture's coverage map.
        self.begin_stroke();
        // Single dabs (stroke starts, shape taps) paint at full pressure:
        // resolve(1, 0) returns exactly size/opacity, angle 0 keeps the
        // dab round unless the preset squeezes it.
        let size = brush.resolve_size(1.0, 0.0);
        let opacity = brush.resolve_opacity(1.0, 0.0);
        self.dab_dyn(brush, color, x, y, size, opacity, 0.0);
    }

    /// Stamp one dab with pre-resolved size/opacity/angle, mirrored per
    /// [`Document::symmetry`]. The [`Document::stroke_to_dyn`] walker
    /// resolves dynamics per dab and calls this; [`Document::dab`] is the
    /// full-pressure shorthand above.
    #[allow(clippy::too_many_arguments)]
    fn dab_dyn(
        &mut self,
        brush: &Brush,
        color: Color,
        x: f32,
        y: f32,
        size: f32,
        opacity: f32,
        angle: f32,
    ) {
        let (ccx, ccy) = self.symmetry_center_px();
        let pts = self
            .symmetry
            .mirrored_points(self.symmetry_segments, ccx, ccy, x, y);
        let mut done: Vec<(f32, f32)> = Vec::with_capacity(4);
        let target = self.paint_target;
        let orientation = dab_orientation(brush, angle);
        let count = shape_count_for(brush, x, y);
        for (mx, my) in pts {
            if done
                .iter()
                .any(|&(dx, dy)| (dx - mx).abs() < 0.001 && (dy - my).abs() < 0.001)
            {
                continue;
            }
            done.push((mx, my));
            for index in 0..count {
                let a = orientation + angle_jitter(brush, mx, my, index);
                let (dab_size, dab_opacity) = jittered_dab(brush, mx, my, index, size, opacity);
                match target {
                    super::document::PaintTarget::Color => {
                        self.dab_one_resolved(brush, color, mx, my, dab_size, dab_opacity, a)
                    }
                    super::document::PaintTarget::Mask => {
                        self.dab_mask_resolved(brush, color, mx, my, dab_size, dab_opacity, a)
                    }
                }
            }
        }
    }

    /// Mask dab through the parametric falloff or the stamp tip; shared
    /// selection/symmetry gating comes from [`Document::dab`].
    #[allow(clippy::too_many_arguments)]
    fn dab_mask_resolved(
        &mut self,
        brush: &Brush,
        color: Color,
        x: f32,
        y: f32,
        size: f32,
        opacity: f32,
        angle: f32,
    ) {
        if brush.tip.is_some() {
            return self.dab_mask_stamp_resolved(brush, color, x, y, size, opacity, angle);
        }
        let (w_u32, h_u32) = (self.w, self.h);
        let radius = (size * 0.5).max(0.5);
        let strength = (opacity * size.min(1.0)).clamp(0.0, 1.0);
        if strength <= 0.0 || size <= 0.0 || self.layers.get(self.active).is_none() {
            return;
        }
        // Fully outside: a complete no-op (no mask, no dirty rect).
        if x + radius < 0.0
            || y + radius < 0.0
            || x - radius >= w_u32 as f32
            || y - radius >= h_u32 as f32
        {
            return;
        }
        // First mask stroke materializes a white mask (covered by the
        // pre-stroke checkpoint, so undo removes it again).
        if self.layers[self.active].mask.is_none() {
            self.add_mask(self.active);
        }
        // One extra pixel each way for the anti-aliasing rim.
        let x0 = (x - radius - 1.0).floor().max(0.0) as u32;
        let y0 = (y - radius - 1.0).floor().max(0.0) as u32;
        let x1 = (x + radius + 1.0).ceil().min(w_u32 as f32 - 1.0).max(0.0) as u32;
        let y1 = (y + radius + 1.0).ceil().min(h_u32 as f32 - 1.0).max(0.0) as u32;
        self.mark_dirty(x0, y0, x1, y1);
        let selection = self.selection;
        let active = self.active;
        if active >= self.layers.len() {
            return;
        }
        // Moved layers stay fully paintable: absorb the offset (slides
        // buffer, trims far tails, content unmoved) before resolving slots.
        if self.absorb_offset(active, x0, y0, x1, y1) {
            self.mark_full_dirty();
        }
        let sel_mask = self.selection_mask.as_deref();
        let w = w_u32;
        let value = if brush.eraser {
            255.0
        } else {
            let c = color.0;
            (299.0 * c[0] as f32 + 587.0 * c[1] as f32 + 114.0 * c[2] as f32) / 1000.0
        };
        Document::grow_stroke_rect(&mut self.stroke_rect, x0, y0, x1, y1);
        let mut peak = self.stroke_peak.as_mut();
        let edge = brush.softness.clamp(0.0, 1.0);
        // Layer offset (Move tool): canvas pixel -> buffer slot.
        let (lox, loy) = (self.layers[active].ox, self.layers[active].oy);
        let bh = h_u32 as i32;
        let round = dab_roundness(brush, x, y);
        let (cos_a, sin_a) = (angle.cos(), angle.sin());
        let grain = clamp01(brush.grain);
        let Some(layer_mask) = self.layers[active].mask.as_mut() else {
            return;
        };
        for py in y0..=y1 {
            for px in x0..=x1 {
                if let Some((sx0, sy0, sx1, sy1)) = selection {
                    if px < sx0 || px > sx1 || py < sy0 || py > sy1 {
                        continue;
                    }
                }
                if let Some(m) = sel_mask {
                    if m.get((py * w + px) as usize).copied().unwrap_or(0) == 0 {
                        continue;
                    }
                }
                // Dab-frame coords: u along the stroke, v across (squeezed
                // by roundness). At roundness 1.0 this is the old circle.
                let (dx, dy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                let (u, v) = (dx * cos_a + dy * sin_a, (-dx * sin_a + dy * cos_a) / round);
                let d = (u * u + v * v).sqrt() / radius;
                if d > 1.0 + 0.5 / radius {
                    continue;
                }
                let falloff = if edge <= 0.0 || d <= 1.0 - edge {
                    1.0
                } else {
                    (1.0 - (d - (1.0 - edge)) / edge).clamp(0.0, 1.0)
                };
                let coverage = ((1.0 - d) * radius + 0.5).clamp(0.0, 1.0);
                let mut a = (falloff * coverage * strength * Document::sel_k(sel_mask, w, px, py))
                    .clamp(0.0, 1.0);
                if grain > 0.0 {
                    a *= 1.0 - grain * (1.0 - grain_sample(brush, px, py, size));
                }
                if a <= 0.0 {
                    continue;
                }
                // Store at the layer-offset slot (paint lands shifted on
                // moved layers, exactly where the composite reads it).
                let (lx, ly) = (px as i32 - lox, py as i32 - loy);
                if lx < 0 || ly < 0 || lx >= w as i32 || ly >= bh {
                    continue;
                }
                let o = (ly as u32 * w + lx as u32) as usize;
                if o >= layer_mask.len() {
                    continue;
                }
                // Mask coverage telescopes like Normal paint: the strongest
                // dab this stroke sets the pixel, later dabs add nothing.
                let a = match peak.as_mut() {
                    Some(p) => {
                        let i = (py * w + px) as usize;
                        let seen = p.get(i);
                        let da = (a * 255.0).round() as u8;
                        if da <= seen {
                            continue;
                        }
                        p.set(i, da);
                        Document::stroke_delta(da, seen) as f32 / 255.0
                    }
                    None => a,
                };
                layer_mask.set(
                    o,
                    (value * a + layer_mask.get(o) as f32 * (1.0 - a)).round() as u8,
                );
            }
        }
    }

    /// Mask dab through an imported stamp tip (bilinear at brush size, like
    /// [`Document::dab_stamp`] but writing luminance).
    #[allow(clippy::too_many_arguments)]
    fn dab_mask_stamp_resolved(
        &mut self,
        brush: &Brush,
        color: Color,
        x: f32,
        y: f32,
        size: f32,
        opacity: f32,
        angle: f32,
    ) {
        let Some(tip) = brush.tip.as_ref() else {
            return;
        };
        if tip.w == 0 || tip.h == 0 || tip.mask.len() != (tip.w * tip.h) as usize {
            return;
        }
        let (w_u32, h_u32) = (self.w, self.h);
        let strength = (opacity * size.min(1.0)).clamp(0.0, 1.0);
        if strength <= 0.0 || size <= 0.0 || self.layers.get(self.active).is_none() {
            return;
        }
        let scale = size / tip.w as f32;
        let th = (tip.h as f32 * scale).max(1.0);
        // A steered stamp sweeps its corners: bound the circumscribed
        // circle so rotation never clips. Unsteered keeps the old box.
        let follow = brush.angle_follow || angle != 0.0 || brush.angle != 0.0;
        let (ex, ey) = if follow {
            let r = (size * size + th * th).sqrt() * 0.5;
            (r, r)
        } else {
            (size * 0.5, th * 0.5)
        };
        // Fully outside: a complete no-op (no mask, no dirty rect).
        if x + ex < 0.0 || y + ey < 0.0 || x - ex >= w_u32 as f32 || y - ey >= h_u32 as f32 {
            return;
        }
        if self.layers[self.active].mask.is_none() {
            self.add_mask(self.active);
        }
        let x0 = (x - ex).floor().max(0.0) as u32;
        let y0 = (y - ey).floor().max(0.0) as u32;
        let x1 = (x + ex).ceil().min(w_u32 as f32 - 1.0).max(0.0) as u32;
        let y1 = (y + ey).ceil().min(h_u32 as f32 - 1.0).max(0.0) as u32;
        self.mark_dirty(x0, y0, x1, y1);
        let selection = self.selection;
        let active = self.active;
        if active >= self.layers.len() {
            return;
        }
        // Moved layers stay fully paintable: absorb the offset (slides
        // buffer, trims far tails, content unmoved) before resolving slots.
        if self.absorb_offset(active, x0, y0, x1, y1) {
            self.mark_full_dirty();
        }
        let sel_mask = self.selection_mask.as_deref();
        let w = w_u32;
        let value = if brush.eraser {
            255.0
        } else {
            let c = color.0;
            (299.0 * c[0] as f32 + 587.0 * c[1] as f32 + 114.0 * c[2] as f32) / 1000.0
        };
        let round = dab_roundness(brush, x, y);
        // Layer offset (Move tool): canvas pixel -> buffer slot.
        let (lox, loy) = (self.layers[active].ox, self.layers[active].oy);
        let bh = h_u32 as i32;
        Document::grow_stroke_rect(&mut self.stroke_rect, x0, y0, x1, y1);
        let mut peak = self.stroke_peak.as_mut();
        let (cos_a, sin_a) = (angle.cos(), angle.sin());
        let grain = clamp01(brush.grain);
        let half_w = size * 0.5;
        let half_h = th * 0.5;
        let Some(layer_mask) = self.layers[active].mask.as_mut() else {
            return;
        };
        for py in y0..=y1 {
            for px in x0..=x1 {
                if let Some((sx0, sy0, sx1, sy1)) = selection {
                    if px < sx0 || px > sx1 || py < sy0 || py > sy1 {
                        continue;
                    }
                }
                if let Some(m) = sel_mask {
                    if m.get((py * w + px) as usize).copied().unwrap_or(0) == 0 {
                        continue;
                    }
                }
                // Stamp-frame coords: un-rotate into the tip when steered,
                // squeeze across the stroke by roundness. Off-tip pixels
                // are transparent (clean rotated corners).
                let (ox, oy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                let (rx, ry) = if follow {
                    (ox * cos_a + oy * sin_a, (-ox * sin_a + oy * cos_a) / round)
                } else {
                    (ox, oy / round)
                };
                let texel = sample_tip(tip, (rx + half_w) / scale, (ry + half_h) / scale);
                if texel <= 0.0 {
                    continue;
                }
                let mut a =
                    (texel * strength * Document::sel_k(sel_mask, w, px, py)).clamp(0.0, 1.0);
                if grain > 0.0 {
                    a *= 1.0 - grain * (1.0 - grain_sample(brush, px, py, size));
                }
                if a <= 0.0 {
                    continue;
                }
                // Store at the layer-offset slot (see parametric mask dab).
                let (lx, ly) = (px as i32 - lox, py as i32 - loy);
                if lx < 0 || ly < 0 || lx >= w as i32 || ly >= bh {
                    continue;
                }
                let o = (ly as u32 * w + lx as u32) as usize;
                if o >= layer_mask.len() {
                    continue;
                }
                // Mask coverage telescopes like Normal paint (see the round
                // mask dab).
                let a = match peak.as_mut() {
                    Some(p) => {
                        let i = (py * w + px) as usize;
                        let seen = p.get(i);
                        let da = (a * 255.0).round() as u8;
                        if da <= seen {
                            continue;
                        }
                        p.set(i, da);
                        Document::stroke_delta(da, seen) as f32 / 255.0
                    }
                    None => a,
                };
                layer_mask.set(
                    o,
                    (value * a + layer_mask.get(o) as f32 * (1.0 - a)).round() as u8,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn dab_one_resolved(
        &mut self,
        brush: &Brush,
        color: Color,
        x: f32,
        y: f32,
        size: f32,
        opacity: f32,
        angle: f32,
    ) {
        if brush.tip.is_some() {
            return self.dab_stamp_resolved(brush, color, x, y, size, opacity, angle);
        }
        let (w_u32, h_u32) = (self.w, self.h);
        let (w, h) = (w_u32 as f32, h_u32 as f32);
        let radius = (size * 0.5).max(0.5);
        let strength = (opacity * size.min(1.0)).clamp(0.0, 1.0);
        if strength <= 0.0 || size <= 0.0 || self.layers.get(self.active).is_none() {
            return;
        }
        // One extra pixel each way: the anti-aliasing rim reaches half a
        // pixel past the geometric disc.
        let x0 = (x - radius - 1.0).floor().max(0.0) as u32;
        let y0 = (y - radius - 1.0).floor().max(0.0) as u32;
        let x1 = (x + radius + 1.0).ceil().min(w - 1.0).max(0.0) as u32;
        let y1 = (y + radius + 1.0).ceil().min(h - 1.0).max(0.0) as u32;
        self.mark_dirty(x0, y0, x1, y1);
        let selection = self.selection;
        let active = self.active;
        if active >= self.layers.len() {
            return;
        }
        // Moved layers stay fully paintable: absorb the offset (slides
        // buffer, trims far tails, content unmoved) before resolving slots.
        if self.absorb_offset(active, x0, y0, x1, y1) {
            self.mark_full_dirty();
        }
        let mask = self.selection_mask.as_deref();
        let w = w_u32;
        // Layer offset (Move tool): canvas pixel -> buffer slot.
        let (lox, loy) = (self.layers[active].ox, self.layers[active].oy);
        let bh = h_u32 as i32;
        Document::grow_stroke_rect(&mut self.stroke_rect, x0, y0, x1, y1);
        // Normal-blend paint and the eraser telescope: a pixel keeps its
        // strongest dab this stroke, so overlapping dabs stop summing past
        // the brush opacity. Other blends fold per dab, as defined.
        let track = brush.eraser || brush.blend == super::document::BlendMode::Normal;
        let mut peak = self.stroke_peak.as_mut().filter(|_| track);
        let layer = &mut self.layers[active];
        for py in y0..=y1 {
            for px in x0..=x1 {
                if let Some((sx0, sy0, sx1, sy1)) = selection {
                    if px < sx0 || px > sx1 || py < sy0 || py > sy1 {
                        continue;
                    }
                }
                if let Some(m) = mask {
                    if m.get((py * w + px) as usize).copied().unwrap_or(0) == 0 {
                        continue;
                    }
                }
                // 1.0 inside the hard core, linear fade across the soft edge.
                let edge = brush.softness.clamp(0.0, 1.0);
                let round = dab_roundness(brush, x, y);
                let (cos_a, sin_a) = (angle.cos(), angle.sin());
                let grain = clamp01(brush.grain);
                // Dab-frame coords (see mask dab): roundness 1.0 keeps the
                // old circular falloff bit-for-bit.
                let (dx, dy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                let (u, v) = (dx * cos_a + dy * sin_a, (-dx * sin_a + dy * cos_a) / round);
                let d = (u * u + v * v).sqrt() / radius;
                if d > 1.0 + 0.5 / radius {
                    continue;
                }
                let falloff = if edge <= 0.0 {
                    // A hard disc is all core; the coverage term below is
                    // what gives it an edge.
                    1.0
                } else if d <= 1.0 - edge {
                    1.0
                } else {
                    (1.0 - (d - (1.0 - edge)) / edge).clamp(0.0, 1.0)
                };
                // Analytic coverage across the last pixel of the rim: a hard
                // disc keeps its edge but stops stair-stepping.
                let coverage = ((1.0 - d) * radius + 0.5).clamp(0.0, 1.0);
                let mut fa = (falloff * coverage * strength * Document::sel_k(mask, w, px, py))
                    .clamp(0.0, 1.0);
                if grain > 0.0 {
                    fa *= 1.0 - grain * (1.0 - grain_sample(brush, px, py, size));
                }
                // M39 depth: dual-tip texture, then wet-edge fringe boost.
                if brush.dual_tip.is_some() {
                    fa *= dual_factor(&brush.dual_tip, brush.dual_scale, size, dx, dy);
                }
                fa = wet_boost(fa, brush.wet_edge);
                let a = (fa * 255.0).round() as u8;
                if a == 0 {
                    continue;
                }
                // Store at the layer-offset slot (see mask dabs).
                let (lx, ly) = (px as i32 - lox, py as i32 - loy);
                if lx < 0 || ly < 0 || lx >= w as i32 || ly >= bh {
                    continue;
                }
                let o = ((ly as u32 * w + lx as u32) * 4) as usize;
                if o + 3 >= layer.pixels.len() {
                    continue;
                }
                // Composite only what this dab adds over the stroke peak;
                // repeated dabs over one pixel add nothing further.
                let a = match peak.as_mut() {
                    Some(p) => {
                        let i = (py * w + px) as usize;
                        let seen = p.get(i);
                        if a <= seen {
                            continue;
                        }
                        p.set(i, a);
                        Document::stroke_delta(a, seen)
                    }
                    None => a,
                };
                if brush.eraser {
                    // Destination-out: scale existing alpha down. A pixel the
                    // erase takes to alpha 0 is canonicalised to [0, 0, 0, 0]
                    // so invisible pixels carry no colour: all-zero tiles are
                    // then exactly the invisible ones the v12 writer omits.
                    // Alpha > 0 keeps its RGB untouched.
                    let keep = 255u16.saturating_sub(a as u16);
                    let alpha = ((layer.pixels.get(o + 3) as u16 * keep) / 255) as u8;
                    if alpha == 0 {
                        layer.pixels.set4(o, [0, 0, 0, 0]);
                    } else {
                        layer.pixels.set(o + 3, alpha);
                    }
                } else {
                    composite_dab(&mut layer.pixels, o, color.0, a as f32 / 255.0, brush.blend);
                }
            }
        }
    }

    /// Stamp a grayscale-tip dab centered at `(x, y)`: the tip is scaled so
    /// its width spans `brush.size` canvas pixels (bilinear, so a magnified
    /// tip keeps a soft edge), and each texel contributes `mask * opacity`
    /// exactly like falloff alpha.
    /// Selection, symmetry and eraser handling match [`Document::dab`].
    #[allow(clippy::too_many_arguments)]
    fn dab_stamp_resolved(
        &mut self,
        brush: &Brush,
        color: Color,
        x: f32,
        y: f32,
        size: f32,
        opacity: f32,
        angle: f32,
    ) {
        let Some(tip) = brush.tip.as_ref() else {
            return;
        };
        if tip.w == 0 || tip.h == 0 || tip.mask.len() != (tip.w * tip.h) as usize {
            return;
        }
        let (w_u32, h_u32) = (self.w, self.h);
        let (w, h) = (w_u32 as f32, h_u32 as f32);
        let strength = (opacity * size.min(1.0)).clamp(0.0, 1.0);
        if strength <= 0.0 || size <= 0.0 || self.layers.get(self.active).is_none() {
            return;
        }
        let scale = size / tip.w as f32;
        let th = (tip.h as f32 * scale).max(1.0);
        // Steered stamps sweep their corners: bound the circumscribed
        // circle so rotation never clips. Unsteered keeps the old box.
        let follow = brush.angle_follow || angle != 0.0 || brush.angle != 0.0;
        let (ex, ey) = if follow {
            let r = (size * size + th * th).sqrt() * 0.5;
            (r, r)
        } else {
            (size * 0.5, th * 0.5)
        };
        let x0 = (x - ex).floor().max(0.0) as u32;
        let y0 = (y - ey).floor().max(0.0) as u32;
        let x1 = (x + ex).ceil().min(w - 1.0).max(0.0) as u32;
        let y1 = (y + ey).ceil().min(h - 1.0).max(0.0) as u32;
        self.mark_dirty(x0, y0, x1, y1);
        let selection = self.selection;
        let active = self.active;
        if active >= self.layers.len() {
            return;
        }
        // Moved layers stay fully paintable: absorb the offset (slides
        // buffer, trims far tails, content unmoved) before resolving slots.
        if self.absorb_offset(active, x0, y0, x1, y1) {
            self.mark_full_dirty();
        }
        let mask = self.selection_mask.as_deref();
        let w = w_u32;
        let round = dab_roundness(brush, x, y);
        // Layer offset (Move tool): canvas pixel -> buffer slot.
        let (lox, loy) = (self.layers[active].ox, self.layers[active].oy);
        let bh = h_u32 as i32;
        Document::grow_stroke_rect(&mut self.stroke_rect, x0, y0, x1, y1);
        let track = brush.eraser || brush.blend == super::document::BlendMode::Normal;
        let mut peak = self.stroke_peak.as_mut().filter(|_| track);
        let layer = &mut self.layers[active];
        let (cos_a, sin_a) = (angle.cos(), angle.sin());
        let grain = clamp01(brush.grain);
        let half_w = size * 0.5;
        let half_h = th * 0.5;
        for py in y0..=y1 {
            for px in x0..=x1 {
                if let Some((sx0, sy0, sx1, sy1)) = selection {
                    if px < sx0 || px > sx1 || py < sy0 || py > sy1 {
                        continue;
                    }
                }
                if let Some(m) = mask {
                    if m.get((py * w + px) as usize).copied().unwrap_or(0) == 0 {
                        continue;
                    }
                }
                // Stamp-frame coords (see mask stamp): un-rotate when
                // steered, squeeze across the stroke by roundness. Off-tip
                // pixels are transparent (clean rotated corners).
                let (ox, oy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                let (rx, ry) = if follow {
                    (ox * cos_a + oy * sin_a, (-ox * sin_a + oy * cos_a) / round)
                } else {
                    (ox, oy / round)
                };
                let texel = sample_tip(tip, (rx + half_w) / scale, (ry + half_h) / scale);
                if texel <= 0.0 {
                    continue;
                }
                let mut fa = (texel * strength * Document::sel_k(mask, w, px, py)).clamp(0.0, 1.0);
                if grain > 0.0 {
                    fa *= 1.0 - grain * (1.0 - grain_sample(brush, px, py, size));
                }
                // M39 depth: dual-tip texture, then wet-edge fringe boost.
                if brush.dual_tip.is_some() {
                    fa *= dual_factor(&brush.dual_tip, brush.dual_scale, size, ox, oy);
                }
                fa = wet_boost(fa, brush.wet_edge);
                let a = (fa * 255.0).round() as u8;
                if a == 0 {
                    continue;
                }
                // Store at the layer-offset slot (see mask dabs).
                let (lx, ly) = (px as i32 - lox, py as i32 - loy);
                if lx < 0 || ly < 0 || lx >= w as i32 || ly >= bh {
                    continue;
                }
                let o = ((ly as u32 * w + lx as u32) * 4) as usize;
                if o + 3 >= layer.pixels.len() {
                    continue;
                }
                let a = match peak.as_mut() {
                    Some(p) => {
                        let i = (py * w + px) as usize;
                        let seen = p.get(i);
                        if a <= seen {
                            continue;
                        }
                        p.set(i, a);
                        Document::stroke_delta(a, seen)
                    }
                    None => a,
                };
                if brush.eraser {
                    // Destination-out, canonicalised like the round dab: an
                    // alpha-0 pixel becomes [0, 0, 0, 0] (see `dab`).
                    let keep = 255u16.saturating_sub(a as u16);
                    let alpha = ((layer.pixels.get(o + 3) as u16 * keep) / 255) as u8;
                    if alpha == 0 {
                        layer.pixels.set4(o, [0, 0, 0, 0]);
                    } else {
                        layer.pixels.set(o + 3, alpha);
                    }
                } else {
                    composite_dab(&mut layer.pixels, o, color.0, a as f32 / 255.0, brush.blend);
                }
            }
        }
    }

    /// Advance a stroke from `from` to `to`, laying dabs every
    /// `spacing * size` pixels. Returns the leftover distance for chaining.
    /// Call [`Document::checkpoint`] once when the stroke starts.
    /// Full-pressure shorthand over [`Document::stroke_to_dyn`]: the segment
    /// direction still steers `angle_follow` brushes (shapes, previews).
    pub fn stroke_to(
        &mut self,
        brush: &Brush,
        color: Color,
        from: Option<(f32, f32)>,
        to: (f32, f32),
        leftover: f32,
    ) -> (Option<(f32, f32)>, f32) {
        let angle = match from {
            Some((sx, sy)) => (to.1 - sy).atan2(to.0 - sx),
            None => 0.0,
        };
        let dyn_ = DabDynamics {
            pressure: 1.0,
            speed_norm: 0.0,
            angle,
            taper: 1.0,
        };
        self.stroke_to_dyn(brush, color, from, dyn_, to, dyn_, leftover)
    }

    /// Advance a stroke with per-endpoint dynamics, interpolating pressure,
    /// speed and direction per dab so segments taper, fade and turn across
    /// their dabs. `from_dyn` is ignored when `from` is `None` (stroke start
    /// paints one full `to_dyn` dab). Returns the leftover distance plus the
    /// new endpoint for chaining. Call [`Document::checkpoint`] once when
    /// the stroke starts.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_to_dyn(
        &mut self,
        brush: &Brush,
        color: Color,
        from: Option<(f32, f32)>,
        from_dyn: DabDynamics,
        to: (f32, f32),
        to_dyn: DabDynamics,
        leftover: f32,
    ) -> (Option<(f32, f32)>, f32) {
        let Some((sx, sy)) = from else {
            // Stroke start: a fresh coverage peak keeps this gesture's dabs
            // from piling onto each other.
            self.begin_stroke();
            let taper = clamp01(to_dyn.taper);
            let size = brush.resolve_size(to_dyn.pressure, to_dyn.speed_norm)
                * brush.taper.size_factor(taper, to_dyn.pressure);
            let opacity = brush.resolve_opacity(to_dyn.pressure, to_dyn.speed_norm)
                * (1.0 - clamp01(brush.taper.opacity) * (1.0 - taper));
            let (ox, oy) = scatter_offset(brush, to.0, to.1, size);
            self.dab_dyn(
                brush,
                color,
                to.0 + ox,
                to.1 + oy,
                size,
                opacity,
                to_dyn.angle,
            );
            return (Some(to), 0.0);
        };
        let (dx, dy) = (to.0 - sx, to.1 - sy);
        let dist = (dx * dx + dy * dy).sqrt();
        if dist <= 0.0 {
            return (from, leftover);
        }
        // Lerp direction as unit vectors (stable across +/-PI).
        let (fx, fy) = (from_dyn.angle.cos(), from_dyn.angle.sin());
        let (tx, ty) = (to_dyn.angle.cos(), to_dyn.angle.sin());
        let mut stamp_at = |t: f32| {
            let pressure = from_dyn.pressure + (to_dyn.pressure - from_dyn.pressure) * t;
            let speed = from_dyn.speed_norm + (to_dyn.speed_norm - from_dyn.speed_norm) * t;
            let vx = fx + (tx - fx) * t;
            let vy = fy + (ty - fy) * t;
            let angle = vy.atan2(vx);
            let taper = clamp01(from_dyn.taper + (to_dyn.taper - from_dyn.taper) * t);
            let size =
                brush.resolve_size(pressure, speed) * brush.taper.size_factor(taper, pressure);
            let opacity = brush.resolve_opacity(pressure, speed)
                * (1.0 - clamp01(brush.taper.opacity) * (1.0 - taper));
            let (x, y) = (sx + dx * t, sy + dy * t);
            let (ox, oy) = scatter_offset(brush, x, y, size);
            self.dab_dyn(brush, color, x + ox, y + oy, size, opacity, angle);
            // Step by the dab size actually drawn: spacing is relative to
            // the dab, so a tapering tip keeps overlap (and a dotted brush
            // keeps its relative gaps) all the way to the point.
            size
        };
        let mut travelled = leftover;
        let mut last_dab = None;
        while travelled <= dist {
            let size = stamp_at(travelled / dist);
            last_dab = Some(travelled);
            travelled += (size * brush.spacing).max(1.0);
        }
        let endpoint_painted = last_dab.map(|last| dist - last <= 0.5).unwrap_or(false);
        if !endpoint_painted {
            let _ = stamp_at(1.0);
        }
        (Some(to), travelled - dist)
    }

    /// Advance a smudge stroke: pick up color under the dab and push it
    /// along the path (wet-mixer lite). `strength` 0..=1 scales the mix and
    /// multiplies with `pressure`; size/spacing/softness/roundness come from
    /// the brush, color/opacity/grain do not apply. The finger (running
    /// pickup color) resets when `from` is `None` (stroke start paints one
    /// dab at `to`). Returns the leftover distance plus the new endpoint
    /// for chaining. Call [`Document::checkpoint`] once when the stroke
    /// starts. Color layers only: on a mask target this is a documented
    /// no-op (the app steers there with a status hint).
    pub fn smudge_to(
        &mut self,
        brush: &Brush,
        strength: f32,
        pressure: f32,
        from: Option<(f32, f32)>,
        to: (f32, f32),
        leftover: f32,
    ) -> (Option<(f32, f32)>, f32) {
        if from.is_none() {
            self.smudge_finger = None;
        }
        let size = brush.resolve_size(pressure, 0.0);
        let eff = (strength * pressure.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let step = (size * brush.spacing).max(1.0);
        let Some((sx, sy)) = from else {
            let (ox, oy) = scatter_offset(brush, to.0, to.1, size);
            self.smudge_dab(brush, eff, to.0 + ox, to.1 + oy, size);
            return (Some(to), 0.0);
        };
        let (dx, dy) = (to.0 - sx, to.1 - sy);
        let dist = (dx * dx + dy * dy).sqrt();
        if dist <= 0.0 {
            return (from, leftover);
        }
        let mut travelled = leftover;
        while travelled <= dist {
            let t = travelled / dist;
            let (x, y) = (sx + dx * t, sy + dy * t);
            let (ox, oy) = scatter_offset(brush, x, y, size);
            self.smudge_dab(brush, eff, x + ox, y + oy, size);
            travelled += step;
        }
        (Some(to), travelled - dist)
    }

    /// One smudge dab through the symmetry mirrors (deduped like paint
    /// dabs); mask-target no-op lives here so the walker stays simple.
    fn smudge_dab(&mut self, brush: &Brush, strength: f32, x: f32, y: f32, size: f32) {
        if strength <= 0.0 || size <= 0.0 {
            return;
        }
        if self.paint_target != super::document::PaintTarget::Color {
            return;
        }
        let (ccx, ccy) = self.symmetry_center_px();
        let pts = self
            .symmetry
            .mirrored_points(self.symmetry_segments, ccx, ccy, x, y);
        let mut done: Vec<(f32, f32)> = Vec::with_capacity(4);
        for (mx, my) in pts {
            if done
                .iter()
                .any(|&(dx, dy)| (dx - mx).abs() < 0.001 && (dy - my).abs() < 0.001)
            {
                continue;
            }
            done.push((mx, my));
            self.smudge_dab_resolved(brush, strength, mx, my, size);
        }
    }

    /// Smudge one dab: average the destination disc into the finger, then
    /// mix the finger back over it (falloff-weighted, selection-weighted).
    /// Straight-channel lerp, alpha included, so paint spreads honestly.
    fn smudge_dab_resolved(&mut self, brush: &Brush, strength: f32, x: f32, y: f32, size: f32) {
        let (w_u32, h_u32) = (self.w, self.h);
        let radius = (size * 0.5).max(0.5);
        if self.layers.get(self.active).is_none() {
            return;
        }
        if x + radius < 0.0
            || y + radius < 0.0
            || x - radius >= w_u32 as f32
            || y - radius >= h_u32 as f32
        {
            return;
        }
        let x0 = (x - radius).floor().max(0.0) as u32;
        let y0 = (y - radius).floor().max(0.0) as u32;
        let x1 = (x + radius).ceil().min(w_u32 as f32 - 1.0).max(0.0) as u32;
        let y1 = (y + radius).ceil().min(h_u32 as f32 - 1.0).max(0.0) as u32;
        self.mark_dirty(x0, y0, x1, y1);
        let selection = self.selection;
        let active = self.active;
        if active >= self.layers.len() {
            return;
        }
        let mask = self.selection_mask.as_deref();
        let w = w_u32;
        let (lox, loy) = (self.layers[active].ox, self.layers[active].oy);
        let bh = h_u32 as i32;
        // Pickup: the core pixel biased with a little disc average. Pure
        // core is noisy on texture, pure average dilutes a hard dot to
        // nothing on the first dab; the blend smears strongly and still
        // depletes as fresh ground dilutes the disc share.
        let mut sum = [0f32; 4];
        let mut count = 0f32;
        for py in y0..=y1 {
            for px in x0..=x1 {
                let (lx, ly) = (px as i32 - lox, py as i32 - loy);
                if lx < 0 || ly < 0 || lx >= w as i32 || ly >= bh {
                    continue;
                }
                let o = ((ly as u32 * w + lx as u32) * 4) as usize;
                let pxs = self.layers[active].pixels.get4(o);
                for c in 0..4 {
                    sum[c] += pxs[c] as f32;
                }
                count += 1.0;
            }
        }
        if count <= 0.0 {
            return;
        }
        let avg = [
            sum[0] / count,
            sum[1] / count,
            sum[2] / count,
            sum[3] / count,
        ];
        let (clx, cly) = (x.round() as i32 - lox, y.round() as i32 - loy);
        let core = if clx >= 0 && cly >= 0 && clx < w as i32 && cly < bh {
            let o = ((cly as u32 * w + clx as u32) * 4) as usize;
            let p = self.layers[active].pixels.get4(o);
            Some([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32])
        } else {
            None
        };
        let pickup = match core {
            Some(c) => [
                c[0] * 0.75 + avg[0] * 0.25,
                c[1] * 0.75 + avg[1] * 0.25,
                c[2] * 0.75 + avg[2] * 0.25,
                c[3] * 0.75 + avg[3] * 0.25,
            ],
            None => avg,
        };
        let finger = match self.smudge_finger {
            Some(f) => [
                f[0] + (pickup[0] - f[0]) * 0.5,
                f[1] + (pickup[1] - f[1]) * 0.5,
                f[2] + (pickup[2] - f[2]) * 0.5,
                f[3] + (pickup[3] - f[3]) * 0.5,
            ],
            None => pickup,
        };
        self.smudge_finger = Some(finger);
        // Deposit: lerp destination toward the finger under the falloff.
        let edge = brush.softness.clamp(0.0, 1.0);
        let round = dab_roundness(brush, x, y);
        let layer = &mut self.layers[active];
        for py in y0..=y1 {
            for px in x0..=x1 {
                if let Some((sx0, sy0, sx1, sy1)) = selection {
                    if px < sx0 || px > sx1 || py < sy0 || py > sy1 {
                        continue;
                    }
                }
                if let Some(m) = mask {
                    if m.get((py * w + px) as usize).copied().unwrap_or(0) == 0 {
                        continue;
                    }
                }
                let (dx, dy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                let (u, v) = (dx, dy / round);
                let d = (u * u + v * v).sqrt() / radius;
                if d > 1.0 {
                    continue;
                }
                let falloff = if d <= 1.0 - edge {
                    1.0
                } else {
                    1.0 - (d - (1.0 - edge)) / edge.max(0.001)
                };
                let k = (falloff.clamp(0.0, 1.0) * strength * Self::sel_k(mask, w, px, py))
                    .clamp(0.0, 1.0);
                if k <= 0.0 {
                    continue;
                }
                let (lx, ly) = (px as i32 - lox, py as i32 - loy);
                if lx < 0 || ly < 0 || lx >= w as i32 || ly >= bh {
                    continue;
                }
                let o = ((ly as u32 * w + lx as u32) * 4) as usize;
                if o + 3 >= layer.pixels.len() {
                    continue;
                }
                let mut pxs = layer.pixels.get4(o);
                for (d, f) in pxs.iter_mut().zip(finger.iter()) {
                    let v = *d as f32;
                    *d = (v + (*f - v) * k).round().clamp(0.0, 255.0) as u8;
                }
                layer.pixels.set4(o, pxs);
            }
        }
    }
}

/// Orientation for one stamp: full stroke direction when `angle_follow`
/// is on, otherwise the direction scaled by `shape_rotation` (-1 inverse,
/// 0 upright, 1 follows).
fn dab_orientation(brush: &Brush, stroke_angle: f32) -> f32 {
    let follow = if brush.angle_follow {
        stroke_angle
    } else {
        stroke_angle * brush.shape_rotation.clamp(-1.0, 1.0)
    };
    brush.angle + follow
}

/// Effective roundness for one dab, optionally compressed by deterministic
/// per-position jitter.
fn dab_roundness(brush: &Brush, x: f32, y: f32) -> f32 {
    let roundness = brush.roundness.clamp(0.1, 1.0);
    let jitter = brush.roundness_jitter.clamp(0.0, 1.0);
    if jitter <= 0.0 {
        return roundness;
    }
    let qx = (x * 2.0).round() as i32;
    let qy = (y * 2.0).round() as i32;
    let factor = 1.0 - jitter * hash01(qx ^ 0x6a09, qy ^ 0x3c6e);
    (roundness * factor).max(0.1)
}

/// Size and opacity for one stamped shape after deterministic dynamics jitter.
fn jittered_dab(brush: &Brush, x: f32, y: f32, index: u8, size: f32, opacity: f32) -> (f32, f32) {
    let qx = (x * 2.0).round() as i32 ^ index as i32;
    let qy = (y * 2.0).round() as i32;
    let size_factor = 1.0 - brush.size_jitter.clamp(0.0, 1.0) * hash01(qx ^ 0x1f83, qy);
    let opacity_factor =
        1.0 - brush.opacity_jitter.clamp(0.0, 1.0) * hash01(qx ^ 0x5be0, qy ^ 0x2c1b);
    (size * size_factor, opacity * opacity_factor)
}

/// Shapes for one dab after `shape_count_jitter`: a deterministic subset of
/// the configured count, never below one.
fn shape_count_for(brush: &Brush, x: f32, y: f32) -> u8 {
    let base = brush.shape_count.clamp(1, 16);
    let jitter = brush.shape_count_jitter.clamp(0.0, 1.0);
    if jitter <= 0.0 || base <= 1 {
        return base;
    }
    let qx = (x * 2.0).round() as i32;
    let qy = (y * 2.0).round() as i32;
    let factor = 1.0 - jitter * hash01(qx ^ 0x2c1b, qy ^ 0x7f4a);
    ((base as f32 * factor).round() as u8).clamp(1, base)
}

/// Deterministic rotation offset for the `index`-th shape of a dab. The
/// hash keeps replayed strokes identical while `angle_scatter` widens the
/// offset from upright (0) to a full turn (1).
fn angle_jitter(brush: &Brush, x: f32, y: f32, index: u8) -> f32 {
    let sc = brush.angle_scatter.clamp(0.0, 1.0);
    if sc <= 0.0 {
        return 0.0;
    }
    let qx = (x * 2.0).round() as i32;
    let qy = (y * 2.0).round() as i32;
    let seed = (index as i32).wrapping_mul(0x9e37);
    (hash01(qx ^ seed, qy ^ 0x17ab) * 2.0 - 1.0) * sc * std::f32::consts::PI
}

/// Deterministic scatter offset for a dab center: uniform in
/// `[-scatter*size/2, +scatter*size/2]` per axis, hashed from the dab
/// position (quantized) so strokes replay identically. Returns (0, 0)
/// when `scatter` is 0.
fn scatter_offset(brush: &Brush, x: f32, y: f32, size: f32) -> (f32, f32) {
    let sc = brush.scatter.clamp(0.0, 1.0);
    if sc <= 0.0 || size <= 0.0 {
        return (0.0, 0.0);
    }
    let qx = (x * 2.0).round() as i32;
    let qy = (y * 2.0).round() as i32;
    let ox = (hash01(qx, qy ^ 0x51ab) - 0.5) * sc * size;
    let oy = (hash01(qx ^ 0x2b7e, qy) - 0.5) * sc * size;
    (ox, oy)
}

/// A named brush for a `.pcbrush` library file: display name plus settings.
/// Library brushes never erase; the Eraser tool covers that.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BrushPreset {
    pub name: String,
    pub brush: Brush,
}

/// Maximum brushes in one library file (keeps imports cheap and bounded).
pub const MAX_LIBRARY_BRUSHES: usize = 64;

/// Shared validation for one imported brush. Names are trimmed; numeric
/// ranges match what the desktop sliders can produce and more.
fn check_preset(name: &str, brush: &Brush) -> Result<String, String> {
    let clean = name.trim().to_string();
    if clean.is_empty() {
        return Err("brush has an empty name".to_string());
    }
    if clean.chars().count() > 64 {
        return Err(format!("brush name too long: {clean}"));
    }
    if !(0.0 < brush.size && brush.size <= 400.0) {
        return Err(format!("brush '{clean}' has a bad size"));
    }
    if !(0.0 <= brush.opacity && brush.opacity <= 1.0) {
        return Err(format!("brush '{clean}' has a bad opacity"));
    }
    if !(0.0 <= brush.softness && brush.softness <= 1.0) {
        return Err(format!("brush '{clean}' has a bad softness"));
    }
    if !(0.0 < brush.spacing && brush.spacing <= 2.0) {
        return Err(format!("brush '{clean}' has a bad spacing"));
    }
    if !(-1.0 <= brush.pressure_size && brush.pressure_size <= 1.0) {
        return Err(format!("brush '{clean}' has a bad pressure-size"));
    }
    if !(-1.0 <= brush.pressure_opacity && brush.pressure_opacity <= 1.0) {
        return Err(format!("brush '{clean}' has a bad pressure-opacity"));
    }
    if let Some(curve) = &brush.pressure_curve {
        if !(2..=64).contains(&curve.len()) {
            return Err(format!("brush '{clean}' has a bad pressure curve"));
        }
        let mut previous = f32::NEG_INFINITY;
        for point in curve {
            if !point[0].is_finite()
                || !point[1].is_finite()
                || !(0.0..=1.0).contains(&point[0])
                || !(0.0..=1.0).contains(&point[1])
                || point[0] <= previous
            {
                return Err(format!("brush '{clean}' has a bad pressure curve"));
            }
            previous = point[0];
        }
    }
    if let Some(curve) = &brush.pressure_opacity_curve {
        if !(2..=64).contains(&curve.len()) {
            return Err(format!("brush '{clean}' has a bad pressure-opacity curve"));
        }
        let mut previous = f32::NEG_INFINITY;
        for point in curve {
            if !point[0].is_finite()
                || !point[1].is_finite()
                || !(0.0..=1.0).contains(&point[0])
                || !(0.0..=1.0).contains(&point[1])
                || point[0] <= previous
            {
                return Err(format!("brush '{clean}' has a bad pressure-opacity curve"));
            }
            previous = point[0];
        }
    }
    if !(0.0 <= brush.velocity_size && brush.velocity_size <= 1.0) {
        return Err(format!("brush '{clean}' has a bad velocity-size"));
    }
    if !(0.0 <= brush.velocity_opacity && brush.velocity_opacity <= 1.0) {
        return Err(format!("brush '{clean}' has a bad velocity-opacity"));
    }
    if let Some(curve) = &brush.velocity_size_curve {
        if !(2..=64).contains(&curve.len()) {
            return Err(format!("brush '{clean}' has a bad velocity-size curve"));
        }
        let mut previous = f32::NEG_INFINITY;
        for point in curve {
            if !point[0].is_finite()
                || !point[1].is_finite()
                || !(0.0..=1.0).contains(&point[0])
                || !(0.0..=1.0).contains(&point[1])
                || point[0] <= previous
            {
                return Err(format!("brush '{clean}' has a bad velocity-size curve"));
            }
            previous = point[0];
        }
    }
    if let Some(curve) = &brush.velocity_opacity_curve {
        if !(2..=64).contains(&curve.len()) {
            return Err(format!("brush '{clean}' has a bad velocity-opacity curve"));
        }
        let mut previous = f32::NEG_INFINITY;
        for point in curve {
            if !point[0].is_finite()
                || !point[1].is_finite()
                || !(0.0..=1.0).contains(&point[0])
                || !(0.0..=1.0).contains(&point[1])
                || point[0] <= previous
            {
                return Err(format!("brush '{clean}' has a bad velocity-opacity curve"));
            }
            previous = point[0];
        }
    }
    if !(0.0 <= brush.grain && brush.grain <= 1.0) {
        return Err(format!("brush '{clean}' has a bad grain"));
    }
    if !(0.25 <= brush.grain_scale && brush.grain_scale <= 4.0) {
        return Err(format!("brush '{clean}' has a bad grain scale"));
    }
    if !(0.0..=1.0).contains(&brush.taper.start)
        || !(0.0..=1.0).contains(&brush.taper.end)
        || !(0.0..=1.0).contains(&brush.taper.size)
        || !(0.0..=1.0).contains(&brush.taper.shape)
        || !(0.0..=1.0).contains(&brush.taper.opacity)
        || !(0.0..=1.0).contains(&brush.taper.pressure)
    {
        return Err(format!("brush '{clean}' has bad taper settings"));
    }
    if !(0.1 <= brush.roundness && brush.roundness <= 1.0) {
        return Err(format!("brush '{clean}' has a bad roundness"));
    }
    if !(0.0 <= brush.roundness_jitter && brush.roundness_jitter <= 1.0) {
        return Err(format!("brush '{clean}' has a bad roundness jitter"));
    }
    if !(0.0 <= brush.size_jitter && brush.size_jitter <= 1.0) {
        return Err(format!("brush '{clean}' has a bad size jitter"));
    }
    if !(0.0 <= brush.opacity_jitter && brush.opacity_jitter <= 1.0) {
        return Err(format!("brush '{clean}' has a bad opacity jitter"));
    }
    if !(0.0 <= brush.scatter && brush.scatter <= 1.0) {
        return Err(format!("brush '{clean}' has a bad scatter"));
    }
    if !(-1.0 <= brush.shape_rotation && brush.shape_rotation <= 1.0) {
        return Err(format!("brush '{clean}' has a bad shape rotation"));
    }
    if !(0.0 <= brush.angle_scatter && brush.angle_scatter <= 1.0) {
        return Err(format!("brush '{clean}' has a bad angle scatter"));
    }
    if !(1..=16).contains(&brush.shape_count) {
        return Err(format!("brush '{clean}' has a bad shape count"));
    }
    if !(0.0 <= brush.shape_count_jitter && brush.shape_count_jitter <= 1.0) {
        return Err(format!("brush '{clean}' has a bad shape count jitter"));
    }
    if !(0.0 <= brush.stabilizer && brush.stabilizer <= 1.0) {
        return Err(format!("brush '{clean}' has a bad stabilizer"));
    }
    if brush.eraser {
        return Err(format!("brush '{clean}' must not be an eraser"));
    }
    Ok(clean)
}

/// Save brush presets to a `.pcbrush` file (pretty JSON array of
/// `{name, brush}` records). Eraser flags are forced off on export.
pub fn save_brush_library(path: &std::path::Path, presets: &[BrushPreset]) -> Result<(), String> {
    if presets.is_empty() {
        return Err("nothing to export: the brush library is empty".to_string());
    }
    if presets.len() > MAX_LIBRARY_BRUSHES {
        return Err(format!("too many brushes (max {MAX_LIBRARY_BRUSHES})"));
    }
    let mut clean = Vec::with_capacity(presets.len());
    for p in presets {
        let forced = Brush {
            eraser: false,
            ..p.brush.clone()
        };
        let name = check_preset(&p.name, &forced)?;
        clean.push(BrushPreset {
            name,
            brush: forced,
        });
    }
    let text = serde_json::to_string_pretty(&clean).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// Load a `.pcbrush` file, validating every record. Malformed JSON,
/// out-of-range numbers and eraser brushes are all rejected.
pub fn open_brush_library(path: &std::path::Path) -> Result<Vec<BrushPreset>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let raw: Vec<BrushPreset> =
        serde_json::from_str(&text).map_err(|_| "not a brush library file".to_string())?;
    if raw.is_empty() {
        return Err("brush library is empty".to_string());
    }
    if raw.len() > MAX_LIBRARY_BRUSHES {
        return Err(format!("too many brushes (max {MAX_LIBRARY_BRUSHES})"));
    }
    let mut clean = Vec::with_capacity(raw.len());
    for p in &raw {
        let name = check_preset(&p.name, &p.brush)?;
        clean.push(BrushPreset {
            name,
            brush: p.brush.clone(),
        });
    }
    Ok(clean)
}

/// Built-in brush presets: name plus settings. Original set, roughly grouped
/// from hard sketching to soft painting. M14 gives each preset its own
/// dynamics voice: Pencil bites with pressure and grain, Ink smooths and
/// follows direction, Marker is a flat chisel, Airbrush answers pressure
/// with flow rather than size, Chalk is all grain and scatter.
pub fn presets() -> Vec<(&'static str, Brush)> {
    vec![
        (
            "Pencil",
            Brush {
                size: 3.0,
                opacity: 1.0,
                softness: 0.15,
                spacing: 0.1,
                eraser: false,
                tip: None,
                pressure_size: 0.8,
                pressure_opacity: 0.7,
                velocity_size: 0.5,
                velocity_opacity: 0.4,
                velocity_size_curve: None,
                velocity_opacity_curve: None,
                grain: 0.35,
                grain_scale: 0.8,
                grain_tip: None,
                grain_angle: 0.0,
                roundness: 1.0,
                roundness_jitter: 0.0,
                size_jitter: 0.0,
                opacity_jitter: 0.0,
                angle: 0.0,
                angle_follow: false,
                scatter: 0.05,
                stabilizer: 0.25,
                blend: super::document::BlendMode::Normal,
                wet_edge: 0.0,
                dual_tip: None,
                dual_scale: 0.5,
                shape_rotation: 0.0,
                angle_scatter: 0.0,
                shape_count: 1,
                shape_count_jitter: 0.0,
                pressure_curve: None,
                pressure_opacity_curve: None,
                taper: Taper {
                    start: 0.25,
                    end: 0.35,
                    size: 0.7,
                    shape: 1.0,
                    ..Taper::NONE
                },
            },
        ),
        (
            "Ink Pen",
            Brush {
                size: 6.0,
                opacity: 1.0,
                softness: 0.3,
                spacing: 0.1,
                eraser: false,
                tip: None,
                pressure_size: 0.9,
                pressure_opacity: 0.8,
                velocity_size: 0.4,
                velocity_opacity: 0.3,
                velocity_size_curve: None,
                velocity_opacity_curve: None,
                grain: 0.0,
                grain_scale: 1.0,
                grain_tip: None,
                grain_angle: 0.0,
                roundness: 0.6,
                roundness_jitter: 0.0,
                size_jitter: 0.0,
                opacity_jitter: 0.0,
                angle: 0.0,
                angle_follow: true,
                scatter: 0.0,
                stabilizer: 0.5,
                blend: super::document::BlendMode::Normal,
                wet_edge: 0.0,
                dual_tip: None,
                dual_scale: 0.5,
                shape_rotation: 0.0,
                angle_scatter: 0.0,
                shape_count: 1,
                shape_count_jitter: 0.0,
                pressure_curve: None,
                pressure_opacity_curve: None,
                taper: Taper {
                    start: 0.2,
                    end: 0.4,
                    size: 0.8,
                    shape: 1.0,
                    ..Taper::NONE
                },
            },
        ),
        (
            "Round",
            Brush {
                size: 24.0,
                opacity: 1.0,
                softness: 0.12,
                spacing: 0.12,
                eraser: false,
                tip: None,
                pressure_size: 0.7,
                pressure_opacity: 0.6,
                velocity_size: 0.0,
                velocity_opacity: 0.0,
                velocity_size_curve: None,
                velocity_opacity_curve: None,
                grain: 0.0,
                grain_scale: 1.0,
                grain_tip: None,
                grain_angle: 0.0,
                roundness: 1.0,
                roundness_jitter: 0.0,
                size_jitter: 0.0,
                opacity_jitter: 0.0,
                angle: 0.0,
                angle_follow: false,
                scatter: 0.0,
                stabilizer: 0.35,
                blend: super::document::BlendMode::Normal,
                wet_edge: 0.0,
                dual_tip: None,
                dual_scale: 0.5,
                shape_rotation: 0.0,
                angle_scatter: 0.0,
                shape_count: 1,
                shape_count_jitter: 0.0,
                pressure_curve: None,
                pressure_opacity_curve: None,
                taper: Taper::NONE,
            },
        ),
        (
            "Marker",
            Brush {
                size: 28.0,
                opacity: 0.7,
                softness: 0.05,
                spacing: 0.06,
                eraser: false,
                tip: None,
                pressure_size: 0.4,
                pressure_opacity: 0.5,
                velocity_size: 0.0,
                velocity_opacity: 0.0,
                velocity_size_curve: None,
                velocity_opacity_curve: None,
                grain: 0.0,
                grain_scale: 1.0,
                grain_tip: None,
                grain_angle: 0.0,
                roundness: 0.7,
                roundness_jitter: 0.0,
                size_jitter: 0.0,
                opacity_jitter: 0.0,
                angle: 0.0,
                angle_follow: true,
                scatter: 0.0,
                stabilizer: 0.3,
                blend: super::document::BlendMode::Normal,
                wet_edge: 0.0,
                dual_tip: None,
                dual_scale: 0.5,
                shape_rotation: 0.0,
                angle_scatter: 0.0,
                shape_count: 1,
                shape_count_jitter: 0.0,
                pressure_curve: None,
                pressure_opacity_curve: None,
                taper: Taper::NONE,
            },
        ),
        (
            "Airbrush",
            Brush {
                size: 64.0,
                opacity: 0.3,
                softness: 1.0,
                spacing: 0.04,
                eraser: false,
                tip: None,
                pressure_size: 0.2,
                pressure_opacity: 1.0,
                velocity_size: 0.0,
                velocity_opacity: 0.0,
                velocity_size_curve: None,
                velocity_opacity_curve: None,
                grain: 0.0,
                grain_scale: 1.5,
                grain_tip: None,
                grain_angle: 0.0,
                roundness: 1.0,
                roundness_jitter: 0.0,
                size_jitter: 0.0,
                opacity_jitter: 0.0,
                angle: 0.0,
                angle_follow: false,
                scatter: 0.0,
                stabilizer: 0.4,
                blend: super::document::BlendMode::Normal,
                wet_edge: 0.0,
                dual_tip: None,
                dual_scale: 0.5,
                shape_rotation: 0.0,
                angle_scatter: 0.0,
                shape_count: 1,
                shape_count_jitter: 0.0,
                pressure_curve: None,
                pressure_opacity_curve: None,
                taper: Taper::NONE,
            },
        ),
        (
            "Chalk",
            Brush {
                size: 22.0,
                opacity: 1.0,
                softness: 0.4,
                spacing: 0.1,
                eraser: false,
                tip: None,
                pressure_size: 0.6,
                pressure_opacity: 0.6,
                velocity_size: 0.0,
                velocity_opacity: 0.0,
                velocity_size_curve: None,
                velocity_opacity_curve: None,
                grain: 0.45,
                grain_scale: 1.3,
                grain_tip: None,
                grain_angle: 0.0,
                roundness: 1.0,
                roundness_jitter: 0.0,
                size_jitter: 0.0,
                opacity_jitter: 0.0,
                angle: 0.0,
                angle_follow: false,
                scatter: 0.06,
                stabilizer: 0.2,
                blend: super::document::BlendMode::Normal,
                wet_edge: 0.0,
                dual_tip: None,
                dual_scale: 0.5,
                shape_rotation: 0.0,
                angle_scatter: 0.0,
                shape_count: 1,
                shape_count_jitter: 0.0,
                pressure_curve: None,
                pressure_opacity_curve: None,
                taper: Taper::NONE,
            },
        ),
    ]
}
impl Document {
    /// Paint one pixel with source-over at brush opacity, honoring selection
    /// and mirroring per [`Document::symmetry`].
    pub(crate) fn put(&mut self, brush: &Brush, color: Color, px: u32, py: u32) {
        let (ccx, ccy) = self.symmetry_center_px();
        let pts =
            self.symmetry
                .mirrored_pixels(self.w, self.h, self.symmetry_segments, ccx, ccy, px, py);
        let mut done: Vec<(u32, u32)> = Vec::with_capacity(4);
        for (mx, my) in pts {
            if done.contains(&(mx, my)) {
                continue;
            }
            done.push((mx, my));
            self.put_one(brush, color, mx, my);
        }
    }

    fn put_one(&mut self, brush: &Brush, color: Color, px: u32, py: u32) {
        if px >= self.w || py >= self.h {
            return;
        }
        if let Some((sx0, sy0, sx1, sy1)) = self.selection {
            if px < sx0 || px > sx1 || py < sy0 || py > sy1 {
                return;
            }
        }
        if let Some(mask) = self.selection_mask.as_deref() {
            if mask.get((py * self.w + px) as usize).copied().unwrap_or(0) == 0 {
                return;
            }
        }
        let (w, h) = (self.w, self.h);
        // Graduated feathered masks scale the paint (binary masks: full).
        let sel_k = Document::sel_k(self.selection_mask.as_deref(), self.w, px, py);
        // Layer offset: store at the buffer slot for this canvas pixel.
        let Some(layer) = self.active_layer_mut() else {
            return;
        };
        let (lox, loy) = (layer.ox, layer.oy);
        let (lx, ly) = (px as i32 - lox, py as i32 - loy);
        if lx < 0 || ly < 0 || lx >= w as i32 || ly >= h as i32 {
            return;
        }
        let o = ((ly as u32 * w + lx as u32) * 4) as usize;
        if o + 3 >= layer.pixels.len() {
            return;
        }
        let src_a = brush.opacity.clamp(0.0, 1.0) * sel_k;
        let mut dst = layer.pixels.get4(o);
        let dst_a = dst[3] as f32 / 255.0;
        let out_a = src_a + dst_a * (1.0 - src_a);
        if out_a > 0.0 {
            let c = color.0;
            for ch in 0..3 {
                dst[ch] = ((c[ch] as f32 * src_a + dst[ch] as f32 * dst_a * (1.0 - src_a)) / out_a)
                    .round() as u8;
            }
            dst[3] = (out_a * 255.0).round() as u8;
            layer.pixels.set4(o, dst);
        }
        let w = self.w;
        let h = self.h;
        self.mark_dirty(px.min(w - 1), py.min(h - 1), px.min(w - 1), py.min(h - 1));
    }

    /// Straight segment between two canvas points. Caller checkpoints first.
    /// Returns leftover spacing for chaining segments.
    pub fn shape_line(
        &mut self,
        brush: &Brush,
        color: Color,
        from: Option<(f32, f32)>,
        to: (f32, f32),
        leftover: f32,
    ) -> (Option<(f32, f32)>, f32) {
        self.stroke_to(brush, color, from, to, leftover)
    }

    /// Closed corner loop (last == first) for a rectangle outline, in the
    /// drag order supplied. Shared with the app's drag preview.
    pub fn rect_vertices(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(f32, f32)> {
        vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]
    }

    /// Rectangle outline through dab segments. Caller checkpoints first.
    pub fn shape_rect(&mut self, brush: &Brush, color: Color, x0: f32, y0: f32, x1: f32, y1: f32) {
        let pts = Self::rect_vertices(x0, y0, x1, y1);
        let mut last = None;
        let mut left = 0.0;
        for &p in &pts {
            let r = self.stroke_to(brush, color, last, p, left);
            last = r.0;
            left = r.1;
        }
    }

    /// Vertex loop for an ellipse inscribed in the box (closed: the last
    /// sample lands on the first). Empty for degenerate boxes (< 2px on
    /// either axis), mirroring [`Self::shape_ellipse`].
    pub fn ellipse_vertices(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(f32, f32)> {
        let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
        let (rx, ry) = ((x1 - x0).abs() * 0.5, (y1 - y0).abs() * 0.5);
        if rx < 1.0 || ry < 1.0 {
            return Vec::new();
        }
        let n = (((rx + ry) * std::f32::consts::PI * 0.5).round() as usize).clamp(24, 720);
        let mut v = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            v.push((cx + rx * t.cos(), cy + ry * t.sin()));
        }
        v
    }

    /// Ellipse outline inside the box. Caller checkpoints first.
    pub fn shape_ellipse(
        &mut self,
        brush: &Brush,
        color: Color,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    ) {
        let pts = Self::ellipse_vertices(x0, y0, x1, y1);
        let mut last = None;
        let mut left = 0.0;
        for &p in &pts {
            let r = self.stroke_to(brush, color, last, p, left);
            last = r.0;
            left = r.1;
        }
    }

    /// Solid fill of a rectangle. Caller checkpoints first.
    pub fn fill_rect(&mut self, brush: &Brush, color: Color, x0: u32, y0: u32, x1: u32, y1: u32) {
        let (x0, x1) = (
            x0.min(x1).min(self.w.saturating_sub(1)),
            x0.max(x1).min(self.w.saturating_sub(1)),
        );
        let (y0, y1) = (
            y0.min(y1).min(self.h.saturating_sub(1)),
            y0.max(y1).min(self.h.saturating_sub(1)),
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.put(brush, color, x, y);
            }
        }
    }

    /// Solid fill of an ellipse inside the box. Caller checkpoints first.
    pub fn fill_ellipse(
        &mut self,
        brush: &Brush,
        color: Color,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    ) {
        let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
        let (rx, ry) = ((x1 - x0).abs() * 0.5, (y1 - y0).abs() * 0.5);
        if rx < 0.5 || ry < 0.5 {
            return;
        }
        let (ix0, iy0) = (
            x0.min(x1).floor().max(0.0) as u32,
            y0.min(y1).floor().max(0.0) as u32,
        );
        let (ix1, iy1) = (x0.max(x1).ceil() as u32, y0.max(y1).ceil() as u32);
        for y in iy0..=iy1.min(self.h.saturating_sub(1)) {
            for x in ix0..=ix1.min(self.w.saturating_sub(1)) {
                let dx = (x as f32 + 0.5 - cx) / rx;
                let dy = (y as f32 + 0.5 - cy) / ry;
                if dx * dx + dy * dy <= 1.0 {
                    self.put(brush, color, x, y);
                }
            }
        }
    }

    /// Closed vertex loop (last == first) for a polygonal shape in the box.
    /// Triangle/diamond/star are fixed; `polygon` is the regular N-gon
    /// (`sides` clamped 3..=12, first vertex on top). Shared with the
    /// app's drag preview so it traces exactly what a commit draws.
    pub fn poly_vertices(
        kind: ShapePoly,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        sides: u8,
    ) -> Vec<(f32, f32)> {
        let (lo_x, hi_x) = (x0.min(x1), x0.max(x1));
        let (lo_y, hi_y) = (y0.min(y1), y0.max(y1));
        let (cx, cy) = ((lo_x + hi_x) * 0.5, (lo_y + hi_y) * 0.5);
        let close = |mut v: Vec<(f32, f32)>| {
            if let Some(&first) = v.first() {
                v.push(first);
            }
            v
        };
        match kind {
            ShapePoly::Triangle => close(vec![(cx, lo_y), (hi_x, hi_y), (lo_x, hi_y)]),
            ShapePoly::Diamond => close(vec![(cx, lo_y), (hi_x, cy), (cx, hi_y), (lo_x, cy)]),
            ShapePoly::Star => {
                let r = ((hi_x - lo_x).min(hi_y - lo_y)) * 0.5;
                let mut v = Vec::with_capacity(11);
                for i in 0..10 {
                    let rr = if i % 2 == 0 { r } else { r * 0.382 };
                    let t = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    v.push((cx + rr * t.cos(), cy + rr * t.sin()));
                }
                close(v)
            }
            ShapePoly::Polygon => {
                let n = sides.clamp(3, 12) as usize;
                let (rx, ry) = ((hi_x - lo_x) * 0.5, (hi_y - lo_y) * 0.5);
                let mut v = Vec::with_capacity(n + 1);
                for i in 0..n {
                    let t =
                        -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / n as f32;
                    v.push((cx + rx * t.cos(), cy + ry * t.sin()));
                }
                close(v)
            }
        }
    }

    /// Open stroke path for an arrow along the drag direction: shaft plus
    /// two head wings (backtracked, like a hand stroke). Caller checkpoints.
    /// Shared with the app's drag preview.
    pub fn arrow_path(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(f32, f32)> {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 2.0 {
            return vec![(x0, y0), (x1, y1)];
        }
        let (ux, uy) = (dx / len, dy / len);
        let (tip_x, tip_y) = (x1, y1);
        let head_len = (len * 0.28).clamp(6.0, 48.0);
        let head_w = (len * 0.16).clamp(4.0, 32.0);
        // Perpendicular for the wing spread.
        let (px, py) = (-uy, ux);
        let b = (tip_x - ux * head_len, tip_y - uy * head_len);
        vec![
            (x0, y0),
            (tip_x, tip_y),
            (b.0 + px * head_w, b.1 + py * head_w),
            (tip_x, tip_y),
            (b.0 - px * head_w, b.1 - py * head_w),
        ]
    }

    /// Outline of a polygonal shape through dab segments. Caller checkpoints.
    #[allow(clippy::too_many_arguments)]
    pub fn shape_poly(
        &mut self,
        brush: &Brush,
        color: Color,
        kind: ShapePoly,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        sides: u8,
    ) {
        let pts = Self::poly_vertices(kind, x0, y0, x1, y1, sides);
        let mut last = None;
        let mut left = 0.0;
        for &p in &pts {
            let r = self.stroke_to(brush, color, last, p, left);
            last = r.0;
            left = r.1;
        }
    }

    /// Arrow outline (shaft + head). Caller checkpoints first.
    pub fn shape_arrow(&mut self, brush: &Brush, color: Color, x0: f32, y0: f32, x1: f32, y1: f32) {
        let pts = Self::arrow_path(x0, y0, x1, y1);
        let mut last = None;
        let mut left = 0.0;
        for &p in &pts {
            let r = self.stroke_to(brush, color, last, p, left);
            last = r.0;
            left = r.1;
        }
    }

    /// Solid even-odd fill of a polygonal shape. Caller checkpoints first.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_poly(
        &mut self,
        brush: &Brush,
        color: Color,
        kind: ShapePoly,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        sides: u8,
    ) {
        let pts = Self::poly_vertices(kind, x0, y0, x1, y1, sides);
        if pts.len() < 4 {
            return;
        }
        let (ix0, iy0) = (
            x0.min(x1).floor().max(0.0) as u32,
            y0.min(y1).floor().max(0.0) as u32,
        );
        let (ix1, iy1) = (x0.max(x1).ceil() as u32, y0.max(y1).ceil() as u32);
        for y in iy0..=iy1.min(self.h.saturating_sub(1)) {
            for x in ix0..=ix1.min(self.w.saturating_sub(1)) {
                // Even-odd ray cast from the pixel center.
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let mut inside = false;
                let mut j = pts.len() - 1;
                for i in 0..pts.len() {
                    let (xi, yi) = pts[i];
                    let (xj, yj) = pts[j];
                    if (yi > py) != (yj > py) {
                        let at = xi + (py - yi) / (yj - yi) * (xj - xi);
                        if px < at {
                            inside = !inside;
                        }
                    }
                    j = i;
                }
                if inside {
                    self.put(brush, color, x, y);
                }
            }
        }
    }

    /// Flood-fill the contiguous region containing `(x, y)` on the active
    /// layer with `color` at brush opacity. A pixel belongs to the region
    /// when every channel is within `tolerance` of the seed pixel. Honors
    /// the active selection and mirrors across symmetry axes.
    /// `grow` extends the matched region outward by up to that many px
    /// (straight-line distance), painting over the anti-aliased fringe left
    /// by tolerance misses. Growth stays inside the selection and canvas
    /// edges, and stops at opaque pixels clearly darker than the fill (line
    /// art), so a fill never spills across its outline. Caller checkpoints
    /// first.
    pub fn flood_fill(
        &mut self,
        brush: &Brush,
        color: Color,
        x: u32,
        y: u32,
        tolerance: u8,
        grow: u8,
    ) {
        if self.active >= self.layers.len() || !self.pixel_selected(x, y) {
            return;
        }
        let (w, h) = (self.w, self.h);
        let (lox, loy) = (self.layers[self.active].ox, self.layers[self.active].oy);
        // Canvas cell -> buffer offset, if the layer covers it. Uncovered
        // canvas reads as transparent content (fill can flow through it
        // when it matches, but there is no slot to store into).
        let slot = |cx: u32, cy: u32| -> Option<usize> {
            let lx = cx as i32 - lox;
            let ly = cy as i32 - loy;
            if lx < 0 || ly < 0 || lx >= w as i32 || ly >= h as i32 {
                return None;
            }
            Some(((ly as u32 * w + lx as u32) * 4) as usize)
        };
        // Seed on uncovered canvas has no buffer to fill: no-op.
        let Some(seed_off) = slot(x, y) else { return };
        // Read the seed through the tile accessors: a full-layer dense copy
        // just to sample four bytes is 256 MiB of traffic on an 8K layer.
        let target = {
            let pixels = &self.layers[self.active].pixels;
            if seed_off + 3 >= pixels.len() {
                return;
            }
            pixels.get4(seed_off)
        };
        let opacity = brush.opacity.clamp(0.0, 1.0);
        // A seed already carrying the fill color is a no-op only without
        // growth; with `grow` the second fill must still extend the region.
        if grow == 0 && blend_over(target, color, opacity) == target {
            return;
        }
        let matches = |px: &[u8]| {
            px[0].abs_diff(target[0]) <= tolerance
                && px[1].abs_diff(target[1]) <= tolerance
                && px[2].abs_diff(target[2]) <= tolerance
                && px[3].abs_diff(target[3]) <= tolerance
        };
        // Mirror the seed across symmetry axes (deduped).
        let mut seeds = Vec::with_capacity(4);
        let (ccx, ccy) = self.symmetry_center_px();
        for (sx, sy) in self
            .symmetry
            .mirrored_pixels(w, h, self.symmetry_segments, ccx, ccy, x, y)
        {
            if sx < w && sy < h && self.pixel_selected(sx, sy) && !seeds.contains(&(sx, sy)) {
                seeds.push((sx, sy));
            }
        }
        // Selection consulted per pixel without borrowing the layer.
        let sel = self.selection;
        let mask = self.selection_mask.clone();
        let selected = |px: u32, py: u32| {
            if px >= w || py >= h {
                return false;
            }
            if let Some((a, b, c, d)) = sel {
                if px < a || px > c || py < b || py > d {
                    return false;
                }
            }
            if let Some(m) = mask.as_deref() {
                if m[(py * w + px) as usize] == 0 {
                    return false;
                }
            }
            true
        };
        let layer = &mut self.layers[self.active];
        // One bit per canvas pixel (`w * h` bits, 8 MiB at 8K): the visited
        // set is a membership test, not a value, so a byte per pixel was 8x
        // larger than necessary.
        let mut visited = vec![0u8; (w as usize * h as usize).div_ceil(8)];
        let mut stack = seeds;
        let mut any = false;
        // Grow-only on purpose: the growth pass reads `matched`, the common
        // `grow == 0` path never does, so a whole-canvas fill at 8K must not
        // accumulate ~0.5 GiB of coordinates nobody reads. With `grow > 0`
        // this is filled exactly as before, so the empty check still means
        // "the seed region matched nothing".
        let mut matched: Vec<(u32, u32)> = Vec::new();
        while let Some((cx, cy)) = stack.pop() {
            let idx = (cy * w + cx) as usize;
            let (byte, bit) = (idx / 8, 1u8 << (idx % 8));
            if visited[byte] & bit != 0 {
                continue;
            }
            visited[byte] |= bit;
            let content: [u8; 4] = match slot(cx, cy) {
                Some(o) if o + 3 < layer.pixels.len() => layer.pixels.get4(o),
                _ => [0, 0, 0, 0],
            };
            if !matches(&content) {
                continue;
            }
            if grow > 0 {
                matched.push((cx, cy));
            }
            // Store only where the buffer exists (see `slot`).
            if let Some(o) = slot(cx, cy) {
                if o + 3 < layer.pixels.len() {
                    layer.pixels.set4(o, blend_over(content, color, opacity));
                    any = true;
                }
            }
            if cx > 0 && selected(cx - 1, cy) {
                stack.push((cx - 1, cy));
            }
            if cx + 1 < w && selected(cx + 1, cy) {
                stack.push((cx + 1, cy));
            }
            if cy > 0 && selected(cx, cy - 1) {
                stack.push((cx, cy - 1));
            }
            if cy + 1 < h && selected(cx, cy + 1) {
                stack.push((cx, cy + 1));
            }
        }
        // Grow: expand the matched region outward by up to `grow` px,
        // painting over the fringe that tolerance left behind. Measured
        // with a chamfer distance transform (ortho 100 / diag 141 — exact
        // on the axes and 45° diagonals, up to ~8% long at 1:2 slopes, the
        // conservative side) so N px means N px radially in EVERY
        // direction: 4-neighbor rings under-reach the diagonals by
        // √2 and leave halo arcs along a circle's 45° sectors. The BFS's
        // `visited` map cannot stand in for region membership (it also
        // holds rejected pixels — the fringe itself), hence the separate
        // map. Growth stays gated by selection and canvas edges.
        if grow > 0 && !matched.is_empty() {
            const ORTHO: u32 = 100;
            const DIAG: u32 = 141;
            // Cost ≤ limit spans at most `grow` px of displacement. The
            // +60 slack covers half a cell (the tolerance boundary sits
            // between cell centers, so centre-to-centre distance reads up
            // to ~0.5 px long) plus a little chamfer error at shallow
            // angles; without it, diagonal notches of the digitized
            // boundary leave sub-pixel halo specks at grow N. Empirically
            // (3 px ramp test) survivors sit ≤241 and the nearest black
            // line pixel at 282, so the slack clears fringe without
            // nibbling the stroke.
            let limit = grow as u32 * ORTHO + 60;
            let mut in_region = vec![0u8; (w * h) as usize];
            let (mut bx0, mut by0, mut bx1, mut by1) = (w, h, 0u32, 0u32);
            for &(cx, cy) in &matched {
                in_region[(cy * w + cx) as usize] = 1;
                bx0 = bx0.min(cx);
                by0 = by0.min(cy);
                bx1 = bx1.max(cx);
                by1 = by1.max(cy);
            }
            // Cost ≤ limit spans at most grow + 0.6 px of displacement;
            // pad one more cell for the grid offset at the edges.
            let pad = grow as u32 + 1;
            bx0 = bx0.saturating_sub(pad);
            by0 = by0.saturating_sub(pad);
            bx1 = (bx1 + pad).min(w - 1);
            by1 = (by1 + pad).min(h - 1);
            let (bw, bh) = ((bx1 - bx0 + 1) as usize, (by1 - by0 + 1) as usize);
            let inf = u32::MAX / 2;
            // Opaque pixels clearly darker than the fill are line art:
            // growth may neither run through them nor paint over them. The
            // 140 floor keeps a dark fill from walling off its own fringe.
            let fill_brightness = color.0[0].max(color.0[1]).max(color.0[2]);
            let barrier = ((fill_brightness as u32 * 3 / 5) as u8).max(140);
            let mut dist = vec![inf; bw * bh];
            let mut open = vec![false; bw * bh];
            for by in 0..bh {
                for bx in 0..bw {
                    let i = by * bw + bx;
                    let (nx, ny) = (bx0 + bx as u32, by0 + by as u32);
                    let region = in_region[(ny * w + nx) as usize] == 1;
                    let mut is_open = selected(nx, ny);
                    if is_open && !region {
                        if let Some(o) = slot(nx, ny) {
                            if o + 3 < layer.pixels.len() {
                                let px = layer.pixels.get4(o);
                                if px[3] >= 128 && px[0].max(px[1]).max(px[2]) < barrier {
                                    is_open = false;
                                }
                            }
                        }
                    }
                    open[i] = is_open;
                    if region {
                        dist[i] = 0;
                    }
                }
            }
            for by in 0..bh {
                for bx in 0..bw {
                    let i = by * bw + bx;
                    if dist[i] == 0 || !open[i] {
                        continue;
                    }
                    let mut d = dist[i];
                    if bx > 0 {
                        d = d.min(dist[i - 1].saturating_add(ORTHO));
                    }
                    if by > 0 {
                        d = d.min(dist[i - bw].saturating_add(ORTHO));
                    }
                    if bx > 0 && by > 0 {
                        d = d.min(dist[i - bw - 1].saturating_add(DIAG));
                    }
                    if bx + 1 < bw && by > 0 {
                        d = d.min(dist[i - bw + 1].saturating_add(DIAG));
                    }
                    dist[i] = d;
                }
            }
            for by in (0..bh).rev() {
                for bx in (0..bw).rev() {
                    let i = by * bw + bx;
                    if dist[i] == 0 || !open[i] {
                        continue;
                    }
                    let mut d = dist[i];
                    if bx + 1 < bw {
                        d = d.min(dist[i + 1].saturating_add(ORTHO));
                    }
                    if by + 1 < bh {
                        d = d.min(dist[i + bw].saturating_add(ORTHO));
                    }
                    if bx + 1 < bw && by + 1 < bh {
                        d = d.min(dist[i + bw + 1].saturating_add(DIAG));
                    }
                    if bx > 0 && by + 1 < bh {
                        d = d.min(dist[i + bw - 1].saturating_add(DIAG));
                    }
                    dist[i] = d;
                }
            }
            for by in 0..bh {
                for bx in 0..bw {
                    let i = by * bw + bx;
                    if dist[i] == 0 || dist[i] > limit || !open[i] {
                        continue;
                    }
                    let (nx, ny) = (bx0 + bx as u32, by0 + by as u32);
                    if let Some(o) = slot(nx, ny) {
                        if o + 3 < layer.pixels.len() {
                            let content = layer.pixels.get4(o);
                            layer.pixels.set4(o, blend_over(content, color, opacity));
                            any = true;
                        }
                    }
                }
            }
        }
        if any {
            self.mark_full_dirty();
        }
    }
}

/// Source-over blend of `color` at `opacity` onto `dst`. Matches the math in
/// [`Document::dab`] so fills look like brush paint.
fn blend_over(dst: [u8; 4], color: Color, opacity: f32) -> [u8; 4] {
    let src_a = opacity.clamp(0.0, 1.0);
    let dst_a = dst[3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return [0, 0, 0, 0];
    }
    let c = color.0;
    [
        ((c[0] as f32 * src_a + dst[0] as f32 * dst_a * (1.0 - src_a)) / out_a).round() as u8,
        ((c[1] as f32 * src_a + dst[1] as f32 * dst_a * (1.0 - src_a)) / out_a).round() as u8,
        ((c[2] as f32 * src_a + dst[2] as f32 * dst_a * (1.0 - src_a)) / out_a).round() as u8,
        (out_a * 255.0).round() as u8,
    ]
}
pub struct Stroke<'a> {
    doc: &'a mut Document,
    brush: Brush,
    color: Color,
    last: Option<(f32, f32)>,
    last_dyn: DabDynamics,
    leftover: f32,
}

impl<'a> Stroke<'a> {
    /// Checkpoint the active layer for undo, then start painting.
    pub fn begin(doc: &'a mut Document, brush: Brush, color: Color) -> Self {
        doc.checkpoint();
        Self {
            doc,
            brush,
            color,
            last: None,
            last_dyn: DabDynamics::default(),
            leftover: 0.0,
        }
    }

    /// Feed a pointer position (canvas pixels).
    pub fn push(&mut self, x: f32, y: f32) {
        self.push_dyn(x, y, DabDynamics::default());
    }

    /// Feed a pointer position with dynamics (pressure/speed/direction).
    pub fn push_dyn(&mut self, x: f32, y: f32, dyn_: DabDynamics) {
        let (last, leftover) = self.doc.stroke_to_dyn(
            &self.brush,
            self.color,
            self.last,
            self.last_dyn,
            (x, y),
            dyn_,
            self.leftover,
        );
        self.last = last;
        self.last_dyn = dyn_;
        self.leftover = leftover;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flood_fill_stays_inside_barrier() {
        let mut doc = Document::new(10, 10);
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        // Opaque red wall splitting the canvas.
        doc.fill_rect(&brush, Color::rgb(255, 0, 0), 5, 0, 5, 9);
        doc.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 0, 0);
        let at = |d: &Document, x: u32, y: u32| {
            let o = ((y * 10 + x) * 4) as usize;
            [
                d.layers[0].pixels.get(o),
                d.layers[0].pixels.get(o + 1),
                d.layers[0].pixels.get(o + 2),
                d.layers[0].pixels.get(o + 3),
            ]
        };
        assert_eq!(at(&doc, 0, 5), [0, 0, 255, 255]);
        assert_eq!(at(&doc, 5, 5), [255, 0, 0, 255]);
        assert_eq!(at(&doc, 9, 5)[3], 0);
    }

    #[test]
    fn flood_fill_mirrors_with_symmetry() {
        use crate::document::SymmetryMode;
        let mut doc = Document::new(11, 11);
        doc.symmetry = SymmetryMode::Horizontal;
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        doc.fill_rect(&brush, Color::rgb(255, 0, 0), 5, 0, 5, 10);
        doc.flood_fill(&brush, Color::rgb(0, 255, 0), 1, 5, 0, 0);
        let at = |d: &Document, x: u32, y: u32| {
            let o = ((y * 11 + x) * 4) as usize;
            [
                d.layers[0].pixels.get(o),
                d.layers[0].pixels.get(o + 1),
                d.layers[0].pixels.get(o + 2),
                d.layers[0].pixels.get(o + 3),
            ]
        };
        assert_eq!(at(&doc, 1, 5), [0, 255, 0, 255]);
        assert_eq!(at(&doc, 9, 5), [0, 255, 0, 255]);
        assert_eq!(at(&doc, 5, 5), [255, 0, 0, 255]);
    }

    #[test]
    fn flood_fill_tolerance_widens_the_region() {
        // Two near-identical reds side by side (ColorDrop-style threshold).
        let two_tone = |doc: &mut Document| {
            let brush = Brush {
                opacity: 1.0,
                ..Default::default()
            };
            doc.fill_rect(&brush, Color::rgb(200, 0, 0), 0, 0, 4, 9);
            doc.fill_rect(&brush, Color::rgb(206, 0, 0), 5, 0, 9, 9);
        };
        let at = |d: &Document, x: u32, y: u32| {
            let o = ((y * 10 + x) * 4) as usize;
            [
                d.layers[0].pixels.get(o),
                d.layers[0].pixels.get(o + 1),
                d.layers[0].pixels.get(o + 2),
            ]
        };
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        // Strict: only the seed side fills.
        let mut tight = Document::new(10, 10);
        two_tone(&mut tight);
        tight.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 0, 0);
        assert_eq!(at(&tight, 0, 5), [0, 0, 255]);
        assert_eq!(at(&tight, 9, 5), [206, 0, 0]);
        // Loose: the neighbor tone joins the region.
        let mut loose = Document::new(10, 10);
        two_tone(&mut loose);
        loose.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 10, 0);
        assert_eq!(at(&loose, 0, 5), [0, 0, 255]);
        assert_eq!(at(&loose, 9, 5), [0, 0, 255]);
    }

    #[test]
    fn flood_fill_restore_and_refill_matches_direct() {
        // The ColorDrop preview loop (snapshot, restore, re-fill at a new
        // tolerance) must equal a single direct fill at that tolerance.
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        let mut direct = Document::new(10, 10);
        direct.fill_rect(&brush, Color::rgb(200, 0, 0), 0, 0, 4, 9);
        direct.fill_rect(&brush, Color::rgb(206, 0, 0), 5, 0, 9, 9);
        direct.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 10, 0);
        let mut preview = Document::new(10, 10);
        preview.fill_rect(&brush, Color::rgb(200, 0, 0), 0, 0, 4, 9);
        preview.fill_rect(&brush, Color::rgb(206, 0, 0), 5, 0, 9, 9);
        let snap = preview.layers[0].pixels.clone();
        preview.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 0, 0);
        preview.layers[0].pixels = snap;
        preview.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 10, 0);
        assert_eq!(preview.layers[0].pixels, direct.layers[0].pixels);
    }

    #[test]
    fn flood_fill_grow_covers_the_fringe() {
        // Anti-aliased line art: the fill-matching edge is white (cols 0-4),
        // the soft fringe (col 5) and the dark core (col 6) are not, the
        // far side (cols 7-9) is white again. Grow paints over the fringe.
        let scene = |doc: &mut Document| {
            let brush = Brush {
                opacity: 1.0,
                ..Default::default()
            };
            doc.fill_rect(&brush, Color::rgb(255, 255, 255), 0, 0, 9, 9);
            doc.fill_rect(&brush, Color::rgb(160, 160, 160), 5, 0, 5, 9);
            doc.fill_rect(&brush, Color::rgb(0, 0, 0), 6, 0, 6, 9);
            doc.fill_rect(&brush, Color::rgb(255, 255, 255), 7, 0, 9, 9);
        };
        let at = |d: &Document, x: u32, y: u32| {
            let o = ((y * 10 + x) * 4) as usize;
            [
                d.layers[0].pixels.get(o),
                d.layers[0].pixels.get(o + 1),
                d.layers[0].pixels.get(o + 2),
                d.layers[0].pixels.get(o + 3),
            ]
        };
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        // Growth 0 (the old behavior): the fringe survives.
        let mut flat = Document::new(10, 10);
        scene(&mut flat);
        flat.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 32, 0);
        assert_eq!(at(&flat, 4, 5), [0, 0, 255, 255]);
        assert_eq!(at(&flat, 5, 5), [160, 160, 160, 255]);
        // Growth 1: the fringe is covered, the dark core (distance 2) is not.
        let mut grown = Document::new(10, 10);
        scene(&mut grown);
        grown.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 32, 1);
        assert_eq!(at(&grown, 5, 5), [0, 0, 255, 255]);
        assert_eq!(at(&grown, 6, 5), [0, 0, 0, 255]);
        assert_eq!(at(&grown, 7, 5), [255, 255, 255, 255]);
    }

    #[test]
    fn flood_fill_again_with_more_grow_covers_the_fringe() {
        // A second fill lands on a seed that already has the fill color;
        // it must still honor a larger Grow instead of early-returning.
        let scene = |doc: &mut Document| {
            let brush = Brush {
                opacity: 1.0,
                ..Default::default()
            };
            doc.fill_rect(&brush, Color::rgb(255, 255, 255), 0, 0, 9, 9);
            doc.fill_rect(&brush, Color::rgb(160, 160, 160), 5, 0, 5, 9);
            doc.fill_rect(&brush, Color::rgb(0, 0, 0), 6, 0, 6, 9);
            doc.fill_rect(&brush, Color::rgb(255, 255, 255), 7, 0, 9, 9);
        };
        let at = |d: &Document, x: u32, y: u32| {
            let o = ((y * 10 + x) * 4) as usize;
            [
                d.layers[0].pixels.get(o),
                d.layers[0].pixels.get(o + 1),
                d.layers[0].pixels.get(o + 2),
                d.layers[0].pixels.get(o + 3),
            ]
        };
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        let mut doc = Document::new(10, 10);
        scene(&mut doc);
        doc.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 32, 0);
        assert_eq!(at(&doc, 5, 5), [160, 160, 160, 255]);
        doc.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 32, 1);
        assert_eq!(at(&doc, 5, 5), [0, 0, 255, 255], "grow ignored on re-fill");
        assert_eq!(at(&doc, 6, 5), [0, 0, 0, 255]);
        assert_eq!(at(&doc, 7, 5), [255, 255, 255, 255]);
    }

    #[test]
    fn flood_fill_grow_never_crosses_a_dark_line() {
        // A full-height black line splits white paper. Even a huge Grow
        // must not paint through the outline onto the other side.
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        let mut doc = Document::new(21, 21);
        doc.fill_rect(&brush, Color::rgb(255, 255, 255), 0, 0, 20, 20);
        doc.fill_rect(&brush, Color::rgb(0, 0, 0), 10, 0, 10, 20);
        doc.flood_fill(&brush, Color::rgb(0, 0, 255), 3, 3, 32, 16);
        let at = |x: u32, y: u32| {
            let o = ((y * 21 + x) * 4) as usize;
            [
                doc.layers[0].pixels.get(o),
                doc.layers[0].pixels.get(o + 1),
                doc.layers[0].pixels.get(o + 2),
                doc.layers[0].pixels.get(o + 3),
            ]
        };
        assert_eq!(at(9, 10), [0, 0, 255, 255]);
        assert_eq!(at(10, 10), [0, 0, 0, 255]);
        assert_eq!(at(11, 10), [255, 255, 255, 255], "grow crossed darkness");
        assert_eq!(at(20, 20), [255, 255, 255, 255]);
    }

    #[test]
    fn flood_fill_grow_respects_selection() {
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        let mut doc = Document::new(10, 10);
        doc.selection = Some((0, 0, 4, 9));
        // Gray column 4 matches nothing, but it is inside the selection:
        // growth may paint it. Column 5 is outside the selection: blocked.
        doc.fill_rect(&brush, Color::rgb(160, 160, 160), 4, 0, 4, 9);
        doc.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 32, 5);
        let at = |d: &Document, x: u32, y: u32| {
            let o = ((y * 10 + x) * 4) as usize;
            [
                d.layers[0].pixels.get(o),
                d.layers[0].pixels.get(o + 1),
                d.layers[0].pixels.get(o + 2),
                d.layers[0].pixels.get(o + 3),
            ]
        };
        assert_eq!(at(&doc, 3, 5), [0, 0, 255, 255]);
        assert_eq!(at(&doc, 4, 5), [0, 0, 255, 255]);
        assert_eq!(at(&doc, 5, 5)[3], 0);
    }

    #[test]
    fn flood_fill_grow_mirrors_with_symmetry() {
        use crate::document::SymmetryMode;
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        let mut doc = Document::new(11, 11);
        doc.symmetry = SymmetryMode::Horizontal;
        doc.fill_rect(&brush, Color::rgb(160, 160, 160), 4, 0, 4, 10);
        doc.fill_rect(&brush, Color::rgb(255, 0, 0), 5, 0, 5, 10);
        doc.fill_rect(&brush, Color::rgb(160, 160, 160), 6, 0, 6, 10);
        doc.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 5, 32, 1);
        let at = |d: &Document, x: u32, y: u32| {
            let o = ((y * 11 + x) * 4) as usize;
            [
                d.layers[0].pixels.get(o),
                d.layers[0].pixels.get(o + 1),
                d.layers[0].pixels.get(o + 2),
                d.layers[0].pixels.get(o + 3),
            ]
        };
        assert_eq!(at(&doc, 4, 5), [0, 0, 255, 255]);
        assert_eq!(at(&doc, 6, 5), [0, 0, 255, 255]);
        assert_eq!(at(&doc, 5, 5), [255, 0, 0, 255]);
    }

    #[test]
    fn flood_fill_grow_clears_a_realistic_antialiased_edge() {
        // A 4 px black ring with a 3 px anti-aliased inner ramp (white at
        // d=17 fading to black at d=20) — the halo case from the field:
        // fill inside, a light ring survives at tolerance 32.
        let scene = |doc: &mut Document| {
            let brush = Brush {
                opacity: 1.0,
                ..Default::default()
            };
            doc.fill_rect(&brush, Color::rgb(255, 255, 255), 0, 0, 63, 63);
            for y in 0..64u32 {
                for x in 0..64u32 {
                    let dx = x as f32 - 31.5;
                    let dy = y as f32 - 31.5;
                    let d = (dx * dx + dy * dy).sqrt();
                    let v = if (17.0..20.0).contains(&d) {
                        (255.0 * (1.0 - (d - 17.0) / 3.0)).round() as u8
                    } else if (20.0..=24.0).contains(&d) {
                        0
                    } else {
                        continue;
                    };
                    let o = ((y * 64 + x) * 4) as usize;
                    doc.layers[0].pixels.set(o, v);
                    doc.layers[0].pixels.set(o + 1, v);
                    doc.layers[0].pixels.set(o + 2, v);
                }
            }
        };
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        // Grow clears the pale part of the anti-aliased ramp (v >= 153, the
        // white-fill barrier) but stops at the darker ramp and the black
        // core: it never paints through line art.
        let pale_halo = |d: &Document| {
            d.layers[0]
                .pixels
                .to_dense()
                .chunks(4)
                .filter(|p| (153..=222).contains(&p[0]))
                .count()
        };
        let dark_ramp = |d: &Document| {
            d.layers[0]
                .pixels
                .to_dense()
                .chunks(4)
                .filter(|p| (1..=152).contains(&p[0]))
                .count()
        };
        let black_core = |d: &Document| {
            d.layers[0]
                .pixels
                .to_dense()
                .chunks(4)
                .filter(|p| p[0] == 0 && p[1] == 0 && p[2] == 0)
                .count()
        };
        let mut flat = Document::new(64, 64);
        scene(&mut flat);
        flat.flood_fill(&brush, Color::rgb(0, 0, 255), 32, 32, 32, 0);
        let mut grown = Document::new(64, 64);
        scene(&mut grown);
        grown.flood_fill(&brush, Color::rgb(0, 0, 255), 32, 32, 32, 2);
        // The halo is real and grow 2 as the default clears its pale part...
        assert!(pale_halo(&flat) > 0, "no halo to fix");
        assert_eq!(pale_halo(&grown), 0, "pale halo survived grow 2");
        // ...without eating the dark ramp or the stroke's black core.
        assert_eq!(dark_ramp(&grown), dark_ramp(&flat), "grow ate the ramp");
        assert_eq!(black_core(&grown), black_core(&flat));
    }

    #[test]
    fn flood_fill_grow_clamps_at_canvas_edges() {
        let brush = Brush {
            opacity: 1.0,
            ..Default::default()
        };
        let mut doc = Document::new(3, 3);
        // Only the center matches; a huge growth floods the rest.
        doc.fill_rect(&brush, Color::rgb(255, 255, 255), 1, 1, 1, 1);
        doc.flood_fill(&brush, Color::rgb(0, 0, 255), 1, 1, 0, 200);
        for y in 0..3u32 {
            for x in 0..3u32 {
                let o = ((y * 3 + x) * 4) as usize;
                assert_eq!(
                    [
                        doc.layers[0].pixels.get(o),
                        doc.layers[0].pixels.get(o + 1),
                        doc.layers[0].pixels.get(o + 2),
                        doc.layers[0].pixels.get(o + 3),
                    ],
                    [0, 0, 255, 255]
                );
            }
        }
    }

    #[test]
    fn stamp_dabs_fade_through_feathered_masks() {
        use crate::document::Document;
        let mut doc = Document::new(20, 20);
        doc.selection = Some((5, 5, 14, 14));
        assert!(doc.feather_selection(3));
        let ramp = doc.selection_mask.clone().unwrap()[(4 * 20 + 10) as usize];
        assert!(ramp > 0 && ramp < 255, "{ramp}");
        // Solid 2x2 stamp tip through the ramp paints partially.
        let tip = BrushTip::from_mask(2, 2, vec![255; 4]).unwrap();
        let stamp = Brush {
            size: 4.0,
            tip: Some(tip),
            ..Brush::default()
        };
        doc.dab(&stamp, Color::rgb(255, 255, 255), 10.5, 4.5);
        let mut peak = 0u8;
        for y in 3..=5 {
            for x in 9..=11 {
                peak = peak.max(doc.layers[0].pixels.get(((y * 20 + x) * 4) as usize + 3));
            }
        }
        assert!(peak > 0 && peak < 255, "{peak}");
    }

    #[test]
    fn symmetry_mirrors_dabs() {
        use crate::document::SymmetryMode;
        // Off: one-sided.
        let mut doc = Document::new(21, 21);
        let brush = Brush {
            size: 3.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(255, 0, 0), 2.5, 10.5);
        let at =
            |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 21 + x) * 4) as usize + 3);
        assert!(at(&doc, 2, 10) > 0);
        assert_eq!(at(&doc, 18, 10), 0);
        // Horizontal: mirrored left-right.
        let mut doc = Document::new(21, 21);
        doc.symmetry = SymmetryMode::Horizontal;
        doc.dab(&brush, Color::rgb(255, 0, 0), 2.5, 10.5);
        assert!(at(&doc, 2, 10) > 0);
        assert!(at(&doc, 18, 10) > 0);
        // Both: four quadrants.
        let mut doc = Document::new(21, 21);
        doc.symmetry = SymmetryMode::Both;
        doc.dab(&brush, Color::rgb(0, 255, 0), 2.5, 3.5);
        assert!(at(&doc, 2, 3) > 0);
        assert!(at(&doc, 18, 3) > 0);
        assert!(at(&doc, 2, 17) > 0);
        assert!(at(&doc, 18, 17) > 0);
    }

    #[test]
    fn shapes_paint_pixels() {
        let mut doc = Document::new(32, 32);
        let brush = Brush {
            size: 4.0,
            ..Default::default()
        };
        doc.shape_rect(&brush, Color::rgb(255, 0, 0), 4.0, 4.0, 20.0, 20.0);
        assert!(doc.layers[0].pixels.to_dense().iter().any(|&b| b != 0));
        let mut doc2 = Document::new(32, 32);
        doc2.shape_ellipse(&brush, Color::rgb(0, 255, 0), 4.0, 4.0, 28.0, 28.0);
        assert!(doc2.layers[0].pixels.to_dense().iter().any(|&b| b != 0));
        let mut doc3 = Document::new(32, 32);
        doc3.fill_rect(&brush, Color::rgb(0, 0, 255), 2, 2, 6, 6);
        let o = ((4 * 32 + 4) * 4) as usize;
        assert_eq!(doc3.layers[0].pixels.get(o + 3), 255);
        let mut doc4 = Document::new(32, 32);
        doc4.fill_ellipse(&brush, Color::rgb(1, 2, 3), 2.0, 2.0, 10.0, 10.0);
        assert!(doc4.layers[0].pixels.to_dense().iter().any(|&b| b != 0));
    }

    #[test]
    fn poly_shapes_outline_and_fill() {
        use ShapePoly::*;
        let brush = Brush {
            size: 3.0,
            ..Default::default()
        };
        for kind in [Triangle, Diamond, Star, Polygon] {
            // Outline paints.
            let mut doc = Document::new(48, 48);
            doc.shape_poly(&brush, Color::rgb(255, 0, 0), kind, 6.0, 6.0, 42.0, 42.0, 6);
            assert!(
                doc.layers[0].pixels.to_dense().iter().any(|&b| b != 0),
                "{kind:?} outline"
            );
            // Fill covers the middle but not the far corner.
            let mut doc = Document::new(48, 48);
            doc.fill_poly(&brush, Color::rgb(0, 255, 0), kind, 6.0, 6.0, 42.0, 42.0, 6);
            let mid = ((24 * 48 + 24) * 4) as usize;
            assert_eq!(
                doc.layers[0].pixels.get(mid + 3),
                255,
                "{kind:?} fill middle"
            );
            let corner = ((2 * 48 + 2) * 4) as usize;
            assert_eq!(
                doc.layers[0].pixels.get(corner + 3),
                0,
                "{kind:?} fill corner"
            );
        }
        // Polygon sides clamp: 2 behaves like a triangle, 99 like 12.
        let mut doc = Document::new(48, 48);
        doc.fill_poly(
            &brush,
            Color::rgb(1, 1, 1),
            Polygon,
            6.0,
            6.0,
            42.0,
            42.0,
            2,
        );
        let mid = ((30 * 48 + 24) * 4) as usize;
        assert_eq!(doc.layers[0].pixels.get(mid + 3), 255);
        // Arrow paints along its drag direction.
        let mut doc = Document::new(48, 48);
        doc.shape_arrow(&brush, Color::rgb(0, 0, 255), 4.0, 24.0, 44.0, 24.0);
        assert!(doc.layers[0].pixels.to_dense().iter().any(|&b| b != 0));
        let tip = ((24 * 48 + 43) * 4) as usize;
        assert!(
            doc.layers[0].pixels.get(tip + 3) > 0,
            "arrowhead lands at the tip"
        );
    }

    #[test]
    fn ellipse_vertices_closed_on_curve() {
        let pts = Document::ellipse_vertices(4.0, 6.0, 28.0, 22.0);
        assert!(pts.len() >= 25, "sampled loop, got {}", pts.len());
        let (first, last) = (pts.first().unwrap(), pts.last().unwrap());
        assert!(
            (first.0 - last.0).abs() < 1e-3 && (first.1 - last.1).abs() < 1e-3,
            "loop closes: {first:?} vs {last:?}"
        );
        for &(x, y) in &pts {
            let e = ((x - 16.0) / 12.0).powi(2) + ((y - 14.0) / 8.0).powi(2);
            assert!((e - 1.0).abs() < 1e-3, "on-curve: {e}");
        }
    }

    #[test]
    fn ellipse_vertices_degenerate_empty() {
        assert!(Document::ellipse_vertices(10.0, 10.0, 11.5, 30.0).is_empty());
        assert!(Document::ellipse_vertices(10.0, 10.0, 30.0, 11.5).is_empty());
    }

    #[test]
    fn rect_vertices_closed_loop() {
        let pts = Document::rect_vertices(4.0, 8.0, 20.0, 26.0);
        assert_eq!(
            pts,
            vec![
                (4.0, 8.0),
                (20.0, 8.0),
                (20.0, 26.0),
                (4.0, 26.0),
                (4.0, 8.0)
            ]
        );
    }

    #[test]
    fn stroke_paints_and_eraser_clears() {
        let mut doc = Document::new(32, 32);
        let brush = Brush {
            size: 8.0,
            ..Default::default()
        };
        let mut s = Stroke::begin(&mut doc, brush, Color::rgb(255, 0, 0));
        s.push(16.0, 16.0);
        drop(s);
        assert!(doc.layers[0].pixels.to_dense().iter().any(|&b| b != 0));
        let mut e = Stroke::begin(
            &mut doc,
            Brush {
                eraser: true,
                size: 32.0,
                softness: 0.0,
                ..Default::default()
            },
            Color::rgb(0, 0, 0),
        );
        e.push(16.0, 16.0);
        drop(e);
        // Eraser removes alpha (destination-out); pixels it takes to alpha 0
        // are canonicalised to [0, 0, 0, 0], so nothing invisible survives.
        assert!(doc.layers[0].pixels.to_dense().iter().all(|&b| b == 0));
    }

    #[test]
    fn dab_paints_across_a_tile_seam() {
        let mut doc = Document::new(600, 100);
        let brush = Brush {
            size: 24.0,
            softness: 0.0,
            opacity: 1.0,
            ..Default::default()
        };
        doc.dab(&brush, Color([255, 0, 0, 255]), 256.0, 50.0);
        let px = &doc.layers[0].pixels;
        assert_eq!(px.get4(((50 * 600 + 250) * 4) as usize)[3], 255);
        assert_eq!(px.get4(((50 * 600 + 262) * 4) as usize)[3], 255);
        assert_eq!(px.alloc_bytes(), 2 * 256 * 1024, "both tiles exist");
    }

    #[test]
    fn eraser_clears_but_keeps_scene_undoable() {
        let mut doc = Document::new(300, 300);
        let brush = Brush {
            size: 40.0,
            softness: 0.0,
            ..Default::default()
        };
        doc.dab(&brush, Color([10, 20, 30, 255]), 150.0, 150.0);
        doc.checkpoint();
        let eraser = Brush {
            eraser: true,
            ..brush
        };
        doc.dab(&eraser, Color([0, 0, 0, 255]), 150.0, 150.0);
        // Destination-out clears alpha, and a pixel it takes to alpha 0 is
        // canonicalised to [0, 0, 0, 0]: invisible pixels carry no colour, so
        // the v12 writer's all-zero-tile omission drops exactly the invisible
        // tiles on reload ("erased costs nothing" holds at any RGB). Partial
        // erasure keeps its RGB — only alpha 0 is canonical.
        assert_eq!(
            doc.layers[0].pixels.get4(((150 * 300 + 150) * 4) as usize),
            [0, 0, 0, 0]
        );
        assert!(doc.undo());
        assert_eq!(
            doc.layers[0]
                .pixels
                .get(((150 * 300 + 150) * 4 + 3) as usize),
            255
        );
    }

    fn sample_library() -> Vec<BrushPreset> {
        presets()
            .into_iter()
            .map(|(name, brush)| BrushPreset {
                name: name.to_string(),
                brush,
            })
            .collect()
    }

    #[test]
    fn pencil_and_ink_pen_taper_their_ends() {
        // Sketch pens ship tapered: the stock brushes should leave the box
        // with a lifted entry and exit, smoothstepped so the tip is round
        // rather than a linear leaf.
        for (name, brush) in presets() {
            if name != "Pencil" && name != "Ink Pen" {
                continue;
            }
            assert!(brush.taper.start > 0.0, "{name} has no start taper");
            assert!(brush.taper.end > 0.0, "{name} has no end taper");
            assert_eq!(brush.taper.shape, 1.0, "{name} taper is not smooth");
            assert!(check_preset(name, &brush).is_ok(), "{name}");
        }
    }

    #[test]
    fn dynamics_resolve_identity_at_full_pressure() {
        let brush = Brush::default();
        assert_eq!(brush.resolve_size(1.0, 0.0), brush.size);
        assert_eq!(brush.resolve_opacity(1.0, 0.0), brush.opacity);
    }

    #[test]
    fn pressure_curve_remaps_size_and_opacity() {
        let brush = Brush {
            size: 20.0,
            opacity: 1.0,
            pressure_size: 1.0,
            pressure_opacity: 1.0,
            velocity_size: 0.0,
            velocity_opacity: 0.0,
            pressure_curve: Some(vec![[0.0, 0.0], [1.0, 0.5]]),
            ..Default::default()
        };
        assert!((brush.resolve_size(1.0, 0.0) - 10.0).abs() < 0.001);
        assert!((brush.resolve_opacity(1.0, 0.0) - 0.5).abs() < 0.001);
        assert!((sample_curve(&[[0.0, 0.0], [0.5, 1.0], [1.0, 1.0]], 0.25) - 0.5).abs() < 0.001);
    }

    #[test]
    fn pressure_tapers_size_and_opacity() {
        let brush = Brush {
            size: 20.0,
            opacity: 1.0,
            pressure_size: 0.8,
            pressure_opacity: 0.6,
            velocity_size: 0.0,
            velocity_opacity: 0.0,
            ..Default::default()
        };
        // p=0 keeps (1-0.8)=20% of size, (1-0.6)=40% of opacity.
        assert!((brush.resolve_size(0.0, 0.0) - 4.0).abs() < 0.001);
        assert!((brush.resolve_opacity(0.0, 0.0) - 0.4).abs() < 0.001);
        // Disabled amounts are no-ops at any pressure.
        let flat = Brush {
            pressure_size: 0.0,
            pressure_opacity: 0.0,
            ..brush.clone()
        };
        assert_eq!(flat.resolve_size(0.0, 0.0), 20.0);
        assert_eq!(flat.resolve_opacity(0.0, 0.0), 1.0);
    }

    #[test]
    fn taper_factor_ramps_start_and_end_lengths() {
        assert_eq!(Taper::NONE.factor_at(0.0), 1.0);
        assert_eq!(Taper::NONE.factor_at(1.0), 1.0);
        let start = Taper {
            start: 0.5,
            ..Taper::NONE
        };
        assert_eq!(start.factor_at(0.0), 0.0);
        assert!((start.factor_at(0.25) - 0.5).abs() < 0.001);
        assert_eq!(start.factor_at(0.5), 1.0);
        assert_eq!(start.factor_at(1.0), 1.0);
        let end = Taper {
            end: 1.0,
            ..Taper::NONE
        };
        assert_eq!(end.factor_at(0.0), 1.0);
        assert!((end.factor_at(0.5) - 0.5).abs() < 0.001);
        assert_eq!(end.factor_at(1.0), 0.0);
        // Overlapping lengths meet at the middle with a full-size peak.
        let both = Taper {
            start: 1.0,
            end: 1.0,
            ..Taper::NONE
        };
        assert!((both.factor_at(0.5) - 1.0).abs() < 0.001);
        // Shape blends the linear ramp toward a smoothstep.
        let shaped = Taper {
            start: 1.0,
            shape: 1.0,
            ..Taper::NONE
        };
        assert!((shaped.factor_at(0.25) - 0.15625).abs() < 0.001);
    }

    #[test]
    fn taper_ends_fade_over_a_fixed_distance() {
        let end = Taper {
            end: 0.5,
            ..Taper::NONE
        };
        // Six brush diameters: a 20 px brush fades over 120 px, so the
        // 0.5 setting tapers the last 60.
        assert_eq!(end.distance_factor(0.0, 0.0, 20.0), 0.0);
        assert!((end.distance_factor(0.0, 30.0, 20.0) - 0.5).abs() < 0.001);
        assert_eq!(end.distance_factor(0.0, 60.0, 20.0), 1.0);
    }

    #[test]
    fn taper_start_does_not_depend_on_stroke_length() {
        let start = Taper {
            start: 0.5,
            ..Taper::NONE
        };
        // Midway through the 60 px start ramp stays 0.5 whether the stroke
        // ends at 61 px or runs on for hundreds: live dabs need no total.
        assert!((start.distance_factor(30.0, 0.0, 20.0) - 0.5).abs() < 0.001);
        assert_eq!(start.distance_factor(200.0, 0.0, 20.0), 1.0);
    }

    #[test]
    fn live_taper_matches_the_release_width_until_the_tail() {
        // What the pen draws while down must be what a lift leaves behind:
        // the start ramp is live, the end ramp is only known on release.
        let taper = Taper {
            start: 0.25,
            end: 0.35,
            size: 0.7,
            shape: 1.0,
            ..Taper::NONE
        };
        let (size, total) = (6.0, 60.0);
        let span = size * TAPER_DIAMETERS;
        assert_eq!(taper.live_factor(0.0, size), 0.0, "start touches down");
        let last_full = total - clamp01(taper.end) * span - 1.0;
        for d in [span * 0.5, 30.0, 40.0, last_full] {
            assert_eq!(
                taper.live_factor(d, size),
                taper.distance_factor(d, total - d, size),
                "live width differs from the release width at {d} px"
            );
        }
        assert_eq!(
            taper.size_factor(taper.live_factor(span * 2.0, size), 1.0),
            1.0,
            "a live dab past the start ramp is full width"
        );
        // The end ramp still narrows the tail on release.
        assert_eq!(taper.distance_factor(total, 0.0, size), 0.0);
    }

    #[test]
    fn zero_to_end_means_the_stroke_is_over() {
        // Regression: the live path passed 0 for `to_end`, which is not
        // "end unknown" but "already at the end", so every live dab took
        // the full end ramp (30% width for the default pen preset).
        let taper = Taper {
            end: 0.35,
            size: 0.7,
            ..Taper::NONE
        };
        assert_eq!(taper.distance_factor(100.0, 0.0, 6.0), 0.0);
        assert_eq!(taper.live_factor(100.0, 6.0), 1.0);
    }

    #[test]
    fn taper_size_and_pressure_scale_the_transition() {
        let full = Taper {
            start: 0.5,
            size: 1.0,
            ..Taper::NONE
        };
        assert_eq!(full.size_factor(0.0, 1.0), 0.0);
        assert!((full.size_factor(0.5, 1.0) - 0.5).abs() < 0.001);
        assert_eq!(full.size_factor(1.0, 1.0), 1.0);

        let flat = Taper { size: 0.0, ..full };
        assert_eq!(flat.size_factor(0.0, 1.0), 1.0);
        assert_eq!(flat.size_factor(0.5, 1.0), 1.0);

        let pressured = Taper {
            pressure: 1.0,
            ..full
        };
        assert!((pressured.size_factor(0.5, 1.0) - 0.5).abs() < 0.001);
        assert!(pressured.size_factor(0.5, 0.0) < 0.5);
        assert_eq!(pressured.size_factor(1.0, 0.0), 1.0);
    }

    #[test]
    fn taper_size_zero_keeps_the_tip_full_width() {
        let brush = Brush {
            size: 6.0,
            opacity: 1.0,
            softness: 0.0,
            spacing: 0.25,
            taper: Taper {
                start: 1.0,
                size: 0.0,
                ..Taper::NONE
            },
            ..Default::default()
        };
        let dyn0 = DabDynamics {
            taper: 0.0,
            ..Default::default()
        };
        let mut doc = Document::new(32, 12);
        doc.stroke_to_dyn(
            &brush,
            Color::rgb(255, 0, 0),
            None,
            dyn0,
            (6.0, 6.0),
            dyn0,
            0.0,
        );
        let alpha = doc.layers[0].pixels.get(((6 * doc.w + 6) * 4 + 3) as usize);
        assert!(alpha > 0, "taper size 0 should keep the tip full width");
    }

    #[test]
    fn stroke_taper_scales_size_and_opacity() {
        let brush = Brush {
            size: 6.0,
            opacity: 1.0,
            softness: 0.0,
            spacing: 0.25,
            taper: Taper {
                opacity: 1.0,
                ..Taper::NONE
            },
            ..Default::default()
        };
        let color = Color::rgb(255, 0, 0);
        let mut doc = Document::new(32, 12);
        let dyn0 = DabDynamics {
            taper: 0.0,
            ..Default::default()
        };
        doc.stroke_to_dyn(&brush, color, None, dyn0, (2.0, 6.0), dyn0, 0.0);
        doc.stroke_to_dyn(
            &brush,
            color,
            Some((2.0, 6.0)),
            dyn0,
            (16.0, 6.0),
            dyn0,
            0.0,
        );
        doc.stroke_to_dyn(
            &brush,
            color,
            Some((16.0, 6.0)),
            dyn0,
            (30.0, 6.0),
            DabDynamics::default(),
            0.0,
        );
        let alpha = |x: u32| doc.layers[0].pixels.get(((6 * doc.w + x) * 4 + 3) as usize);
        assert_eq!(alpha(3), 0);
        assert!(alpha(30) > 200, "full-taper alpha {}", alpha(30));
        assert!(alpha(20) > 0 && alpha(20) < alpha(30));
    }

    #[test]
    fn subpixel_dab_fades_instead_of_painting_full_alpha() {
        let brush = Brush {
            size: 0.25,
            opacity: 1.0,
            softness: 0.0,
            ..Default::default()
        };
        let mut doc = Document::new(8, 8);
        doc.dab(&brush, Color::rgb(255, 0, 0), 4.5, 4.5);
        let alpha = doc.layers[0].pixels.get(((4 * doc.w + 4) * 4 + 3) as usize);
        assert!(alpha > 0 && alpha < 128, "subpixel alpha {alpha}");
    }

    #[test]
    fn taper_width_profile_is_monotonic() {
        let brush = Brush {
            size: 40.0,
            opacity: 1.0,
            softness: 0.0,
            spacing: 0.25,
            taper: Taper {
                start: 0.25,
                end: 0.25,
                size: 1.0,
                ..Taper::NONE
            },
            ..Default::default()
        };
        let color = Color::rgb(255, 0, 0);
        let mut doc = Document::new(240, 80);
        let dyn0 = DabDynamics {
            taper: 0.0,
            ..Default::default()
        };
        let dyn_mid = DabDynamics {
            taper: 1.0,
            ..Default::default()
        };
        doc.stroke_to_dyn(&brush, color, None, dyn0, (20.0, 40.0), dyn0, 0.0);
        doc.stroke_to_dyn(
            &brush,
            color,
            Some((20.0, 40.0)),
            dyn0,
            (120.0, 40.0),
            dyn_mid,
            0.0,
        );
        doc.stroke_to_dyn(
            &brush,
            color,
            Some((120.0, 40.0)),
            dyn_mid,
            (220.0, 40.0),
            dyn0,
            0.0,
        );
        let width = |x: u32| {
            (0..doc.h)
                .filter(|&y| doc.layers[0].pixels.get(((y * doc.w + x) * 4 + 3) as usize) > 0)
                .count()
        };
        assert!(width(30) < width(60), "{} vs {}", width(30), width(60));
        assert!(width(60) < width(100), "{} vs {}", width(60), width(100));
        assert!(width(100) > width(180), "{} vs {}", width(100), width(180));
        assert!(width(180) > width(200), "{} vs {}", width(180), width(200));
        assert!(width(200) >= width(215), "{} vs {}", width(200), width(215));
    }

    #[test]
    fn tapered_tail_has_no_gaps() {
        let brush = Brush {
            size: 40.0,
            opacity: 1.0,
            softness: 0.0,
            spacing: 0.5,
            taper: Taper {
                end: 1.0,
                size: 0.8,
                ..Taper::NONE
            },
            ..Default::default()
        };
        let color = Color::rgb(255, 0, 0);
        let head = DabDynamics::default();
        let tail = DabDynamics {
            taper: 0.0,
            ..DabDynamics::default()
        };
        let mut doc = Document::new(260, 140);
        doc.stroke_to_dyn(&brush, color, None, head, (20.0, 100.0), head, 0.0);
        doc.stroke_to_dyn(
            &brush,
            color,
            Some((20.0, 100.0)),
            head,
            (220.0, 100.0),
            tail,
            0.0,
        );
        let alpha = |x: u32| {
            doc.layers[0]
                .pixels
                .get(((100 * doc.w + x) * 4 + 3) as usize)
        };
        let gap = (20..=220).find(|&x| alpha(x) == 0);
        assert_eq!(gap, None, "first tapered-tail gap at x={gap:?}");
    }

    #[test]
    fn short_segment_stamps_its_endpoint() {
        let brush = Brush {
            size: 24.0,
            opacity: 1.0,
            softness: 0.0,
            spacing: 1.0,
            ..Default::default()
        };
        let color = Color::rgb(255, 0, 0);
        let dyn_ = DabDynamics::default();
        let mut doc = Document::new(64, 32);
        doc.stroke_to_dyn(&brush, color, None, dyn_, (8.0, 16.0), dyn_, 0.0);
        doc.stroke_to_dyn(
            &brush,
            color,
            Some((8.0, 16.0)),
            dyn_,
            (28.0, 16.0),
            dyn_,
            0.0,
        );
        let alpha = doc.layers[0]
            .pixels
            .get(((16 * doc.w + 28) * 4 + 3) as usize);
        assert!(alpha > 0, "short segment endpoint alpha {alpha}");
    }

    #[test]
    fn signed_pressure_dynamics_extend_beyond_full_pressure() {
        let brush = Brush {
            size: 20.0,
            opacity: 0.5,
            pressure_size: -0.5,
            pressure_opacity: -0.5,
            velocity_size: 0.0,
            velocity_opacity: 0.0,
            ..Default::default()
        };
        assert!((brush.resolve_size(0.0, 0.0) - 30.0).abs() < 0.001);
        assert!((brush.resolve_size(1.0, 0.0) - 20.0).abs() < 0.001);
        assert!((brush.resolve_opacity(0.0, 0.0) - 0.75).abs() < 0.001);
        assert!((brush.resolve_opacity(1.0, 0.0) - 0.5).abs() < 0.001);
    }

    #[test]
    fn velocity_curves_replace_scalar_dynamics() {
        let brush = Brush {
            size: 20.0,
            opacity: 1.0,
            pressure_size: 0.0,
            pressure_opacity: 0.0,
            velocity_size: 0.1,
            velocity_opacity: 0.1,
            velocity_size_curve: Some(vec![[0.0, 1.0], [0.5, 1.0], [0.8, 0.0], [1.0, 0.0]]),
            velocity_opacity_curve: Some(vec![[0.0, 1.0], [1.0, 0.0]]),
            ..Default::default()
        };
        assert!((brush.resolve_size(1.0, 0.0) - 20.0).abs() < 0.001);
        assert!((brush.resolve_size(1.0, 0.4) - 20.0).abs() < 0.001);
        assert!(brush.resolve_size(1.0, 0.8).abs() < 0.001);
        assert!((brush.resolve_opacity(1.0, 0.5) - 0.5).abs() < 0.001);
        assert!(brush.resolve_opacity(1.0, 1.0).abs() < 0.001);
    }

    #[test]
    fn velocity_thins_and_fades_when_a_brush_opts_in() {
        // Speed dynamics moved to opt-in (the default is flat), so the M7
        // weights are pinned through an explicit brush.
        let brush = Brush {
            velocity_size: 0.6,
            velocity_opacity: 0.6,
            ..Default::default()
        };
        let size_full = brush.resolve_size(1.0, 1.0);
        let op_full = brush.resolve_opacity(1.0, 1.0);
        assert!(size_full < brush.size);
        assert!(op_full < brush.opacity);
        // Old M7 weights preserved: size 0.6, opacity 0.4 at full speed.
        assert!((size_full - brush.size * (1.0 - 0.6 * 0.6)).abs() < 0.001);
        assert!((op_full - brush.opacity * (1.0 - 0.4 * 0.6)).abs() < 0.001);
    }

    #[test]
    fn old_brush_json_loads_with_new_defaults() {
        let old = r#"[{"name":"Old","brush":{"size":10.0,"opacity":1.0,"softness":0.5,"spacing":0.2,"eraser":false}}]"#;
        let libs: Vec<BrushPreset> = serde_json::from_str(old).unwrap();
        assert_eq!(libs.len(), 1);
        let want = Brush {
            size: 10.0,
            softness: 0.5,
            spacing: 0.2,
            ..Default::default()
        };
        assert_eq!(libs[0].brush, want);
    }

    #[test]
    fn old_taper_json_gets_full_size_and_no_pressure() {
        let old = r#"[{"name":"Old","brush":{"size":10.0,"opacity":1.0,"softness":0.5,"spacing":0.2,"eraser":false,"taper":{"start":0.2,"end":0.2,"shape":0.0,"opacity":0.0}}}]"#;
        let libs: Vec<BrushPreset> = serde_json::from_str(old).unwrap();
        assert_eq!(libs[0].brush.taper.size, 1.0);
        assert_eq!(libs[0].brush.taper.pressure, 0.0);
    }

    fn alpha_sum(doc: &Document) -> u64 {
        doc.layers[0]
            .pixels
            .to_dense()
            .iter()
            .skip(3)
            .step_by(4)
            .map(|&b| b as u64)
            .sum()
    }

    fn painted_bbox(doc: &Document) -> Option<(u32, u32, u32, u32)> {
        let (w, h) = (doc.w, doc.h);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        let mut any = false;
        for y in 0..h {
            for x in 0..w {
                if doc.layers[0].pixels.get(((y * w + x) * 4 + 3) as usize) > 0 {
                    any = true;
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        any.then_some((x0, y0, x1, y1))
    }

    #[test]
    fn light_pressure_stroke_paints_less() {
        let brush = Brush {
            size: 12.0,
            softness: 0.0,
            spacing: 0.1,
            ..Default::default()
        };
        let stroke = |pressure: f32| {
            let mut doc = Document::new(64, 64);
            let d = DabDynamics {
                pressure,
                speed_norm: 0.0,
                angle: 0.0,
                taper: 1.0,
            };
            doc.stroke_to_dyn(
                &brush,
                Color::rgb(255, 0, 0),
                Some((10.0, 32.0)),
                d,
                (54.0, 32.0),
                d,
                0.0,
            );
            doc
        };
        let full = alpha_sum(&stroke(1.0));
        let light = alpha_sum(&stroke(0.2));
        assert!(full > 0 && light > 0);
        assert!(light < full, "light {light} vs full {full}");
    }

    #[test]
    fn grain_breaks_up_the_dab() {
        let clean = Brush {
            size: 20.0,
            softness: 0.0,
            spacing: 0.5,
            ..Default::default()
        };
        let grained = Brush {
            grain: 0.8,
            grain_scale: 1.0,
            ..clean.clone()
        };
        let paint = |brush: &Brush| {
            let mut doc = Document::new(48, 48);
            doc.dab(brush, Color::rgb(255, 255, 255), 24.0, 24.0);
            doc
        };
        let (c, g) = (paint(&clean), paint(&grained));
        assert!(alpha_sum(&g) < alpha_sum(&c));
        // Same footprint (grain never fully erases), different values.
        assert_eq!(painted_bbox(&g), painted_bbox(&c));
        assert_ne!(c.layers[0].pixels, g.layers[0].pixels);
    }

    #[test]
    fn imported_grain_texture_breaks_up_the_dab() {
        let clean = Brush {
            size: 20.0,
            softness: 0.0,
            spacing: 0.5,
            ..Default::default()
        };
        let texture = BrushTip::from_mask(2, 2, vec![255, 128, 128, 255]).unwrap();
        let grained = Brush {
            grain: 1.0,
            grain_scale: 1.0,
            grain_tip: Some(texture),
            ..clean.clone()
        };
        let paint = |brush: &Brush| {
            let mut doc = Document::new(48, 48);
            doc.dab(brush, Color::rgb(255, 255, 255), 24.0, 24.0);
            doc
        };
        let (c, g) = (paint(&clean), paint(&grained));
        assert!(alpha_sum(&g) < alpha_sum(&c));
        assert_eq!(painted_bbox(&g), painted_bbox(&c));
        assert_ne!(c.layers[0].pixels, g.layers[0].pixels);
    }

    #[test]
    fn static_angle_rotates_the_stamp() {
        let tip = BrushTip::from_mask(8, 4, vec![255u8; 32]).unwrap();
        let flat = Brush {
            size: 8.0,
            opacity: 1.0,
            spacing: 2.0,
            tip: Some(tip),
            ..Default::default()
        };
        let turned = Brush {
            angle: std::f32::consts::FRAC_PI_2,
            ..flat.clone()
        };
        let bbox = |brush: &Brush| {
            let mut doc = Document::new(48, 48);
            doc.dab(brush, Color::rgb(255, 255, 255), 24.0, 24.0);
            painted_bbox(&doc).unwrap()
        };
        let (fx0, fy0, fx1, fy1) = bbox(&flat);
        let (tx0, ty0, tx1, ty1) = bbox(&turned);
        assert!(fx1 - fx0 > fy1 - fy0);
        assert!(ty1 - ty0 > tx1 - tx0);
    }

    #[test]
    fn rotated_grain_texture_wraps_inside_the_mask() {
        let tip = BrushTip::from_mask(256, 256, vec![128u8; 256 * 256]).unwrap();
        let brush = Brush {
            grain_tip: Some(tip),
            grain_angle: 90f32.to_radians(),
            grain_scale: 0.25,
            ..Default::default()
        };
        for py in 0..512 {
            for px in 0..512 {
                let _ = grain_sample(&brush, px, py, 22.0);
            }
        }
    }

    #[test]
    fn rotated_grain_samples_stay_in_bounds() {
        let tip = BrushTip::from_mask(256, 256, vec![255u8; 256 * 256]).unwrap();
        let brush = Brush {
            grain: 1.0,
            grain_scale: 0.25,
            grain_angle: std::f32::consts::FRAC_PI_2,
            grain_tip: Some(tip),
            ..Default::default()
        };
        for y in 0..512 {
            for x in 0..512 {
                let sample = grain_sample(&brush, x, y, 22.0);
                assert!((0.0..=1.0).contains(&sample));
            }
        }
    }

    #[test]
    fn roundness_squeezes_across_the_stroke() {
        let paint = |roundness: f32| {
            let brush = Brush {
                size: 20.0,
                softness: 0.0,
                spacing: 0.5,
                roundness,
                ..Default::default()
            };
            let mut doc = Document::new(48, 48);
            doc.dab(&brush, Color::rgb(255, 255, 255), 24.0, 24.0);
            doc
        };
        let (x0r, y0r, x1r, y1r) = painted_bbox(&paint(1.0)).unwrap();
        let (x0f, y0f, x1f, y1f) = painted_bbox(&paint(0.3)).unwrap();
        // Angle 0: u along x stays full width, v (y) is squeezed.
        assert_eq!((x0f, x1f), (x0r, x1r));
        assert!(
            y1f - y0f < y1r - y0r,
            "flat {} vs round {}",
            y1f - y0f,
            y1r - y0r
        );
    }

    #[test]
    fn scatter_moves_dabs() {
        let plain = Brush {
            size: 12.0,
            softness: 0.5,
            spacing: 0.2,
            ..Default::default()
        };
        let scattered = Brush {
            scatter: 0.6,
            ..plain.clone()
        };
        let paint = |brush: &Brush| {
            let mut doc = Document::new(64, 64);
            let d = DabDynamics::default();
            doc.stroke_to_dyn(
                brush,
                Color::rgb(255, 0, 0),
                Some((8.0, 32.0)),
                d,
                (56.0, 32.0),
                d,
                0.0,
            );
            doc
        };
        assert_ne!(
            paint(&plain).layers[0].pixels,
            paint(&scattered).layers[0].pixels
        );
    }

    #[test]
    fn angle_follow_steers_stamp_orientation() {
        // Wide flat tip: horizontal strokes stamp wide, vertical tall.
        let tip = BrushTip::from_mask(8, 4, vec![255u8; 32]).unwrap();
        let brush = Brush {
            size: 8.0,
            opacity: 1.0,
            spacing: 2.0,
            angle_follow: true,
            ..Default::default()
        };
        let brush = Brush {
            tip: Some(tip),
            ..brush
        };
        let paint = |from: (f32, f32), to: (f32, f32), angle: f32| {
            let mut doc = Document::new(48, 48);
            let d = DabDynamics {
                pressure: 1.0,
                speed_norm: 0.0,
                angle,
                taper: 1.0,
            };
            // Short segment + wide spacing: exactly one dab at `from`.
            doc.stroke_to_dyn(&brush, Color::rgb(255, 255, 255), Some(from), d, to, d, 0.0);
            painted_bbox(&doc).unwrap()
        };
        let (hx0, hy0, hx1, hy1) = paint((24.0, 24.0), (32.0, 24.0), 0.0);
        let (vx0, vy0, vx1, vy1) = paint((24.0, 24.0), (24.0, 32.0), std::f32::consts::FRAC_PI_2);
        assert!(hx1 - hx0 > hy1 - hy0, "horizontal should be wide");
        assert!(vy1 - vy0 > vx1 - vx0, "vertical should be tall");
    }

    #[test]
    fn coincident_shapes_share_the_stroke_opacity() {
        // Shapes of one dab land on the same spot unless scatter/jitter
        // separates them: they are one paint event, so the ceiling stays
        // the brush opacity instead of stacking shape_count times.
        let single = Brush {
            size: 20.0,
            opacity: 0.25,
            spacing: 2.0,
            ..Default::default()
        };
        let multiple = Brush {
            shape_count: 4,
            ..single.clone()
        };
        let alpha = |brush: &Brush| {
            let mut doc = Document::new(48, 48);
            doc.dab(brush, Color::rgb(255, 0, 0), 24.0, 24.0);
            doc.layers[0].pixels.get((24 * 48 + 24) * 4 + 3)
        };
        assert_eq!(alpha(&multiple), alpha(&single));
        assert!(alpha(&single) <= 64, "{}", alpha(&single));
    }

    #[test]
    fn shape_count_jitter_stays_within_bounds_and_varies() {
        let brush = Brush {
            shape_count: 8,
            shape_count_jitter: 1.0,
            ..Default::default()
        };
        let counts: Vec<u8> = (0..64)
            .map(|i| shape_count_for(&brush, i as f32 * 3.7, i as f32 * 1.9))
            .collect();
        assert!(counts.iter().all(|&count| (1..=8).contains(&count)));
        assert!(counts.iter().any(|&count| count < 8));
        let again: Vec<u8> = (0..64)
            .map(|i| shape_count_for(&brush, i as f32 * 3.7, i as f32 * 1.9))
            .collect();
        assert_eq!(counts, again);
        let fixed = Brush {
            shape_count_jitter: 0.0,
            ..brush
        };
        assert_eq!(shape_count_for(&fixed, 12.0, 9.0), 8);
    }

    #[test]
    fn roundness_jitter_varies_per_dab_and_stays_in_range() {
        let brush = Brush {
            roundness: 1.0,
            roundness_jitter: 1.0,
            ..Default::default()
        };
        let values: Vec<f32> = (0..64)
            .map(|i| dab_roundness(&brush, i as f32 * 3.7, i as f32 * 1.9))
            .collect();
        assert!(values.iter().all(|&value| (0.1..=1.0).contains(&value)));
        assert!(values.iter().any(|&value| value < 1.0));
        let again: Vec<f32> = (0..64)
            .map(|i| dab_roundness(&brush, i as f32 * 3.7, i as f32 * 1.9))
            .collect();
        assert_eq!(values, again);
        let fixed = Brush {
            roundness_jitter: 0.0,
            ..brush
        };
        assert_eq!(dab_roundness(&fixed, 12.0, 9.0), 1.0);
    }

    #[test]
    fn size_and_opacity_jitter_vary_and_are_deterministic() {
        let brush = Brush {
            size_jitter: 1.0,
            opacity_jitter: 1.0,
            ..Default::default()
        };
        let values: Vec<(f32, f32)> = (0..64)
            .map(|i| jittered_dab(&brush, i as f32 * 3.7, i as f32 * 1.9, 0, 10.0, 1.0))
            .collect();
        assert!(
            values
                .iter()
                .all(|&(size, opacity)| (0.0..=10.0).contains(&size)
                    && (0.0..=1.0).contains(&opacity))
        );
        assert!(values.iter().any(|&(size, _)| size < 10.0));
        assert!(values.iter().any(|&(_, opacity)| opacity < 1.0));
        let again: Vec<(f32, f32)> = (0..64)
            .map(|i| jittered_dab(&brush, i as f32 * 3.7, i as f32 * 1.9, 0, 10.0, 1.0))
            .collect();
        assert_eq!(values, again);
        let fixed = Brush {
            size_jitter: 0.0,
            opacity_jitter: 0.0,
            ..brush
        };
        assert_eq!(jittered_dab(&fixed, 12.0, 9.0, 2, 10.0, 0.8), (10.0, 0.8));
    }

    #[test]
    fn shape_rotation_follows_the_stroke_without_angle_follow() {
        let tip = BrushTip::from_mask(8, 4, vec![255u8; 32]).unwrap();
        let brush = Brush {
            size: 8.0,
            opacity: 1.0,
            spacing: 2.0,
            tip: Some(tip),
            shape_rotation: 1.0,
            ..Default::default()
        };
        let paint = |from: (f32, f32), to: (f32, f32), angle: f32| {
            let mut doc = Document::new(48, 48);
            let d = DabDynamics {
                pressure: 1.0,
                speed_norm: 0.0,
                angle,
                taper: 1.0,
            };
            doc.stroke_to_dyn(&brush, Color::rgb(255, 255, 255), Some(from), d, to, d, 0.0);
            painted_bbox(&doc).unwrap()
        };
        let (hx0, hy0, hx1, hy1) = paint((24.0, 24.0), (32.0, 24.0), 0.0);
        let (vx0, vy0, vx1, vy1) = paint((24.0, 24.0), (24.0, 32.0), std::f32::consts::FRAC_PI_2);
        assert!(hx1 - hx0 > hy1 - hy0, "horizontal should be wide");
        assert!(vy1 - vy0 > vx1 - vx0, "vertical should be tall");
    }

    #[test]
    fn angle_scatter_rotates_the_stamp() {
        let tip = BrushTip::from_mask(8, 4, vec![255u8; 32]).unwrap();
        let brush = Brush {
            size: 8.0,
            opacity: 1.0,
            spacing: 2.0,
            tip: Some(tip),
            angle_scatter: 1.0,
            shape_count: 4,
            ..Default::default()
        };
        let paint = |brush: &Brush| {
            let mut doc = Document::new(48, 48);
            doc.dab(brush, Color::rgb(255, 255, 255), 24.0, 24.0);
            doc.layers[0].pixels.clone()
        };
        assert_ne!(
            paint(&brush),
            paint(&Brush {
                angle_scatter: 0.0,
                ..brush.clone()
            })
        );
    }

    #[test]
    fn validation_rejects_new_ranges() {
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-lib-m14-range.pcbrush");
        for brush in [
            Brush {
                pressure_size: 2.0,
                ..Default::default()
            },
            Brush {
                pressure_opacity: -1.5,
                ..Default::default()
            },
            Brush {
                velocity_size: 1.5,
                ..Default::default()
            },
            Brush {
                grain: 1.5,
                ..Default::default()
            },
            Brush {
                grain_scale: 9.0,
                ..Default::default()
            },
            Brush {
                roundness: 0.0,
                ..Default::default()
            },
            Brush {
                taper: Taper {
                    size: 1.5,
                    ..Taper::NONE
                },
                ..Default::default()
            },
            Brush {
                taper: Taper {
                    pressure: -0.1,
                    ..Taper::NONE
                },
                ..Default::default()
            },
            Brush {
                roundness_jitter: 1.5,
                ..Default::default()
            },
            Brush {
                size_jitter: 1.5,
                ..Default::default()
            },
            Brush {
                opacity_jitter: -0.5,
                ..Default::default()
            },
            Brush {
                scatter: -1.0,
                ..Default::default()
            },
            Brush {
                shape_rotation: 2.0,
                ..Default::default()
            },
            Brush {
                angle_scatter: 1.5,
                ..Default::default()
            },
            Brush {
                shape_count: 0,
                ..Default::default()
            },
            Brush {
                shape_count_jitter: 1.5,
                ..Default::default()
            },
            Brush {
                pressure_curve: Some(vec![[0.0, 0.0]]),
                ..Default::default()
            },
            Brush {
                pressure_opacity_curve: Some(vec![[0.0, 0.0]]),
                ..Default::default()
            },
            Brush {
                velocity_size_curve: Some(vec![[0.0, 0.0]]),
                ..Default::default()
            },
            Brush {
                velocity_opacity_curve: Some(vec![[0.0, 0.0]]),
                ..Default::default()
            },
            Brush {
                stabilizer: 2.0,
                ..Default::default()
            },
        ] {
            let libs = vec![BrushPreset {
                name: "Bad".to_string(),
                brush,
            }];
            assert!(save_brush_library(&path, &libs).is_err());
        }
        let signed = vec![BrushPreset {
            name: "Signed".to_string(),
            brush: Brush {
                pressure_size: -0.5,
                pressure_opacity: -0.5,
                ..Default::default()
            },
        }];
        save_brush_library(&path, &signed).unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn brush_library_roundtrips_builtins() {
        let libs = sample_library();
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-lib-roundtrip.pcbrush");
        save_brush_library(&path, &libs).unwrap();
        let back = open_brush_library(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back, libs);
    }

    #[test]
    fn brush_library_export_forces_eraser_off() {
        let mut libs = sample_library();
        libs[0].brush.eraser = true;
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-lib-eraser.pcbrush");
        save_brush_library(&path, &libs).unwrap();
        let back = open_brush_library(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(!back[0].brush.eraser);
    }

    #[test]
    fn brush_library_rejects_garbage() {
        let dir = std::env::temp_dir();
        // Empty export.
        let path = dir.join("procraft-lib-empty.pcbrush");
        assert!(save_brush_library(&path, &[]).is_err());
        // Not JSON at all.
        std::fs::write(&path, "this is not json").unwrap();
        assert!(open_brush_library(&path).is_err());
        // Empty JSON array.
        std::fs::write(&path, "[]").unwrap();
        assert!(open_brush_library(&path).is_err());
        // Empty name.
        let bad = vec![BrushPreset {
            name: "   ".to_string(),
            brush: Brush::default(),
        }];
        std::fs::write(&path, serde_json::to_string(&bad).unwrap()).unwrap();
        assert!(open_brush_library(&path).is_err());
        // Out-of-range size.
        let bad = vec![BrushPreset {
            name: "Big".to_string(),
            brush: Brush {
                size: 9999.0,
                ..Brush::default()
            },
        }];
        std::fs::write(&path, serde_json::to_string(&bad).unwrap()).unwrap();
        assert!(open_brush_library(&path).is_err());
        // Eraser brush.
        let bad = vec![BrushPreset {
            name: "Eraser".to_string(),
            brush: Brush {
                eraser: true,
                ..Brush::default()
            },
        }];
        std::fs::write(&path, serde_json::to_string(&bad).unwrap()).unwrap();
        assert!(open_brush_library(&path).is_err());
        // Too many brushes.
        let many: Vec<BrushPreset> = (0..MAX_LIBRARY_BRUSHES + 1)
            .map(|i| BrushPreset {
                name: format!("b{i}"),
                brush: Brush::default(),
            })
            .collect();
        assert!(save_brush_library(&path, &many).is_err());
        std::fs::write(&path, serde_json::to_string(&many).unwrap()).unwrap();
        assert!(open_brush_library(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    /// Byte offset of pixel (x, y) on a w-wide canvas (test helper).
    fn pxo(w: u32, x: u32, y: u32) -> usize {
        ((y * w + x) * 4) as usize
    }

    fn smudge_brush() -> Brush {
        Brush {
            size: 5.0,
            softness: 0.0,
            spacing: 0.2,
            ..Brush::default()
        }
    }

    #[test]
    fn smudge_drags_color_down_the_stroke() {
        let mut doc = Document::new(16, 4);
        // Solid red dot at (2, 1).
        doc.layers[0].pixels.set(pxo(16, 2, 1), 255);
        doc.layers[0].pixels.set(pxo(16, 2, 1) + 3, 255);
        let brush = smudge_brush();
        let (last, left) = doc.smudge_to(&brush, 1.0, 1.0, None, (2.0, 1.0), 0.0);
        assert!(last == Some((2.0, 1.0)) && left == 0.0);
        let (_, _) = doc.smudge_to(&brush, 1.0, 1.0, last, (12.0, 1.0), left);
        let at = |x: u32| doc.layers[0].pixels.get(pxo(16, x, 1));
        // Red travelled right; the source faded as the finger left it.
        assert!(at(8) > 40, "mid-stroke red = {}", at(8));
        assert!(at(2) < 255, "source faded = {}", at(2));
    }

    #[test]
    fn smudge_zero_strength_is_a_noop() {
        let mut doc = Document::new(8, 8);
        doc.layers[0].pixels.set(pxo(8, 4, 4), 200);
        let before = doc.layers[0].pixels.clone();
        let brush = smudge_brush();
        doc.smudge_to(&brush, 0.0, 1.0, None, (4.0, 4.0), 0.0);
        doc.smudge_to(&brush, 1.0, 0.0, None, (4.0, 4.0), 0.0);
        assert_eq!(doc.layers[0].pixels, before);
    }

    #[test]
    fn smudge_finger_resets_each_stroke() {
        let mut doc = Document::new(16, 4);
        doc.layers[0].pixels.set(pxo(16, 2, 1), 255);
        doc.layers[0].pixels.set(pxo(16, 2, 1) + 3, 255);
        let brush = smudge_brush();
        // First stroke picks up red at (2,1).
        doc.smudge_to(&brush, 1.0, 1.0, None, (2.0, 1.0), 0.0);
        // Second stroke starts on empty black: a shared finger would
        // drag red in from the first stroke.
        doc.smudge_to(&brush, 1.0, 1.0, None, (12.0, 1.0), 0.0);
        let at = |x: u32| doc.layers[0].pixels.get(pxo(16, x, 1));
        assert_eq!(at(12), 0);
    }

    #[test]
    fn smudge_honors_rect_selection() {
        let mut doc = Document::new(16, 4);
        // Left half solid red, right half transparent.
        for x in 0..8u32 {
            doc.layers[0].pixels.set(pxo(16, x, 1), 255);
            doc.layers[0].pixels.set(pxo(16, x, 1) + 3, 255);
        }
        // Only the left half is selected; smudge across the whole row.
        doc.selection = Some((0, 1, 7, 1));
        let brush = smudge_brush();
        let (last, left) = doc.smudge_to(&brush, 1.0, 1.0, None, (0.0, 1.0), 0.0);
        doc.smudge_to(&brush, 1.0, 1.0, last, (15.0, 1.0), left);
        // Unselected half stays transparent (no red dragged across).
        for x in 8..16u32 {
            let o = pxo(16, x, 1);
            assert_eq!(doc.layers[0].pixels.get(o + 3), 0, "x={x}");
        }
    }

    #[test]
    fn per_brush_blend_difference_mixes_backdrop() {
        use crate::document::{BlendMode, Document};
        let paint = |mode: BlendMode| {
            let mut doc = Document::new(8, 8);
            let base = Brush {
                size: 20.0,
                softness: 0.0,
                opacity: 1.0,
                ..Default::default()
            };
            doc.dab(&base, Color::rgb(0, 255, 0), 3.5, 3.5);
            let mut brush = Brush {
                size: 20.0,
                softness: 0.0,
                opacity: 1.0,
                blend: mode,
                ..Default::default()
            };
            brush.tip = None;
            doc.dab(&brush, Color::rgb(255, 0, 0), 3.5, 3.5);
            let o = ((3 * 8 + 3) * 4) as usize;
            [
                doc.layers[0].pixels.get(o),
                doc.layers[0].pixels.get(o + 1),
                doc.layers[0].pixels.get(o + 2),
                doc.layers[0].pixels.get(o + 3),
            ]
        };
        // Normal covers; Difference(|red - green|) lands on yellow.
        assert_eq!(paint(BlendMode::Normal), [255, 0, 0, 255]);
        assert_eq!(paint(BlendMode::Difference), [255, 255, 0, 255]);
    }

    #[test]
    fn wet_edge_lifts_fringe_alpha() {
        use crate::document::Document;
        let sum_alpha = |wet: f32| {
            let mut doc = Document::new(24, 24);
            let brush = Brush {
                size: 16.0,
                softness: 0.8,
                opacity: 0.5,
                wet_edge: wet,
                ..Default::default()
            };
            doc.dab(&brush, Color::rgb(255, 255, 255), 11.5, 11.5);
            doc.layers[0]
                .pixels
                .to_dense()
                .iter()
                .skip(3)
                .step_by(4)
                .map(|&a| a as u32)
                .sum::<u32>()
        };
        let dry = sum_alpha(0.0);
        let wet = sum_alpha(1.0);
        assert!(dry > 0 && wet > dry, "dry={dry} wet={wet}");
    }

    #[test]
    fn dual_tip_suppresses_where_secondary_is_clear() {
        use crate::document::Document;
        // 2x2 secondary: ink only at its top-left texel.
        let dual = BrushTip::from_mask(2, 2, vec![255, 0, 0, 0]).unwrap();
        let mut doc = Document::new(16, 16);
        let brush = Brush {
            size: 8.0,
            softness: 0.0,
            opacity: 1.0,
            dual_tip: Some(dual),
            dual_scale: 1.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(255, 255, 255), 7.5, 7.5);
        let on = doc.layers[0]
            .pixels
            .to_dense()
            .iter()
            .filter(|&&b| b != 0)
            .count();
        let mut plain = Document::new(16, 16);
        plain.dab(
            &Brush {
                size: 8.0,
                softness: 0.0,
                opacity: 1.0,
                ..Default::default()
            },
            Color::rgb(255, 255, 255),
            7.5,
            7.5,
        );
        let full = plain.layers[0]
            .pixels
            .to_dense()
            .iter()
            .filter(|&&b| b != 0)
            .count();
        assert!(full > 0 && on < full, "full={full} dual={on}");
    }

    #[test]
    fn scaled_tip_edge_is_bilinear_not_blocky() {
        // A 2x2 tip blown up to 32 px: nearest sampling flips a texel
        // column 0 -> 255 within one pixel (visible steps on a big round
        // brush); bilinear spreads the same edge over a texel.
        let mut doc = Document::new(48, 48);
        let brush = Brush {
            size: 32.0,
            softness: 0.0,
            opacity: 1.0,
            pressure_size: 0.0,
            pressure_opacity: 0.0,
            tip: Some(BrushTip {
                w: 2,
                h: 2,
                mask: vec![0, 255, 0, 255],
            }),
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(255, 255, 255), 24.0, 24.0);
        let alpha = |x: u32, y: u32| doc.layers[0].pixels.get(((y * 48 + x) * 4 + 3) as usize);
        let row: Vec<u8> = (8..=40).map(|x| alpha(x, 24)).collect();
        let partial = row.iter().filter(|&&a| a > 0 && a < 255).count();
        assert!(partial >= 3, "tip edge is blocky: {row:?}");
        assert!(
            row.iter().any(|&a| a > 200),
            "tip lost its solid side: {row:?}"
        );
    }

    #[test]
    fn hard_dab_edge_is_anti_aliased() {
        // A hard disc used to flip 0 -> 255 in one pixel, which is what
        // makes thin strokes stair-step and short caps read as blunt cuts.
        let mut doc = Document::new(64, 64);
        let brush = Brush {
            size: 20.0,
            softness: 0.0,
            opacity: 1.0,
            pressure_size: 0.0,
            pressure_opacity: 0.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(20, 90, 220), 32.3, 31.7);
        let alpha = |x: u32, y: u32| doc.layers[0].pixels.get(((y * 64 + x) * 4) as usize + 3);
        let row: Vec<u8> = (15..=48).map(|x| alpha(x, 31)).collect();
        let partial = row.iter().filter(|&&a| a > 0 && a < 255).count();
        assert!(partial >= 2, "rim has no partial coverage: {row:?}");
        let solid = row.iter().filter(|&&a| a == 255).count();
        assert!(solid >= 10, "core lost its solidity: {row:?}");
    }

    #[test]
    fn hard_dab_edge_ramps_instead_of_jumping() {
        // Neighbouring pixels across the rim never jump straight from
        // transparent to opaque: every side of the disc shows a step.
        let mut doc = Document::new(64, 64);
        let brush = Brush {
            size: 20.0,
            softness: 0.0,
            opacity: 1.0,
            pressure_size: 0.0,
            pressure_opacity: 0.0,
            ..Default::default()
        };
        doc.dab(&brush, Color::rgb(20, 90, 220), 31.4, 32.6);
        let alpha = |x: u32, y: u32| doc.layers[0].pixels.get(((y * 64 + x) * 4) as usize + 3);
        for y in 20..=44 {
            let mut left = None;
            for x in 16..=32 {
                let a = alpha(x, y);
                if a > 0 {
                    left = Some(a);
                    break;
                }
            }
            if let Some(a) = left {
                assert!(a < 255, "left rim of row {y} starts at 255 ({a})");
            }
        }
    }

    #[test]
    fn default_brush_edge_is_nearly_hard() {
        // The app boots into Brush::default() and the Round preset mirrors
        // it. Softness above a quarter turns every fresh stroke into an
        // airbrush: the solid core shrinks to 40% of the radius and the rim
        // ramps over 6 px at size 24, which reads as out of focus next to a
        // hard default round.
        let default = Brush::default();
        assert!(
            default.softness <= 0.25,
            "default softness {} paints an airbrush edge",
            default.softness
        );
        let round = presets()
            .into_iter()
            .find(|(name, _)| *name == "Round")
            .map(|(_, b)| b)
            .expect("Round preset");
        assert!(round.softness <= 0.25, "Round softness {}", round.softness);
        // At most ~1 px of rim per side at size 24, and a solid core.
        let mut doc = Document::new(64, 64);
        doc.dab(&default, Color::rgb(20, 90, 220), 32.0, 32.0);
        let alpha = |x: u32| doc.layers[0].pixels.get(((32 * 64 + x) * 4) as usize + 3);
        let row: Vec<u8> = (18..=46).map(alpha).collect();
        let partial = row.iter().filter(|&&a| a > 0 && a < 255).count();
        assert!(partial <= 6, "default rim ramps over {partial} px: {row:?}");
        let solid = row.iter().filter(|&&a| a >= 250).count();
        assert!(solid >= 16, "default core only {solid} px solid: {row:?}");
    }

    /// One straight stroke through the engine, chained like the app does.
    fn stroke_flat(doc: &mut Document, brush: &Brush, y: f32, x0: f32, x1: f32) {
        let mut last = None;
        let mut left = 0.0;
        let d = DabDynamics {
            pressure: 1.0,
            speed_norm: 0.0,
            angle: 0.0,
            taper: 1.0,
        };
        let mut x = x0;
        while x <= x1 {
            let (l, lo) =
                doc.stroke_to_dyn(brush, Color::rgb(10, 10, 10), last, d, (x, y), d, left);
            last = l;
            left = lo;
            x += 2.0;
        }
    }

    fn max_alpha(doc: &Document) -> u8 {
        (0..doc.h)
            .flat_map(|y| (0..doc.w).map(move |x| (x, y)))
            .map(|(x, y)| doc.layers[0].pixels.get(((y * doc.w + x) * 4) as usize + 3))
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn one_stroke_stops_at_brush_opacity() {
        // 0.25 opacity used to build to ~0.99 where dabs overlapped: every
        // soft or translucent brush painted as a solid tube.
        let mut doc = Document::new(140, 60);
        let brush = Brush {
            size: 16.0,
            opacity: 0.25,
            softness: 0.8,
            spacing: 0.1,
            pressure_size: 0.0,
            pressure_opacity: 0.0,
            velocity_size: 0.0,
            velocity_opacity: 0.0,
            grain: 0.0,
            scatter: 0.0,
            stabilizer: 0.0,
            ..Default::default()
        };
        stroke_flat(&mut doc, &brush, 30.0, 12.0, 128.0);
        let peak = max_alpha(&doc);
        assert!(
            (56..=70).contains(&peak),
            "stroke should cap near 0.25*255, got {peak}"
        );
    }

    #[test]
    fn soft_stroke_keeps_its_feathered_edge() {
        let mut doc = Document::new(140, 80);
        let brush = Brush {
            size: 40.0,
            opacity: 1.0,
            softness: 1.0,
            spacing: 0.1,
            pressure_size: 0.0,
            pressure_opacity: 0.0,
            velocity_size: 0.0,
            velocity_opacity: 0.0,
            grain: 0.0,
            scatter: 0.0,
            stabilizer: 0.0,
            ..Default::default()
        };
        stroke_flat(&mut doc, &brush, 40.0, 12.0, 128.0);
        let at = |dy: u32| {
            doc.layers[0]
                .pixels
                .get((((40 - dy) * 140 + 70) * 4) as usize + 3)
        };
        // Sampled a quarter of the way out, the falloff must still read
        // like a gradient instead of a saturated plateau.
        assert!(at(0) > 240, "center {}", at(0));
        assert!(at(15) < 140, "edge saturated at +15px: {}", at(15));
        assert!(at(15) < at(0), "no gradient: {} vs {}", at(15), at(0));
    }

    #[test]
    fn separate_strokes_still_build_up() {
        let mut doc = Document::new(140, 60);
        let brush = Brush {
            size: 16.0,
            opacity: 0.5,
            softness: 0.6,
            spacing: 0.1,
            pressure_size: 0.0,
            pressure_opacity: 0.0,
            velocity_size: 0.0,
            velocity_opacity: 0.0,
            grain: 0.0,
            scatter: 0.0,
            stabilizer: 0.0,
            ..Default::default()
        };
        stroke_flat(&mut doc, &brush, 30.0, 12.0, 128.0);
        let first = max_alpha(&doc);
        stroke_flat(&mut doc, &brush, 30.0, 12.0, 128.0);
        let second = max_alpha(&doc);
        assert!((120..=140).contains(&first), "first {first}");
        assert!(second > 175, "second stroke did not build up: {second}");
    }

    #[test]
    fn eraser_stroke_stops_at_its_opacity() {
        let mut doc = Document::new(140, 60);
        let paint = Brush {
            size: 30.0,
            opacity: 1.0,
            softness: 0.0,
            spacing: 0.1,
            pressure_size: 0.0,
            pressure_opacity: 0.0,
            velocity_size: 0.0,
            velocity_opacity: 0.0,
            ..Default::default()
        };
        stroke_flat(&mut doc, &paint, 30.0, 12.0, 128.0);
        let eraser = Brush {
            eraser: true,
            opacity: 0.5,
            ..paint.clone()
        };
        stroke_flat(&mut doc, &eraser, 30.0, 12.0, 128.0);
        let center = doc.layers[0].pixels.get(((30 * 140 + 70) * 4) as usize + 3);
        assert!(
            (110..=145).contains(&center),
            "eraser at 0.5 left alpha {center}"
        );
    }

    #[test]
    fn old_brush_files_default_the_new_depth_fields() {
        // Pre-M39 library JSON has no blend/wet/dual keys: same defaults.
        // Every present key mirrors Brush::default(), so the assertion stays
        // about the absent ones.
        let old = r#"{"size":24.0,"opacity":1.0,"softness":0.2,"spacing":0.15,"eraser":false,"tip":null,"pressure_size":0.7,"pressure_opacity":0.6,"velocity_size":0.0,"velocity_opacity":0.0,"grain":0.0,"grain_scale":1.0,"roundness":1.0,"angle_follow":false,"scatter":0.0,"stabilizer":0.35}"#;
        let brush: Brush = serde_json::from_str(old).unwrap();
        assert_eq!(brush, Brush::default());
    }

    // --- Built-in preset character ---
    //
    // A mouse has no pressure sensor, so speed was the only dynamics signal
    // and every preset thinned into a thread between full-size blobs. These
    // tests pin the preset character at full pressure and zero speed, which
    // is exactly what a mouse stroke looks like apart from the speed term.

    fn preset_brush(name: &str) -> Brush {
        presets()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map(|(_, b)| b)
            .unwrap_or_else(|| panic!("no preset named {name}"))
    }

    /// One straight full-pressure, zero-speed stroke across a fresh document.
    fn paint_straight(name: &str, w: u32, h: u32) -> Document {
        paint_straight_brush(&preset_brush(name), w, h)
    }

    fn paint_straight_brush(brush: &Brush, w: u32, h: u32) -> Document {
        let mut doc = Document::new(w, h);
        let d = DabDynamics::default();
        let y = h as f32 / 2.0;
        let (x0, x1) = (8.0, w as f32 - 8.0);
        doc.stroke_to_dyn(brush, Color::rgb(0, 0, 0), None, d, (x0, y), d, 0.0);
        doc.stroke_to_dyn(
            brush,
            Color::rgb(0, 0, 0),
            Some((x0, y)),
            d,
            (x1, y),
            d,
            0.0,
        );
        doc.end_stroke();
        doc
    }

    fn alpha_at(doc: &Document, x: u32, y: u32) -> u8 {
        doc.layers[0].pixels.get(((y * doc.w + x) * 4 + 3) as usize)
    }

    fn partial_rows(doc: &Document, x: u32) -> usize {
        (0..doc.h)
            .filter(|&y| {
                let a = alpha_at(doc, x, y);
                a > 0 && a < 200
            })
            .count()
    }

    #[test]
    fn presets_that_used_speed_no_longer_thin_or_fade_with_it() {
        for name in ["Round", "Marker", "Airbrush", "Chalk"] {
            let b = preset_brush(name);
            assert!(
                (b.resolve_size(1.0, 1.0) - b.resolve_size(1.0, 0.0)).abs() < 1e-4,
                "{name} still thins with speed"
            );
            assert!(
                (b.resolve_opacity(1.0, 1.0) - b.resolve_opacity(1.0, 0.0)).abs() < 1e-4,
                "{name} still fades with speed"
            );
        }
    }

    #[test]
    fn default_brush_paints_the_same_width_at_any_speed() {
        let b = Brush::default();
        assert_eq!(b.resolve_size(1.0, 1.0), b.size);
        assert_eq!(b.resolve_opacity(1.0, 1.0), b.opacity);
    }

    #[test]
    fn pencil_and_ink_keep_their_speed_response() {
        assert!(preset_brush("Pencil").velocity_size > 0.0);
        assert!(preset_brush("Ink Pen").velocity_size > 0.0);
    }

    #[test]
    fn round_preset_draws_one_even_width() {
        let doc = paint_straight("Round", 200, 60);
        let col = |x: u32| (0..60).filter(|&y| alpha_at(&doc, x, y) > 0).count();
        let (a, b, c) = (col(40), col(100), col(160));
        assert!(b >= 20, "round stroke too thin: {b} px");
        assert!(
            (a as i32 - b as i32).abs() <= 2 && (c as i32 - b as i32).abs() <= 2,
            "round width wanders: {a}/{b}/{c}"
        );
    }

    #[test]
    fn round_preset_keeps_a_crisp_edge() {
        let doc = paint_straight("Round", 200, 60);
        let partial = partial_rows(&doc, 100);
        assert!(partial <= 4, "round rim is soft: {partial} px");
    }

    #[test]
    fn marker_preset_lays_a_hard_chisel_band() {
        let doc = paint_straight("Marker", 240, 80);
        let width = (0..80).filter(|&y| alpha_at(&doc, 120, y) > 0).count();
        assert!((14..=24).contains(&width), "marker width {width} px");
        let core = alpha_at(&doc, 120, 40);
        assert!(
            (150..=200).contains(&core),
            "marker ink should stay translucent, core alpha {core}"
        );
        // A hard nib keeps full ink at 80% of its half-width; a soft one has
        // already faded there. The chisel's top/bottom are a long ellipse
        // tangent, so the nib is judged across its short axis, not by a
        // count of rim pixels.
        let near_rim = alpha_at(&doc, 120, 48);
        assert!(
            near_rim as u32 * 10 >= core as u32 * 9,
            "marker nib is soft: {near_rim} at 80% radius, core {core}"
        );
    }

    #[test]
    fn chalk_preset_keeps_a_continuous_dry_core() {
        let doc = paint_straight("Chalk", 260, 80);
        let holes: Vec<u32> = (30..230).filter(|&x| alpha_at(&doc, x, 40) < 110).collect();
        assert!(holes.is_empty(), "chalk core breaks at {holes:?}");
        let row: Vec<u8> = (30..230).map(|x| alpha_at(&doc, x, 40)).collect();
        let (min, max) = (*row.iter().min().unwrap(), *row.iter().max().unwrap());
        assert!(max - min >= 20, "chalk lost its grain: {min}..{max}");
    }

    #[test]
    fn grain_cell_shrinks_with_the_dab() {
        assert_eq!(grain_cell(22.0, 1.3), 4);
        assert_eq!(grain_cell(12.0, 1.3), 4);
        assert_eq!(grain_cell(6.0, 1.3), 2);
        assert_eq!(grain_cell(3.0, 1.3), 1);
        for size in 1..=64u32 {
            for cell in 1..=12u32 {
                let got = grain_cell(size as f32, cell as f32 / 3.0);
                assert!(got <= cell, "size {size} cell {cell}: grew to {got}");
                assert!(
                    got * 3 <= size.max(3),
                    "size {size} cell {cell}: {got} is wider than a third of the dab"
                );
            }
        }
    }

    #[test]
    fn chalk_small_size_grain_is_finer_than_the_dab() {
        let mut brush = preset_brush("Chalk");
        brush.size = 6.0;
        let doc = paint_straight_brush(&brush, 200, 40);
        let sum = |y: u32| -> u32 { (20..180).map(|x| alpha_at(&doc, x, y) as u32).sum() };
        let core_y = (0..doc.h).max_by_key(|&y| sum(y)).unwrap();
        let row: Vec<u8> = (20..180).map(|x| alpha_at(&doc, x, core_y)).collect();
        assert!(row.iter().all(|&a| a >= 100), "chalk 6 px core breaks: {row:?}");
        let steps = row.windows(2).filter(|p| p[0].abs_diff(p[1]) >= 40).count();
        assert!(
            steps >= 25,
            "chalk 6 px grain too coarse: {steps} steps in {row:?}"
        );
    }

    #[test]
    fn airbrush_preset_is_a_smooth_even_haze() {
        let doc = paint_straight("Airbrush", 260, 120);
        let row = |y: u32| -> Vec<i32> {
            (40..220).map(|x| alpha_at(&doc, x, y) as i32).collect()
        };
        let center = row(60);
        let (min, max) = (*center.iter().min().unwrap(), *center.iter().max().unwrap());
        assert!(max - min <= 6, "airbrush speckles {min}..{max}");
        assert!((60..=100).contains(&max), "airbrush core alpha {max}");
        let off = *row(75).iter().max().unwrap();
        assert!(off < max, "airbrush has no falloff: {max} on axis, {off} off");
    }
}
