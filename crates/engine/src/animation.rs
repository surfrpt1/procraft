//! Animation Assist: layers as frames, onion skins, animated GIF export.
//!
//! Original clean-room code. The model is deliberately Procreate-like and
//! simple: every layer is one frame, bottom-to-top is first-to-last.
//! Hidden layers are skipped by onion skins and GIF export (but stay in
//! the frame count so unhiding never renumbers). Playback state itself
//! lives app-side; this module is pure document math plus the exporter.

use super::document::Layer;
use super::document::{BlendMode, Document};
use image::{
    codecs::gif::{GifEncoder, Repeat},
    Delay, Frame, RgbaImage,
};

/// Playback/export rate bounds (fps).
pub const MIN_ANIM_FPS: u32 = 1;
pub const MAX_ANIM_FPS: u32 = 60;
/// Frame-hold bounds: each frame shows for `hold` ticks (uniform, Procreate
/// "hold"-like pacing without per-frame bookkeeping).
pub const MIN_ANIM_HOLD: u32 = 1;
pub const MAX_ANIM_HOLD: u32 = 8;
/// Sanity cap on exported frames (layer counts never reach it in practice).
pub const MAX_GIF_FRAMES: usize = 256;
/// M41 Time-lapse: frames staged per session (ring-evicted past this).
pub const MAX_LAPSE_FRAMES: usize = 180;
/// M41 Time-lapse: staged snapshots downscale to this longest side.
pub const LAPSE_SIDE: u32 = 320;

/// Playback loop direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopMode {
    #[default]
    Forward,
    PingPong,
}

impl LoopMode {
    pub const ALL: [LoopMode; 2] = [LoopMode::Forward, LoopMode::PingPong];

    pub fn name(self) -> &'static str {
        match self {
            LoopMode::Forward => "Forward",
            LoopMode::PingPong => "Ping-pong",
        }
    }
}

/// Visible layer indices in stack order (the frame sequence): hidden
/// layers are skipped by playback, onion skins and GIF export alike.
pub fn visible_frames(doc: &Document) -> Vec<usize> {
    doc.layers
        .iter()
        .enumerate()
        .filter(|(i, _)| doc.is_layer_shown(*i))
        .map(|(i, _)| i)
        .collect()
}

/// Playback tick over visible frames only, so what plays matches what the
/// GIF exporter writes. A hidden or deleted current frame (manually
/// selected mid-playback) joins the sequence in the travel direction;
/// fully-hidden documents hold their position instead of panicking.
pub fn step_playback_visible(
    doc: &Document,
    pos: usize,
    fwd: bool,
    mode: LoopMode,
) -> (usize, bool) {
    let vis = visible_frames(doc);
    if vis.is_empty() {
        return (pos, true);
    }
    if vis.len() == 1 {
        return (vis[0], true);
    }
    if let Some(k) = vis.iter().position(|&i| i == pos) {
        return match mode {
            LoopMode::Forward => (vis[(k + 1) % vis.len()], true),
            LoopMode::PingPong => {
                if fwd {
                    if k + 1 < vis.len() {
                        (vis[k + 1], true)
                    } else {
                        (vis[k - 1], false)
                    }
                } else if k > 0 {
                    (vis[k - 1], false)
                } else {
                    (vis[k + 1], true)
                }
            }
        };
    }
    if fwd {
        (
            vis.iter().copied().find(|&i| i > pos).unwrap_or(vis[0]),
            true,
        )
    } else {
        (
            vis.iter()
                .rev()
                .copied()
                .find(|&i| i < pos)
                .unwrap_or(vis[vis.len() - 1]),
            false,
        )
    }
}
/// Advance one playback tick over `len` frames from `pos`: returns the next
/// position plus the outgoing travel direction (matters only for
/// ping-pong). Empty stacks and racing deletes (out-of-range `pos`) reset
/// to frame 0 going forward instead of panicking.
pub fn step_playback(len: usize, pos: usize, fwd: bool, mode: LoopMode) -> (usize, bool) {
    if len == 0 || pos >= len {
        return (0, true);
    }
    match mode {
        LoopMode::Forward => ((pos + 1) % len, true),
        LoopMode::PingPong => {
            if len == 1 {
                return (0, true);
            }
            if fwd {
                if pos + 1 < len {
                    (pos + 1, true)
                } else {
                    (pos - 1, false)
                }
            } else if pos > 0 {
                (pos - 1, false)
            } else {
                (pos + 1, true)
            }
        }
    }
}

