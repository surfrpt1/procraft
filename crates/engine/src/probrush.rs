//! Third-party brush import: Photoshop `.abr`, Procreate `.brush`/`.brushset`,
//! Clip Studio Paint `.sut`, Krita `.kpp`/`.bundle`, GIMP `.gbr` and plain tip
//! images. All parsers are
//! original code written from public format notes; anything unexpected is a
//! clean error, never silent garbage. Every format degrades to the same two
//! things ProCraft understands: parametric settings and a grayscale stamp tip.

use super::brush::{Brush, BrushTip, Taper, MAX_LIBRARY_BRUSHES, MAX_TIP_SIDE};
use crate::brush::open_brush_library;
use std::collections::HashSet;

mod brush_archive;
mod bundled;
mod csp;

/// One imported brush: display name plus ready-to-paint settings.
#[derive(Clone, Debug)]
pub struct ImportedBrush {
    pub name: String,
    pub brush: Brush,
}

/// Result of importing a file: usable brushes plus anything that was skipped.
#[derive(Clone, Debug, Default)]
pub struct ImportOutcome {
    pub brushes: Vec<ImportedBrush>,
    pub warnings: Vec<String>,
    pub skipped: usize,
}

/// Hard caps: files above this are rejected before parsing.
const MAX_FILE_BYTES: usize = 200 * 1024 * 1024;
const MAX_ZIP_UNPACKED: u64 = 200 * 1024 * 1024;

/// Third-party brushes start at the scale of ProCraft's largest built-in
/// default (64 px). A batch is scaled uniformly so its relative size
/// hierarchy survives. ProCraft's own `.pcbrush` files keep their authored
/// sizes instead, so re-exporting a brush never changes it.
const IMPORTED_BRUSH_DEFAULT_MAX: f32 = 64.0;

fn read_capped(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES as u64 {
        return Err("brush file is too large".to_string());
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

fn file_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Imported brush".to_string())
}

fn clean_name(raw: &str, fallback: &str) -> String {
    let t = raw.trim().replace(['\r', '\n', '\t'], " ");
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return fallback.to_string();
    }
    let mut short: String = t.chars().take(64).collect();
    if short.trim().is_empty() {
        short = fallback.to_string();
    }
    short
}

fn clamp_spacing(v: f32, fallback: f32) -> f32 {
    if v.is_finite() && v > 0.0 && v <= 2.0 {
        v
    } else {
        fallback
    }
}

/// Downscale a mask to fit [`MAX_TIP_SIDE`], nearest-neighbor.
fn fit_tip(w: u32, h: u32, mask: Vec<u8>) -> (u32, u32, Vec<u8>) {
    let big = w.max(h);
    if big <= MAX_TIP_SIDE {
        return (w, h, mask);
    }
    let nw = ((w as u64 * MAX_TIP_SIDE as u64) / big as u64).max(1) as u32;
    let nh = ((h as u64 * MAX_TIP_SIDE as u64) / big as u64).max(1) as u32;
    let mut out = vec![0u8; (nw * nh) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let sx = ((x as u64 * w as u64) / nw as u64).min(w as u64 - 1) as usize;
            let sy = ((y as u64 * h as u64) / nh as u64).min(h as u64 - 1) as usize;
            out[(y * nw + x) as usize] = mask[sy * w as usize + sx];
        }
    }
    (nw, nh, out)
}

/// Trim fully transparent rows and columns around a mask.
fn trim_mask(w: u32, h: u32, mask: Vec<u8>) -> Result<(u32, u32, Vec<u8>), String> {
    if w == 0 || h == 0 || mask.len() != (w * h) as usize {
        return Err("brush tip has corrupt pixels".to_string());
    }
    let (mut min_x, mut min_y) = (w, h);
    let (mut max_x, mut max_y) = (0, 0);
    for y in 0..h {
        for x in 0..w {
            if mask[(y * w + x) as usize] > 0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x > max_x || min_y > max_y {
        return Err("tip image is empty".to_string());
    }
    let (nw, nh) = (max_x - min_x + 1, max_y - min_y + 1);
    let mut out = vec![0u8; (nw * nh) as usize];
    for y in 0..nh {
        let src = ((min_y + y) * w + min_x) as usize;
        let dst = (y * nw) as usize;
        out[dst..dst + nw as usize].copy_from_slice(&mask[src..src + nw as usize]);
    }
    Ok((nw, nh, out))
}

/// Build the stamp brush every importer converges on.
const DEFAULT_PROCREATE_TIP_SIDE: u32 = 256;

fn procedural_round_tip(side: u32) -> (u32, u32, Vec<u8>) {
    let side = side.max(2);
    let radius = side as f32 * 0.5;
    let feather = 1.5 / radius;
    let mut mask = Vec::with_capacity((side * side) as usize);
    for y in 0..side {
        for x in 0..side {
            let dx = x as f32 + 0.5 - radius;
            let dy = y as f32 + 0.5 - radius;
            let d = (dx * dx + dy * dy).sqrt() / radius;
            let alpha = ((1.0 - d) / feather).clamp(0.0, 1.0);
            mask.push((alpha * 255.0).round() as u8);
        }
    }
    (side, side, mask)
}

fn stamp_brush(
    name: String,
    w: u32,
    h: u32,
    mask: Vec<u8>,
    spacing: f32,
) -> Result<ImportedBrush, String> {
    let (w, h, mask) = fit_tip(w, h, mask);
    let tip = BrushTip::from_mask(w, h, mask)?;
    Ok(ImportedBrush {
        name,
        brush: Brush {
            size: w as f32,
            opacity: 1.0,
            softness: 0.0,
            spacing: clamp_spacing(spacing, 0.2),
            eraser: false,
            tip: Some(tip),
            ..Brush::default()
        },
    })
}

fn dedupe_within(list: &mut [ImportedBrush]) {
    let mut seen: Vec<String> = Vec::new();
    for b in list.iter_mut() {
        let base = b.name.clone();
        if !seen.contains(&base) {
            seen.push(base);
            continue;
        }
        for n in 2..=MAX_LIBRARY_BRUSHES + 2 {
            let cand = format!("{base} ({n})");
            if !seen.contains(&cand) {
                b.name = cand.clone();
                seen.push(cand);
                break;
            }
        }
    }
}

fn cap_list(mut list: Vec<ImportedBrush>) -> Result<Vec<ImportedBrush>, String> {
    if list.is_empty() {
        return Err("no brushes found in file".to_string());
    }
    dedupe_within(&mut list);
    list.truncate(MAX_LIBRARY_BRUSHES);
    Ok(list)
}

/// Import brushes from any supported file, picked by extension:
/// `.pcbrush`/`.json`, `.abr`, `.brush`/`.brushset`, `.kpp`/`.bundle`,
/// `.sut`, `.gbr`, or tip images (png/jpg/webp/bmp/gif/tiff).
pub fn import_brush_file(path: &std::path::Path) -> Result<Vec<ImportedBrush>, String> {
    import_brush_file_report(path).map(|outcome| outcome.brushes)
}

/// Like [`import_brush_file`], but keeps warnings and skipped-entry counts.
pub fn import_brush_file_report(path: &std::path::Path) -> Result<ImportOutcome, String> {
    let bytes = read_capped(path)?;
    if bytes.is_empty() {
        return Err("brush file is empty".to_string());
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let stem = file_stem(path);
    let mut outcome = match ext.as_str() {
        "pcbrush" | "json" => {
            let libs = open_brush_library(path)?;
            let total = libs.len();
            let brushes = cap_list(
                libs.into_iter()
                    .map(|p| ImportedBrush {
                        name: p.name,
                        brush: p.brush,
                    })
                    .collect(),
            )?;
            let mut warnings = Vec::new();
            let skipped = total.saturating_sub(brushes.len());
            if skipped > 0 {
                warnings.push(format!(
                    "Skipped {skipped} presets after the {MAX_LIBRARY_BRUSHES} brush limit."
                ));
            }
            // ProCraft-authored libraries round-trip exactly.
            return Ok(ImportOutcome {
                brushes,
                warnings,
                skipped,
            });
        }
        "abr" => import_abr_report(&bytes)?,
        "brush" | "brushset" => outcome_from_brushes(import_procreate(&bytes, &stem)?),
        "kpp" => outcome_from_brushes(import_kpp(path, &bytes, &stem)?),
        "bundle" => import_krita_bundle_report(&bytes)?,
        "sut" => outcome_from_brushes(csp::import_sut(&bytes, &stem)?),
        "gbr" => outcome_from_brushes(import_gbr(&bytes, &stem)?),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "tif" | "tiff" => {
            outcome_from_brushes(import_image_tip(&bytes, &stem)?)
        }
        _ => return Err(format!("unsupported brush file type '.{ext}'")),
    };
    normalize_imported_sizes(&mut outcome.brushes);
    Ok(outcome)
}

fn outcome_from_brushes(brushes: Vec<ImportedBrush>) -> ImportOutcome {
    ImportOutcome {
        brushes,
        warnings: Vec::new(),
        skipped: 0,
    }
}

fn normalize_imported_sizes(brushes: &mut [ImportedBrush]) {
    let max = brushes
        .iter()
        .map(|imported| imported.brush.size)
        .fold(0.0_f32, f32::max);
    if !max.is_finite() || max <= IMPORTED_BRUSH_DEFAULT_MAX {
        return;
    }
    let scale = IMPORTED_BRUSH_DEFAULT_MAX / max;
    for imported in brushes {
        imported.brush.size = (imported.brush.size * scale).clamp(1.0, IMPORTED_BRUSH_DEFAULT_MAX);
    }
}

/// Render the current brush as a grayscale tip PNG for other software:
/// imported stamps export as-is, parametric brushes export their soft-round
/// falloff disc. Usable as a tip/shape image in Photoshop, Procreate (as a
/// Shape source), Krita and GIMP.
pub fn export_tip_png(path: &std::path::Path, brush: &Brush) -> Result<(), String> {
    let (w, h, mask) = if let Some(tip) = brush.tip.as_ref() {
        if tip.w == 0 || tip.h == 0 || tip.mask.len() != (tip.w * tip.h) as usize {
            return Err("brush tip is corrupt".to_string());
        }
        (tip.w, tip.h, tip.mask.clone())
    } else {
        let side = brush.size.clamp(8.0, 256.0).round() as u32;
        let radius = side as f32 * 0.5;
        let mut mask = vec![0u8; (side * side) as usize];
        for y in 0..side {
            for x in 0..side {
                let d = ((x as f32 + 0.5 - radius).hypot(y as f32 + 0.5 - radius)) / radius;
                if d > 1.0 {
                    continue;
                }
                let edge = brush.softness.clamp(0.0, 1.0);
                let f = if d <= 1.0 - edge {
                    1.0
                } else {
                    1.0 - (d - (1.0 - edge)) / edge.max(0.001)
                };
                mask[(y * side + x) as usize] = (f.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
        (side, side, mask)
    };
    let img = image::GrayImage::from_raw(w, h, mask).ok_or("tip render failed".to_string())?;
    img.save(path).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Tiny big-endian cursor with strict bounds checks.
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn left(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.left() < n {
            return Err("brush file is truncated".to_string());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u16be(&mut self) -> Result<u16, String> {
        Ok(u16::from_be_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| "bad u16".to_string())?,
        ))
    }
    fn u32be(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| "bad u32".to_string())?,
        ))
    }
    fn i32be(&mut self) -> Result<i32, String> {
        Ok(i32::from_be_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| "bad i32".to_string())?,
        ))
    }
}

// ---------------------------------------------------------------------------
// GIMP .gbr (public spec: big-endian header, "GIMP" magic, spacing, name).
// Grayscale semantics follow GIMP: dark = opaque.
// ---------------------------------------------------------------------------

fn import_gbr(bytes: &[u8], stem: &str) -> Result<Vec<ImportedBrush>, String> {
    let mut c = Cursor::new(bytes);
    let header_size = c.u32be()? as usize;
    if header_size < 20 || header_size > bytes.len() {
        return Err("not a GIMP brush file".to_string());
    }
    let version = c.u32be()?;
    if version != 1 && version != 2 {
        return Err(format!("unsupported GBR version {version}"));
    }
    let w = c.u32be()?;
    let h = c.u32be()?;
    if w == 0 || h == 0 || w > MAX_TIP_SIDE * 2 || h > MAX_TIP_SIDE * 2 {
        return Err("GBR brush has bad dimensions".to_string());
    }
    let depth = c.u32be()?;
    if depth != 1 && depth != 2 && depth != 4 {
        return Err("GBR brush has an unsupported depth".to_string());
    }
    let spacing: f32;

    let name: String = if version == 2 {
        if c.take(4)? != b"GIMP" {
            return Err("not a GIMP brush file".to_string());
        }
        spacing = c.u32be()? as f32 / 100.0;
        let name_len = header_size.saturating_sub(c.pos);
        let raw = c.take(name_len)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        clean_name(
            &raw[..end].iter().map(|&b| b as char).collect::<String>(),
            stem,
        )
    } else {
        spacing = 0.2;
        let name_len = header_size.saturating_sub(c.pos);
        let raw = c.take(name_len)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        clean_name(
            &raw[..end].iter().map(|&b| b as char).collect::<String>(),
            stem,
        )
    };
    // Body starts where the header ends.
    if bytes.len() < header_size {
        return Err("brush file is truncated".to_string());
    }
    let body = &bytes[header_size..];
    let expect = (w as usize) * (h as usize) * (depth as usize);
    if body.len() < expect {
        return Err("brush file is truncated".to_string());
    }
    let mut mask = vec![0u8; (w * h) as usize];
    match depth {
        1 => {
            for (i, m) in mask.iter_mut().enumerate() {
                *m = 255u8.saturating_sub(body[i]);
            }
        }
        2 => {
            for (i, m) in mask.iter_mut().enumerate() {
                let g = body[i * 2];
                let a = body[i * 2 + 1];
                *m = (((255u16.saturating_sub(g as u16)) * a as u16) / 255) as u8;
            }
        }
        _ => {
            for (i, m) in mask.iter_mut().enumerate() {
                *m = body[i * 4 + 3];
            }
        }
    }
    cap_list(vec![stamp_brush(name, w, h, mask, spacing)?])
}

// ---------------------------------------------------------------------------
// Plain tip images (PNG/JPG/WebP/BMP/GIF/TIFF): alpha shape, otherwise
// luminance with polarity inferred from the border.
// ---------------------------------------------------------------------------

fn import_image_tip(bytes: &[u8], stem: &str) -> Result<Vec<ImportedBrush>, String> {
    let img = image::load_from_memory(bytes).map_err(|_| "image is unreadable".to_string())?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err("tip image has bad dimensions".to_string());
    }
    let has_alpha = rgba.pixels().any(|p| p[3] < 250);
    let (mut min_luma, mut max_luma) = (255u32, 0u32);
    let (mut border_sum, mut border_n) = (0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let p = rgba.get_pixel(x, y);
            let luma = (299u32 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32) / 1000;
            min_luma = min_luma.min(luma);
            max_luma = max_luma.max(luma);
            if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                border_sum += luma as u64;
                border_n += 1;
            }
        }
    }
    let light_border = border_n > 0 && border_sum / border_n >= 128;
    let invert = !has_alpha && light_border && max_luma.saturating_sub(min_luma) > 8;
    let mut mask = Vec::with_capacity((w * h) as usize);
    for p in rgba.pixels() {
        if has_alpha {
            mask.push(p[3]);
        } else {
            let luma = (299u32 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32) / 1000;
            mask.push(if invert { 255 - luma as u8 } else { luma as u8 });
        }
    }
    let (w, h, mask) = trim_mask(w, h, mask)?;
    cap_list(vec![stamp_brush(
        clean_name(stem, "Image tip"),
        w,
        h,
        mask,
        0.15,
    )?])
}

