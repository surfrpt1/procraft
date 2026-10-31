use crate::*;

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
        const MAX_SIDE: u32 = 1024;
        let (bytes, tw, th) = if w.max(h) > MAX_SIDE {
            gallery_thumb(&pixels, w, h, MAX_SIDE)
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
