//! User-defined composite panels (`PanelKind::Custom(id)`), stored globally in
//! `custom_panels.json` beside `preferences.json`. Each custom panel combines any selection of
//! built-in panels either as internal tabs or as vertical/horizontal splits, and can be docked,
//! floated, resized and saved in workspace presets just like built-in panels.

use std::path::Path;

use egui::{Align2, Rect, Sense, Stroke, Vec2, pos2};
use serde::{Deserialize, Serialize};

use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::theme::Tokens;

/// Persisted custom panels file beside `preferences.json`.
pub const CUSTOM_PANELS_FILE: &str = "custom_panels.json";
/// Maximum number of custom panel slots (`Custom(0)`..`Custom(15)`).
pub const MAX_CUSTOM_PANELS: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CustomPanelLayout {
    #[default]
    Tabs,
    SplitVertical,
    SplitHorizontal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomPanelDef {
    pub id: u8,
    pub name: String,
    pub panels: Vec<PanelKind>,
    pub layout: CustomPanelLayout,
    pub active_tab: usize,
}

impl Default for CustomPanelDef {
    fn default() -> Self {
        Self { id: 0, name: String::new(), panels: Vec::new(), layout: CustomPanelLayout::Tabs, active_tab: 0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomPanelStore {
    pub panels: Vec<CustomPanelDef>,
}

impl CustomPanelStore {
    pub fn load(path: &Path) -> Self {
        use std::io::Read as _;
        const MAX_BYTES: u64 = 2 * 1024 * 1024;
        let Some(bytes) = std::fs::File::open(path).ok().and_then(|file| {
            let mut bytes = Vec::new();
            file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
            (bytes.len() as u64 <= MAX_BYTES).then_some(bytes)
        }) else {
            return Self::default();
        };
        let mut store: Self = serde_json::from_slice(&bytes).unwrap_or_default();
        // Never allow recursive custom-in-custom panels.
        for def in &mut store.panels {
            def.panels.retain(|p| !matches!(p, PanelKind::Custom(_)));
        }
        store
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }

    pub fn get(&self, id: u8) -> Option<&CustomPanelDef> {
        self.panels.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: u8) -> Option<&mut CustomPanelDef> {
        self.panels.iter_mut().find(|p| p.id == id)
    }

    pub fn title(&self, id: u8) -> Option<&str> {
        self.get(id).map(|p| p.name.as_str()).filter(|n| !n.trim().is_empty())
    }

    pub fn next_slot(&self) -> Option<u8> {
        (0..MAX_CUSTOM_PANELS as u8).find(|id| !self.panels.iter().any(|p| p.id == *id))
    }
}

/// State for the Custom Panels editor dialog.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomPanelDialog {
    pub selected: Option<u8>,
}

/// Render a custom panel inside `rect`.
pub fn show_custom_panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, id: u8) {
    let t = app.tokens;
    let Some(def) = app.custom_panels.get(id).cloned() else {
        empty_state(app, ui, rect, id, &t);
        return;
    };
    let sub_panels: Vec<PanelKind> = def.panels.iter().copied().filter(|p| !matches!(p, PanelKind::Custom(_))).collect();
    if sub_panels.is_empty() {
        empty_state(app, ui, rect, id, &t);
        return;
    }

    match def.layout {
        CustomPanelLayout::Tabs => {
            let bar_h = 26.0_f32.min(rect.height());
            let bar_rect = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + bar_h));
            let body_rect = Rect::from_min_max(pos2(rect.min.x, bar_rect.max.y), rect.max);

            ui.painter().rect_filled(bar_rect, 0.0, t.tab_bg);
            ui.painter().line_segment([pos2(bar_rect.min.x, bar_rect.max.y), bar_rect.max], Stroke::new(1.0, t.separator));

            let gear_w = 24.0;
            let gear_rect = Rect::from_min_max(pos2((bar_rect.max.x - gear_w).max(bar_rect.min.x), bar_rect.min.y), bar_rect.max);
            let gear_resp = ui
                .interact(gear_rect, egui::Id::unique(("custom-panel-edit", id)), Sense::click())
                .on_hover_text(tl!("Edit Custom Panels…"));
            let gear_col = if gear_resp.hovered() { t.text } else { t.text_dim };
            ui.painter().text(gear_rect.center(), Align2::CENTER_CENTER, "⚙", Tokens::ui(12.0), gear_col);
            if gear_resp.clicked() {
                app.ui.custom_panel_dialog = Some(CustomPanelDialog { selected: Some(id) });
            }

            let tabs_rect = Rect::from_min_max(bar_rect.min, pos2(gear_rect.min.x, bar_rect.max.y));
            let active_idx = def.active_tab.min(sub_panels.len().saturating_sub(1));
            let mut next_active = active_idx;
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(tabs_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
            child.set_clip_rect(tabs_rect.intersect(ui.clip_rect()));
            egui::ScrollArea::horizontal().id_salt(("custom-panel-tabs", id)).show(&mut child, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, sub) in sub_panels.iter().enumerate() {
                    let selected = i == active_idx;
                    let title = crate::i18n::t(sub.title());
                    let text_col = if selected { t.tab_text_active } else { t.tab_text };
                    let resp = ui.selectable_label(selected, egui::RichText::new(title).font(Tokens::ui(11.5)).color(text_col));
                    if resp.clicked() {
                        next_active = i;
                    }
                }
            });
            if next_active != def.active_tab
                && let Some(entry) = app.custom_panels.get_mut(id)
            {
                entry.active_tab = next_active;
                let _ = app.save_custom_panels();
            }

            if body_rect.height() > 2.0
                && let Some(&active_panel) = sub_panels.get(next_active)
            {
                crate::panels::show(app, ui, active_panel, body_rect);
            }
        }
        CustomPanelLayout::SplitVertical => {
            let n = sub_panels.len() as f32;
            let gap = 2.0;
            let total_gaps = gap * (sub_panels.len().saturating_sub(1) as f32);
            let each_h = ((rect.height() - total_gaps) / n).max(1.0);
            for (i, &sub) in sub_panels.iter().enumerate() {
                let y0 = rect.min.y + i as f32 * (each_h + gap);
                let y1 = if i + 1 == sub_panels.len() { rect.max.y } else { (y0 + each_h).min(rect.max.y) };
                let sub_rect = Rect::from_min_max(pos2(rect.min.x, y0), pos2(rect.max.x, y1));
                crate::panels::show(app, ui, sub, sub_rect);
                if i + 1 < sub_panels.len() {
                    ui.painter().line_segment([pos2(rect.min.x, y1 + 0.5), pos2(rect.max.x, y1 + 0.5)], Stroke::new(1.0, t.separator));
                }
            }
        }
        CustomPanelLayout::SplitHorizontal => {
            let n = sub_panels.len() as f32;
            let gap = 2.0;
            let total_gaps = gap * (sub_panels.len().saturating_sub(1) as f32);
            let each_w = ((rect.width() - total_gaps) / n).max(1.0);
            for (i, &sub) in sub_panels.iter().enumerate() {
                let x0 = rect.min.x + i as f32 * (each_w + gap);
                let x1 = if i + 1 == sub_panels.len() { rect.max.x } else { (x0 + each_w).min(rect.max.x) };
                let sub_rect = Rect::from_min_max(pos2(x0, rect.min.y), pos2(x1, rect.max.y));
                crate::panels::show(app, ui, sub, sub_rect);
                if i + 1 < sub_panels.len() {
                    ui.painter().line_segment([pos2(x1 + 0.5, rect.min.y), pos2(x1 + 0.5, rect.max.y)], Stroke::new(1.0, t.separator));
                }
            }
        }
    }
}