fn decode_png_mask(png: &[u8], white_is_ink: bool) -> Result<(u32, u32, Vec<u8>), String> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|_| "embedded PNG is unreadable".to_string())?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err("embedded PNG has bad dimensions".to_string());
    }
    let mut mask = Vec::with_capacity((w * h) as usize);
    for p in rgba.pixels() {
        let luma = (299u32 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32) / 1000;
        let ink = if white_is_ink { luma } else { 255 - luma };
        mask.push(((ink * p[3] as u32) / 255) as u8);
    }
    Ok((w, h, mask))
}

/// Decode a generic image (PNG/JPG/WebP/BMP/GIF/TIFF) into a grayscale mask.
/// `white_is_ink` determines whether white pixels (255) are treated as ink or as transparency.
pub(super) fn decode_image_mask(
    bytes: &[u8],
    white_is_ink: bool,
) -> Result<(u32, u32, Vec<u8>), String> {
    let img = image::load_from_memory(bytes)
        .map_err(|_| "embedded image is unreadable".to_string())?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err("embedded image has bad dimensions".to_string());
    }
    let mut mask = Vec::with_capacity((w * h) as usize);
    for p in img.pixels() {
        let luma = (299u32 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32) / 1000;
        let ink = if white_is_ink { luma } else { 255 - luma };
        mask.push(((ink * p[3] as u32) / 255) as u8);
    }
    Ok((w, h, mask))
}

// ---------------------------------------------------------------------------
// Procreate .brush / .brushset: plain ZIPs; Shape.png is the tip, white ink.
// ---------------------------------------------------------------------------

