use crate::*;

/// Longest side kept for a pinned reference. The pin is never painted as
/// canvas content, so a phone photo is shrunk before it reaches the UI.
pub(crate) const REFERENCE_MAX_SIDE: u32 = 1024;

impl ProCraftApp {
    /// Best-effort delete of the autosave recovery file (after an explicit
    /// save, load, or clean exit — anything that makes it stale). Drops any
    /// encode in flight so it cannot land after the delete.
    pub(crate) fn clear_recovery(&mut self) {
        self.autosave_job = None;
        let _ = std::fs::remove_file(&self.recovery_path);
    }

    /// Draw the eyedropper loupe for canvas point `(cx, cy)` (already
    /// clamped): magnified composite ring parked above the pointer (below
    /// near the top edge, shifted inside near the sides) with the
    /// would-be picked color read out.
    pub(crate) fn paint_picker_loupe(
        &self,
        painter: &egui::Painter,
        ctx: &egui::Context,
        pos: egui::Pos2,
        cx: f32,
        cy: f32,
    ) {
        const SIDE: u32 = 11;
        const CELL: f32 = 12.0;
        let size = SIDE as f32 * CELL;
        let (cells, ci, cj) = loupe_cells(&self.doc, cx as u32, cy as u32, SIDE);
        let center = cells[(cj * SIDE + ci) as usize];
        let screen = ctx.viewport_rect();
        let mut at = egui::Pos2::new(pos.x + 24.0, pos.y - size - 48.0);
        if at.y < screen.min.y + 8.0 {
            at.y = pos.y + 28.0;
        }
        if at.x + size + 12.0 > screen.max.x - 8.0 {
            at.x = screen.max.x - 8.0 - size - 12.0;
        }
        if at.x < screen.min.x + 8.0 {
            at.x = screen.min.x + 8.0;
        }
        paint_loupe(
            painter,
            &cells,
            SIDE,
            (ci, cj),
            at,
            CELL,
            [center[0], center[1], center[2]],
        );
    }

    /// Reset view-side state after the canvas changes size (crop, canvas
    /// resize): the texture rebuilds at the new dims, ghosts and mask
    /// edges recompute, transient drags die, and the size panel re-adopts
    /// the real dimensions.
    pub(crate) fn after_canvas_change(&mut self) {
        self.texture = None;
        self.composite.invalidate();
        self.ghost_dirty = true;
        self.sel_edges_dirty = true;
        self.lasso_poly.clear();
        self.crop_rect = None;
        self.crop_anchor = None;
        self.crop_current = None;
        self.move_from = None;
        self.liquify_from = None;
        self.doc.liquify_end();
        // M48 pins are canvas-sized state: a new/resized canvas
        // invalidates them (they were seeded at the old dimensions).
        self.xform = XformState::off();
        // Same for the Transform-layer button session base.
        self.tform_press = TformPress::new();
        self.canvas_sync = (self.doc.w, self.doc.h);
        self.canvas_w = self.doc.w;
        self.canvas_h = self.doc.h;
    }

    /// M33 Gallery: adopt an opened/saved/duplicated project — one place
    /// for the reset + recent + status so every entry path behaves alike.
    pub(crate) fn adopt_project(&mut self, doc: Document, path: &str, verb: &str) {
        self.doc = doc;
        self.clear_recovery();
        push_recent(&mut self.recent, path);
        self.after_canvas_change();
        self.gallery_keys.clear();
        self.status = format!("{verb} {path}");
    }

    /// M33 Gallery: start a blank canvas at `w` x `h` (clamped to the
    /// engine cap). No recent entry until it is saved somewhere.
    pub(crate) fn new_canvas(&mut self, w: u32, h: u32) {
        let w = w.clamp(1, MAX_CANVAS_SIDE);
        let h = h.clamp(1, MAX_CANVAS_SIDE);
        self.doc = Document::new(w, h);
        self.clear_recovery();
        self.after_canvas_change();
        self.status = format!("New canvas {w} x {h}. Time-lapse records under Animation.");
    }

