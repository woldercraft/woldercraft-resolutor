//! The header bar (Premiere 26 layout, 38 pt): Home and Import / Edit / Export on the left, the
//! document title centred, the workspace name and quick actions on the right. On macOS the menus
//! live in the native menu bar (set up by the app); elsewhere a compact in-window menu bar follows
//! the mode tabs.

use egui::{Color32, Rect, Sense, Stroke, pos2, vec2};

use crate::FilmcraftApp;
use crate::icons::{self, Icon};
use crate::state::Mode;
use crate::theme::Tokens;

/// Height of the header bar. With `integrated_titlebar` it sits in the macOS title-bar strip.
pub const HEIGHT: f32 = 38.0;

/// Draws the header bar in `rect` and handles its clicks: Home, the mode tabs, the workspace name
/// and its menu (the workspaces, then Reset to Saved Layout, Save Changes to this Workspace, Save as
/// New Workspace… and Edit Workspaces…), Discord and the quick actions.
pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.header_bg);
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, Color32::BLACK));
    let drag = ui.interact(rect, egui::Id::unique("header-drag"), Sense::click_and_drag());
    if drag.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
    }
    let mut x = rect.min.x + if app.integrated_titlebar { 104.0 } else { 14.0 };
    // Home (Projects)
    let home = Rect::from_center_size(pos2(x + 10.0, rect.center().y), vec2(26.0, 26.0));
    let hresp = ui.interact(home, egui::Id::unique("hdr-home"), Sense::click()).on_hover_text(tl!("Projects"));
    app.auto.add("header.home", home, "Home");
    if hresp.hovered() {
        p.rect_filled(home, 4.0, t.hover);
    }
    icons::paint(&p, home.shrink(5.0), Icon::Home, if hresp.hovered() { t.text } else { t.text_dim });
    if hresp.clicked() {
        app.ui.mode = Mode::Import;
    }
    x = home.max.x + 16.0;
    let mut left_end = x;
    // In-window menus when there is no native menu bar.
    if app.ui.show_menu_bar {
        let menu_rect = Rect::from_min_max(pos2(x + 6.0, rect.min.y + 7.0), pos2(x + 520.0, rect.max.y - 7.0));
        let mut mu = ui.new_child(egui::UiBuilder::new().max_rect(menu_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        mu.style_mut().visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        mu.style_mut().visuals.widgets.inactive.bg_stroke = Stroke::NONE;
        mu.style_mut().visuals.override_text_color = Some(t.text_dim);
        crate::menus::menu_bar(app, &mut mu);
        left_end = mu.min_rect().max.x;
    }
    // Document title, centred
    let title = if app.session.is_dirty() { tlf!("{name} - Edited", name = app.session.project.name) } else { app.session.project.name.clone() };
    // Right cluster: icons at ~38 pt pitch, then workspace name in caps.
    let mut rx = rect.max.x - 14.0;
    let mut btn = |ui: &mut egui::Ui, icon: Icon, id: &str, tip: &str, app: &mut FilmcraftApp| -> egui::Response {
        let r = Rect::from_center_size(pos2(rx - 14.0, rect.center().y), vec2(28.0, 28.0));
        rx -= 38.0;
        let resp = ui.interact(r, egui::Id::unique(("hdr", id)), Sense::click()).on_hover_text(tip);
        app.auto.add(&format!("header.{id}"), r, tip);
        if resp.hovered() {
            ui.painter().rect_filled(r, 4.0, t.hover);
        }
        icons::paint(ui.painter(), r.shrink(6.0), icon, if resp.hovered() { t.text } else { t.text_dim });
        resp
    };
    if btn(ui, Icon::Fullscreen, "fullscreen", tl!("Full screen"), app).clicked() {
        let fs = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
    }
    if btn(ui, Icon::Speaker, "volume", tl!("Volume"), app).clicked() {
        app.ui.status = tl!("Master volume: use the Audio Track Mixer").into();
    }
    if btn(ui, Icon::Search, "search", tl!("Search"), app).clicked() {
        app.show_panel(crate::dock::PanelKind::Effects);
    }
    if btn(ui, Icon::Bell, "notifications", tl!("Progress"), app).clicked() {
        app.ui.mode = Mode::Export;
    }
    // Quick Export: a popup with File Name & Location, a preset list and Export (Premiere 26)
    let qx = rect.max.x - 14.0 - 4.0 * 38.0; // the fifth button from the right
    if btn(ui, Icon::Export, "quickExport", tl!("Quick Export"), app).clicked() {
        app.ui.export.quick_open = !app.ui.export.quick_open;
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("quick-export-toggled"), true));
    }
    crate::panels::export_mode::quick_export(app, ui.ctx(), pos2(qx - 340.0, rect.max.y + 4.0));
    // Appearance Mode: click for the next one (Auto, Light, Dark); the icon shows the current one.
    let (icon, mode) = match app.session.prefs.appearance.appearance_mode.as_str() {
        "auto" => (Icon::Monitor, tl!("Sync with system")),
        "light" => (Icon::Sun, tl!("Light")),
        _ => (Icon::Moon, tl!("Dark")),
    };
    let tip = format!("{}: {mode}", tl!("Appearance Mode"));
    let appearance = btn(ui, icon, "appearance", &tip, app);
    appearance.context_menu(|ui| crate::panels::settings::appearance_context_menu(app, ui));
    if appearance.clicked() {
        crate::panels::settings::cycle_appearance(app, ui.ctx());
    }
    let ws_resp = btn(ui, Icon::Workspaces, "workspaces", tl!("Layout Presets"), app);
    // active layout preset name (caps), or "PRESETS" when empty
    let preset_label = if app.ui.active_preset.is_empty() { tl!("Layout Presets").to_string() } else { app.ui.active_preset.clone() };
    let ws = crate::i18n::to_uppercase(&preset_label);
    let wg = p.layout_no_wrap(ws.clone(), Tokens::ui(11.0), t.text_dim);
    let wr = Rect::from_min_size(pos2(rx - wg.size().x + 10.0, rect.center().y - 10.0), vec2(wg.size().x + 8.0, 20.0));
    let wresp = ui.interact(wr, egui::Id::unique("hdr-ws-name"), Sense::click());
    app.auto.add("header.workspaceName", wr, &ws);
    p.galley_with_override_text_color(pos2(wr.min.x + 4.0, rect.center().y - wg.size().y / 2.0), wg, if wresp.hovered() { t.text } else { t.text_dim });
    // Inspector toggle button (DaVinci Resolve-style)
    let insp_open = crate::dock::LayoutExt::is_inspector_open(&app.ui.dock);
    let insp_label = tl!("Inspector");
    let insp_g = p.layout_no_wrap(insp_label.to_string(), Tokens::semibold(11.0), if insp_open { Color32::WHITE } else { t.text_dim });
    let insp_w = insp_g.size().x + 20.0;
    let insp_r = Rect::from_min_size(pos2(wr.min.x - insp_w - 10.0, rect.center().y - 11.0), vec2(insp_w, 22.0));
    let insp_resp = ui
        .interact(insp_r, egui::Id::unique("hdr-inspector"), Sense::click())
        .on_hover_text(tl!("Show or hide the Inspector (Video / Audio / Properties)"));
    app.auto.add("header.inspector", insp_r, insp_label);
    let insp_bg = if insp_open {
        if insp_resp.hovered() { t.accent_hover } else { t.accent.gamma_multiply(0.85) }
    } else if insp_resp.hovered() {
        t.hover
    } else {
        t.tl_header_bg
    };
    ui.painter().rect_filled(insp_r, 4.0, insp_bg);
    ui.painter().rect_stroke(
        insp_r,
        4.0,
        Stroke::new(1.0, if insp_open { t.accent } else { t.separator }),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        pos2(insp_r.center().x - insp_g.size().x * 0.5, insp_r.center().y - insp_g.size().y * 0.5),
        insp_g,
        if insp_open { Color32::WHITE } else { t.text_dim },
    );
    if insp_resp.clicked() {
        crate::dock::LayoutExt::toggle_inspector(&mut app.ui.dock);
    }

    // Localized menus and user titles can be wider than English. Use the actual gap.
    let left = left_end + 12.0;
    let right = insp_r.min.x - 12.0;
        if right > left + 40.0 {
            let mut job = egui::text::LayoutJob::simple_singleline(title, Tokens::ui(14.0), t.tab_text_active);
            job.wrap.max_width = right - left;
            job.wrap.max_rows = 1;
            let galley = p.layout_job(job);
            let half = galley.size().x / 2.0;
            let center = if rect.center().x - half >= left && rect.center().x + half <= right { rect.center().x } else { (left + right) / 2.0 };
            p.with_clip_rect(Rect::from_min_max(pos2(left, rect.min.y), pos2(right, rect.max.y))).galley(
                pos2(center - half, rect.center().y - galley.size().y / 2.0),
                galley,
                t.tab_text_active,
            );
    }
    let popup_id = egui::Id::unique("workspaces-popup");
    if ws_resp.clicked() || wresp.clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(popup_id, true));
    }
    let open = ui.ctx().data(|d| d.get_temp::<bool>(popup_id).unwrap_or(false));
    if open {
        let anchor = pos2(wr.min.x, rect.max.y + 4.0);
        let area = egui::Area::new(popup_id.with("area")).order(egui::Order::Foreground).fixed_pos(anchor).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                    ui.set_min_width(220.0);
                    let preset_names = crate::dock::preset_names(&app.header_presets);
                    if preset_names.is_empty() {
                        ui.add_enabled(false, egui::Label::new(tl!("No saved presets")));
                    } else {
                        for p_name in &preset_names {
                            let sel = app.ui.active_preset == *p_name;
                            if ui.selectable_label(sel, p_name).clicked() {
                                app.apply_header_preset(p_name);
                                ui.ctx().data_mut(|d| d.insert_temp(popup_id, false));
                            }
                        }
                    }
                    ui.separator();
                    let has_active = !app.ui.active_preset.is_empty()
                        && app.header_presets.saved.iter().any(|s| s.name == app.ui.active_preset);
                    if has_active {
                        let reset_resp = ui.button(tl!("Reset Preset"));
                        app.auto.add("header.presets.reset", reset_resp.rect, tl!("Reset Preset"));
                        if reset_resp.clicked() {
                            let active = app.ui.active_preset.clone();
                            app.apply_header_preset(&active);
                            ui.ctx().data_mut(|d| d.insert_temp(popup_id, false));
                        }
                        let save_resp = ui.button(tl!("Save Changes to this Preset"));
                        app.auto.add("header.presets.saveChanges", save_resp.rect, tl!("Save Changes to this Preset"));
                        if save_resp.clicked() {
                            let active = app.ui.active_preset.clone();
                            if let Err(e) = crate::panels::workspaces::save_header_preset(app, &active) {
                                app.ui.status = e;
                            }
                            ui.ctx().data_mut(|d| d.insert_temp(popup_id, false));
                        }
                    }
                    let save_as_resp = ui.button(tl!("Save as New Preset…"));
                    app.auto.add("header.presets.saveAs", save_as_resp.rect, tl!("Save as New Preset…"));
                    if save_as_resp.clicked() {
                        app.ui.preset_dialog = Some(crate::state::WorkspaceDialog::SaveAs { name: tl!("Untitled Preset").into() });
                        ui.ctx().data_mut(|d| d.insert_temp(popup_id, false));
                    }
                    let edit_resp = ui.add_enabled(!preset_names.is_empty(), egui::Button::new(tl!("Edit Presets…")));
                    app.auto.add("header.presets.edit", edit_resp.rect, tl!("Edit Presets…"));
                    if edit_resp.clicked() {
                        app.ui.preset_dialog = Some(crate::state::WorkspaceDialog::Edit { selected: None, name: String::new() });
                        ui.ctx().data_mut(|d| d.insert_temp(popup_id, false));
                    }
                });
            });
        });
        if area.response.clicked_elsewhere() && !(ws_resp.clicked() || wresp.clicked()) {
            ui.ctx().data_mut(|d| d.insert_temp(popup_id, false));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_name_uppercasing_is_turkish_aware() {
        crate::i18n::set_current(crate::i18n::Language::Tr);
        assert_eq!(crate::i18n::to_uppercase("Birleştirme"), "BİRLEŞTİRME");
        assert_eq!(crate::i18n::to_uppercase("Altyazılar ve Grafikler"), "ALTYAZILAR VE GRAFİKLER");
        crate::i18n::set_current(crate::i18n::Language::En);
        assert_eq!(crate::i18n::to_uppercase("Birleştirme"), "BIRLEŞTIRME");
    }
}