fn zip_names(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> Vec<String> {
    (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .collect()
}

fn is_procreate_junk(name: &str) -> bool {
    let path = name.replace('\\', "/");
    if path
        .split('/')
        .any(|part| part.eq_ignore_ascii_case("__MACOSX"))
    {
        return true;
    }
    path.rsplit('/')
        .next()
        .map(|file| file.starts_with("._") || file.eq_ignore_ascii_case(".DS_Store"))
        .unwrap_or(false)
}

fn brush_name_from_entry(entry: &str, fallback: &str) -> String {
    let path = entry.replace('\\', "/");
    let mut parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    parts.pop();
    if let Some(dir) = parts.pop() {
        if !dir.starts_with("__MACOSX") && dir != "." {
            return clean_name(dir, fallback);
        }
    }
    clean_name(fallback, "Procreate brush")
}

fn brush_archive_sibling(names: &[String], shape_name: &str, file: &str) -> Option<String> {
    let shape = shape_name.replace('\\', "/");
    let dir = shape.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let want = if dir.is_empty() {
        file.to_ascii_lowercase()
    } else {
        format!("{}/{}", dir.to_ascii_lowercase(), file.to_ascii_lowercase())
    };
    names
        .iter()
        .find(|name| name.replace('\\', "/").to_ascii_lowercase() == want)
        .cloned()
}

fn find_bundle_tip(names: &[String], requested: &str) -> Option<String> {
    let want = requested
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase();
    if want.is_empty() {
        return None;
    }
    let normalized: Vec<(String, &String)> = names
        .iter()
        .map(|name| (name.replace('\\', "/").to_ascii_lowercase(), name))
        .collect();
    if let Some((_, name)) = normalized.iter().find(|(lower, _)| *lower == want) {
        return Some((*name).clone());
    }
    let suffix = format!("/{want}");
    if let Some((_, name)) = normalized
        .iter()
        .find(|(lower, _)| lower.ends_with(&suffix))
    {
        return Some((*name).clone());
    }
    let base = want.rsplit('/').next().unwrap_or(&want);
    let mut matches = normalized
        .iter()
        .filter(|(lower, _)| lower.rsplit('/').next() == Some(base))
        .map(|(_, name)| (*name).clone());
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first)
}

/// Load a bundled Procreate asset that the `.brushset` happens to ship.
///
/// Most sets only name the asset (`bundledGrainPath`), because it lives inside
/// the Procreate app. A few include a copy next to the brush; when the exact
/// file is there it beats any approximation, so decode it instead.
fn read_bundled_asset(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    names: &[String],
    requested: &str,
    white_is_ink: bool,
) -> Option<(u32, u32, Vec<u8>)> {
    let entry = find_bundle_tip(names, requested)?;
    let bytes = read_zip_entry(archive, &entry)?;
    decode_image_mask(&bytes, white_is_ink).ok()
}

fn read_zip_entry(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    name: &str,
) -> Option<Vec<u8>> {
    let mut file = archive.by_name(name).ok()?;
    if file.size() > MAX_ZIP_UNPACKED {
        return None;
    }
    let mut bytes = Vec::new();
    use std::io::Read;
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn brush_archive_spacing(raw: f32) -> Option<f32> {
    if !raw.is_finite() || raw <= 0.0 {
        return None;
    }
    let percent = (72.943 * raw.clamp(0.0, 2.0).powf(0.4558)).round();
    Some((percent / 100.0).clamp(0.01, 2.0))
}

fn brush_archive_grain_scale(raw: f32) -> Option<f32> {
    if !raw.is_finite() || raw < 0.0 {
        return None;
    }
    // Procreate exposes grain scale as round(25 * sqrt(raw)) over 0..=16;
    // map that GUI percentage onto the engine's 0.25 (fine) ..= 4.0 (coarse).
    let gui = 25.0 * raw.clamp(0.0, 16.0).sqrt() / 100.0;
    Some((0.25 + gui * 3.75).clamp(0.25, 4.0))
}

/// Attach a grain texture to a Procreate brush.
///
/// The brush's own `Grain.png` wins. Otherwise the archive may name a
/// Procreate built-in grain (`bundledGrainPath`); those assets ship with the
/// app rather than the brush, so they are reproduced clean-room.
fn attach_procreate_grain(
    imported: &mut ImportedBrush,
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    names: &[String],
    entry: &str,
    settings: Option<&brush_archive::BrushArchiveSettings>,
) {
    if let Some(grain_name) = brush_archive_sibling(names, entry, "Grain.png") {
        if let Some(png) = read_zip_entry(archive, &grain_name) {
            if let Ok((gw, gh, gmask)) = decode_png_mask(&png, true) {
                let (gw, gh, gmask) = fit_tip(gw, gh, gmask);
                if let Ok(grain) = BrushTip::from_mask(gw, gh, gmask) {
                    imported.brush.grain_tip = Some(grain);
                    return;
                }
            }
        }
    }
    if imported.brush.grain_tip.is_some() {
        return;
    }
    let Some(settings) = settings else {
        return;
    };
    let Some(grain_path) = settings.string("bundledGrainPath") else {
        return;
    };
    if grain_path.is_empty() || grain_path == "$null" {
        return;
    }
    if let Some((gw, gh, gmask)) = read_bundled_asset(archive, names, grain_path, true) {
        let (gw, gh, gmask) = fit_tip(gw, gh, gmask);
        if let Ok(grain) = BrushTip::from_mask(gw, gh, gmask) {
            imported.brush.grain_tip = Some(grain);
            return;
        }
    }
    let (gw, gh, gmask) = bundled::bundled_grain(grain_path, DEFAULT_PROCREATE_TIP_SIDE, 1.0);
    if let Ok(grain) = BrushTip::from_mask(gw, gh, gmask) {
        imported.brush.grain_tip = Some(grain);
    }
}

fn apply_brush_archive_settings(
    imported: &mut ImportedBrush,
    settings: &brush_archive::BrushArchiveSettings,
) {
    imported.brush.pressure_size = 0.0;
    imported.brush.pressure_opacity = 0.0;
    imported.brush.velocity_size = 0.0;
    imported.brush.velocity_opacity = 0.0;
    if let Some(name) = settings.string("name") {
        imported.name = clean_name(name.trim_start_matches('/'), &imported.name);
    }
    if let Some(raw) = settings.f32("plotSpacing") {
        if let Some(spacing) = brush_archive_spacing(raw) {
            imported.brush.spacing = spacing;
        }
    }
    // Procreate stores paintSize as a fraction of its 256 px Shape canvas;
    // maxSize is only the slider's UI limit, not the current size.
    if let Some(value) = settings.f32("paintSize") {
        if value.is_finite() && value > 0.0 {
            imported.brush.size = (DEFAULT_PROCREATE_TIP_SIDE as f32 * value).clamp(1.0, 400.0);
        }
    }
    if let Some(value) = settings.f32("paintOpacity") {
        imported.brush.opacity = value.clamp(0.0, 1.0);
    }
    if let Some(value) = settings.f32("maxOpacity") {
        imported.brush.opacity = (imported.brush.opacity * value.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    }
    if let Some(value) = settings.f32("shapeRoundness") {
        imported.brush.roundness = value.clamp(0.1, 1.0);
    }
    let roundness_jitter = settings
        .f32("jitterShapeRoundness")
        .unwrap_or(0.0)
        .max(settings.f32("jitterShapeRoundnessX").unwrap_or(0.0))
        .clamp(0.0, 1.0);
    if roundness_jitter > 0.0 {
        imported.brush.roundness_jitter = roundness_jitter;
    }
    if let Some(value) = settings.f32("dynamicsJitterSize") {
        imported.brush.size_jitter = value.clamp(0.0, 1.0);
    }
    if let Some(value) = settings.f32("dynamicsJitterOpacity") {
        imported.brush.opacity_jitter = value.clamp(0.0, 1.0);
    }
    if let Some(value) = settings.f32("shapeRotation") {
        imported.brush.shape_rotation = value.clamp(-1.0, 1.0);
    }
    if let Some(value) = settings.f32("shapeScatter") {
        imported.brush.scatter = value.clamp(0.0, 1.0);
        imported.brush.angle_scatter = (value * 0.5).clamp(0.0, 1.0);
    }
    if let Some(value) = settings.f32("shapeCount") {
        imported.brush.shape_count = (value * 16.0).round().clamp(1.0, 16.0) as u8;
    }
    if let Some(value) = settings.f32("shapeCountJitter") {
        imported.brush.shape_count_jitter = value.clamp(0.0, 1.0);
    }
    // Procreate's "Shape Randomise" shuffles which shape is stamped when the
    // brush carries several shapes. With a single tip it has no effect, so
    // only single-out a rotation jitter when there are shapes to shuffle.
    if settings.bool("shapeRandomise").unwrap_or(false)
        && imported.brush.shape_count > 1
        && imported.brush.angle_scatter < 1.0
    {
        imported.brush.angle_scatter = 1.0;
    }
    if let Some(value) = settings.f32("wetEdgesAmount") {
        imported.brush.wet_edge = value.clamp(0.0, 1.0);
    }
    if let Some(value) = settings.f32("grainDepth") {
        if imported.brush.grain_tip.is_some() {
            imported.brush.grain = value.clamp(0.0, 1.0);
        }
    }
    // Grain textures are attached by `attach_procreate_grain` before these
    // settings are applied, so `grainDepth` above is authoritative.
    if let Some(raw) = settings.f32("textureScale") {
        if let Some(scale) = brush_archive_grain_scale(raw) {
            imported.brush.grain_scale = scale;
        }
    }
    if let Some(zoom) = settings.f32("textureZoom") {
        imported.brush.grain_scale = (imported.brush.grain_scale * zoom).clamp(0.25, 16.0);
    }
    let taper_start = settings
        .f32("taperStartLength")
        .unwrap_or(0.0)
        .max(settings.f32("pencilTaperStartLength").unwrap_or(0.0));
    let taper_end = settings
        .f32("taperEndLength")
        .unwrap_or(0.0)
        .max(settings.f32("pencilTaperEndLength").unwrap_or(0.0));
    let taper_shape = settings
        .f32("taperShape")
        .filter(|value| *value > 0.0)
        .or_else(|| settings.f32("pencilTaperShape"))
        .unwrap_or(0.0);
    let taper_size = match (settings.f32("taperSize"), settings.f32("pencilTaperSize")) {
        (Some(touch), Some(pencil)) => touch.max(pencil),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => 1.0,
    };
    let taper_opacity = settings
        .f32("taperOpacity")
        .unwrap_or(0.0)
        .max(settings.f32("pencilTaperOpacity").unwrap_or(0.0));
    let taper_pressure = settings.f32("taperPressure").unwrap_or(0.0);
    imported.brush.taper = Taper {
        start: taper_start.clamp(0.0, 1.0),
        end: taper_end.clamp(0.0, 1.0),
        size: taper_size.clamp(0.0, 1.0),
        shape: taper_shape.clamp(0.0, 1.0),
        opacity: taper_opacity.clamp(0.0, 1.0),
        pressure: taper_pressure.clamp(0.0, 1.0),
    };
    if let Some(value) = settings.f32("dynamicsPressureSize") {
        imported.brush.pressure_size = value.clamp(-1.0, 1.0);
    }
    if let Some(value) = settings.f32("dynamicsPressureOpacity") {
        imported.brush.pressure_opacity = value.clamp(-1.0, 1.0);
    }
    if let Some(value) = settings.f32("dynamicsSpeedSize") {
        imported.brush.velocity_size = value.clamp(0.0, 1.0);
    }
    if let Some(value) = settings.f32("dynamicsSpeedOpacity") {
        imported.brush.velocity_opacity = value.clamp(0.0, 1.0);
    }
    if let Some(curve) = settings.pressure_size_curve() {
        imported.brush.pressure_curve = Some(curve);
    }
    if let Some(curve) = settings.pressure_opacity_curve() {
        imported.brush.pressure_opacity_curve = Some(curve);
    }
}

fn import_procreate(bytes: &[u8], stem: &str) -> Result<Vec<ImportedBrush>, String> {
    import_procreate_recursive(bytes, stem, 0)
}

fn import_procreate_recursive(
    bytes: &[u8],
    stem: &str,
    depth: u8,
) -> Result<Vec<ImportedBrush>, String> {
    if depth > 1 {
        return Err("nested brush archives too deep".to_string());
    }
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|_| "not a Procreate brush file".to_string())?;
    if archive.len() > 4096 {
        return Err("brush archive lists too many files".to_string());
    }
    let names = zip_names(&mut archive);
    let mut list = Vec::new();
    let mut seen = HashSet::new();
    for name in &names {
        let lower = name.to_ascii_lowercase().replace('\\', "/");
        if is_procreate_junk(&lower) || !seen.insert(lower.clone()) {
            continue;
        }
        let is_shape = lower
            .rsplit('/')
            .next()
            .map(|f| f == "shape.png")
            .unwrap_or(false);
        if is_shape {
            let mut f = archive
                .by_name(name)
                .map_err(|_| "brush archive is corrupt".to_string())?;
            if f.size() > MAX_ZIP_UNPACKED {
                continue;
            }
            let mut png = Vec::new();
            use std::io::Read;
            f.read_to_end(&mut png)
                .map_err(|_| "brush archive is corrupt".to_string())?;
            drop(f);
            let archive_name = brush_archive_sibling(&names, name, "Brush.archive");
            let settings = archive_name.as_deref().and_then(|archive_name| {
                read_zip_entry(&mut archive, archive_name)
                    .and_then(|bytes| brush_archive::BrushArchiveSettings::parse(&bytes).ok())
            });
            let white_is_ink = !settings
                .as_ref()
                .and_then(|settings| settings.bool("shapeInverted"))
                .unwrap_or(false);
            let (w, h, mask) = decode_png_mask(&png, white_is_ink)?;
            let brush_name = brush_name_from_entry(name, stem);
            let mut imported = stamp_brush(brush_name, w, h, mask, 0.15)?;
            attach_procreate_grain(&mut imported, &mut archive, &names, name, settings.as_ref());
            if let Some(settings) = settings.as_ref() {
                apply_brush_archive_settings(&mut imported, settings);
            }
            list.push(imported);
            if list.len() >= MAX_LIBRARY_BRUSHES {
                break;
            }
            continue;
        }
        // Procreate brushes without a Shape.png use Brush.archive settings and
        // the bundled preset shape; fall back to a procedural round tip.
        let is_archive = lower.rsplit('/').next() == Some("brush.archive")
            && !lower.split('/').any(|part| part == "reset");
        if is_archive {
            if brush_archive_sibling(&names, name, "Shape.png").is_some() {
                continue;
            }
            let Some(archive_bytes) = read_zip_entry(&mut archive, name) else {
                continue;
            };
            let settings = brush_archive::BrushArchiveSettings::parse(&archive_bytes).ok();
            let brush_name = settings
                .as_ref()
                .and_then(|settings| settings.string("name"))
                .map(|name| clean_name(name, "Procreate brush"))
                .unwrap_or_else(|| brush_name_from_entry(name, stem));
            let inverted = settings
                .as_ref()
                .and_then(|settings| settings.bool("shapeInverted"))
                .unwrap_or(false);
            let (w, h, mask) = match settings
                .as_ref()
                .and_then(|settings| settings.string("bundledShapePath"))
            {
                Some(path) => match read_bundled_asset(&mut archive, &names, path, !inverted) {
                    Some(asset) => asset,
                    None => {
                        let (w, h, mut mask) =
                            bundled::bundled_shape(path, DEFAULT_PROCREATE_TIP_SIDE);
                        if inverted {
                            for value in &mut mask {
                                *value = 255 - *value;
                            }
                        }
                        (w, h, mask)
                    }
                },
                None => procedural_round_tip(DEFAULT_PROCREATE_TIP_SIDE),
            };
            let mut imported = stamp_brush(brush_name, w, h, mask, 0.15)?;
            attach_procreate_grain(&mut imported, &mut archive, &names, name, settings.as_ref());
            if let Some(settings) = settings.as_ref() {
                apply_brush_archive_settings(&mut imported, settings);
            }
            list.push(imported);
            if list.len() >= MAX_LIBRARY_BRUSHES {
                break;
            }
            continue;
        }
        // A .brushset may nest whole .brush zips instead of folders.
        if depth == 0 && lower.ends_with(".brush") {
            let mut f = archive
                .by_name(name)
                .map_err(|_| "brush archive is corrupt".to_string())?;
            if f.size() > MAX_ZIP_UNPACKED || f.size() < 4 {
                continue;
            }
            let mut nested = Vec::new();
            use std::io::Read;
            f.read_to_end(&mut nested)
                .map_err(|_| "brush archive is corrupt".to_string())?;
            drop(f);
            if nested.starts_with(b"PK") {
                let inner_stem = name.rsplit('/').next().unwrap_or(name);
                let inner_stem = inner_stem.strip_suffix(".brush").unwrap_or(inner_stem);
                if let Ok(mut inner) = import_procreate_recursive(&nested, inner_stem, depth + 1) {
                    list.append(&mut inner);
                }
                if list.len() >= MAX_LIBRARY_BRUSHES {
                    break;
                }
            }
        }
    }
    if list.is_empty() {
        return Err("no Shape.png tips found in Procreate file".to_string());
    }
    cap_list(list)
}

// ---------------------------------------------------------------------------
// Krita .kpp: a PNG whose compressed "preset" text chunk holds the settings
// XML; stamp tips ride inside as base64 PNGs. Verified against real preset
// XML (`<Brush spacing="0.2" type="kis_..._brush" .../>` + `<param>` pairs).
// ---------------------------------------------------------------------------

fn png_text_chunks(png: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    if png.len() < 8 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("not a Krita preset file".to_string());
    }
    let mut out = Vec::new();
    let mut pos = 8usize;
    let mut chunks = 0usize;
    while pos + 8 <= png.len() && chunks < 256 {
        chunks += 1;
        let len = u32::from_be_bytes(
            png[pos..pos + 4]
                .try_into()
                .map_err(|_| "PNG is corrupt".to_string())?,
        ) as usize;
        let kind = &png[pos + 4..pos + 8];
        pos += 8;
        if len > png.len().saturating_sub(pos + 4) {
            return Err("PNG is corrupt".to_string());
        }
        let data = &png[pos..pos + len];
        pos += len + 4; // skip data + CRC
        match kind {
            b"zTXt" => {
                if let Some(z) = data.iter().position(|&b| b == 0) {
                    let keyword = String::from_utf8_lossy(&data[..z]).to_string();
                    if data.len() > z + 2 {
                        // method byte must be 0 (deflate).
                        if data[z + 1] != 0 {
                            continue;
                        }
                        let mut dec = flate2::read::ZlibDecoder::new(&data[z + 2..]);
                        let mut text = Vec::new();
                        use std::io::Read;
                        if dec.read_to_end(&mut text).is_ok() {
                            out.push((keyword, text));
                        }
                    }
                }
            }
            b"tEXt" => {
                if let Some(z) = data.iter().position(|&b| b == 0) {
                    let keyword = String::from_utf8_lossy(&data[..z]).to_string();
                    out.push((keyword, data[z + 1..].to_vec()));
                }
            }
            b"IEND" => break,
            _ => {}
        }
        if pos >= png.len() {
            break;
        }
    }
    Ok(out)
}

/// Pull `attr="value"` from the first `<tag ...>` occurrence.
fn tag_attr(xml: &str, tag: &str, attr: &str) -> Option<String> {
    let mut search = 0usize;
    while let Some(start) = xml[search..].find(&format!("<{tag}")) {
        let abs = search + start;
        let end = xml[abs..].find('>')? + abs;
        let head = &xml[abs..end.min(abs + 2048)];
        let key = format!("{attr}=\"");
        if let Some(vs) = head.find(&key) {
            let vs = vs + key.len();
            if let Some(ve) = head[vs..].find('"') {
                return Some(head[vs..vs + ve].to_string());
            }
        }
        search = end + 1;
    }
    None
}

/// First `<param ... name="X" ...>value</param>` for any of `names`.
/// Real Krita files carry the value in a CDATA body
/// (`<param type="string" name="OpacityValue"><![CDATA[1]]></param>`);
/// a `value="..."` attribute is accepted as fallback. Matches the full
/// attribute exactly, so `MaskingBrush/Preset/OpacityValue` never shadows
/// the top-level `OpacityValue`.
fn param_value(xml: &str, names: &[&str]) -> Option<f32> {
    let mut search = 0usize;
    while let Some(start) = xml[search..].find("<param") {
        let abs = search + start;
        let end = xml[abs..].find('>')? + abs;
        let head = &xml[abs..end.min(abs + 1024)];
        let hit = names
            .iter()
            .any(|n| head.contains(&format!("name=\"{n}\"")));
        if !hit {
            search = end + 1;
            continue;
        }
        // Attribute form first.
        if let Some(vs) = head.find("value=\"") {
            let vs = vs + 7;
            if let Some(ve) = head[vs..].find('"') {
                if let Ok(v) = head[vs..vs + ve].parse::<f32>() {
                    return Some(v);
                }
            }
        }
        // CDATA/body form.
        let close = xml[end..].find("</param>")? + end;
        let mut body: String = xml[end + 1..close].to_string();
        body = body.replace("<![CDATA[", "").replace("]]>", "");
        if let Ok(v) = body.trim().parse::<f32>() {
            return Some(v);
        }
        search = end + 1;
    }
    None
}

/// First `diameter="..."` (the main brush's `MaskGenerator` comes before any
/// masking/texture brush definitions).
fn mask_diameter(xml: &str) -> Option<f32> {
    let mut search = 0usize;
    while let Some(start) = xml[search..].find("diameter=\"") {
        let vs = search + start + 10;
        if let Some(ve) = xml[vs..].find('"') {
            if let Ok(v) = xml[vs..vs + ve].parse::<f32>() {
                return Some(v);
            }
            search = vs + ve + 1;
        } else {
            break;
        }
    }
    None
}

/// Text of every CDATA segment (embedded resources live here).
fn cdata_segments(xml: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut s = 0usize;
    while let Some(st) = xml[s..].find("<![CDATA[") {
        let abs = s + st + 9;
        let end = xml[abs..].find("]]>").map(|e| abs + e).unwrap_or(abs);
        out.push(&xml[abs..end]);
        s = end + 3;
        if out.len() >= 64 {
            break;
        }
    }
    out
}

/// Try every long base64 run in the XML as an embedded tip PNG: first the
/// CDATA segments (where Krita stores resource blobs), then any other long
/// alphanumeric run as fallback.
fn embedded_tip(xml: &str) -> Option<(u32, u32, Vec<u8>)> {
    use base64::Engine;
    let mut candidates: Vec<String> = Vec::new();
    for seg in cdata_segments(xml) {
        let flat: String = seg.chars().filter(|c| !c.is_whitespace()).collect();
        if flat.len() >= 512 {
            candidates.push(flat);
        }
        if candidates.len() >= 8 {
            break;
        }
    }
    if candidates.is_empty() {
        let bytes = xml.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() && candidates.len() < 8 {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'+'
                    || bytes[i] == b'/'
                    || bytes[i] == b'=')
            {
                i += 1;
            }
            if i - start >= 512 {
                let mut cand = &xml[start..i];
                cand = &cand[..cand.len() - cand.len() % 4];
                candidates.push(cand.to_string());
            }
            i += 1;
        }
    }
    for cand in &candidates {
        if let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(cand) {
            if raw.len() >= 8 && raw.len() <= 8 * 1024 * 1024 && raw.starts_with(b"\x89PNG") {
                if let Ok(tip) = decode_png_mask(&raw, false) {
                    let dark_sum: u64 = tip.2.iter().map(|&b| b as u64).sum();
                    if dark_sum > 0 {
                        return Some(tip);
                    }
                    if let Ok(alt) = decode_png_mask(&raw, true) {
                        let sum: u64 = alt.2.iter().map(|&b| b as u64).sum();
                        if sum > 0 {
                            return Some(alt);
                        }
                    }
                }
            }
        }
    }
    None
}