    /// M33 Gallery: rebuild cached thumbnails when the recent list
    /// changed. Each path is opened, flattened, and downscaled; missing
    /// or corrupt files yield `None` (placeholder row, kept so the user
    /// can Remove it). Runs on demand, not per frame.
    pub(crate) fn refresh_gallery_thumbs(&mut self, ctx: &egui::Context) {
        if self.gallery_keys == self.recent {
            return;
        }
        self.gallery_thumbs.clear();
        for (idx, path) in self.recent.iter().enumerate() {
            let tex = Document::open_project(std::path::Path::new(path))
                .ok()
                .and_then(|doc| {
                    let (bytes, tw, th) = sampled_thumb(&doc, GALLERY_THUMB_SIDE);
                    if bytes.is_empty() {
                        return None;
                    }
                    let img = ColorImage::from_rgba_unmultiplied([tw, th], &bytes);
                    Some(ctx.load_texture(format!("gallery-{idx}"), img, TextureOptions::LINEAR))
                });
            self.gallery_thumbs.push(tex);
        }
        self.gallery_keys = self.recent.clone();
    }

    /// M33 Gallery: copy `recent[idx]` to a sibling ` copyN.procraft`
    /// file and adopt it. Errors surface in the status line.
    pub(crate) fn duplicate_recent(&mut self, idx: usize) {
        let Some(src) = self.recent.get(idx).cloned() else {
            return;
        };
        let src_path = std::path::PathBuf::from(&src);
        let doc = match Document::open_project(&src_path) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Duplicate failed to open {src}: {e}");
                return;
            }
        };
        let stem = src_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("artwork");
        // Strip an older " copyN" tail so re-duplicates don't stack names.
        let base = stem.rsplit_once(" copy").map(|(b, _)| b).unwrap_or(stem);
        let mut dest = src_path.clone();
        let mut n = 1u32;
        loop {
            let name = if n == 1 {
                format!("{base} copy.procraft")
            } else {
                format!("{base} copy{n}.procraft")
            };
            dest.set_file_name(&name);
            if !dest.exists() {
                break;
            }
            n += 1;
            if n > 999 {
                self.status = "Duplicate failed: too many copies.".to_string();
                return;
            }
        }
        match doc.save_project(&dest) {
            Ok(()) => {
                let s = dest.to_string_lossy().to_string();
                self.adopt_project(doc, &s, "Duplicated");
            }
            Err(e) => self.status = format!("Duplicate save failed: {e}"),
        }
    }

    /// M34 Reference: pin RGBA bytes as the companion image (session-only;
    /// never touches the canvas or undo). Oversized images are downscaled
    /// through the Gallery helper so a phone photo can't OOM the session.
    pub(crate) fn set_reference(&mut self, pixels: Vec<u8>, w: u32, h: u32, from: &str) {
        if pixels.is_empty() || w == 0 || h == 0 {
            self.status = format!("Reference load failed: empty image ({from}).");
            return;
        }
        let (bytes, tw, th) = if w.max(h) > REFERENCE_MAX_SIDE {
            gallery_thumb(&pixels, w, h, REFERENCE_MAX_SIDE)
        } else {
            (pixels, w, h)
        };
        if bytes.is_empty() {
            self.status = format!("Reference load failed: bad pixels ({from}).");
            return;
        }
        self.ref_image = Some((bytes, tw, th));
        self.ref_tex = None;
        self.ref_open = true;
        self.status = format!("Reference pinned ({from}).");
    }

    /// M34 Reference: decode a picked file off the UI thread. A photo
    /// decodes to hundreds of MB at full resolution before the pin shrinks
    /// it, which stalled the whole app for seconds on iPad; the worker does
    /// decode + shrink and returns only the small buffer.
    pub(crate) fn start_reference_load(&mut self, path: std::path::PathBuf) {
        let from = format!("{}", path.display());
        let (tx, rx) = std::sync::mpsc::channel();
        let name = from.clone();
        std::thread::spawn(move || {
            let result = decode_reference(&path).map(|(bytes, w, h)| (bytes, w, h, name));
            // The receiver is dropped when a newer pick replaces this one.
            let _ = tx.send(result);
        });
        self.ref_job = Some(rx);
        self.status = format!("Loading reference {from}…");
    }

    /// Poll the reference decoder; cheap when nothing is in flight.
    pub(crate) fn poll_reference_load(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.ref_job else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok((bytes, w, h, from))) => {
                self.ref_job = None;
                self.set_reference(bytes, w, h, &from);
            }
            Ok(Err(e)) => {
                self.ref_job = None;
                self.status = format!("Reference load failed: {e}");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.ref_job = None;
                self.status = "Reference load failed: decoder stopped.".to_string();
            }
        }
    }

    /// M34 Reference: upload the companion texture on demand (load/replace
    /// only, never per frame).
    pub(crate) fn refresh_ref_tex(&mut self, ctx: &egui::Context) {
        if self.ref_tex.is_none() {
            if let Some((bytes, w, h)) = &self.ref_image {
                let img = ColorImage::from_rgba_unmultiplied([*w as usize, *h as usize], bytes);
                self.ref_tex = Some(ctx.load_texture("reference", img, TextureOptions::LINEAR));
            }
        }
    }

    /// Shared guard for pixel-writing entry points: a live text layer's
    /// pixels are engine-owned (repainted from the block on every edit), so
    /// writers refuse and point at Rasterize instead of silently fighting
    /// the next re-render (or losing the edit).
    pub(crate) fn text_paint_blocked(&mut self) -> bool {
        if self
            .doc
            .layers
            .get(self.doc.active)
            .is_some_and(|l| l.is_text())
        {
            self.status = "Text layer — tap Rasterize (Layers) to paint.".to_string();
            true
        } else {
            false
        }
    }
}

