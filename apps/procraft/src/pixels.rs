use crate::*;

impl ProCraftApp {
    pub(crate) fn base_brush(&self) -> Brush {
        Brush {
            eraser: self.tool == Tool::Eraser,
            ..self.brush.clone()
        }
    }

    pub(crate) fn current_color(&self) -> Color {
        Color::rgb(self.color[0], self.color[1], self.color[2])
    }

    /// Re-apply the last fill after a Tolerance/Grow slider move. Drops the
    /// session (leaving the canvas untouched) if another edit or a context
    /// change made the snapshot stale.
    pub(crate) fn reapply_live_fill(&mut self) {
        let Some(mut fp) = self.fill_preview.take() else {
            return;
        };
        let layer_ok = self
            .doc
            .layers
            .get(fp.layer)
            .is_some_and(|l| l.pixels.fingerprint() == fp.live_hash);
        if !layer_ok || !fill_context_matches(&self.doc, &fp) {
            return;
        }
        fill_from_baseline(&mut self.doc, &mut fp, self.fill_tolerance, self.fill_grow);
        self.status = format!(
            "Fill preview — tolerance {}, grow {}.",
            fp.applied, self.fill_grow
        );
        self.fill_preview = Some(fp);
    }

    /// Rebuild the cached mask-edge points for marching ants. Rect-only
    /// selections contribute their perimeter; masks contribute pixels on
    /// the nonzero boundary (feather fringe included, so the crawl shows
    /// the true extent). Runs once per selection change, not per frame.
    pub(crate) fn rebuild_sel_edges(&mut self) {
        self.sel_edges.clear();
        let (w, h) = (self.doc.w, self.doc.h);
        if w == 0 || h == 0 {
            return;
        }
        if let Some(mask) = self.doc.selection_mask.as_deref() {
            if mask.len() != (w * h) as usize {
                return;
            }
            let at = |x: u32, y: u32| mask[(y * w + x) as usize];
            for y in 0..h {
                for x in 0..w {
                    if at(x, y) == 0 {
                        continue;
                    }
                    let edge = x == 0
                        || y == 0
                        || x + 1 == w
                        || y + 1 == h
                        || at(x - 1, y) == 0
                        || at(x + 1, y) == 0
                        || at(x, y - 1) == 0
                        || at(x, y + 1) == 0;
                    if edge {
                        self.sel_edges.push((x, y));
                    }
                }
            }
        } else if let Some((x0, y0, x1, y1)) = self.doc.selection {
            let (x1, y1) = (x1.min(w - 1), y1.min(h - 1));
            for x in x0..=x1 {
                self.sel_edges.push((x, y0));
                if y1 != y0 {
                    self.sel_edges.push((x, y1));
                }
            }
            for y in y0 + 1..y1 {
                self.sel_edges.push((x0, y));
                if x1 != x0 {
                    self.sel_edges.push((x1, y));
                }
            }
        }
    }

    /// Whether the viewport-resolution fallback (spec §4) drives the canvas
    /// this frame. Mask view and Animation Assist opt out: they keep their
    /// own full uploads. Zoomed far out also takes the fallback, because the
    /// canvas texture is minified with bilinear sampling only (no mipmaps on
    /// wgpu) and thin strokes alias into stripes.
    pub(crate) fn viewport_canvas(&self, ctx: &egui::Context) -> bool {
        !self.view_mask
            && !self.anim_assist
            && (self.zoom < canvas::MINIFIED_ZOOM
                || canvas::viewport_active(
                    viewport_forced(),
                    self.doc.w.max(self.doc.h),
                    ctx.input(|i| i.max_texture_side),
                ))
    }