/// Clamp a frame position into the layer stack (`None` when empty, which
/// documents never are, but importers must not be trusted).
pub fn clamp_frame(doc: &Document, pos: usize) -> Option<usize> {
    if doc.layers.is_empty() {
        None
    } else {
        Some(pos.min(doc.layers.len() - 1))
    }
}

/// Nearest visible layer strictly below `index` (onion: previous frame).
pub fn prev_frame(doc: &Document, index: usize) -> Option<usize> {
    (0..index.min(doc.layers.len()))
        .rev()
        .find(|&i| doc.is_layer_shown(i))
}

/// Nearest visible layer strictly above `index` (onion: next frame).
pub fn next_frame(doc: &Document, index: usize) -> Option<usize> {
    ((index + 1)..doc.layers.len()).find(|&i| doc.is_layer_shown(i))
}

/// One pixel of a single-frame raster: the page background with the
/// frame's layer composited over it (Normal blend, opacity + group
/// opacity + reveal mask, Move offset honored, clipping intentionally
/// ignored — a cel stands alone).
fn frame_pixel(doc: &Document, src: Option<(&Layer, f32)>, x: u32, y: u32) -> [u8; 4] {
    let mut out = doc.background;
    let Some((layer, op)) = src else {
        return out;
    };
    // Buffer content sits at the layer offset (Move tool).
    let (lx, ly) = (x as i32 - layer.ox, y as i32 - layer.oy);
    if lx < 0 || ly < 0 || lx >= doc.w as i32 || ly >= doc.h as i32 {
        return out;
    }
    let o = ((ly as u32 * doc.w + lx as u32) * 4) as usize;
    if o + 4 > layer.pixels.len() {
        return out;
    }
    let px = layer.pixels.get4(o);
    let mut a = px[3] as f32 / 255.0 * op;
    if let Some(m) = layer.mask.as_ref() {
        a *= m.get(o / 4) as f32 / 255.0;
    }
    if a <= 0.0 {
        return out;
    }
    for (dst, src) in out[..3].iter_mut().zip(&px[..3]) {
        *dst = (*src as f32 * a + *dst as f32 * (1.0 - a)).round() as u8;
    }
    out[3] = 255;
    out
}

/// Render one frame: the layer (Normal blend, opacity + group opacity +
/// reveal mask applied, clipping intentionally ignored — a cel stands
/// alone) over the page background. Opaque RGBA out. `index` out of range renders the bare
/// background instead of failing, so playback never panics on a racing
/// layer delete.
pub fn rasterize_frame(doc: &Document, index: usize) -> Vec<u8> {
    let layer = doc.layers.get(index);
    let src = layer.map(|l| (l, l.opacity.clamp(0.0, 1.0) * doc.group_opacity(index)));
    let mut out = vec![0u8; (doc.w * doc.h) as usize * 4];
    for y in 0..doc.h {
        for x in 0..doc.w {
            let d = ((y * doc.w + x) * 4) as usize;
            out[d..d + 4].copy_from_slice(&frame_pixel(doc, src, x, y));
        }
    }
    out
}

