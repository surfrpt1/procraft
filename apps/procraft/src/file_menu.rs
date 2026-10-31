use crate::*;

impl ProCraftApp {
    /// File menu items shared by the top bar and the mobile cluster.
    pub(crate) fn file_menu_ui(
        &mut self,
        ui: &mut egui::Ui,
        layout: BarLayout,
        bar_t: &design::Tokens,
        max_h: Option<f32>,
    ) {
        if layout == BarLayout::Tablet {
            ui.spacing_mut().interact_size.y = bar_t.control_h;
        }
        // A landscape phone cannot show Save/Open/PSD in one popup: cap it
        // above the bar and scroll instead of clipping the last rows.
        let out = egui::ScrollArea::vertical()
            .id_salt("file-menu-scroll")
            .max_height(max_h.unwrap_or(f32::INFINITY))
            .show(ui, |ui| {
                if ui.button("Save PNG").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("PNG", &["png"])
                        .save_file()
                    {
                        // The dialog may return an extensionless path (always
                        // on iOS); the encoder needs the suffix to pick PNG.
                        let path = ensure_extension(path, "png");
                        match self.doc.save_png(&path) {
                            Ok(()) => {
                                self.status = format!("Saved {}", path.display());
                                #[cfg(target_os = "ios")]
                                rfd::ios_share_file(&path);
                                #[cfg(target_os = "android")]
                                rfd::android_scan_file(&path);
                            }
                            Err(e) => self.status = format!("Save failed: {e}"),
                        }
                    } else {
                        note_save_declined(&mut self.status, "PNG");
                    }
                }
                stepped_slider(ui, &mut self.jpeg_quality, 1..=100, 1.0, "JPEG q");
                if ui.button("Save JPEG").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("JPEG", &["jpg", "jpeg"])
                        .save_file()
                    {
                        let path = ensure_extension(path, "jpg");
                        match self.doc.save_jpeg(&path, self.jpeg_quality) {
                            Ok(()) => {
                                self.status = format!(
                                    "Saved {} (quality {})",
                                    path.display(),
                                    self.jpeg_quality
                                );
                                #[cfg(target_os = "ios")]
                                rfd::ios_share_file(&path);
                                #[cfg(target_os = "android")]
                                rfd::android_scan_file(&path);
                            }
                            Err(e) => self.status = format!("Save failed: {e}"),
                        }
                    } else {
                        note_save_declined(&mut self.status, "JPEG");
                    }
                }
                if ui.button("Save TIFF").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("TIFF", &["tif", "tiff"])
                        .save_file()
                    {
                        let path = ensure_extension(path, "tiff");
                        match self.doc.save_tiff(&path) {
                            Ok(()) => {
                                self.status = format!("Saved {}", path.display());
                                #[cfg(target_os = "ios")]
                                rfd::ios_share_file(&path);
                                #[cfg(target_os = "android")]
                                rfd::android_scan_file(&path);
                            }
                            Err(e) => self.status = format!("Save failed: {e}"),
                        }
                    } else {
                        note_save_declined(&mut self.status, "TIFF");
                    }
                }
                if ui.button("Open image").clicked() {
                    ui.close();
                    #[cfg(not(target_os = "ios"))]
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter(
                            "Images",
                            &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"],
                        )
                        .pick_file()
                    {
                        match self.doc.load_background(&path) {
                            Ok(()) => {
                                self.texture = None;
                                self.composite.invalidate();
                                self.lasso_points.clear();
                                self.lasso_poly.clear();
                                self.status = format!("Opened {}", path.display());
                            }
                            Err(e) => self.status = format!("Open failed: {e}"),
                        }
                    }
                    // iOS: presents the picker; open_picked() loads on resolve.
                    #[cfg(target_os = "ios")]
                    {
                        let _ = rfd::FileDialog::new()
                            .add_filter(
                                "Images",
                                &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"],
                            )
                            .pick_file();
                        self.pending_open = Some(PendingOpen::Image);
                        self.status = "Pick an image…".to_string();
                    }
                    #[cfg(target_os = "android")]
                    {
                        self.android_browse = Some(PendingOpen::Image);
                    }
                }
                ui.separator();
                if ui.button("Save project").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("ProCraft project", &["procraft"])
                        .save_file()
                    {
                        let path = ensure_extension(path, "procraft");
                        match self.doc.save_project(&path) {
                            Ok(()) => {
                                let s = path.to_string_lossy().to_string();
                                push_recent(&mut self.recent, &s);
                                self.gallery_keys.clear();
                                self.status = format!("Project saved {}", path.display());
                                self.clear_recovery();
                                #[cfg(target_os = "ios")]
                                rfd::ios_share_file(&path);
                                #[cfg(target_os = "android")]
                                rfd::android_scan_file(&path);
                            }
                            Err(e) => self.status = format!("Project save failed: {e}"),
                        }
                    } else {
                        note_save_declined(&mut self.status, "Project");
                    }
                }
                if ui.button("Open project").clicked() {
                    ui.close();
                    #[cfg(not(target_os = "ios"))]
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("ProCraft project", &["procraft"])
                        .pick_file()
                    {
                        match Document::open_project(&path) {
                            Ok(doc) => {
                                let s = path.to_string_lossy().to_string();
                                self.adopt_project(doc, &s, "Project opened");
                            }
                            Err(e) => self.status = format!("Project open failed: {e}"),
                        }
                    }
                    #[cfg(target_os = "ios")]
                    {
                        let _ = rfd::FileDialog::new()
                            .add_filter("ProCraft project", &["procraft"])
                            .pick_file();
                        self.pending_open = Some(PendingOpen::Project);
                        self.status = "Pick a project…".to_string();
                    }
                    #[cfg(target_os = "android")]
                    {
                        self.android_browse = Some(PendingOpen::Project);
                    }
                }
                ui.separator();
                if ui.button("PSD export").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Photoshop document", &["psd"])
                        .save_file()
                    {
                        let path = ensure_extension(path, "psd");
                        match save_psd(&path, &self.doc) {
                            Ok(()) => {
                                self.status = format!(
                                    "PSD saved {} (masks do not round-trip)",
                                    path.display()
                                );
                                #[cfg(target_os = "ios")]
                                rfd::ios_share_file(&path);
                                #[cfg(target_os = "android")]
                                rfd::android_scan_file(&path);
                            }
                            Err(e) => self.status = format!("PSD save failed: {e}"),
                        }
                    } else {
                        note_save_declined(&mut self.status, "PSD");
                    }
                }
                if ui.button("Open PSD").clicked() {
                    ui.close();
                    #[cfg(not(target_os = "ios"))]
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Photoshop document", &["psd"])
                        .pick_file()
                    {
                        match open_psd(&path) {
                            Ok(doc) => {
                                self.doc = doc;
                                self.clear_recovery();
                                self.texture = None;
                                self.composite.invalidate();
                                self.lasso_points.clear();
                                self.lasso_poly.clear();
                                self.status =
                                    format!("PSD opened {} (masks do not import)", path.display());
                            }
                            Err(e) => self.status = format!("PSD open failed: {e}"),
                        }
                    }
                    #[cfg(target_os = "ios")]
                    {
                        let _ = rfd::FileDialog::new()
                            .add_filter("Photoshop document", &["psd"])
                            .pick_file();
                        self.pending_open = Some(PendingOpen::Psd);
                        self.status = "Pick a PSD…".to_string();
                    }
                    #[cfg(target_os = "android")]
                    {
                        self.android_browse = Some(PendingOpen::Psd);
                    }
                }
            });
        let fade = menu_fade_height(
            out.content_size.y,
            out.inner_rect.height(),
            out.state.offset.y,
        );
        if fade > 0.0 {
            fade_bottom_edge(ui, out.inner_rect, ui.visuals().window_fill(), fade);
        }
    }
}
