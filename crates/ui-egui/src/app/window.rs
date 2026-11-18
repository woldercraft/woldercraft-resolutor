use super::*;

impl FilmcraftApp {
    fn workspaces_path(&self) -> Option<std::path::PathBuf> {
        self.session.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(dock::WORKSPACES_FILE))
    }

    fn header_presets_path(&self) -> Option<std::path::PathBuf> {
        self.session.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(dock::HEADER_PRESETS_FILE))
    }

    fn custom_panels_path(&self) -> Option<std::path::PathBuf> {
        self.session.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(panels::custom_panels::CUSTOM_PANELS_FILE))
    }

    /// Persist user-defined custom panels to `custom_panels.json`.
    pub fn save_custom_panels(&self) -> Result<(), String> {
        match self.custom_panels_path() {
            Some(path) => self.custom_panels.save(&path).map_err(|e| format!("saving custom panels: {e}")),
            None => Ok(()),
        }
    }

    /// Replace the saved workspaces and write them to the data directory.
    pub fn set_workspaces(&mut self, w: dock::WorkspacePrefs) -> Result<(), String> {
        self.workspaces = w;
        match self.workspaces_path() {
            Some(p) => self.workspaces.save(&p).map_err(|e| format!("saving workspaces: {e}")),
            None => Ok(()),
        }
    }

    /// Replace the saved top-bar layout presets and write them to the data directory.
    pub fn set_header_presets(&mut self, p: dock::WorkspacePrefs) -> Result<(), String> {
        self.header_presets = p;
        match self.header_presets_path() {
            Some(path) => self.header_presets.save(&path).map_err(|e| format!("saving layout presets: {e}")),
            None => Ok(()),
        }
    }

    /// Apply a saved top-bar layout preset without modifying `Window ▸ Workspaces`.
    pub fn apply_header_preset(&mut self, name: &str) {
        if self.workspace_restored && self.ui.active_preset != name {
            self.save_current_workspace();
        }
        let Some(layout) = dock::saved_preset_docking(&self.header_presets, name) else {
            return;
        };
        self.ui.mode = state::Mode::Edit;
        self.ui.dock = layout;
        self.ui.active_preset = name.to_string();
        self.pending_panel_raise = None;
        self.ui.keys.maximized = None;
        if self.header_presets.current != name {
            let mut next = self.header_presets.clone();
            next.current = name.to_string();
            if let Err(e) = self.set_header_presets(next) {
                self.ui.status = e;
            }
        }
        self.last_saved_dock = Some((format!("preset:{name}"), self.ui.dock.clone()));
        self.dock_dirty_since = None;
    }

    pub(crate) fn rename_saved_dock_key(&mut self, from: &str, to: &str) {
        if let Some((key, _)) = self.last_saved_dock.as_mut()
            && key == from
        {
            *key = to.to_string();
        }
    }

    pub(crate) fn switch_workspace_discarding(&mut self, name: &str) {
        self.last_saved_dock = Some((self.current_dock_save_key(), self.ui.dock.clone()));
        self.dock_dirty_since = None;
        self.set_workspace(name);
    }

    pub fn set_workspace(&mut self, name: &str) {
        if self.workspace_restored && (self.ui.workspace != name || !self.ui.active_preset.is_empty()) {
            self.save_current_workspace();
        }
        self.ui.active_preset.clear();
        if !self.header_presets.current.is_empty() {
            let mut next_presets = self.header_presets.clone();
            next_presets.current.clear();
            let _ = self.set_header_presets(next_presets);
        }
        self.ui.workspace = name.to_string();
        self.ui.dock = dock::saved_docking(&self.workspaces, name);
        self.pending_panel_raise = None;
        self.ui.keys.maximized = None;
        if self.workspaces.current != name {
            let mut next = self.workspaces.clone();
            next.current = name.to_string();
            if let Err(e) = self.set_workspaces(next) {
                self.ui.status = e;
            }
        }
        if name == "Color" {
            self.ui.show_scopes = false;
        }
        self.last_saved_dock = Some((self.ui.workspace.clone(), self.ui.dock.clone()));
        self.dock_dirty_since = None;
    }

    fn current_dock_save_key(&self) -> String {
        if self.ui.active_preset.is_empty() { self.ui.workspace.clone() } else { format!("preset:{}", self.ui.active_preset) }
    }

    /// Persist the current dock layout for the active workspace or preset when it differs from disk.
    pub(crate) fn save_current_workspace(&mut self) {
        if !self.workspace_restored {
            return;
        }
        let save_key = self.current_dock_save_key();
        if self.last_saved_dock.as_ref().is_some_and(|(key, layout)| key == &save_key && layout == &self.ui.dock) {
            self.dock_dirty_since = None;
            return;
        }
        if !self.ui.active_preset.is_empty() {
            let preset_name = self.ui.active_preset.clone();
            let Ok(entry) = dock::SavedWorkspace::new(&preset_name, &self.ui.dock) else {
                return;
            };
            let mut next = self.header_presets.clone();
            match next.saved.iter_mut().find(|s| s.name == preset_name) {
                Some(s) => *s = entry,
                None => next.saved.push(entry),
            }
            next.current = preset_name;
            if let Err(e) = self.set_header_presets(next) {
                self.ui.status = e;
            } else {
                self.last_saved_dock = Some((save_key, self.ui.dock.clone()));
                self.dock_dirty_since = None;
            }
            return;
        }
        let Ok(entry) = dock::SavedWorkspace::new(&self.ui.workspace, &self.ui.dock) else {
            return;
        };
        let mut next = self.workspaces.clone();
        match next.saved.iter_mut().find(|s| s.name == self.ui.workspace) {
            Some(s) => *s = entry,
            None => next.saved.push(entry),
        }
        next.current = self.ui.workspace.clone();
        if let Err(e) = self.set_workspaces(next) {
            self.ui.status = e;
        } else {
            self.last_saved_dock = Some((save_key, self.ui.dock.clone()));
            self.dock_dirty_since = None;
        }
    }

    /// Debounced auto-save of layout changes (~1s after the user stops dragging/resizing).
    pub(crate) fn maybe_persist_workspace(&mut self, ctx: &egui::Context) {
        if !self.workspace_restored {
            return;
        }
        let save_key = self.current_dock_save_key();
        let same = self.last_saved_dock.as_ref().is_some_and(|(key, layout)| key == &save_key && layout == &self.ui.dock);
        if same {
            self.dock_dirty_since = None;
            return;
        }
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        if pointer_down {
            self.dock_dirty_since = Some(web_time::Instant::now());
            return;
        }
        let now = web_time::Instant::now();
        let since = *self.dock_dirty_since.get_or_insert(now);
        if now.duration_since(since) >= std::time::Duration::from_secs(1) {
            self.save_current_workspace();
        }
    }

    pub fn show_panel(&mut self, p: PanelKind) {
        // closed while floating: it floats again where it was (#1985); taken before the Timeline's
        // own restore below docks it
        let open = self.ui.dock.contains(&p);
        let refloat = match self.ui.closed_floating.iter().position(|(k, _)| *k == p) {
            Some(i) if !open => Some(self.ui.closed_floating.remove(i).1),
            _ => None,
        };
        if p == PanelKind::Timeline {
            self.ui.dock.restore_timeline();
        }
        if !self.ui.dock.contains(&p) {
            let near = match p {
                // a side panel beside Properties (or Essential Sound), never over the Program monitor
                PanelKind::TextToSpeech => [PanelKind::Properties, PanelKind::EssentialSound, PanelKind::EssentialGraphics]
                    .into_iter()
                    .find(|k| self.ui.dock.contains(k))
                    .unwrap_or(PanelKind::Program),
                PanelKind::LumetriColor
                | PanelKind::EssentialGraphics
                | PanelKind::EssentialSound
                | PanelKind::InspectorVideo
                | PanelKind::InspectorAudio
                | PanelKind::Properties => PanelKind::Program,
                PanelKind::Source
                | PanelKind::EffectControls
                | PanelKind::AudioClipMixer
                | PanelKind::Metadata
                | PanelKind::LumetriScopes
                | PanelKind::AudioTrackMixer
                | PanelKind::Text
                | PanelKind::ReferenceMonitor
                | PanelKind::Timecode => PanelKind::Source,
                _ => PanelKind::Project,
            };
            self.ui.dock.open_near(p, near);
        }
        if let Some(rect) = refloat.filter(|r| r.iter().all(|v| v.is_finite())) {
            let rect = [rect[0], rect[1], rect[2].max(160.0), rect[3].max(120.0)];
            if let Err(e) = self.apply_dock(craft_ui::docking::Action::Float { panel: p, rect }) {
                // refused (it is the only docked panel left): it stays docked, and its floating
                // place is kept rather than lost
                log::warn!("could not float {} again: {e}", p.id());
                self.ui.closed_floating.push((p, rect));
            }
        }
        let _ = self.apply_dock(craft_ui::docking::Action::Activate { panel: p });
        self.ui.focused = p;
    }

    /// Reveal in Project: bring the Project panel forward, showing the bin that holds `item` with
    /// the search cleared, so the (already selected) item is on show.
    pub(super) fn reveal_in_project(&mut self, item: filmcraft_project::ItemId) {
        // the bins on the way down to the item (bounded: a project file can nest bins arbitrarily deep)
        fn path(b: &filmcraft_project::Bin, item: filmcraft_project::ItemId, depth: usize, out: &mut Vec<u64>) -> bool {
            if depth > 256 {
                return false;
            }
            for c in &b.children {
                match c {
                    filmcraft_project::BinEntry::Item(i) if *i == item => return true,
                    filmcraft_project::BinEntry::Bin(inner) => {
                        out.push(inner.id.0);
                        if path(inner, item, depth + 1, out) {
                            return true;
                        }
                        out.pop();
                    }
                    _ => {}
                }
            }
            false
        }
        let mut bins = Vec::new();
        let found = path(&self.session.project.root, item, 0, &mut bins);
        // the list shows the whole tree: open the bins on the way. Icons and freeform show one bin
        // at a time: go into the one that holds the item.
        let list = self.session.prefs.project_panel.view.mode == filmcraft_engine::project_panel::ViewMode::List;
        self.ui.project_panel.bin = if list || !found { None } else { bins.last().copied() };
        if list {
            for b in bins {
                if !self.ui.expanded_bins.contains(&b) {
                    self.ui.expanded_bins.push(b);
                }
            }
        }
        self.ui.project_panel.active_tab = None;
        self.ui.project_panel.selected_bin = None;
        self.ui.project_search.clear();
        self.show_panel(PanelKind::Project);
    }
}