/// Decode a reference image and shrink it to [`REFERENCE_MAX_SIDE`] while
/// still on the worker thread: `resize_exact` builds the small buffer in the
/// file's own pixel format, so the full-resolution RGBA copy never exists.
fn decode_reference(path: &std::path::Path) -> Result<(Vec<u8>, u32, u32), String> {
    let img = image::open(path).map_err(|e| e.to_string())?;
    let (w, h) = (img.width().max(1), img.height().max(1));
    let img = if w.max(h) > REFERENCE_MAX_SIDE {
        let scale = REFERENCE_MAX_SIDE as f32 / w.max(h) as f32;
        img.resize_exact(
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (tw, th) = (rgba.width(), rgba.height());
    Ok((rgba.into_raw(), tw, th))
}

#[cfg(test)]
mod reference_tests {
    use super::*;

    #[test]
    fn decode_reference_shrinks_to_the_cap() {
        let path = std::env::temp_dir().join("procraft_ref_decode_test.png");
        let img = image::RgbImage::from_fn(2048, 64, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 7])
        });
        img.save(&path).expect("write test image");
        let (bytes, w, h) = decode_reference(&path).expect("decode");
        let _ = std::fs::remove_file(&path);
        assert_eq!((w, h), (REFERENCE_MAX_SIDE, REFERENCE_MAX_SIDE / 32));
        assert_eq!(bytes.len(), (w * h * 4) as usize);
    }

    #[test]
    fn decode_reference_keeps_small_images_whole() {
        let path = std::env::temp_dir().join("procraft_ref_small_test.png");
        let img = image::RgbImage::from_fn(40, 20, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 7])
        });
        img.save(&path).expect("write test image");
        let (bytes, w, h) = decode_reference(&path).expect("decode");
        let _ = std::fs::remove_file(&path);
        assert_eq!((w, h), (40, 20));
        assert_eq!(bytes.len(), 40 * 20 * 4);
    }
}