    /// Upload only what changed since the last frame (no-op when clean).
    /// Composite view uploads one `set_partial` per tile under the dirty
    /// rect; pan, zoom and rotation never recomposite.
    pub(crate) fn refresh_texture(&mut self, ctx: &egui::Context) {
        // "View" can be checked on a layer with no mask. If that view would
        // also suppress the viewport fallback this document needs, drop it
        // for the frame rather than ask for a canvas texture this GPU cannot
        // hold.
        let has_mask = self
            .doc
            .layers
            .get(self.doc.active)
            .is_some_and(|l| l.mask.is_some());
        if canvas::maskless_view_blocks_fallback(
            self.view_mask,
            has_mask,
            self.anim_assist,
            canvas::viewport_active(
                viewport_forced(),
                self.doc.w.max(self.doc.h),
                ctx.input(|i| i.max_texture_side),
            ),
        ) {
            self.view_mask = false;
            let msg = "This layer has no mask.";
            if self.status != msg {
                self.status = msg.to_string();
            }
        }
        // Viewport-resolution fallback (spec §4): the draw path composites
        // the visible rect, so there is nothing to upload here. Drop the
        // full-canvas texture — at 8K it is 268 MB of VRAM this path never
        // samples — but keep the composite cache: it is exactly what the
        // viewport reads, the one deliberate exception to "texture dropped
        // ⇒ cache invalidated".
        if self.viewport_canvas(ctx) {
            self.texture = None;
            return;
        }
        // Target A from here on: the viewport texture is dead weight, and a
        // dropped handle forces a full pass if the fallback returns.
        if self.view_tex.texture().is_some() {
            self.view_tex.clear();
        }
        // Mask viewer: grayscale of the active layer's mask. The decision
        // comes first because `take_dirty` is destructive: with the view on
        // and no mask on the layer, the dirty rect must reach the composite
        // path below (which patches tiles) instead of being drained here and
        // leaving the composite to flatten the whole canvas.
        match mask_action(self.view_mask, has_mask, self.mask_shown) {
            // The canvas texture holds mask pixels that no longer apply:
            // drop it once so the composite path starts from a clean
            // `texture == None` instead of patching tiles over mask pixels.
            MaskAction::Drop => {
                self.mask_shown = false;
                self.texture = None;
                self.composite.invalidate();
                // Fall through: the composite / assist path rebuilds.
            }
            // Nothing mask-specific this frame: the dirty rect is *not*
            // drained — the composite / assist path owns it.
            MaskAction::Idle => {
                if self.view_mask {
                    let msg = "This layer has no mask.";
                    if self.status != msg {
                        self.status = msg.to_string();
                    }
                }
            }
            // The layer has a mask and the view is on: build or patch the
            // mask texture. Draining the dirty rect means a mask dab re-reads
            // and re-uploads only that rect; the full canvas is built once
            // per texture (open, resize, view toggle), never per frame.
            MaskAction::Show => {
                let dirty = self.doc.take_dirty();
                // The texture is canvas-sized, so a GPU that cannot hold one
                // — or a document above the 8K cap this GPU-backed path was
                // sized for — would reject the upload in the backend: say so
                // instead.
                if !canvas::canvas_texture_fits(
                    self.doc.w.max(self.doc.h),
                    ctx.input(|i| i.max_texture_side),
                ) {
                    let msg = "Mask view needs a canvas texture this GPU cannot allocate.";
                    if self.status != msg {
                        self.status = msg.to_string();
                    }
                    self.texture = None;
                    self.mask_shown = true;
                    self.composite.invalidate();
                    return;
                }
                // The canvas texture is not the composite here, so no composite
                // tile can be trusted when the view returns.
                self.composite.invalidate();
                if self.texture.is_none() {
                    if let Some(mv) = self.doc.mask_view() {
                        let img = ColorImage::from_rgba_unmultiplied(
                            [self.doc.w as usize, self.doc.h as usize],
                            &mv,
                        );
                        self.texture =
                            Some(ctx.load_texture("canvas", img, TextureOptions::LINEAR));
                        self.mask_shown = true;
                        return;
                    }
                    // No mask to build from after all (mid-state): the slot
                    // does not hold mask pixels, so leaving the view must
                    // not drop the composite texture a second time.
                    self.mask_shown = false;
                } else {
                    match dirty {
                        Some((x0, y0, x1, y1)) => match self.doc.mask_view_region(x0, y0, x1, y1) {
                            Some(bytes) => {
                                let img = ColorImage::from_rgba_unmultiplied(
                                    [(x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize],
                                    &bytes,
                                );
                                if let Some(handle) = &mut self.texture {
                                    handle.set_partial(
                                        [x0 as usize, y0 as usize],
                                        img,
                                        TextureOptions::LINEAR,
                                    );
                                }
                                self.mask_shown = true;
                                return;
                            }
                            // Mask gone: fall through and rebuild from the
                            // composite instead of leaving a stale texture.
                            None => {
                                self.texture = None;
                                self.mask_shown = false;
                            }
                        },
                        // Nothing changed: the upload is already correct.
                        None => {
                            self.mask_shown = true;
                            return;
                        }
                    }
                }
            }
        }
        // Animation Assist: the canvas shows the current frame only
        // (active layer over the page), not the full composite.
        if self.anim_assist && !self.doc.layers.is_empty() {
            // The frame texture is canvas-sized too: a GPU that cannot hold
            // one — or a document above the 8K cap — would reject the upload
            // in the backend (a legacy 16384² document asks for ~1 GiB), so
            // say so instead of building it.
            if !canvas::canvas_texture_fits(
                self.doc.w.max(self.doc.h),
                ctx.input(|i| i.max_texture_side),
            ) {
                let msg = "Animation Assist needs a canvas texture this GPU cannot allocate.";
                if self.status != msg {
                    self.status = msg.to_string();
                }
                self.texture = None;
                self.anim_shown = None;
                self.composite.invalidate();
                return;
            }
            // Frame view is not the composite (dab rects are consumed
            // here, never by the tile cache): drop the tiles.
            self.composite.invalidate();
            let active = self.doc.active.min(self.doc.layers.len() - 1);
            if self.texture.is_none() || self.anim_shown != Some(active) {
                let bytes = rasterize_frame(&self.doc, active);
                let img = ColorImage::from_rgba_unmultiplied(
                    [self.doc.w as usize, self.doc.h as usize],
                    &bytes,
                );
                self.texture = Some(ctx.load_texture("canvas", img, TextureOptions::LINEAR));
                self.anim_shown = Some(active);
                return;
            }
            if let Some((x0, y0, x1, y1)) = self.doc.take_dirty() {
                let bytes = rasterize_frame_region(&self.doc, active, x0, y0, x1, y1);
                let w = (x1 - x0 + 1) as usize;
                let h = ((y1 - y0 + 1) as usize).max(bytes.len() / 4 / w.max(1));
                let img = ColorImage::from_rgba_unmultiplied([w, h], &bytes);
                if let Some(handle) = &mut self.texture {
                    handle.set_partial([x0 as usize, y0 as usize], img, TextureOptions::LINEAR);
                }
            }
            return;
        }
        self.anim_shown = None;
        // Full upload: new/changed texture. `full_image` interleaves the
        // tiles row by row and crops the last column and row, so the dense
        // buffer is exactly `w * h * 4` and the texture stays canvas-sized.
        if self.texture.is_none() {
            let plan = self.doc.stack_plan();
            let img = canvas::full_image(&self.doc, &plan, &mut self.composite);
            let image = ColorImage::from_rgba_unmultiplied(
                [self.doc.w as usize, self.doc.h as usize],
                &img,
            );
            self.texture = Some(ctx.load_texture("canvas", image, TextureOptions::LINEAR));
            // The upload already covers the whole canvas.
            self.doc.take_dirty();
            return;
        }
        if let Some((x0, y0, x1, y1)) = self.doc.take_dirty() {
            // One plan per pass: it snapshots the revision, and the next
            // dab bumps it (a retained plan would be stale).
            let plan = self.doc.stack_plan();
            let (x0, y0) = (
                x0.min(self.doc.w.saturating_sub(1)),
                y0.min(self.doc.h.saturating_sub(1)),
            );
            let (x1, y1) = (
                x1.min(self.doc.w.saturating_sub(1)),
                y1.min(self.doc.h.saturating_sub(1)),
            );
            // Recomposite only the dirty band of each cached tile: the rest
            // of the 256² square is still valid from the previous event.
            let rect = (x0, y0, x1, y1);
            self.composite.patch(&self.doc, &plan, rect);
            for ty in y0 / TILE..=y1 / TILE {
                for tx in x0 / TILE..=x1 / TILE {
                    let tile = self.composite.tile(&self.doc, &plan, tx, ty);
                    let (rgba, ox, oy, w, h) =
                        canvas::tile_image_region(&self.doc, tile, tx, ty, rect);
                    if w == 0 || h == 0 {
                        continue;
                    }
                    let image = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                    if let Some(handle) = &mut self.texture {
                        handle.set_partial([ox, oy], image, TextureOptions::LINEAR);
                    }
                }
            }
        }
    }

    /// Rebuild half-res onion ghost textures when the neighbor pair, the
    /// per-side strengths, or layer content (`ghost_dirty`) changed.
    /// No-op otherwise.
    pub(crate) fn refresh_ghosts(&mut self, ctx: &egui::Context) {
        let active = clamp_frame(&self.doc, self.doc.active).unwrap_or(0);
        let bits = |v: f32| (v.clamp(0.05, 0.8) * 1000.0).round() as u32;
        let pair = if self.anim_assist && self.onion && !self.doc.layers.is_empty() {
            (
                prev_frame(&self.doc, active),
                next_frame(&self.doc, active),
                bits(self.onion_prev),
                bits(self.onion_next),
            )
        } else {
            (None, None, bits(self.onion_prev), bits(self.onion_next))
        };
        if !self.ghost_dirty && pair == self.ghost_key {
            return;
        }
        self.ghost_dirty = false;
        self.ghost_key = pair;
        let (w, h) = (self.doc.w as usize, self.doc.h as usize);
        let load = |ctx: &egui::Context,
                    name: &str,
                    idx: Option<usize>,
                    amount: f32|
         -> Option<TextureHandle> {
            let full = onion_ghost(&self.doc, idx?, amount);
            Some(ctx.load_texture(name, downsample_half(&full, w, h), TextureOptions::LINEAR))
        };
        self.ghost_prev = load(ctx, "ghost-prev", pair.0, self.onion_prev);
        self.ghost_next = load(ctx, "ghost-next", pair.1, self.onion_next);
    }

    pub(crate) fn set_page_color(&mut self, rgb: [u8; 3]) {
        self.doc.background = [rgb[0], rgb[1], rgb[2], 255];
        self.doc.mark_full_dirty();
    }

    /// Mobile: load a resolved file. Mirrors the desktop open arms (kept
    /// inline there so desktop behavior never changes).
    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub(crate) fn open_picked(&mut self, kind: PendingOpen, path: std::path::PathBuf) {
        match kind {
            PendingOpen::Image => match self.doc.load_background(&path) {
                Ok(()) => {
                    self.texture = None;
                    self.composite.invalidate();
                    self.lasso_points.clear();
                    self.lasso_poly.clear();
                    self.status = format!("Opened {}", path.display());
                }
                Err(e) => self.status = format!("Open failed: {e}"),
            },
            PendingOpen::Project => match Document::open_project(&path) {
                Ok(doc) => {
                    let s = path.to_string_lossy().to_string();
                    self.adopt_project(doc, &s, "Project opened");
                }
                Err(e) => self.status = format!("Project open failed: {e}"),
            },
            PendingOpen::Psd => match open_psd(&path) {
                Ok(doc) => {
                    self.doc = doc;
                    self.clear_recovery();
                    self.texture = None;
                    self.composite.invalidate();
                    self.lasso_points.clear();
                    self.lasso_poly.clear();
                    self.status = format!("PSD opened {} (masks do not import)", path.display());
                }
                Err(e) => self.status = format!("PSD open failed: {e}"),
            },
            PendingOpen::Brushes => match load_brush_file_report(&path) {
                Ok(libs) => self.status = self.absorb_brushes(libs, &path),
                Err(e) => self.status = format!("Brush import failed: {e}"),
            },
            PendingOpen::Palette => match load_palette_file(&path) {
                Ok(pal) => self.status = self.absorb_colors(pal, &format!("{}", path.display())),
                Err(e) => self.status = format!("Palette import failed: {e}"),
            },
            PendingOpen::ImagePalette => {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Image")
                    .to_string();
                let from = format!("{}", path.display());
                match image::open(&path).map(|i| i.to_rgba8()) {
                    Ok(img) => {
                        let (w, h) = (img.width(), img.height());
                        self.status = self.image_palette_status(&stem, img.as_raw(), w, h, &from);
                    }
                    Err(e) => self.status = format!("Image open failed: {e}"),
                }
            }
            PendingOpen::Gif => match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| import_gif_document(&b))
            {
                Ok((doc, fps)) => {
                    self.status = format!(
                        "GIF imported {} ({} frames, {} fps).",
                        path.display(),
                        doc.layers.len(),
                        fps
                    );
                    self.adopt_gif(doc, fps);
                }
                Err(e) => self.status = format!("GIF import failed: {e}"),
            },
            // M34 Reference: pin the image beside the canvas (not a layer).
            PendingOpen::Reference => match image::open(&path).map(|i| i.to_rgba8()) {
                Ok(img) => {
                    let (w, h) = (img.width(), img.height());
                    self.set_reference(img.into_raw(), w, h, &format!("{}", path.display()));
                }
                Err(e) => self.status = format!("Reference load failed: {e}"),
            },
            // M38 Typography: validate an imported face, keep its bytes.
            PendingOpen::Font => match std::fs::read(&path) {
                Ok(bytes) => {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("font")
                        .to_string();
                    self.status = self.absorb_font(&bytes, &name);
                }
                Err(e) => self.status = format!("Font import failed: {e}"),
            },
            // M39 Brush depth: take the first stamp tip in the file.
            PendingOpen::DualTip => {
                self.status = self.absorb_dual_tip(&path);
            }
        }
    }