/// Render an inclusive canvas region of one frame (clamped), row-major
/// RGBA — the assist-mode equivalent of `flatten_region` for partial
/// texture uploads while painting the current frame. Only the requested
/// pixels are rendered: painting ticks are O(region), never O(canvas).
pub fn rasterize_frame_region(
    doc: &Document,
    index: usize,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> Vec<u8> {
    let x1 = x1.min(doc.w.saturating_sub(1));
    let y1 = y1.min(doc.h.saturating_sub(1));
    let (x0, y0) = (x0.min(x1), y0.min(y1));
    let layer = doc.layers.get(index);
    let src = layer.map(|l| (l, l.opacity.clamp(0.0, 1.0) * doc.group_opacity(index)));
    let mut out = Vec::with_capacity(((x1 - x0 + 1) * (y1 - y0 + 1) * 4) as usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            out.extend_from_slice(&frame_pixel(doc, src, x, y));
        }
    }
    out
}

/// Onion ghost of one frame: layer RGB with alpha scaled by `strength`
/// (0.0 = invisible), no background — the app tints prev/next and draws
/// these over the current frame. Out-of-range renders transparent.
pub fn onion_ghost(doc: &Document, index: usize, strength: f32) -> Vec<u8> {
    let n = (doc.w * doc.h) as usize;
    let mut out = vec![0u8; n * 4];
    let strength = strength.clamp(0.0, 1.0);
    if strength <= 0.0 {
        return out;
    }
    let Some(layer) = doc.layers.get(index) else {
        return out;
    };
    let op = layer.opacity.clamp(0.0, 1.0) * doc.group_opacity(index);
    let (ox, oy) = (layer.ox, layer.oy);
    for y in 0..doc.h {
        for x in 0..doc.w {
            // Buffer content sits at the layer offset (Move tool).
            let (lx, ly) = (x as i32 - ox, y as i32 - oy);
            if lx < 0 || ly < 0 || lx >= doc.w as i32 || ly >= doc.h as i32 {
                continue;
            }
            let o = ((ly as u32 * doc.w + lx as u32) * 4) as usize;
            if o + 4 > layer.pixels.len() {
                continue;
            }
            let px = layer.pixels.get4(o);
            let mut a = px[3] as f32 / 255.0 * op * strength;
            if let Some(m) = layer.mask.as_ref() {
                a *= m.get(o / 4) as f32 / 255.0;
            }
            if a <= 0.0 {
                continue;
            }
            let d = ((y * doc.w + x) * 4) as usize;
            out[d] = px[0];
            out[d + 1] = px[1];
            out[d + 2] = px[2];
            out[d + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Export visible layers as a looping animated GIF at `fps`. Hidden layers
/// are skipped (documented in the app status line). Errors on bad rates,
/// fully-hidden documents, and absurd frame counts.
pub fn export_gif(doc: &Document, fps: u32) -> Result<Vec<u8>, String> {
    export_gif_ext(doc, fps, LoopMode::Forward)
}

/// Decode an animated GIF into a document: one layer per frame, bottom to
/// top in file order, active on frame 1. Returns the document plus the
/// first frame's rate (clamped, 12 fps fallback) so playback matches the
/// source. Each frame's delay becomes its hold (ticks at that rate, so
/// timing survives the round trip). Caps dimensions, frame count and input
/// size; single-image GIFs import as one layer.
pub fn import_gif_document(bytes: &[u8]) -> Result<(Document, u32), String> {
    use image::{codecs::gif::GifDecoder, AnimationDecoder, ImageDecoder};
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("gif too large (max 64MB)".to_string());
    }
    let dec = GifDecoder::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let (w, h) = dec.dimensions();
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err("gif has bad dimensions".to_string());
    }
    // Buffer frames first: the rate comes from the first delay, but every
    // frame keeps its own hold.
    let mut buffered: Vec<(image::RgbaImage, u32)> = Vec::new();
    for frame in dec.into_frames() {
        let frame = frame.map_err(|e| e.to_string())?;
        let (n, d) = frame.delay().numer_denom_ms();
        let ms = if n > 0 && d > 0 {
            n as f32 / d as f32
        } else {
            1000.0 / 12.0
        };
        let img = frame.into_buffer();
        if img.width() != w || img.height() != h {
            return Err("gif has ragged frames".to_string());
        }
        buffered.push((img, ms.round() as u32));
        if buffered.len() > MAX_GIF_FRAMES {
            return Err(format!("too many frames (max {MAX_GIF_FRAMES})"));
        }
    }
    if buffered.is_empty() {
        return Err("gif has no frames".to_string());
    }
    // Missing delays defaulted to a 12 fps tick above, so the first frame
    // always yields a sane rate.
    let fps = ((1000.0 / buffered[0].1 as f32).round() as u32).clamp(MIN_ANIM_FPS, MAX_ANIM_FPS);
    let mut layers = Vec::new();
    for (img, ms) in buffered {
        let hold = ((ms as f32 * fps as f32) / 1000.0).round() as u32;
        layers.push(Layer {
            name: format!("Frame {}", layers.len() + 1),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            pixels: crate::Raster::from_dense(w, h, 4, &img.into_raw()),
            mask: None,
            clip: false,
            ox: 0,
            oy: 0,
            hold: hold.clamp(MIN_ANIM_HOLD, MAX_ANIM_HOLD),
            group: None,
            text: None,
        });
    }
    Ok((Document::from_layers(w, h, layers, 0), fps))
}

/// [`export_gif`] plus loop mode: ping-pong appends the interior frames in
/// reverse (no duplicated endpoints). Every frame's delay is its own hold
/// (ticks at `fps`), so per-frame timing reaches the file.
pub fn export_gif_ext(doc: &Document, fps: u32, mode: LoopMode) -> Result<Vec<u8>, String> {
    if !(MIN_ANIM_FPS..=MAX_ANIM_FPS).contains(&fps) {
        return Err(format!("bad animation rate: {fps}"));
    }
    if doc.w == 0 || doc.h == 0 {
        return Err("nothing to animate: empty canvas".to_string());
    }
    let frames: Vec<usize> = doc
        .layers
        .iter()
        .enumerate()
        .filter(|(i, _)| doc.is_layer_shown(*i))
        .map(|(i, _)| i)
        .collect();
    if frames.is_empty() {
        return Err("nothing to animate: every layer is hidden".to_string());
    }
    if frames.len() > MAX_GIF_FRAMES {
        return Err(format!("too many frames (max {MAX_GIF_FRAMES})"));
    }
    let mut seq = frames;
    if mode == LoopMode::PingPong && seq.len() > 2 {
        let interior: Vec<usize> = seq[1..seq.len() - 1].iter().rev().copied().collect();
        seq.extend(interior);
    }
    let mut buf = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut buf);
        enc.set_repeat(Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        for idx in seq {
            let px = rasterize_frame(doc, idx);
            let img = RgbaImage::from_raw(doc.w, doc.h, px).ok_or("frame raster failed")?;
            let hold = doc.layers[idx].hold.clamp(MIN_ANIM_HOLD, MAX_ANIM_HOLD);
            let delay = Delay::from_numer_denom_ms(1000 * hold, fps);
            let frame = Frame::from_parts(img, 0, 0, delay);
            enc.encode_frame(frame).map_err(|e| e.to_string())?;
        }
    }
    Ok(buf)
}

/// M41 Time-lapse export: flat RGBA snapshots (staged app-side while
/// recording) as a looping GIF at `fps`. Frames are resampled to the
/// first frame's dims, so a mid-session canvas resize can't rag the
/// file. Empty input and bad rates fail honestly.
pub fn export_timelapse(frames: &[(Vec<u8>, u32, u32)], fps: u32) -> Result<Vec<u8>, String> {
    if !(MIN_ANIM_FPS..=MAX_ANIM_FPS).contains(&fps) {
        return Err(format!("bad time-lapse rate: {fps}"));
    }
    if frames.is_empty() {
        return Err("no time-lapse frames staged".to_string());
    }
    if frames.len() > MAX_GIF_FRAMES {
        return Err(format!("too many frames (max {MAX_GIF_FRAMES})"));
    }
    let (w0, h0) = (frames[0].1, frames[0].2);
    if w0 == 0 || h0 == 0 || w0 > 4096 || h0 > 4096 {
        return Err("time-lapse has bad dimensions".to_string());
    }
    let mut buf = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut buf);
        enc.set_repeat(Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        for (bytes, w, h) in frames {
            if bytes.len() != (*w as usize) * (*h as usize) * 4 {
                return Err("time-lapse frame is corrupt".to_string());
            }
            let img = RgbaImage::from_raw(*w, *h, bytes.clone()).ok_or("frame raster failed")?;
            let img = if (*w, *h) != (w0, h0) {
                image::imageops::resize(&img, w0, h0, image::imageops::FilterType::Triangle)
            } else {
                img
            };
            let delay = Delay::from_numer_denom_ms(1000, fps);
            let frame = Frame::from_parts(img, 0, 0, delay);
            enc.encode_frame(frame).map_err(|e| e.to_string())?;
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::super::brush::{Brush, Color};
    use super::*;

    fn paint_layer(doc: &mut Document, idx: usize, rgb: [u8; 3]) {
        let brush = Brush {
            size: 64.0,
            softness: 0.0,
            ..Default::default()
        };
        doc.active = idx;
        doc.fill_rect(&brush, Color::rgb(rgb[0], rgb[1], rgb[2]), 0, 0, 15, 15);
    }

    #[test]
    fn frame_raster_composites_layer_over_background() {
        let mut doc = Document::new(16, 16);
        paint_layer(&mut doc, 0, [255, 0, 0]);
        let px = rasterize_frame(&doc, 0);
        assert_eq!(&px[0..3], &[255, 0, 0]);
        // Transparent pixels show the page.
        let mut doc2 = Document::new(16, 16);
        doc2.background = [10, 20, 30, 255];
        let px2 = rasterize_frame(&doc2, 0);
        assert_eq!(&px2[0..4], &[10, 20, 30, 255]);
        // Half opacity blends with the page.
        let mut doc3 = Document::new(16, 16);
        paint_layer(&mut doc3, 0, [255, 0, 0]);
        doc3.layers[0].opacity = 0.5;
        let px3 = rasterize_frame(&doc3, 0);
        assert_eq!(&px3[0..4], &[255, 128, 128, 255]);
        // Out-of-range index renders background, never panics.
        assert_eq!(&rasterize_frame(&doc3, 99)[0..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn frame_region_matches_full_raster() {
        let mut doc = Document::new(32, 32);
        paint_layer(&mut doc, 0, [0, 0, 255]);
        let full = rasterize_frame(&doc, 0);
        let reg = rasterize_frame_region(&doc, 0, 4, 6, 11, 13);
        assert_eq!(reg.len(), 8 * 8 * 4);
        for (ry, y) in (6..=13).enumerate() {
            for (rx, x) in (4..=11).enumerate() {
                let ro = (ry * 8 + rx) * 4;
                let fo = ((y * 32 + x) * 4) as usize;
                assert_eq!(&reg[ro..ro + 4], &full[fo..fo + 4]);
            }
        }
    }

    #[test]
    fn frame_region_matches_full_raster_with_offset_and_opacity() {
        let mut doc = Document::new(24, 24);
        doc.background = [10, 20, 30, 255];
        paint_layer(&mut doc, 0, [200, 40, 40]);
        doc.layers[0].opacity = 0.5;
        doc.translate_layer(0, 3, -2);
        let full = rasterize_frame(&doc, 0);
        let reg = rasterize_frame_region(&doc, 0, 0, 0, 23, 23);
        assert_eq!(reg, full);
        // Out-of-range region edges clamp to the canvas.
        let edge = rasterize_frame_region(&doc, 0, 20, 20, 40, 40);
        assert_eq!(edge.len(), 4 * 4 * 4);
        for (i, px) in edge.as_chunks::<4>().0.iter().enumerate() {
            let o = ((i / 4 + 20) * 24 + i % 4 + 20) * 4;
            assert_eq!(px.as_slice(), &full[o..o + 4]);
        }
        // Out-of-range frame index: bare background, matching full raster.
        let bare = rasterize_frame_region(&doc, 99, 5, 5, 6, 6);
        assert_eq!(bare, [10, 20, 30, 255].repeat(4));
    }

    #[test]
    fn frames_and_ghosts_follow_layer_offset() {
        let mut doc = Document::new(16, 16);
        paint_layer(&mut doc, 0, [255, 0, 0]);
        // Shift right 4: canvas 0-3 show page, canvas 4 shows the cel.
        doc.translate_layer(0, 4, 0);
        let px = rasterize_frame(&doc, 0);
        assert_eq!(&px[0..4], &[255, 255, 255, 255]);
        assert_eq!(&px[16..19], &[255, 0, 0]);
        let g = onion_ghost(&doc, 0, 1.0);
        assert_eq!(g[3], 0);
        assert_eq!(&g[16..19], &[255, 0, 0]);
        assert_eq!(g[19], 255);
    }

    #[test]
    fn onion_ghost_scales_alpha_keeps_rgb() {
        let mut doc = Document::new(16, 16);
        paint_layer(&mut doc, 0, [10, 200, 30]);
        let full = onion_ghost(&doc, 0, 1.0);
        assert_eq!(&full[0..3], &[10, 200, 30]);
        assert_eq!(full[3], 255);
        let half = onion_ghost(&doc, 0, 0.5);
        assert_eq!(&half[0..3], &[10, 200, 30]);
        assert!((half[3] as i32 - 128).abs() <= 1, "{}", half[3]);
        assert!(onion_ghost(&doc, 0, 0.0).iter().all(|&b| b == 0));
        assert!(onion_ghost(&doc, 7, 1.0).iter().all(|&b| b == 0));
    }

    #[test]
    fn neighbor_lookup_skips_hidden() {
        let mut doc = Document::new(8, 8);
        doc.add_layer("L2".to_string());
        doc.add_layer("L3".to_string());
        doc.layers[1].visible = false;
        assert_eq!(prev_frame(&doc, 2), Some(0));
        assert_eq!(next_frame(&doc, 0), Some(2));
        assert_eq!(prev_frame(&doc, 0), None);
        assert_eq!(next_frame(&doc, 2), None);
        assert_eq!(clamp_frame(&doc, 99), Some(2));
    }

    #[test]
    fn gif_roundtrips_visible_frames() {
        use image::{codecs::gif::GifDecoder, AnimationDecoder};
        let mut doc = Document::new(16, 16);
        paint_layer(&mut doc, 0, [255, 0, 0]);
        doc.add_layer("F2".to_string());
        paint_layer(&mut doc, 1, [0, 255, 0]);
        doc.add_layer("Hidden".to_string());
        doc.layers[2].visible = false;
        let bytes = export_gif(&doc, 12).unwrap();
        assert_eq!(&bytes[0..6], b"GIF89a");
        let dec = GifDecoder::new(std::io::Cursor::new(&bytes[..])).unwrap();
        let frames: Vec<_> = dec.into_frames().collect::<Result<_, _>>().unwrap();
        // Hidden layer skipped: 2 frames.
        assert_eq!(frames.len(), 2);
        assert_eq!(
            (frames[0].buffer().width(), frames[0].buffer().height()),
            (16, 16)
        );
        // First frame is the red cel.
        assert_eq!(&frames[0].buffer()[(0, 0)].0[0..3], &[255, 0, 0]);
        assert!(export_gif(&doc, 0).is_err());
        assert!(export_gif(&doc, 61).is_err());
        doc.layers[0].visible = false;
        doc.layers[1].visible = false;
        assert!(export_gif(&doc, 12).is_err());
    }

    #[test]
    fn playback_steps_forward_and_bounces() {
        // Forward wraps.
        assert_eq!(step_playback(4, 3, true, LoopMode::Forward), (0, true));
        assert_eq!(step_playback(4, 1, true, LoopMode::Forward), (2, true));
        // Ping-pong bounces at both ends.
        let mut pos = 0;
        let mut fwd = true;
        let mut seq = vec![pos];
        for _ in 0..8 {
            (pos, fwd) = step_playback(3, pos, fwd, LoopMode::PingPong);
            seq.push(pos);
        }
        assert_eq!(seq, vec![0, 1, 2, 1, 0, 1, 2, 1, 0]);
        // Two frames alternate; one frame sits still.
        assert_eq!(step_playback(2, 0, true, LoopMode::PingPong), (1, true));
        assert_eq!(step_playback(2, 1, true, LoopMode::PingPong), (0, false));
        assert_eq!(step_playback(2, 0, false, LoopMode::PingPong), (1, true));
        assert_eq!(step_playback(1, 0, false, LoopMode::PingPong), (0, true));
        // Empty stacks and racing deletes reset instead of panicking.
        assert_eq!(step_playback(0, 0, true, LoopMode::Forward), (0, true));
        assert_eq!(step_playback(3, 9, false, LoopMode::PingPong), (0, true));
    }

    #[test]
    fn gif_ping_pong_reverses_the_interior() {
        use image::{codecs::gif::GifDecoder, AnimationDecoder};
        let mut doc = Document::new(16, 16);
        paint_layer(&mut doc, 0, [255, 0, 0]);
        doc.add_layer("F2".to_string());
        paint_layer(&mut doc, 1, [0, 255, 0]);
        doc.add_layer("F3".to_string());
        paint_layer(&mut doc, 2, [0, 0, 255]);
        let bytes = export_gif_ext(&doc, 12, LoopMode::PingPong).unwrap();
        let dec = GifDecoder::new(std::io::Cursor::new(&bytes[..])).unwrap();
        let frames: Vec<_> = dec.into_frames().collect::<Result<_, _>>().unwrap();
        // 0, 1, 2, 1: endpoints never duplicate.
        assert_eq!(frames.len(), 4);
        assert_eq!(&frames[0].buffer()[(0, 0)].0[0..3], &[255, 0, 0]);
        assert_eq!(&frames[2].buffer()[(0, 0)].0[0..3], &[0, 0, 255]);
        assert_eq!(&frames[3].buffer()[(0, 0)].0[0..3], &[0, 255, 0]);
        // Two frames: nothing to reverse, plain pair.
        doc.layers[2].visible = false;
        let pair = export_gif_ext(&doc, 12, LoopMode::PingPong).unwrap();
        let dec = GifDecoder::new(std::io::Cursor::new(&pair[..])).unwrap();
        let pframes: Vec<_> = dec.into_frames().collect::<Result<_, _>>().unwrap();
        assert_eq!(pframes.len(), 2);
        doc.layers[2].visible = true;
    }

    #[test]
    fn gif_export_uses_per_frame_holds() {
        use image::{codecs::gif::GifDecoder, AnimationDecoder};
        let mut doc = Document::new(16, 16);
        paint_layer(&mut doc, 0, [255, 0, 0]);
        doc.add_layer("F2".to_string());
        paint_layer(&mut doc, 1, [0, 255, 0]);
        doc.layers[0].hold = 1;
        doc.layers[1].hold = 4;
        // 10 fps keeps every delay on a centisecond boundary (100/400 ms).
        let bytes = export_gif_ext(&doc, 10, LoopMode::Forward).unwrap();
        let dec = GifDecoder::new(std::io::Cursor::new(&bytes[..])).unwrap();
        let frames: Vec<_> = dec.into_frames().collect::<Result<_, _>>().unwrap();
        assert_eq!(frames.len(), 2);
        let ms = |f: &image::Frame| {
            let (n, d) = f.delay().numer_denom_ms();
            (n as f32 / d as f32).round() as u32
        };
        assert_eq!(ms(&frames[0]), 100);
        assert_eq!(ms(&frames[1]), 400);
        // Holds ride the GIF round trip back into per-frame holds.
        let (back, fps) = import_gif_document(&bytes).unwrap();
        assert_eq!(fps, 10);
        assert_eq!(back.layers[0].hold, 1);
        assert_eq!(back.layers[1].hold, 4);
    }

    fn three_layers() -> Document {
        let mut doc = Document::new(8, 8);
        doc.add_layer("F2".to_string());
        doc.add_layer("F3".to_string());
        doc
    }

    #[test]
    fn playback_skips_hidden_frames() {
        let mut doc = three_layers();
        assert_eq!(visible_frames(&doc), vec![0, 1, 2]);
        doc.layers[1].visible = false;
        assert_eq!(visible_frames(&doc), vec![0, 2]);
        // Forward cycles the visible pair; ping-pong alternates it.
        assert_eq!(
            step_playback_visible(&doc, 0, true, LoopMode::Forward),
            (2, true)
        );
        assert_eq!(
            step_playback_visible(&doc, 2, true, LoopMode::Forward),
            (0, true)
        );
        assert_eq!(
            step_playback_visible(&doc, 0, true, LoopMode::PingPong),
            (2, true)
        );
        assert_eq!(
            step_playback_visible(&doc, 2, true, LoopMode::PingPong),
            (0, false)
        );
        assert_eq!(
            step_playback_visible(&doc, 0, false, LoopMode::PingPong),
            (2, true)
        );
        // A hidden current frame joins the sequence in travel direction.
        assert_eq!(
            step_playback_visible(&doc, 1, true, LoopMode::Forward),
            (2, true)
        );
        assert_eq!(
            step_playback_visible(&doc, 1, false, LoopMode::PingPong),
            (0, false)
        );
        // One visible frame sits still; none holds position.
        doc.layers[0].visible = false;
        assert_eq!(
            step_playback_visible(&doc, 2, true, LoopMode::PingPong),
            (2, true)
        );
        doc.layers[2].visible = false;
        assert_eq!(
            step_playback_visible(&doc, 1, true, LoopMode::Forward),
            (1, true)
        );
    }

    #[test]
    fn gif_import_roundtrips_frames() {
        let mut doc = Document::new(16, 16);
        paint_layer(&mut doc, 0, [255, 0, 0]);
        doc.add_layer("F2".to_string());
        paint_layer(&mut doc, 1, [0, 0, 255]);
        let bytes = export_gif(&doc, 10).unwrap();
        let (back, fps) = import_gif_document(&bytes).unwrap();
        assert_eq!((back.w, back.h), (16, 16));
        assert_eq!(back.layers.len(), 2);
        assert_eq!(back.active, 0);
        assert_eq!(back.layers[0].name, "Frame 1");
        // First pixels survive the round trip (GIF palette-quantized).
        let at = |d: &Document, i: usize| {
            [
                d.layers[i].pixels.get(0),
                d.layers[i].pixels.get(1),
                d.layers[i].pixels.get(2),
            ]
        };
        assert_eq!(at(&back, 0), [255, 0, 0]);
        assert_eq!(at(&back, 1), [0, 0, 255]);
        assert_eq!(fps, 10);
        assert!(import_gif_document(b"GIF89a nope").is_err());
        assert!(import_gif_document(&[]).is_err());
    }

    #[test]
    fn timelapse_exports_staged_frames() {
        let solid = |rgb: [u8; 3]| {
            let mut v = Vec::with_capacity(8 * 8 * 4);
            for _ in 0..64 {
                v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            (v, 8u32, 8u32)
        };
        let frames = vec![solid([255, 0, 0]), solid([0, 255, 0]), solid([0, 0, 255])];
        let bytes = export_timelapse(&frames, 10).unwrap();
        assert!(bytes.starts_with(b"GIF89a"));
        assert!(bytes.len() > 100);
        // Mixed dims resample to the first frame (no ragged export).
        let odd = vec![solid([255, 0, 0]), (vec![9u8; 4 * 4 * 4], 4u32, 4u32)];
        assert!(export_timelapse(&odd, 10).is_ok());
        assert!(export_timelapse(&[], 10).is_err());
        assert!(export_timelapse(&frames, 0).is_err());
        assert!(export_timelapse(&frames, 61).is_err());
        assert!(export_timelapse(&[(vec![1u8; 5], 8u32, 8u32)], 10).is_err());
    }
}