fn empty_state(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, id: u8, t: &Tokens) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    child.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((rect.height() * 0.35).max(12.0));
            ui.label(egui::RichText::new(tl!("Empty custom panel")).color(t.text_dim));
            ui.add_space(8.0);
            if ui.button(tl!("Configure Panel…")).clicked() {
                if app.custom_panels.get(id).is_none() {
                    app.custom_panels.panels.push(CustomPanelDef {
                        id,
                        name: format!("{} {}", tl!("Custom Panel"), id + 1),
                        panels: vec![PanelKind::Project, PanelKind::Effects],
                        layout: CustomPanelLayout::Tabs,
                        active_tab: 0,
                    });
                    let _ = app.save_custom_panels();
                }
                app.ui.custom_panel_dialog = Some(CustomPanelDialog { selected: Some(id) });
            }
        });
    });
}

/// Modal dialog for creating, editing, reordering and opening custom panels.
pub fn dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.custom_panel_dialog.clone() else { return };
    let mut close = crate::widgets::escape_closes(ctx);
    let mut changed = false;
    let mut open_slot: Option<u8> = None;

    if d.selected.is_none_or(|id| app.custom_panels.get(id).is_none()) {
        d.selected = app.custom_panels.panels.first().map(|p| p.id);
    }

    egui::Window::new(tl!("Custom Panels"))
        .id(egui::Id::unique("custom-panels-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_size(Vec2::new(640.0, 440.0))
        .pivot(Align2::CENTER_CENTER)
        .default_pos(crate::widgets::dialog_center(ctx))
        .show(ctx, |ui| {
            ui.columns(2, |cols| {
                // Left: list of custom panels + New / Delete
                cols[0].vertical(|ui| {
                    ui.label(egui::RichText::new(tl!("Saved Custom Panels")).strong());
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().id_salt("custom-panels-list").max_height(310.0).show(ui, |ui| {
                        if app.custom_panels.panels.is_empty() {
                            ui.label(egui::RichText::new(tl!("No custom panels yet")).weak());
                        }
                        for def in &app.custom_panels.panels {
                            let label = if def.name.trim().is_empty() { tl!("Custom Panel") } else { def.name.as_str() };
                            let sel = d.selected == Some(def.id);
                            if ui.selectable_label(sel, label).clicked() {
                                d.selected = Some(def.id);
                            }
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let can_add = app.custom_panels.next_slot().is_some();
                        if ui.add_enabled(can_add, egui::Button::new(tl!("+ New Panel"))).clicked()
                            && let Some(slot) = app.custom_panels.next_slot()
                        {
                            let num = app.custom_panels.panels.len() + 1;
                            app.custom_panels.panels.push(CustomPanelDef {
                                id: slot,
                                name: format!("{} {num}", tl!("Custom Panel")),
                                panels: vec![PanelKind::Project, PanelKind::MediaBrowser, PanelKind::Effects],
                                layout: CustomPanelLayout::Tabs,
                                active_tab: 0,
                            });
                            d.selected = Some(slot);
                            changed = true;
                        }
                        let can_del = d.selected.is_some();
                        if ui.add_enabled(can_del, egui::Button::new(tl!("Delete"))).clicked()
                            && let Some(sel_id) = d.selected
                        {
                            app.custom_panels.panels.retain(|p| p.id != sel_id);
                            let _ = app.ui.dock.apply(craft_ui::docking::Action::Close { panel: PanelKind::Custom(sel_id) });
                            d.selected = app.custom_panels.panels.first().map(|p| p.id);
                            changed = true;
                        }
                    });
                });

                // Right: editor for the selected custom panel
                cols[1].vertical(|ui| {
                    let Some(sel_id) = d.selected else {
                        ui.label(tl!("Create a custom panel on the left to configure its panels."));
                        return;
                    };
                    let Some(def) = app.custom_panels.get_mut(sel_id) else { return };

                    ui.horizontal(|ui| {
                        ui.label(tl!("Name:"));
                        if ui.text_edit_singleline(&mut def.name).changed() {
                            changed = true;
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(tl!("Layout:"));
                        changed |= ui.selectable_value(&mut def.layout, CustomPanelLayout::Tabs, tl!("Tabs")).changed();
                        changed |= ui.selectable_value(&mut def.layout, CustomPanelLayout::SplitVertical, tl!("Vertical Split")).changed();
                        changed |= ui.selectable_value(&mut def.layout, CustomPanelLayout::SplitHorizontal, tl!("Horizontal Split")).changed();
                    });
                    ui.separator();
                    ui.label(egui::RichText::new(tl!("Included Panels")).strong());
                    let mut move_up: Option<usize> = None;
                    let mut move_down: Option<usize> = None;
                    let mut remove_idx: Option<usize> = None;
                    let len = def.panels.len();
                    egui::ScrollArea::vertical().id_salt("custom-panel-included").max_height(120.0).show(ui, |ui| {
                        if def.panels.is_empty() {
                            ui.label(egui::RichText::new(tl!("Add panels from the list below")).weak());
                        }
                        for (i, p) in def.panels.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(crate::i18n::t(p.title()));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("✕").clicked() {
                                        remove_idx = Some(i);
                                    }
                                    if ui.add_enabled(i + 1 < len, egui::Button::new("▼").small()).clicked() {
                                        move_down = Some(i);
                                    }
                                    if ui.add_enabled(i > 0, egui::Button::new("▲").small()).clicked() {
                                        move_up = Some(i);
                                    }
                                });
                            });
                        }
                    });
                    if let Some(i) = move_up && i > 0 {
                        def.panels.swap(i - 1, i);
                        changed = true;
                    }
                    if let Some(i) = move_down && i + 1 < def.panels.len() {
                        def.panels.swap(i, i + 1);
                        changed = true;
                    }
                    if let Some(i) = remove_idx && i < def.panels.len() {
                        def.panels.remove(i);
                        changed = true;
                    }

                    ui.separator();
                    ui.label(egui::RichText::new(tl!("Add Built-in Panel")).strong());
                    egui::ScrollArea::vertical().id_salt("custom-panel-catalog").max_height(130.0).show(ui, |ui| {
                        for p in PanelKind::ALL {
                            let included = def.panels.contains(&p);
                            let label = if included {
                                format!("✓ {}", crate::i18n::t(p.title()))
                            } else {
                                format!("+ {}", crate::i18n::t(p.title()))
                            };
                            if ui.selectable_label(included, label).clicked() {
                                if included {
                                    def.panels.retain(|x| *x != p);
                                } else {
                                    def.panels.push(p);
                                }
                                changed = true;
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(tl!("Open in Workspace")).clicked() {
                            open_slot = Some(sel_id);
                        }
                        if ui.button(tl!("Close")).clicked() {
                            close = true;
                        }
                    });
                });
            });
        });

    if changed && let Err(e) = app.save_custom_panels() {
        app.ui.status = e;
    }
    if let Some(slot) = open_slot {
        app.show_panel(PanelKind::Custom(slot));
        close = true;
    }
    app.ui.custom_panel_dialog = if close { None } else { Some(d) };
}