fn import_kpp_bytes(
    bytes: &[u8],
    fallback: &str,
    dir: Option<&std::path::Path>,
) -> Result<Vec<ImportedBrush>, String> {
    import_kpp_with_tip(bytes, fallback, |fname| {
        let dir = dir?;
        let raw = std::fs::read(dir.join(fname.trim())).ok()?;
        (raw.len() <= MAX_FILE_BYTES).then_some(raw)
    })
}

fn import_kpp_with_tip<F>(
    bytes: &[u8],
    fallback: &str,
    mut resolve_tip: F,
) -> Result<Vec<ImportedBrush>, String>
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    let chunks = png_text_chunks(bytes)?;
    let xml_bytes = chunks
        .iter()
        .find(|(k, _)| k.to_ascii_lowercase().contains("preset"))
        .map(|(_, v)| v.clone())
        .ok_or("Krita preset has no settings block".to_string())?;
    let xml = String::from_utf8_lossy(&xml_bytes).into_owned();
    if !xml.contains("<Preset") && !xml.contains("<preset") {
        return Err("Krita preset XML is unreadable".to_string());
    }
    let name = tag_attr(&xml, "Preset", "name")
        .or_else(|| tag_attr(&xml, "preset", "name"))
        .map(|n| clean_name(&n, fallback))
        .unwrap_or_else(|| clean_name(fallback, "Krita brush"));
    let spacing = tag_attr(&xml, "Brush", "spacing")
        .and_then(|s| s.parse::<f32>().ok())
        .map(|s| clamp_spacing(s, 0.15))
        .unwrap_or(0.15);
    // Embedded stamp tip wins when present; otherwise the parametric round
    // brush faithfully carries Krita's auto (circle) tips.
    if let Some((w, h, mask)) = embedded_tip(&xml) {
        return cap_list(vec![stamp_brush(name, w, h, mask, spacing)?]);
    }
    // Stamp brush referenced by file (e.g. masking/texture tips): resolve
    // against the .kpp's own directory when possible.
    if let Some(fname) = tag_attr(&xml, "Brush", "filename") {
        if let Some(raw) = resolve_tip(fname.trim()) {
            if let Ok((w, h, mask)) =
                decode_png_mask(&raw, true).or_else(|_| decode_png_mask(&raw, false))
            {
                return cap_list(vec![stamp_brush(name, w, h, mask, spacing)?]);
            }
        }
    }
    // Krita size lives in the MaskGenerator diameter (pixels); opacity is
    // OpacityValue x FlowValue (both per-dab scale factors, defaults 1).
    let size = mask_diameter(&xml)
        .filter(|v| v.is_finite() && *v >= 1.0 && *v <= 400.0)
        .unwrap_or(24.0);
    let opacity = param_value(&xml, &["OpacityValue", "Opacity"])
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0)
        .unwrap_or(1.0);
    let flow = param_value(&xml, &["FlowValue", "Flow"])
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0)
        .unwrap_or(1.0);
    // CompositeOp=erase and pressure curves have no ProCraft equivalent and
    // are intentionally not transferred.
    cap_list(vec![ImportedBrush {
        name,
        brush: Brush {
            size,
            opacity: (opacity * flow).clamp(0.0, 1.0),
            softness: 0.5,
            spacing,
            eraser: false,
            tip: None,
            ..Brush::default()
        },
    }])
}

fn import_kpp(
    path: &std::path::Path,
    bytes: &[u8],
    stem: &str,
) -> Result<Vec<ImportedBrush>, String> {
    import_kpp_bytes(bytes, stem, path.parent())
}

/// Krita `.bundle`: ZIP of many `.kpp` files under `paintoppresets/`.
#[cfg(test)]
fn import_krita_bundle(bytes: &[u8]) -> Result<Vec<ImportedBrush>, String> {
    import_krita_bundle_report(bytes).map(|outcome| outcome.brushes)
}

fn import_krita_bundle_report(bytes: &[u8]) -> Result<ImportOutcome, String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|_| "not a Krita bundle file".to_string())?;
    if archive.len() > 4096 {
        return Err("bundle lists too many files".to_string());
    }
    let names = zip_names(&mut archive);
    let kpp_total = names
        .iter()
        .filter(|name| name.to_ascii_lowercase().ends_with(".kpp"))
        .count();
    let mut list = Vec::new();
    let mut warnings = Vec::new();
    let mut skipped = 0usize;
    let mut processed = 0usize;
    for name in &names {
        if !name.to_ascii_lowercase().ends_with(".kpp") {
            continue;
        }
        processed += 1;
        let mut f = archive
            .by_name(name)
            .map_err(|_| "bundle is corrupt".to_string())?;
        if f.size() > MAX_ZIP_UNPACKED {
            skipped += 1;
            warnings.push(format!("Skipped '{name}': file is too large."));
            continue;
        }
        let mut raw = Vec::new();
        use std::io::Read;
        f.read_to_end(&mut raw)
            .map_err(|_| "bundle is corrupt".to_string())?;
        drop(f);
        let stem = name
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or(name)
            .to_string();
        let stem = stem
            .strip_suffix(".kpp")
            .or_else(|| stem.strip_suffix(".KPP"))
            .unwrap_or(&stem);
        let outcome = import_kpp_with_tip(&raw, stem, |fname| {
            let candidate = find_bundle_tip(&names, fname)?;
            let raw = read_zip_entry(&mut archive, &candidate)?;
            (raw.len() <= MAX_FILE_BYTES).then_some(raw)
        });
        match outcome {
            Ok(mut inner) => list.append(&mut inner),
            Err(err) => {
                skipped += 1;
                warnings.push(format!("Skipped '{stem}': {err}."));
            }
        }
        if list.len() >= MAX_LIBRARY_BRUSHES {
            let remaining = kpp_total.saturating_sub(processed);
            if remaining > 0 {
                skipped += remaining;
                warnings.push(format!(
                    "Skipped {remaining} presets after the {MAX_LIBRARY_BRUSHES} brush limit."
                ));
            }
            break;
        }
    }
    if list.is_empty() {
        return Err("no usable presets found in bundle".to_string());
    }
    Ok(ImportOutcome {
        brushes: cap_list(list)?,
        warnings,
        skipped,
    })
}

