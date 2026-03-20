impl DawApp {
    pub(crate) fn left_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("project_browser")
            .default_width(220.0)
            .resizable(true)
            .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.sidebar_tab == SidebarTab::Project, "Project")
                    .clicked()
                {
                    self.sidebar_tab = SidebarTab::Project;
                }
                if ui
                    .selectable_label(self.sidebar_tab == SidebarTab::Browser, "Browser")
                    .clicked()
                {
                    self.sidebar_tab = SidebarTab::Browser;
                }
            });
            ui.separator();

            match self.sidebar_tab {
                SidebarTab::Project => {
                    let root = if !self.project_path.trim().is_empty() {
                        PathBuf::from(self.project_path.trim())
                    } else {
                        self.default_project_dir().unwrap_or_else(|| PathBuf::from("."))
                    };
                    let root_key = Self::fs_key(&root);
                    if self.fs_expanded.is_empty() {
                        self.fs_expanded.insert(root_key.clone());
                    }
                    let root_label = root
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| root.to_string_lossy().to_string());
                    self.render_fs_row(
                        ui,
                        &root_label,
                        &root_key,
                        0,
                        true,
                        true,
                        FsSource::Project,
                        &root,
                    );
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let entries = self.list_project_entries(&root);
                        if entries.is_empty() {
                            ui.label("(no files)");
                            return;
                        }
                        for entry in entries {
                            self.render_fs_tree(ui, entry, 1, FsSource::Project);
                        }
                    });
                }
                SidebarTab::Browser => {
                    ui.horizontal(|ui| {
                        if ui.button("Add Folder").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                let folder = Self::normalize_windows_path(&folder);
                                let key = folder.to_string_lossy().to_string();
                                if !self.settings.browser_folders.iter().any(|p| p == &key) {
                                    self.settings.browser_folders.push(key.clone());
                                    let _ = self.save_settings();
                                    self.status = format!("Browser folder added: {key}");
                                }
                            }
                        }
                    });
                    ui.add_space(4.0);
                    if self.settings.browser_folders.is_empty() {
                        ui.label("(no folders)");
                        return;
                    }
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let roots = self.settings.browser_folders.clone();
                        for root_str in roots {
                            let root = PathBuf::from(root_str);
                            if !root.exists() {
                                continue;
                            }
                            let root_key = Self::fs_key(&root);
                            if !self.browser_expanded.contains(&root_key) {
                                self.browser_expanded.insert(root_key.clone());
                            }
                            let root_label = root
                                .file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_else(|| root.to_string_lossy().to_string());
                            self.render_fs_row(
                                ui,
                                &root_label,
                                &root_key,
                                0,
                                true,
                                true,
                                FsSource::Browser,
                                &root,
                            );
                            for entry in self.list_project_entries(&root) {
                                self.render_fs_tree(ui, entry, 1, FsSource::Browser);
                            }
                            ui.add_space(2.0);
                        }
                    });
                }
            }
        });
    }

    pub(crate) fn fs_key(path: &Path) -> String {
        path.to_string_lossy().to_string()
    }

    pub(crate) fn list_project_entries(&self, root: &Path) -> Vec<FsEntry> {
        let mut dirs: Vec<FsEntry> = Vec::new();
        let mut files: Vec<FsEntry> = Vec::new();
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                dirs.push(FsEntry {
                    name,
                    path,
                    is_dir: true,
                });
            } else {
                files.push(FsEntry {
                    name,
                    path,
                    is_dir: false,
                });
            }
        }
        dirs.sort_by(|a, b| a.name.cmp(&b.name));
        files.sort_by(|a, b| a.name.cmp(&b.name));
        dirs.extend(files);
        dirs
    }

    pub(crate) fn fs_drag_kind_for_path(path: &Path) -> Option<FsDragKind> {
        let ext = path.extension().and_then(|s| s.to_str())?.to_ascii_lowercase();
        if matches!(ext.as_str(), "mid" | "midi") {
            return Some(FsDragKind::Midi);
        }
        if matches!(ext.as_str(), "wav" | "ogg" | "flac" | "mp3" | "aiff" | "aif") {
            return Some(FsDragKind::Audio);
        }
        None
    }

    pub(crate) fn invalidate_audio_caches_for_path(&self, path: &Path) {
        let key = path.to_string_lossy().to_string();
        self.waveform_cache.borrow_mut().remove(&key);
        self.waveform_color_cache.borrow_mut().remove(&key);
        self.waveform_len_seconds_cache.borrow_mut().remove(&key);
        self.waveform_cache_order.borrow_mut().retain(|entry| entry != &key);
        self.waveform_color_cache_order
            .borrow_mut()
            .retain(|entry| entry != &key);
        self.waveform_len_seconds_cache_order
            .borrow_mut()
            .retain(|entry| entry != &key);
        if let Ok(mut cache) = self.audio_clip_cache.lock() {
            cache.remove(&key);
        }
    }

    pub(crate) fn delete_fs_path(&mut self, path: &Path) -> Result<(), String> {
        if path.is_dir() {
            fs::remove_dir_all(path).map_err(|e| e.to_string())?;
        } else {
            fs::remove_file(path).map_err(|e| e.to_string())?;
            self.invalidate_audio_caches_for_path(path);
        }
        Ok(())
    }

    pub(crate) fn duplicate_fs_path(&mut self, path: &Path) -> Result<PathBuf, String> {
        if path.is_dir() {
            return Err("Duplicate only supports files".to_string());
        }
        let parent = path.parent().ok_or_else(|| "Missing parent folder".to_string())?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("copy");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let mut counter = 1;
        let mut candidate = if ext.is_empty() {
            parent.join(format!("{stem} copy"))
        } else {
            parent.join(format!("{stem} copy.{ext}"))
        };
        while candidate.exists() {
            counter += 1;
            candidate = if ext.is_empty() {
                parent.join(format!("{stem} copy {counter}"))
            } else {
                parent.join(format!("{stem} copy {counter}.{ext}"))
            };
        }
        fs::copy(path, &candidate).map_err(|e| e.to_string())?;
        Ok(candidate)
    }

    pub(crate) fn show_in_explorer(&mut self, path: &Path) {
        #[cfg(target_os = "windows")]
        {
            let mut cmd = std::process::Command::new("explorer");
            if path.is_file() {
                cmd.arg("/select,").arg(path);
            } else {
                cmd.arg(path);
            }
            let _ = cmd.spawn();
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.status = format!("Open path: {}", path.to_string_lossy());
        }
    }

    pub(crate) fn remove_browser_folder(&mut self, path: &Path) {
        let key = path.to_string_lossy().to_string();
        self.settings.browser_folders.retain(|p| p != &key);
        let _ = self.save_settings();
        self.status = format!("Browser folder removed: {key}");
    }

    pub(crate) fn render_fs_tree(&mut self, ui: &mut egui::Ui, entry: FsEntry, depth: usize, source: FsSource) {
        let key = Self::fs_key(&entry.path);
        let is_open = match source {
            FsSource::Project => self.fs_expanded.contains(&key),
            FsSource::Browser => self.browser_expanded.contains(&key),
        };
        let toggled = self.render_fs_row(
            ui,
            &entry.name,
            &key,
            depth,
            entry.is_dir,
            is_open,
            source,
            &entry.path,
        );
        if entry.is_dir {
            if toggled {
                if is_open {
                    match source {
                        FsSource::Project => {
                            self.fs_expanded.remove(&key);
                        }
                        FsSource::Browser => {
                            self.browser_expanded.remove(&key);
                        }
                    }
                } else {
                    match source {
                        FsSource::Project => {
                            self.fs_expanded.insert(key.clone());
                        }
                        FsSource::Browser => {
                            self.browser_expanded.insert(key.clone());
                        }
                    }
                }
            }
            let open = match source {
                FsSource::Project => self.fs_expanded.contains(&key),
                FsSource::Browser => self.browser_expanded.contains(&key),
            };
            if open {
                for child in self.list_project_entries(&entry.path) {
                    self.render_fs_tree(ui, child, depth + 1, source);
                }
            }
        }
    }

    pub(crate) fn render_fs_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        key: &str,
        depth: usize,
        is_dir: bool,
        is_open: bool,
        source: FsSource,
        path: &Path,
    ) -> bool {
        let row_h = 20.0;
        let full_w = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(full_w, row_h), egui::Sense::click_and_drag());
        let selected = match source {
            FsSource::Project => self.fs_selected.as_deref() == Some(key),
            FsSource::Browser => self.browser_selected.as_deref() == Some(key),
        };
        let hovered = response.hovered();
        if selected || hovered {
            let color = if selected {
                egui::Color32::from_rgb(38, 52, 76)
            } else {
                egui::Color32::from_rgb(30, 36, 44)
            };
            ui.painter().rect_filled(rect, 4.0, color);
        }

        let indent = 12.0;
        let x = rect.min.x + indent * depth as f32 + 6.0;
        let center_y = rect.center().y;
        let icon_color = if is_dir {
            egui::Color32::from_rgb(110, 150, 255)
        } else {
            egui::Color32::from_rgb(120, 200, 140)
        };

        if is_dir {
            let tri_size = 8.0;
            let tri_x = x;
            let tri_y = center_y - tri_size * 0.5;
            let points = if is_open {
                vec![
                    egui::pos2(tri_x, tri_y + 1.0),
                    egui::pos2(tri_x + tri_size, tri_y + 1.0),
                    egui::pos2(tri_x + tri_size * 0.5, tri_y + tri_size + 1.0),
                ]
            } else {
                vec![
                    egui::pos2(tri_x, tri_y),
                    egui::pos2(tri_x, tri_y + tri_size),
                    egui::pos2(tri_x + tri_size, tri_y + tri_size * 0.5),
                ]
            };
            ui.painter()
                .add(egui::Shape::convex_polygon(points, icon_color, egui::Stroke::NONE));
        }

        let icon_x = if is_dir { x + 12.0 } else { x + 4.0 };
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(icon_x, center_y),
            egui::vec2(10.0, 10.0),
        );
        ui.painter()
            .rect_filled(icon_rect, 2.0, icon_color.linear_multiply(0.9));

        let text_x = icon_rect.max.x + 6.0;
        let font_id = egui::FontId::proportional(BASE_UI_FONT_SIZE);
        let text_color = if selected {
            egui::Color32::from_rgb(220, 230, 244)
        } else {
            egui::Color32::from_rgb(190, 200, 210)
        };
        let galley = ui
            .fonts(|f| f.layout_no_wrap(label.to_string(), font_id.clone(), text_color));
        let text_pos = egui::pos2(text_x, center_y - galley.size().y * 0.5);
        ui.painter().galley(text_pos, galley.clone(), text_color);

        if !is_dir {
            if let Some(ext) = label.rsplit('.').next() {
                if ext.len() > 0 && ext.len() <= 6 {
                    let badge_text = ext.to_ascii_uppercase();
                    let badge_font = egui::FontId::proportional(BASE_UI_FONT_SIZE);
                    let badge_galley = ui.fonts(|f| {
                        f.layout_no_wrap(
                            badge_text.clone(),
                            badge_font.clone(),
                            egui::Color32::from_rgb(40, 44, 48),
                        )
                    });
                    let badge_size = egui::vec2(badge_galley.size().x + 8.0, 12.0);
                    let badge_x = (text_pos.x + galley.size().x + 6.0)
                        .min(rect.max.x - badge_size.x - 4.0);
                    if badge_x > text_pos.x + 6.0 {
                        let badge_rect = egui::Rect::from_min_size(
                            egui::pos2(badge_x, center_y - badge_size.y * 0.5),
                            badge_size,
                        );
                        ui.painter().rect_filled(
                            badge_rect,
                            6.0,
                            egui::Color32::from_rgb(170, 190, 210),
                        );
                        let badge_pos = egui::pos2(
                            badge_rect.min.x + 4.0,
                            center_y - badge_galley.size().y * 0.5,
                        );
                        ui.painter().galley(
                            badge_pos,
                            badge_galley,
                            egui::Color32::from_rgb(40, 44, 48),
                        );
                    }
                }
            }
        }

        if response.clicked() {
            match source {
                FsSource::Project => self.fs_selected = Some(key.to_string()),
                FsSource::Browser => self.browser_selected = Some(key.to_string()),
            }
        }

        let mut action: Option<(&'static str, PathBuf)> = None;
        let mut open_folder_remove = false;
        response.context_menu(|ui| {
            if !is_dir {
                if ui.button("Delete").clicked() {
                    action = Some(("delete", path.to_path_buf()));
                    ui.close_menu();
                }
                if ui.button("Duplicate").clicked() {
                    action = Some(("duplicate", path.to_path_buf()));
                    ui.close_menu();
                }
            }
            if ui.button("Show in Explorer").clicked() {
                action = Some(("show", path.to_path_buf()));
                ui.close_menu();
            }
            if source == FsSource::Browser && depth == 0 && is_dir {
                if ui.button("Remove Folder").clicked() {
                    open_folder_remove = true;
                    ui.close_menu();
                }
            }
        });
        if let Some((kind, path)) = action {
            match kind {
                "delete" => {
                    if let Err(err) = self.delete_fs_path(&path) {
                        self.status = format!("Delete failed: {err}");
                    }
                }
                "duplicate" => {
                    match self.duplicate_fs_path(&path) {
                        Ok(new_path) => {
                            self.status = format!("Duplicated: {}", new_path.to_string_lossy());
                        }
                        Err(err) => {
                            self.status = format!("Duplicate failed: {err}");
                        }
                    }
                }
                "show" => self.show_in_explorer(&path),
                _ => {}
            }
        }
        if open_folder_remove {
            self.remove_browser_folder(path);
        }

        if !is_dir {
            if response.drag_started() {
                if let Some(kind) = Self::fs_drag_kind_for_path(path) {
                    self.fs_drag = Some(FsDragState {
                        path: path.to_path_buf(),
                        kind,
                    });
                }
            }
        }

        is_dir && response.clicked()
    }
}