    /// Honest brush previews: each preset paints a wavy sample stroke through
    /// the real engine into a thumbnail texture (dotted chalk reads dotted).
    /// Ink follows the theme so cards stay crisp on dark and paper alike.
    /// Built-ins come first, then the user's custom brushes in order.
    pub(crate) fn build_preset_previews(&mut self, ctx: &egui::Context, ink: Color) {
        self.preset_textures.clear();
        let scale = ctx.pixels_per_point();
        let (pw, ph) = preview_size(scale);
        let brushes: Vec<Brush> = procraft_engine::presets()
            .into_iter()
            .map(|(_, b)| b)
            .chain(self.custom_brushes.iter().map(|p| p.brush.clone()))
            .collect();
        for (idx, preset) in brushes.into_iter().enumerate() {
            let mut doc = Document::new(pw, ph);
            let brush = preview_brush(&preset, ph as f32);
            paint_preview_stroke(&mut doc, &brush, ink);
            let img = ColorImage::from_rgba_unmultiplied(
                [pw as usize, ph as usize],
                &doc.layers[0].pixels.to_dense(),
            );
            self.preset_textures.push(ctx.load_texture(
                format!("brush-{idx}"),
                img,
                TextureOptions::LINEAR,
            ));
        }
        self.preview_scale = scale;
    }

    /// Total preview cards currently needed (built-ins + customs).
    pub(crate) fn preview_count(&self) -> usize {
        procraft_engine::presets().len() + self.custom_brushes.len()
    }
}

/// Minimum time between live preview re-applies while a slider is
/// scrubbed. Full-canvas filters cost 15–60 ms at 1280x800 plus a
/// re-flatten and texture upload per tick; without a floor, every slider
/// tick queues one and the UI visibly hangs during the drag.
pub(crate) const PREVIEW_MIN_DT: std::time::Duration = std::time::Duration::from_millis(50);