// ---------------------------------------------------------------------------
// Photoshop .abr: version + 8BIM tagged blocks. Sampled ('samp') tips follow
// the public reverse-engineered v6 layout (38-byte `$`-UUID, subversion-sized
// padding, bounds, 8/16-bit depth, raw/PackBits data); real brush names,
// spacing and diameter come from the 'desc' descriptors via UUID linkage.
// Oversized source tips are downscaled to MAX_TIP_SIDE. Anything off-spec is
// a clean error.
// ---------------------------------------------------------------------------

const MAX_ABR_SOURCE_TIP_SIDE: u32 = MAX_TIP_SIDE * 16;

/// Photoshop PackBits rows: `h` u16 row byte-counts, then PackBits streams.
fn unpack_packbits(data: &[u8], w: u32, h: u32, sample_bytes: u32) -> Result<Vec<u8>, String> {
    let row_len = (w * sample_bytes) as usize;
    let mut c = Cursor::new(data);
    let mut counts = Vec::with_capacity(h as usize);
    for _ in 0..h {
        counts.push(c.u16be()? as usize);
    }
    let mut out = vec![0u8; row_len * h as usize];
    for (row, &count) in counts.iter().enumerate() {
        let stream = c.take(count)?;
        let mut si = 0usize;
        let mut dst = row * row_len;
        let end = dst + row_len;
        while dst < end {
            if si >= stream.len() {
                return Err("ABR RLE row is truncated".to_string());
            }
            let n = stream[si];
            si += 1;
            if n >= 128 {
                let run = 257 - n as usize;
                if si >= stream.len() || dst + run > end {
                    return Err("ABR RLE row is corrupt".to_string());
                }
                let v = stream[si];
                si += 1;
                out[dst..dst + run].fill(v);
                dst += run;
            } else {
                let lit = n as usize + 1;
                if si + lit > stream.len() || dst + lit > end {
                    return Err("ABR RLE row is corrupt".to_string());
                }
                out[dst..dst + lit].copy_from_slice(&stream[si..si + lit]);
                si += lit;
                dst += lit;
            }
        }
    }
    Ok(out)
}

/// Decode a Photoshop descriptor TEXT value (u32 UTF-16 unit count including
/// the null terminator, then UTF-16BE). Returns the string and bytes consumed.
fn desc_text(data: &[u8]) -> Option<(String, usize)> {
    if data.len() < 4 {
        return None;
    }
    let units = u32::from_be_bytes(data[..4].try_into().ok()?) as usize;
    if units == 0 || units > 129 || data.len() < 4 + units * 2 {
        return None;
    }
    let words: Vec<u16> = data[4..4 + units * 2]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    let mut s = String::from_utf16_lossy(&words);
    if s.ends_with('\0') {
        s.pop();
    }
    Some((s, 4 + units * 2))
}

/// Decode a Photoshop descriptor unit-float (`UntF`: 4-char unit + f64).
fn desc_unit_float(data: &[u8]) -> Option<((String, f64), usize)> {
    if data.len() < 12 {
        return None;
    }
    let unit = String::from_utf8_lossy(&data[..4]).into_owned();
    let v = f64::from_be_bytes(data[4..12].try_into().ok()?);
    if !v.is_finite() {
        return None;
    }
    Some(((unit, v), 12))
}

fn norm_uuid(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .trim_matches('\0')
        .trim_start_matches('$')
        .to_ascii_lowercase()
}

#[cfg(test)]
fn import_abr(bytes: &[u8]) -> Result<Vec<ImportedBrush>, String> {
    import_abr_report(bytes).map(|outcome| outcome.brushes)
}

fn import_abr_report(bytes: &[u8]) -> Result<ImportOutcome, String> {
    let mut c = Cursor::new(bytes);
    let version = c.u16be()?;
    if version != 6 && version != 7 && version != 9 && version != 10 {
        return Err(format!(
            "unsupported ABR version {version} (reads v6-v10 sampled tips; export tips as PNG from Photoshop for anything else)"
        ));
    }
    let sub = c.u16be()?;
    let header_pad = match sub {
        1 => 10usize,
        2 => 263usize,
        _ => return Err(format!("unsupported ABR subversion {sub}")),
    };
    // Collect (uuid, tip) in file order plus descriptor entries for naming.
    let mut tips: Vec<(String, BrushTip)> = Vec::new();
    let mut descs: Vec<Vec<u8>> = Vec::new();
    let mut limit_hit = false;
    while c.left() >= 12 {
        if c.take(4)? != b"8BIM" {
            return Err("ABR file is corrupt (bad block signature)".to_string());
        }
        let key = c.take(4)?.to_vec();
        let len = c.u32be()? as usize;
        if len > c.left() {
            return Err("ABR file is truncated".to_string());
        }
        let data = c.take(len)?.to_vec();
        // Blocks pad to a 4-byte boundary, except a final block at EOF.
        let pad = (4 - len % 4) % 4;
        if pad > 0 && c.left() >= pad {
            c.take(pad)?;
        }
        if &key == b"samp" {
            let mut d = Cursor::new(&data);
            let mut entry_no = 0usize;
            while d.left() > 4 + 38 + header_pad + 16 + 2 {
                let entry_len = d.u32be()? as usize;
                let entry_start = d.pos;
                if entry_len > d.left() + 4 {
                    return Err("ABR tip record is truncated".to_string());
                }
                let uuid_raw = d.take(38)?;
                if uuid_raw[37] != 0 {
                    return Err("ABR tip record is corrupt (bad UUID)".to_string());
                }
                let uuid = norm_uuid(&uuid_raw[..37]);
                d.take(header_pad)?;
                let top = d.i32be()?;
                let left = d.i32be()?;
                let bottom = d.i32be()?;
                let right = d.i32be()?;
                let w = right.saturating_sub(left) as u32;
                let h = bottom.saturating_sub(top) as u32;
                if w == 0 || h == 0 || w > MAX_ABR_SOURCE_TIP_SIDE || h > MAX_ABR_SOURCE_TIP_SIDE {
                    return Err("ABR tip has bad dimensions".to_string());
                }
                let depth = d.u16be()?;
                if depth != 8 && depth != 16 {
                    return Err(format!("ABR tip has unsupported depth {depth}"));
                }
                let compression = *d
                    .take(1)?
                    .first()
                    .ok_or("ABR tip is truncated".to_string())?;
                let sample_bytes = (depth / 8) as u32;
                let mask = match compression {
                    0 => {
                        let need = (w * h * sample_bytes) as usize;
                        let raw = d.take(need)?.to_vec();
                        if sample_bytes == 1 {
                            raw
                        } else {
                            raw.as_chunks::<2>()
                                .0
                                .iter()
                                .map(|c| (u16::from_be_bytes([c[0], c[1]]) / 257) as u8)
                                .collect()
                        }
                    }
                    1 => {
                        // RLE payload runs to the entry end (4-byte aligned).
                        let consumed = d.pos - entry_start;
                        let mut payload_len = entry_len.saturating_sub(consumed);
                        // entry_len semantics vary; fall back to block remainder.
                        if payload_len == 0 || d.left() < payload_len {
                            payload_len = d.left();
                        }
                        let payload = d
                            .take(payload_len)
                            .map_err(|e| format!("ABR tip {entry_no} payload: {e}"))?;
                        let raw = unpack_packbits(payload, w, h, sample_bytes)
                            .map_err(|e| format!("ABR tip {entry_no} RLE: {e}"))?;
                        if sample_bytes == 1 {
                            raw
                        } else {
                            raw.as_chunks::<2>()
                                .0
                                .iter()
                                .map(|c| (u16::from_be_bytes([c[0], c[1]]) / 257) as u8)
                                .collect()
                        }
                    }
                    _ => return Err("ABR tip has unknown compression".to_string()),
                };
                // Next entry starts at the next 4-byte boundary.
                let misalign = d.pos % 4;
                if misalign != 0 {
                    d.take(4 - misalign)?;
                }
                let (w, h, mask) = fit_tip(w, h, mask);
                if tips.len() < MAX_LIBRARY_BRUSHES {
                    tips.push((uuid, BrushTip::from_mask(w, h, mask)?));
                } else {
                    limit_hit = true;
                    break;
                }
                entry_no += 1;
            }
        } else if &key == b"desc" {
            // Descriptor data is a nested tree (version + BrshVlLs list +
            // per-preset Objc items), not the flat count+length layout some
            // notes describe. Verified strategy on Photoshop 2026 files:
            // ordered flat scan — the i-th `Nm  `/`sampledData` pair names
            // the i-th brush, and the first `Spcn`/`Dmtr` belong to the main
            // brush (sub-objects like dual-brush serialize later).
            descs.push(data);
        }
    }
    // Name/size/spacing from descriptors via UUID linkage.
    let mut names: Vec<Option<String>> = vec![None; tips.len()];
    let mut sizes: Vec<Option<f32>> = vec![None; tips.len()];
    let mut spacings: Vec<Option<f32>> = vec![None; tips.len()];
    // Concatenate desc blocks (usually one) and scan in file order.
    let mut desc_all = Vec::new();
    for d in &descs {
        desc_all.extend_from_slice(d);
    }
    let entry = desc_all.as_slice();
    let mut found_names = Vec::new();
    let mut found_uuids = Vec::new();
    let mut found_spcns = Vec::new();
    let mut found_dmtrs = Vec::new();
    {
        let mut i = 0usize;
        while i + 8 <= entry.len() {
            if &entry[i..i + 4] == b"Nm  " && &entry[i + 4..i + 8] == b"TEXT" {
                if let Some((s, _)) = desc_text(&entry[i + 8..]) {
                    let s = s.trim().to_string();
                    if !s.is_empty() && s.chars().count() <= 64 {
                        found_names.push(s);
                    }
                }
                i += 8;
            } else if i + 15 <= entry.len()
                && &entry[i..i + 11] == b"sampledData"
                && &entry[i + 11..i + 15] == b"TEXT"
            {
                if let Some((s, _)) = desc_text(&entry[i + 15..]) {
                    found_uuids.push(s.trim().to_ascii_lowercase());
                }
                i += 15;
            } else if &entry[i..i + 4] == b"Spcn" && &entry[i + 4..i + 8] == b"UntF" {
                if let Some(((_, v), _)) = desc_unit_float(&entry[i + 8..]) {
                    found_spcns.push(v);
                }
                i += 8;
            } else if &entry[i..i + 4] == b"Dmtr" && &entry[i + 4..i + 8] == b"UntF" {
                if let Some(((_, v), _)) = desc_unit_float(&entry[i + 8..]) {
                    found_dmtrs.push(v);
                }
                i += 8;
            } else {
                i += 1;
            }
        }
    }
    let mut warnings = Vec::new();
    let mut skipped = 0usize;
    if tips.is_empty() {
        if found_names.is_empty() {
            return Err("no readable tips or presets in ABR file".to_string());
        }
        let mut list = Vec::with_capacity(found_names.len());
        for (n, name) in found_names.iter().enumerate() {
            let dm = found_dmtrs
                .get(n)
                .copied()
                .or(found_dmtrs.first().copied())
                .unwrap_or(24.0) as f32;
            let size = if dm.is_finite() && (1.0..=1024.0).contains(&dm) {
                dm
            } else {
                24.0
            };
            let sp = found_spcns
                .get(n)
                .copied()
                .or(found_spcns.first().copied())
                .unwrap_or(25.0);
            let sp = (sp / 100.0) as f32;
            let spacing = if sp.is_finite() && sp > 0.0 && sp <= 2.0 {
                sp
            } else {
                0.25
            };
            list.push(ImportedBrush {
                name: name.clone(),
                brush: Brush {
                    size,
                    opacity: 1.0,
                    softness: 0.0,
                    spacing,
                    eraser: false,
                    tip: None,
                    ..Brush::default()
                },
            });
        }
        warnings.push("ABR has no sampled tips; imported parametric presets instead.".to_string());
        let brushes = cap_list(list)?;
        return Ok(ImportOutcome {
            brushes,
            warnings,
            skipped,
        });
    }
    let mut matched = vec![false; found_names.len()];
    for (n, name) in found_names.iter().enumerate() {
        let uuid = found_uuids.get(n);
        let slot = uuid
            .and_then(|u| {
                tips.iter().position(|(t, _)| *t == *u).or_else(|| {
                    tips.iter().position(|(t, _)| {
                        let m = t.len().min(u.len()).min(35);
                        m >= 8 && t[..m] == u[..m]
                    })
                })
            })
            .or(if uuid.is_none() && tips.len() == found_names.len() {
                Some(n)
            } else {
                None
            });
        if let Some(i) = slot {
            if i < tips.len() {
                matched[n] = true;
                names[i] = Some(name.clone());
                let dm = found_dmtrs
                    .get(n)
                    .copied()
                    .or(found_dmtrs.first().copied())
                    .unwrap_or(0.0) as f32;
                if dm.is_finite() && (1.0..=1024.0).contains(&dm) {
                    sizes[i] = Some(dm);
                }
                let sp = found_spcns
                    .get(n)
                    .copied()
                    .or(found_spcns.first().copied())
                    .unwrap_or(0.0);
                let sp = (sp / 100.0) as f32;
                if sp.is_finite() && sp > 0.0 && sp <= 2.0 {
                    spacings[i] = Some(sp);
                }
            }
        }
    }
    for (n, name) in found_names.iter().enumerate() {
        if !matched[n] {
            skipped += 1;
            warnings.push(format!("Skipped preset '{name}': no sampled tip."));
        }
    }
    if limit_hit {
        warnings.push(format!(
            "Stopped after {MAX_LIBRARY_BRUSHES} tips; later tips were skipped."
        ));
    }
    let mut list = Vec::with_capacity(tips.len());
    for (i, (uuid, tip)) in tips.into_iter().enumerate() {
        let _ = uuid;
        list.push(ImportedBrush {
            name: names[i]
                .clone()
                .unwrap_or_else(|| format!("ABR Brush {}", i + 1)),
            brush: Brush {
                size: sizes[i].unwrap_or(tip.w as f32),
                opacity: 1.0,
                softness: 0.0,
                spacing: spacings[i].unwrap_or(0.25),
                eraser: false,
                tip: Some(tip),
                ..Brush::default()
            },
        });
    }
    Ok(ImportOutcome {
        brushes: cap_list(list)?,
        warnings,
        skipped,
    })
}

