//! File I/O: image export, background import and project files.

use super::*;

/// Write one raster as a v12 tile table: `u32 count`, then per listed
/// tile `u16 tx, u16 ty, u32 len, len bytes` of zlib-compressed raw tile
/// bytes. Unallocated tiles are omitted, and so are allocated-but-all-zero
/// tiles: an eraser flow zeroes an allocated tile without freeing the slot,
/// so it is visually empty and would otherwise cost its payload on save
/// plus a fresh 256 KiB (64 KiB for masks) on load. A sparse 8K layer
/// therefore saves only tiles that actually hold content.
///
/// The count precedes the records, so it is reserved as a placeholder and
/// patched afterwards: that keeps the write a single streaming pass over
/// every tile (the deflate already touches the bytes) instead of buffering
/// a whole layer's compressed payloads in memory.
fn write_tile_table<W: std::io::Write + std::io::Seek>(
    f: &mut W,
    raster: &Raster,
) -> Result<(), String> {
    use std::io::{SeekFrom, Write};
    let count_at = f.stream_position().map_err(|e| e.to_string())?;
    f.write_all(&0u32.to_le_bytes())
        .map_err(|e| e.to_string())?;
    let mut count = 0u32;
    for (tx, ty, tile) in raster.allocated_tiles() {
        // One scan decides both omission and whether to deflate.
        if !tile.iter().any(|&b| b != 0) {
            continue;
        }
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(tile).map_err(|e| e.to_string())?;
        let packed = enc.finish().map_err(|e| e.to_string())?;
        f.write_all(&(tx as u16).to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.write_all(&(ty as u16).to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.write_all(&(packed.len() as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        f.write_all(&packed).map_err(|e| e.to_string())?;
        count += 1;
    }
    let end = f.stream_position().map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(count_at))
        .map_err(|e| e.to_string())?;
    f.write_all(&count.to_le_bytes())
        .map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(end)).map_err(|e| e.to_string())?;
    Ok(())
}

/// Read a v12 tile table into a fresh raster. `take` hands out the file
/// bytes and fails closed on truncation. Every record is validated against
/// the canvas grid, a sane payload length and the exact tile size, so a
/// corrupt table returns `Err` instead of loading wrong pixels.
fn read_tile_table<'a>(
    take: &mut impl FnMut(usize) -> Result<&'a [u8], String>,
    w: u32,
    h: u32,
    bpp: u8,
) -> Result<Raster, String> {
    use std::io::Read;
    let mut raster = Raster::new(w, h, bpp);
    let slots = raster.tiles_x() as u64 * raster.tiles_y() as u64;
    let count = u32::from_le_bytes(
        take(4)?[..4]
            .try_into()
            .map_err(|_| "bad tile count".to_string())?,
    );
    if count as u64 > slots {
        return Err("project has bad tile count".to_string());
    }
    let tile_len = raster.tile_len();
    for _ in 0..count {
        let tx = u16::from_le_bytes(
            take(2)?[..2]
                .try_into()
                .map_err(|_| "bad tile x".to_string())?,
        ) as u32;
        let ty = u16::from_le_bytes(
            take(2)?[..2]
                .try_into()
                .map_err(|_| "bad tile y".to_string())?,
        ) as u32;
        if tx >= raster.tiles_x() || ty >= raster.tiles_y() {
            return Err("project has out-of-range tile".to_string());
        }
        let len = u32::from_le_bytes(
            take(4)?[..4]
                .try_into()
                .map_err(|_| "bad tile length".to_string())?,
        ) as usize;
        if len == 0 || len > tile_len + 10 * 1024 * 1024 {
            return Err("project has bad tile length".to_string());
        }
        let packed = take(len)?;
        // Bound the inflate at the tile size so a bomb cannot balloon the
        // read; a short or long result is a corrupt tile.
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(packed)
            .take(tile_len as u64 + 1)
            .read_to_end(&mut raw)
            .map_err(|_| "project has corrupt tile".to_string())?;
        if raw.len() != tile_len {
            return Err("project has corrupt tile".to_string());
        }
        raster.tile_mut(tx, ty).copy_from_slice(&raw);
    }
    Ok(raster)
}

impl Document {
    /// Save the composite as PNG.
    pub fn save_png(&self, path: &std::path::Path) -> Result<(), String> {
        let flat = self.try_flatten()?;
        image::save_buffer(path, &flat, self.w, self.h, image::ColorType::Rgba8)
            .map_err(|e| e.to_string())
    }

    /// Save the composite as JPEG at `quality` (1-100, higher keeps more
    /// detail). JPEG stores no alpha; the composite is already opaque
    /// over the background, so this only strips the alpha channel.
    pub fn save_jpeg(&self, path: &std::path::Path, quality: u8) -> Result<(), String> {
        if !(1..=100).contains(&quality) {
            return Err(format!("bad JPEG quality: {quality} (want 1-100)"));
        }
        let flat = self.try_flatten()?;
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
        let flat = self.try_flatten()?;
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
    /// dimensions are checked from the header before the full decode.
    /// That bounds the encoded input, not peak memory: at the
    /// [`MAX_CANVAS_SIDE`] cap a decode can still hold ~268 MB dense
    /// plus ~256 MB of tiles, so the guard stops absurd files rather
    /// than promising the session cannot OOM. The cap matches
    /// `rehome_canvas`.
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
    /// to a `.procraft` file: magic + little-endian records. Version 12
    /// stores per-tile zlib payloads (only allocated 256² tiles are
    /// written), so sparse 8K documents stay small; older readers cleanly
    /// refuse v12 files at the magic. Versions 1-11 stay readable.
    pub fn save_project(&self, path: &std::path::Path) -> Result<(), String> {
        let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
        self.write_project(&mut f)
    }

    /// Encode the project into memory instead of a file. Deflating an 8K
    /// document's tiles takes ~600 ms, so autosave runs this on a worker
    /// thread (the `Document` clone it reads shares every tile through
    /// `Arc`) and the UI thread writes the short result.
    pub fn save_project_bytes(&self) -> Result<Vec<u8>, String> {
        let mut out = std::io::Cursor::new(Vec::new());
        self.write_project(&mut out)?;
        Ok(out.into_inner())
    }

    fn write_project(&self, f: &mut (impl std::io::Write + std::io::Seek)) -> Result<(), String> {
        f.write_all(b"PCRAFT12").map_err(|e| e.to_string())?;
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
            // v12 writes only the tiles that hold content; a dense layer
            // with assigned tiles serializes as a tile table.
            if layer.pixels.w() != self.w || layer.pixels.h() != self.h || layer.pixels.bpp() != 4 {
                return Err("layer has wrong pixel size".to_string());
            }
            write_tile_table(&mut *f, &layer.pixels)?;
            // Optional reveal mask, one byte per pixel, same tile table.
            match layer.mask.as_ref() {
                Some(m) if m.w() == self.w && m.h() == self.h && m.bpp() == 1 => {
                    f.write_all(&[1u8]).map_err(|e| e.to_string())?;
                    write_tile_table(&mut *f, m)?;
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
        let v12 = magic == b"PCRAFT12";
        let v11 = v12 || magic == b"PCRAFT11";
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
            // v12 stores a tile table; v1-v11 store one dense zlib payload,
            // which `Raster::from_dense` splits into tiles on load (all-zero
            // tiles stay unallocated, so a resave flattens identically).
            let pixels = if v12 {
                read_tile_table(&mut take, w, h, 4)?
            } else {
                let raw = if v4 {
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
                Raster::from_dense(w, h, 4, &raw)
            };
            // v5 appends an optional reveal mask per layer: dense in v5-v11,
            // a tile table in v12.
            let mask = if v5 {
                let flag = take(1)?[0];
                if flag == 0 {
                    None
                } else if v12 {
                    Some(read_tile_table(&mut take, w, h, 1)?)
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
                    Some(Raster::from_dense(w, h, 1, &raw))
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
                pixels,
                mask,
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
            stroke_peak: None,
            stroke_rect: None,
            liquify: None,
        })
    }
}