// ---------------------------------------------------------------------------
// Tests: every importer round-trips synthetic files built to the layouts
// above (logic tests, not vendor fixtures).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::brush::BrushPreset;
    use super::super::document::Document;
    use super::*;
    use crate::brush::Color;

    fn paint_stamp_pixels() -> Vec<u8> {
        let mut doc = Document::new(32, 32);
        let tip = BrushTip::from_mask(
            4,
            4,
            vec![0, 0, 0, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0, 0],
        )
        .unwrap();
        let brush = Brush {
            size: 8.0,
            opacity: 1.0,
            tip: Some(tip),
            ..Brush::default()
        };
        doc.dab(&brush, Color::rgb(255, 0, 0), 16.0, 16.0);
        doc.layers[0].pixels.clone()
    }

    #[test]
    fn stamp_dab_paints_tip_shape() {
        let px = paint_stamp_pixels();
        assert!(px.iter().any(|&b| b != 0));
        // Corners of the dab area stay empty: the tip has a square ink
        // block, so the stamp is square-ish, not a soft disc.
        let corner = ((8 * 32 + 8) * 4) as usize;
        assert_eq!(px[corner + 3], 0);
        let center = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(&px[center..center + 3], &[255, 0, 0]);
        assert_eq!(px[center + 3], 255);
    }

    #[test]
    fn stamp_respects_selection() {
        let mut doc = Document::new(32, 32);
        let tip = BrushTip::from_mask(2, 2, vec![255; 4]).unwrap();
        let brush = Brush {
            size: 16.0,
            opacity: 1.0,
            tip: Some(tip),
            ..Brush::default()
        };
        doc.selection = Some((0, 0, 4, 4));
        doc.dab(&brush, Color::rgb(1, 2, 3), 16.0, 16.0);
        assert!(doc.layers[0].pixels.to_dense().iter().all(|&b| b == 0));
    }

    #[test]
    fn tip_brush_survives_pcbrush_roundtrip() {
        let tip = BrushTip::from_mask(2, 2, vec![0, 255, 255, 0]).unwrap();
        let libs = vec![BrushPreset {
            name: "Stamp".to_string(),
            brush: Brush {
                tip: Some(tip),
                ..Brush::default()
            },
        }];
        // Old (v9) files without the tip field still open.
        let dir = std::env::temp_dir();
        let legacy = dir.join("procraft-legacy-brush.pcbrush");
        std::fs::write(&legacy, r#"[{"name":"Old","brush":{"size":9.0,"opacity":0.5,"softness":0.2,"spacing":0.3,"eraser":false}}]"#).unwrap();
        let back = open_brush_library(&legacy).unwrap();
        let _ = std::fs::remove_file(&legacy);
        assert!(back[0].brush.tip.is_none());
        assert_eq!(back[0].brush.size, 9.0);
        // New files keep the tip.
        let path = dir.join("procraft-tip-brush.pcbrush");
        super::super::brush::save_brush_library(&path, &libs).unwrap();
        let back = open_brush_library(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back, libs);
    }

    fn write_gbr(version: u32, depth: u32, w: u32, h: u32, body: &[u8], spacing: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        let name = b"TestTip\0";
        let header_size = if version == 2 {
            28 + name.len()
        } else {
            20 + name.len()
        };
        buf.extend_from_slice(&(header_size as u32).to_be_bytes());
        buf.extend_from_slice(&version.to_be_bytes());
        buf.extend_from_slice(&w.to_be_bytes());
        buf.extend_from_slice(&h.to_be_bytes());
        buf.extend_from_slice(&depth.to_be_bytes());
        if version == 2 {
            buf.extend_from_slice(b"GIMP");
            buf.extend_from_slice(&spacing.to_be_bytes());
        }
        buf.extend_from_slice(name);
        buf.extend_from_slice(body);
        buf
    }

    #[test]
    fn gbr_gray_and_rgba_import() {
        // 2x2 gray: dark paints, white is transparent.
        let raw = write_gbr(2, 1, 2, 2, &[0, 255, 128, 64], 25);
        let list = import_gbr(&raw, "stem").unwrap();
        assert_eq!(list[0].name, "TestTip");
        assert_eq!(
            list[0].brush.tip.as_ref().unwrap().mask,
            vec![255, 0, 127, 191]
        );
        assert!((list[0].brush.spacing - 0.25).abs() < 0.001);
        // RGBA uses the alpha channel.
        let raw = write_gbr(2, 4, 1, 1, &[10, 20, 30, 200], 10);
        let list = import_gbr(&raw, "stem").unwrap();
        assert_eq!(list[0].brush.tip.as_ref().unwrap().mask, vec![200]);
        // v1 has no magic/spacing.
        let raw = write_gbr(1, 1, 1, 1, &[0], 0);
        let list = import_gbr(&raw, "stem").unwrap();
        assert_eq!(list[0].brush.tip.as_ref().unwrap().mask, vec![255]);
        // Bad magic rejected.
        let mut bad = write_gbr(2, 1, 1, 1, &[0], 10);
        bad[20..24].copy_from_slice(b"NOPE");
        assert!(import_gbr(&bad, "stem").is_err());
    }

    fn png_from(img: &image::RgbaImage) -> Vec<u8> {
        let mut buf = Vec::new();
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        buf
    }

    #[test]
    fn image_tip_trims_transparent_padding() {
        let mut img = image::RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 0]));
        for y in 4..6 {
            for x in 4..6 {
                img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let list = import_image_tip(&png_from(&img), "Padded").unwrap();
        let tip = list[0].brush.tip.as_ref().unwrap();
        assert_eq!((tip.w, tip.h), (2, 2));
        assert_eq!(tip.mask, vec![255; 4]);
    }

    #[test]
    fn image_tip_infers_dark_ink_on_a_light_border() {
        let mut img = image::RgbaImage::from_pixel(6, 6, image::Rgba([255, 255, 255, 255]));
        for y in 2..4 {
            for x in 2..4 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let list = import_image_tip(&png_from(&img), "Ink").unwrap();
        let tip = list[0].brush.tip.as_ref().unwrap();
        assert_eq!((tip.w, tip.h), (2, 2));
        assert_eq!(tip.mask, vec![255; 4]);
    }

    #[test]
    fn image_tip_rejects_an_empty_png() {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]));
        let err = import_image_tip(&png_from(&img), "Empty").unwrap_err();
        assert!(err.contains("empty"), "unexpected error: {err}");
    }

    #[test]
    fn image_tip_imports_png() {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 100, 50, 255]));
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-tip-import.png");
        img.save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let list = import_image_tip(&bytes, "MyTip").unwrap();
        assert_eq!(list[0].name, "MyTip");
        let tip = list[0].brush.tip.as_ref().unwrap();
        assert_eq!((tip.w, tip.h), (4, 4));
        assert!(tip.mask.iter().all(|&b| b > 0));
        assert!(import_image_tip(b"not an image", "x").is_err());
    }

    fn png_of(px: [u8; 4], w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(px));
        let mut buf = Vec::new();
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(&img, w, h, image::ExtendedColorType::Rgba8)
            .unwrap();
        buf
    }

    fn zip_with(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            for (name, data) in entries {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                use std::io::Write;
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        buf
    }

    fn procreate_archive(entries: &[(&str, plist::Value)]) -> Vec<u8> {
        let mut root = plist::Dictionary::new();
        for (key, value) in entries {
            root.insert((*key).to_string(), value.clone());
        }
        let mut top = plist::Dictionary::new();
        top.insert("root".to_string(), plist::Value::Uid(plist::Uid::new(1)));
        let mut archive = plist::Dictionary::new();
        archive.insert(
            "$archiver".to_string(),
            plist::Value::String("NSKeyedArchiver".to_string()),
        );
        archive.insert("$version".to_string(), plist::Value::from(100000u64));
        archive.insert("$top".to_string(), plist::Value::Dictionary(top));
        archive.insert(
            "$objects".to_string(),
            plist::Value::Array(vec![
                plist::Value::String("$null".to_string()),
                plist::Value::Dictionary(root),
            ]),
        );
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, &plist::Value::Dictionary(archive)).unwrap();
        bytes
    }

    fn png_black_center() -> Vec<u8> {
        let mut img = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 255, 255, 255]));
        for y in 4..12 {
            for x in 4..12 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let mut buf = Vec::new();
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(&img, 16, 16, image::ExtendedColorType::Rgba8)
            .unwrap();
        buf
    }

    #[test]
    fn procreate_shape_uses_archive_size_and_inversion() {
        let archive = procreate_archive(&[
            ("name", plist::Value::String("Ink".to_string())),
            ("paintSize", plist::Value::Real(0.25)),
            ("dynamicsSpeedSize", plist::Value::Real(0.5)),
            ("dynamicsSpeedOpacity", plist::Value::Real(0.25)),
            ("shapeInverted", plist::Value::Boolean(true)),
        ]);
        let bytes = zip_with(&[
            ("Ink/Shape.png", png_black_center()),
            ("Ink/Brush.archive", archive),
        ]);
        let list = import_procreate(&bytes, "Set").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Ink");
        assert!((list[0].brush.size - 64.0).abs() < 0.01);
        assert!((list[0].brush.velocity_size - 0.5).abs() < 1e-6);
        assert!((list[0].brush.velocity_opacity - 0.25).abs() < 1e-6);
        let tip = list[0].brush.tip.as_ref().unwrap();
        assert_eq!((tip.w, tip.h), (16, 16));
        assert!(tip.mask[8 * 16 + 8] > 240);
        assert!(tip.mask[0] < 15);
    }

    #[test]
    fn procreate_archive_without_shape_uses_round_tip() {
        let archive = procreate_archive(&[
            ("name", plist::Value::String("Pencil".to_string())),
            ("paintSize", plist::Value::Real(0.25)),
            ("grainDepth", plist::Value::Real(1.0)),
        ]);
        let bytes = zip_with(&[
            ("Pencil/Brush.archive", archive.clone()),
            ("Pencil/Reset/Brush.archive", archive),
        ]);
        let list = import_procreate(&bytes, "Set").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Pencil");
        assert!((list[0].brush.size - 64.0).abs() < 0.01);
        assert_eq!(list[0].brush.grain, 0.0);
        assert!(list[0].brush.grain_tip.is_none());
        let tip = list[0].brush.tip.as_ref().unwrap();
        assert_eq!(
            (tip.w, tip.h),
            (DEFAULT_PROCREATE_TIP_SIDE, DEFAULT_PROCREATE_TIP_SIDE)
        );
        let center = (tip.w * tip.h / 2 + tip.w / 2) as usize;
        assert!(tip.mask[center] > 240);
        assert!(tip.mask[0] < 15);
    }

    #[test]
    fn procreate_grain_depth_requires_grain_texture() {
        let archive = procreate_archive(&[
            ("name", plist::Value::String("Chalk".to_string())),
            ("grainDepth", plist::Value::Real(0.5)),
        ]);
        let bytes = zip_with(&[
            ("Chalk/Shape.png", png_black_center()),
            ("Chalk/Grain.png", png_black_center()),
            ("Chalk/Brush.archive", archive),
        ]);
        let list = import_procreate(&bytes, "Set").unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].brush.grain_tip.is_some());
        assert!((list[0].brush.grain - 0.5).abs() < 1e-6);
    }

    #[test]
    fn procreate_brush_and_set_import() {
        let shape = png_of([255, 255, 255, 255], 6, 6);
        // Single .brush: Shape.png at root takes the file stem.
        let bytes = zip_with(&[("Shape.png", shape.clone()), ("Grain.png", shape.clone())]);
        let list = import_procreate(&bytes, "CoolBrush").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "CoolBrush");
        // .brushset: one folder per brush.
        let bytes = zip_with(&[
            ("Ink/Shape.png", shape.clone()),
            ("Ink/Grain.png", shape.clone()),
            ("Chalk/Shape.png", shape.clone()),
        ]);
        let list = import_procreate(&bytes, "Set").unwrap();
        assert_eq!(list.len(), 2);
        let mut names: Vec<String> = list.iter().map(|b| b.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["Chalk".to_string(), "Ink".to_string()]);
        let ink = list.iter().find(|b| b.name == "Ink").unwrap();
        assert!(ink.brush.grain_tip.is_some());
        // No shape at all.
        let bytes = zip_with(&[("Grain.png", shape)]);
        assert!(import_procreate(&bytes, "Empty").is_err());
        assert!(import_procreate(b"not a zip", "x").is_err());
    }

    #[test]
    fn procreate_ignores_mac_metadata_and_duplicate_shapes() {
        let shape = png_of([255, 255, 255, 255], 6, 6);
        let bytes = zip_with(&[
            ("Ink/Shape.png", shape.clone()),
            ("Ink/Shape.PNG", shape.clone()),
            ("__MACOSX/Ink/Shape.png", shape.clone()),
            ("__MACOSX/Ink/._Shape.png", shape.clone()),
            (".DS_Store", b"junk".to_vec()),
        ]);
        let list = import_procreate(&bytes, "Set").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Ink");
    }

    fn kpp_bytes(xml: &str) -> Vec<u8> {
        // Minimal PNG: signature + IHDR + zTXt(preset) + IEND. Pixel data is
        // never decoded, so no IDAT is needed.
        let mut out = Vec::new();
        out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut chunk = |kind: &[u8], data: &[u8]| {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&0u32.to_be_bytes()); // CRC unchecked
        };
        let ihdr = [0u8, 0, 0, 200, 0, 0, 0, 200, 8, 6, 0, 0, 0];
        chunk(b"IHDR", &ihdr);
        let mut z = b"preset\0\x00".to_vec();
        {
            use std::io::Write;
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(xml.as_bytes()).unwrap();
            z.extend_from_slice(&enc.finish().unwrap());
        }
        chunk(b"zTXt", &z);
        chunk(b"IEND", b"");
        out
    }

    #[test]
    fn kpp_parametric_import() {
        // Schema-faithful XML: capital Preset root, CDATA param bodies,
        // MaskGenerator diameter for size, Brush spacing.
        let xml = r#"<Preset paintopid="paintbrush" name="Round-ish"><param type="string" name="OpacityValue"><![CDATA[0.7]]></param><param type="string" name="FlowValue"><![CDATA[0.5]]></param><Brush spacing="0.3" type="auto_brush"><MaskGenerator diameter="42" type="circle"/></Brush></Preset>"#;
        let list = import_kpp_bytes(&kpp_bytes(xml), "file-stem", None).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Round-ish");
        assert_eq!(list[0].brush.size, 42.0);
        assert!((list[0].brush.opacity - 0.35).abs() < 0.001);
        assert!((list[0].brush.spacing - 0.3).abs() < 0.001);
        assert!(list[0].brush.tip.is_none());
        // Missing everything: stem name + defaults.
        let list = import_kpp_bytes(&kpp_bytes("<Preset></Preset>"), "Fallback", None).unwrap();
        assert_eq!(list[0].name, "Fallback");
        assert_eq!(list[0].brush.size, 24.0);
        // Real bundle layout: zip of .kpp files.
        let kpp = kpp_bytes(xml);
        let bundle = zip_with(&[
            ("paintoppresets/A.kpp", kpp.clone()),
            ("preview.png", vec![1, 2, 3]),
        ]);
        let list = import_krita_bundle(&bundle).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Round-ish");
        // Not a PNG at all.
        assert!(import_kpp_bytes(b"hello", "x", None).is_err());
        // PNG without a preset chunk.
        let mut bare = Vec::new();
        bare.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        assert!(import_kpp_bytes(&bare, "x", None).is_err());
    }

    #[test]
    fn krita_bundle_reports_skipped_presets() {
        let xml = r#"<Preset paintopid="paintbrush" name="Good"><Brush spacing="0.2" type="auto_brush"/></Preset>"#;
        let bundle = zip_with(&[
            ("paintoppresets/Good.kpp", kpp_bytes(xml)),
            ("paintoppresets/Bad.kpp", b"not a preset".to_vec()),
        ]);
        let outcome = import_krita_bundle_report(&bundle).unwrap();
        assert_eq!(outcome.brushes.len(), 1);
        assert_eq!(outcome.skipped, 1);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.contains("Bad") && w.contains("Skipped")),
            "warnings: {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn krita_bundle_resolves_external_tip_files() {
        let xml = r#"<Preset name="Textured"><Brush spacing="0.2" type="predefined_brush" filename="brushes/soft.png"><MaskGenerator diameter="20"/></Brush></Preset>"#;
        let tip = png_of([255, 255, 255, 255], 4, 4);
        let bundle = zip_with(&[
            ("paintoppresets/Textured.kpp", kpp_bytes(xml)),
            ("paintoppresets/brushes/soft.png", tip),
        ]);
        let list = import_krita_bundle(&bundle).unwrap();
        assert_eq!(list.len(), 1);
        let tip = list[0].brush.tip.as_ref().expect("external tip");
        assert_eq!((tip.w, tip.h), (4, 4));
    }

    #[test]
    fn kpp_embedded_tip_import() {
        use base64::Engine;
        // 48x48 gradient PNG embedded as base64 CDATA in the XML.
        let mut img = image::RgbaImage::new(48, 48);
        for y in 0..48 {
            for x in 0..48 {
                let v = ((x * y) % 256) as u8;
                img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        let mut tip_png = Vec::new();
        {
            use image::ImageEncoder;
            image::codecs::png::PngEncoder::new(&mut tip_png)
                .write_image(&img, 48, 48, image::ExtendedColorType::Rgba8)
                .unwrap();
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&tip_png);
        assert!(b64.len() >= 512);
        let xml = format!("<Preset name=\"Textured\"><Brush spacing=\"0.2\" type=\"predefined_brush\"/><param type=\"string\" name=\"Tip\"><![CDATA[{b64}]]></param></Preset>");
        let list = import_kpp_bytes(&kpp_bytes(&xml), "stem", None).unwrap();
        assert_eq!(list[0].name, "Textured");
        let tip = list[0].brush.tip.as_ref().expect("embedded tip");
        assert_eq!((tip.w, tip.h), (48, 48));
        assert!(tip.mask.iter().any(|&b| b > 0));
    }

    /// Real v6 layout: u32 len | 38-byte `$`-UUID | 10-byte pad (subv1) |
    /// bounds | u16 depth | u8 compression | data. Entries 4-aligned,
    /// the samp block even-aligned.
    type AbrTip = (Vec<u8>, u32, u32, u16, u8, Vec<u8>);
    fn abr_file(tips: &[AbrTip]) -> Vec<u8> {
        let mut samp = Vec::new();
        for (uuid, w, h, depth, compression, data) in tips {
            let mut item = Vec::new();
            let mut id = vec![b'$'];
            id.extend_from_slice(uuid);
            id.push(0);
            assert_eq!(id.len(), 38);
            item.extend_from_slice(&id);
            item.extend_from_slice(&[0u8; 10]); // subv1 padding
            item.extend_from_slice(&0i32.to_be_bytes()); // top
            item.extend_from_slice(&0i32.to_be_bytes()); // left
            item.extend_from_slice(&(*h as i32).to_be_bytes()); // bottom
            item.extend_from_slice(&(*w as i32).to_be_bytes()); // right
            item.extend_from_slice(&depth.to_be_bytes());
            item.push(*compression);
            item.extend_from_slice(data);
            samp.extend_from_slice(&(item.len() as u32).to_be_bytes());
            samp.extend_from_slice(&item);
            samp.extend_from_slice(&vec![0u8; (4 - item.len() % 4) % 4]);
        }
        abr_wrap(&samp)
    }

    /// v10 subv2 layout: 4-byte length | 38-byte ID | 263-byte pad |
    /// bounds | depth | compression | data.
    fn abr_file_v2(tips: &[AbrTip]) -> Vec<u8> {
        let mut samp = Vec::new();
        for (uuid, w, h, depth, compression, data) in tips {
            let mut item = Vec::new();
            let mut id = vec![b'$'];
            id.extend_from_slice(uuid);
            id.push(0);
            assert_eq!(id.len(), 38);
            item.extend_from_slice(&id);
            item.extend_from_slice(&[0u8; 263]);
            item.extend_from_slice(&0i32.to_be_bytes());
            item.extend_from_slice(&0i32.to_be_bytes());
            item.extend_from_slice(&(*h as i32).to_be_bytes());
            item.extend_from_slice(&(*w as i32).to_be_bytes());
            item.extend_from_slice(&depth.to_be_bytes());
            item.push(*compression);
            item.extend_from_slice(data);
            samp.extend_from_slice(&(item.len() as u32).to_be_bytes());
            samp.extend_from_slice(&item);
            samp.extend_from_slice(&vec![0u8; (4 - item.len() % 4) % 4]);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&10u16.to_be_bytes());
        out.extend_from_slice(&2u16.to_be_bytes());
        out.extend_from_slice(b"8BIM");
        out.extend_from_slice(b"samp");
        out.extend_from_slice(&(samp.len() as u32).to_be_bytes());
        out.extend_from_slice(&samp);
        if samp.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn abr_wrap(samp: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&6u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(b"8BIM");
        out.extend_from_slice(b"samp");
        out.extend_from_slice(&(samp.len() as u32).to_be_bytes());
        out.extend_from_slice(samp);
        if samp.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn desc_text_payload(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let units = (s.chars().count() + 1) as u32;
        out.extend_from_slice(&units.to_be_bytes());
        for u in s.encode_utf16() {
            out.extend_from_slice(&u.to_be_bytes());
        }
        out.extend_from_slice(&0u16.to_be_bytes());
        out
    }

    fn desc_untf_payload(unit: &[u8; 4], v: f64) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(unit);
        out.extend_from_slice(&v.to_be_bytes());
        out
    }

    /// Full v6 file: one 8-bit tip + desc entry naming it with spacing 50%
    /// and diameter 33px.
    fn abr_file_with_desc() -> Vec<u8> {
        let uuid = b"3479c62f-65c9-11de-bdeb-a55e96b1a876";
        let mut samp = Vec::new();
        let mut item = Vec::new();
        let mut id = vec![b'$'];
        id.extend_from_slice(uuid);
        id.push(0);
        item.extend_from_slice(&id);
        item.extend_from_slice(&[0u8; 10]);
        item.extend_from_slice(&0i32.to_be_bytes());
        item.extend_from_slice(&0i32.to_be_bytes());
        item.extend_from_slice(&2i32.to_be_bytes());
        item.extend_from_slice(&3i32.to_be_bytes());
        item.extend_from_slice(&8u16.to_be_bytes());
        item.push(0u8);
        item.extend_from_slice(&[200; 6]);
        samp.extend_from_slice(&(item.len() as u32).to_be_bytes());
        samp.extend_from_slice(&item);
        samp.extend_from_slice(&vec![0u8; (4 - item.len() % 4) % 4]);
        let mut desc_entry = Vec::new();
        desc_entry.extend_from_slice(b"Nm  ");
        desc_entry.extend_from_slice(b"TEXT");
        desc_entry.extend_from_slice(&desc_text_payload("Soft Round"));
        desc_entry.extend_from_slice(b"sampledData");
        desc_entry.extend_from_slice(b"TEXT");
        desc_entry.extend_from_slice(&desc_text_payload("3479c62f-65c9-11de-bdeb-a55e96b1a876"));
        desc_entry.extend_from_slice(b"Spcn");
        desc_entry.extend_from_slice(b"UntF");
        desc_entry.extend_from_slice(&desc_untf_payload(b"#Prc", 50.0));
        desc_entry.extend_from_slice(b"Dmtr");
        desc_entry.extend_from_slice(b"UntF");
        desc_entry.extend_from_slice(&desc_untf_payload(b"#Pxl", 33.0));
        let mut desc = Vec::new();
        desc.extend_from_slice(&1u32.to_be_bytes());
        desc.extend_from_slice(&(desc_entry.len() as u32).to_be_bytes());
        desc.extend_from_slice(&desc_entry);
        // samp block + desc block.
        let mut out = Vec::new();
        out.extend_from_slice(&6u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        for (key, data) in [(b"samp", samp), (b"desc", desc)] {
            out.extend_from_slice(b"8BIM");
            out.extend_from_slice(key);
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(&data);
            if data.len() % 2 == 1 {
                out.push(0);
            }
        }
        out
    }

    fn packbits_row(raw: &[u8]) -> Vec<u8> {
        // Single run (tests use flat rows).
        vec![(257 - raw.len()) as u8, raw[0]]
    }

    #[test]
    fn abr_v6_tip_import() {
        let uuid = b"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_vec();
        // 3x2 8-bit tip, mid-gray.
        let bytes = abr_file(&[(uuid.clone(), 3, 2, 8, 0, vec![200; 6])]);
        let list = import_abr(&bytes).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "ABR Brush 1");
        let tip = list[0].brush.tip.as_ref().unwrap();
        assert_eq!((tip.w, tip.h), (3, 2));
        assert_eq!(tip.mask, vec![200; 6]);
        assert_eq!(list[0].brush.size, 3.0);
        // 16-bit raw scales to 8-bit.
        let mut data16 = Vec::new();
        for v in [0xffffu16, 0x0000u16, 0x8080u16, 0x4040u16] {
            data16.extend_from_slice(&v.to_be_bytes());
        }
        let bytes = abr_file(&[(uuid.clone(), 2, 2, 16, 0, data16)]);
        let list = import_abr(&bytes).unwrap();
        assert_eq!(
            list[0].brush.tip.as_ref().unwrap().mask,
            vec![255, 0, 128, 64]
        );
        // RLE: two flat rows.
        let mut data = Vec::new();
        data.extend_from_slice(&2u16.to_be_bytes());
        data.extend_from_slice(&2u16.to_be_bytes());
        data.extend_from_slice(&packbits_row(&[10, 10, 10]));
        data.extend_from_slice(&packbits_row(&[20, 20, 20]));
        let bytes = abr_file(&[(uuid.clone(), 3, 2, 8, 1, data)]);
        let list = import_abr(&bytes).unwrap();
        assert_eq!(
            list[0].brush.tip.as_ref().unwrap().mask,
            vec![10, 10, 10, 20, 20, 20]
        );
        // Descriptor names + spacing + diameter via UUID linkage.
        let list = import_abr(&abr_file_with_desc()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Soft Round");
        assert_eq!(list[0].brush.size, 33.0);
        assert!((list[0].brush.spacing - 0.5).abs() < 0.001);
        // Old version rejected with guidance.
        let mut old = bytes.clone();
        old[0..2].copy_from_slice(&2u16.to_be_bytes());
        let err = import_abr(&old).unwrap_err();
        assert!(err.contains("version 2"), "{err}");
        // Empty samp block.
        let mut out = Vec::new();
        out.extend_from_slice(&6u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(b"8BIM");
        out.extend_from_slice(b"samp");
        out.extend_from_slice(&0u32.to_be_bytes());
        assert!(import_abr(&out).is_err());
    }

    #[test]
    fn abr_allows_unpadded_final_block() {
        let uuid = b"cccccccc-dddd-eeee-ffff-000000000000".to_vec();
        let mut bytes = abr_file(&[(uuid, 2, 2, 8, 0, vec![128; 4])]);
        bytes.extend_from_slice(b"8BIM");
        bytes.extend_from_slice(b"phry");
        bytes.extend_from_slice(&3u32.to_be_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        let list = import_abr(&bytes).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].brush.tip.as_ref().unwrap().mask, vec![128; 4]);
    }

    #[test]
    fn abr_v10_large_tip_downscales_instead_of_erroring() {
        let uuid = b"bbbbbbbb-cccc-dddd-eeee-ffffffffffff".to_vec();
        let bytes = abr_file_v2(&[(uuid, 600, 600, 8, 0, vec![200; 360_000])]);
        let list = import_abr(&bytes).unwrap();
        let tip = list[0].brush.tip.as_ref().unwrap();
        assert_eq!((tip.w, tip.h), (256, 256));
        assert!(tip.mask.iter().all(|&b| b == 200));
    }

    #[test]
    fn abr_without_tips_reports_parametric_fallback() {
        let full = abr_file_with_desc();
        let desc_pos = full.windows(4).position(|w| w == b"desc").unwrap();
        let desc_len =
            u32::from_be_bytes(full[desc_pos + 4..desc_pos + 8].try_into().unwrap()) as usize;
        let desc = &full[desc_pos + 8..desc_pos + 8 + desc_len];
        let mut out = Vec::new();
        out.extend_from_slice(&6u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(b"8BIM");
        out.extend_from_slice(b"desc");
        out.extend_from_slice(&(desc.len() as u32).to_be_bytes());
        out.extend_from_slice(desc);
        if desc.len() % 2 == 1 {
            out.push(0);
        }
        let outcome = import_abr_report(&out).unwrap();
        assert_eq!(outcome.brushes.len(), 1);
        assert_eq!(outcome.brushes[0].name, "Soft Round");
        assert_eq!(outcome.brushes[0].brush.size, 33.0);
        assert!((outcome.brushes[0].brush.spacing - 0.5).abs() < 0.001);
        assert!(outcome.brushes[0].brush.tip.is_none());
        assert!(
            outcome.warnings.iter().any(|w| w.contains("parametric")),
            "warnings: {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn imported_size_normalization_preserves_ratios() {
        let mut brushes: Vec<ImportedBrush> = [600.0, 300.0, 100.0]
            .into_iter()
            .map(|size| ImportedBrush {
                name: format!("Brush {size}"),
                brush: Brush {
                    size,
                    ..Brush::default()
                },
            })
            .collect();
        normalize_imported_sizes(&mut brushes);
        assert!((brushes[0].brush.size - 64.0).abs() < 1e-4);
        assert!((brushes[1].brush.size - 32.0).abs() < 1e-4);
        assert!((brushes[2].brush.size - (64.0 / 6.0)).abs() < 1e-4);

        let mut small = vec![
            ImportedBrush {
                name: "Small".to_string(),
                brush: Brush {
                    size: 50.0,
                    ..Brush::default()
                },
            },
            ImportedBrush {
                name: "Tiny".to_string(),
                brush: Brush {
                    size: 10.0,
                    ..Brush::default()
                },
            },
        ];
        normalize_imported_sizes(&mut small);
        assert_eq!(small[0].brush.size, 50.0);
        assert_eq!(small[1].brush.size, 10.0);
    }

    #[test]
    fn external_imports_are_capped_but_pcbrush_keeps_its_size() {
        let dir = std::env::temp_dir();
        let brushset = dir.join("procraft-size-normalize.brushset");
        let bytes = zip_with(&[(
            "Big/Brush.archive",
            procreate_archive(&[
                ("name", plist::Value::String("Big".to_string())),
                ("paintSize", plist::Value::Real(0.5)),
            ]),
        )]);
        std::fs::write(&brushset, bytes).unwrap();
        let outcome = import_brush_file_report(&brushset).unwrap();
        let _ = std::fs::remove_file(&brushset);
        assert_eq!(outcome.brushes.len(), 1);
        assert!((outcome.brushes[0].brush.size - 64.0).abs() < 0.01);

        let pcbrush = dir.join("procraft-size-roundtrip.pcbrush");
        let libs = vec![BrushPreset {
            name: "Big".to_string(),
            brush: Brush {
                size: 300.0,
                ..Brush::default()
            },
        }];
        super::super::brush::save_brush_library(&pcbrush, &libs).unwrap();
        let outcome = import_brush_file_report(&pcbrush).unwrap();
        let _ = std::fs::remove_file(&pcbrush);
        assert_eq!(outcome.brushes.len(), 1);
        assert!((outcome.brushes[0].brush.size - 300.0).abs() < 0.01);
    }

    #[test]
    fn dispatch_rejects_unknown_types() {
        let dir = std::env::temp_dir();
        let path = dir.join("procraft-brush-test.xyz");
        std::fs::write(&path, b"data").unwrap();
        assert!(import_brush_file(&path).is_err());
        let _ = std::fs::remove_file(&path);
        let empty = dir.join("procraft-brush-empty.png");
        std::fs::write(&empty, b"").unwrap();
        assert!(import_brush_file(&empty).is_err());
        let _ = std::fs::remove_file(&empty);
    }
}
