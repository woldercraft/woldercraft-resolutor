use egui::{Align2, Color32, FontId, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::EffectInstance;

use super::{ALL_TOOLS, draw_tool_icon, get_tool_def, tool_for_panel};
use crate::FilmcraftApp;

pub(super) fn render_tool_buttons(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    e: &EffectInstance,
    current_panel: crate::dock::PanelKind,
    vertical: bool,
    sections: &mut Vec<&'static str>,
) {
    let t = app.tokens;
    let num_tools = app.ui.lumetri_tool_order.len();
    let current_tool_id = tool_for_panel(current_panel).map(String::from).unwrap_or_else(|| super::normalize_tool_id(&app.ui.lumetri_active_tool).to_string());

    let drag_key = egui::Id::unique(("lumetri_tool_reorder", current_panel));
    let mut dragging_idx: Option<usize> = ui.data(|d| d.get_temp(drag_key));
    let mut item_rects: Vec<(usize, egui::Rect, crate::dock::PanelKind)> = Vec::with_capacity(num_tools);
    let mut detach_panel: Option<crate::dock::PanelKind> = None;

    for i in 0..num_tools {
        let Some(tool_id) = app.ui.lumetri_tool_order.get(i).cloned() else { continue };
        let Some(def) = get_tool_def(&tool_id) else { continue };

        let is_primary = def.id == current_tool_id;
        let is_secondary = current_panel == crate::dock::PanelKind::LumetriColor
            && app.ui.lumetri_dual_view
            && super::normalize_tool_id(&app.ui.lumetri_secondary_tool) == def.id
            && def.id != current_tool_id;
        let is_docked_elsewhere = def.panel_kind() != current_panel
            && (app.ui.dock.contains(&def.panel_kind()) || app.ui.dock.floating.iter().any(|g| g.contains(&def.panel_kind())));
        let is_on = e.param(def.param).and_then(|p| p.value.as_bool()).unwrap_or(def.param != "hsl_on");

        let label_text = crate::i18n::t(def.label);
        let btn_w = if vertical {
            148.0
        } else {
            let text_w = ui.painter().layout_no_wrap(label_text.to_string(), FontId::proportional(11.0), Color32::WHITE).size().x;
            (text_w + 54.0).clamp(84.0, 180.0)
        };
        let btn_size = vec2(btn_w, 26.0);
        let (r, resp) = ui.allocate_exact_size(btn_size, Sense::click_and_drag());
        item_rects.push((i, r, def.panel_kind()));

        if resp.drag_started() {
            dragging_idx = Some(i);
        }
        if resp.dragged() && dragging_idx == Some(i) {
            let strip_bounds = ui.clip_rect().expand(14.0);
            if let Some(pos) = resp.interact_pointer_pos()
                && !strip_bounds.contains(pos)
            {
                detach_panel = Some(def.panel_kind());
                dragging_idx = None;
            }
        }

        // Draw button background & border
        let bg_color = if is_primary {
            t.accent.gamma_multiply(0.24)
        } else if is_secondary {
            t.accent.gamma_multiply(0.12)
        } else if resp.hovered() {
            Color32::from_rgb(38, 42, 52)
        } else {
            Color32::from_rgb(26, 29, 36)
        };

        let border_stroke = if is_primary {
            Stroke::new(1.5, t.accent)
        } else if is_secondary {
            Stroke::new(1.0, Color32::from_rgb(160, 175, 200))
        } else if is_docked_elsewhere {
            Stroke::new(1.0, Color32::from_rgb(60, 110, 160))
        } else if resp.hovered() {
            Stroke::new(1.0, Color32::from_rgb(70, 75, 90))
        } else {
            Stroke::new(1.0, Color32::from_rgb(42, 46, 56))
        };

        ui.painter().rect(r, 4.0, bg_color, border_stroke, StrokeKind::Inside);

        // 1. Clean resolution-independent 6-dot drag grip handle (no missing glyph □□)
        let dot_col = if resp.hovered() || resp.dragged() { Color32::from_rgb(180, 190, 210) } else { Color32::from_rgb(75, 80, 95) };
        let gx = r.min.x + 7.0;
        let gy = r.center().y;
        for col in 0..2 {
            for row in 0..3 {
                let px = gx + col as f32 * 3.5;
                let py = gy + (row as f32 - 1.0) * 3.5;
                ui.painter().circle_filled(pos2(px, py), 1.0, dot_col);
            }
        }

        // 2. Crisp vector icon drawn cleanly (no missing glyph □)
        let text_color = if is_primary {
            Color32::WHITE
        } else if resp.hovered() {
            t.text
        } else {
            t.text_dim
        };
        draw_tool_icon(ui.painter(), pos2(r.min.x + 22.0, r.center().y), def.id, text_color);

        // 3. Translated tool label
        ui.painter().text(pos2(r.min.x + 32.0, r.center().y), Align2::LEFT_CENTER, label_text, FontId::proportional(11.0), text_color);

        // 4. Switch / Bypass indicator dot on the right of the button
        let sw_center = pos2(r.max.x - 11.0, r.center().y);
        let sw_rect = egui::Rect::from_center_size(sw_center, vec2(14.0, 14.0));
        let sw_resp = ui.interact(sw_rect.expand(2.0), ui.scope_id().with("sw_btn").with(i), Sense::click());

        let dot_color = if is_on { Color32::from_rgb(52, 211, 153) } else { Color32::from_rgb(65, 70, 82) };
        ui.painter().circle_filled(sw_center, 4.0, dot_color);
        if is_on {
            ui.painter().circle_stroke(sw_center, 5.5, Stroke::new(1.0, Color32::from_rgb(52, 211, 153).gamma_multiply(0.4)));
        }
        if sw_resp.clicked() {
            sections.push(def.sec);
        }

        // 5. Click button to switch this panel exclusively to the selected tool (drag tab to detach/open alongside)
        if resp.clicked() && !sw_resp.clicked() {
            if current_panel == crate::dock::PanelKind::LumetriColor {
                let shift = ui.ctx().input(|inp| inp.modifiers.shift);
                if app.ui.lumetri_dual_view && shift && def.id != super::normalize_tool_id(&app.ui.lumetri_active_tool) {
                    app.ui.lumetri_secondary_tool = def.id.to_string();
                } else {
                    app.ui.lumetri_active_tool = def.id.to_string();
                }
            } else {
                app.switch_color_window(current_panel, def.panel_kind(), def.id);
            }
        }

        // Hover tooltip explaining click, Shift+click (dual view), and drag
        let dual_hint = if current_panel == crate::dock::PanelKind::LumetriColor && app.ui.lumetri_dual_view {
            format!("\n{}", tl!("• Shift+Click: Set as Dual View secondary tool"))
        } else {
            String::new()
        };
        let hover_msg = format!(
            "{}\n{}{}",
            crate::i18n::t(def.tooltip),
            tlf!("• Click: Switch this window to {label}\n• Drag: Reorder in bar, or drag out to float/dock", label = crate::i18n::t(def.label)),
            dual_hint
        );
        resp.on_hover_text(hover_msg);

        // Automation registers
        app.auto.add(&format!("lumetri.section.{}", def.id), r, def.id);
        app.auto.add(&format!("lumetri.switch.{}", def.id), sw_rect.expand(4.0), &format!("{} on/off", def.id));
        if def.id == "Power Windows" {
            app.auto.add("lumetri.section.Masks", r, "Masks");
            app.auto.add("lumetri.switch.Masks", sw_rect.expand(4.0), "Masks on/off");
        }
    }

    finish_tool_reorder(ui, app, current_panel, vertical, drag_key, detach_panel, dragging_idx, &item_rects, &t);
}

#[allow(clippy::too_many_arguments)]
fn finish_tool_reorder(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    current_panel: crate::dock::PanelKind,
    vertical: bool,
    drag_key: egui::Id,
    detach_panel: Option<crate::dock::PanelKind>,
    mut dragging_idx: Option<usize>,
    item_rects: &[(usize, egui::Rect, crate::dock::PanelKind)],
    t: &crate::theme::Tokens,
) {
    if let Some(target_panel) = detach_panel {
        app.begin_panel_drag(ui.ctx(), target_panel, current_panel);
    } else if let Some(from_idx) = dragging_idx {
        let pointer_down = ui.ctx().input(|inp| inp.pointer.any_down());
        let pointer_pos = ui.ctx().input(|inp| inp.pointer.interact_pos().or(inp.pointer.hover_pos()));
        if let Some(pos) = pointer_pos
            && let Some((to_idx, target_r)) = item_rects.iter().find(|(_, r, _)| r.contains(pos)).map(|(idx, r, _)| (*idx, *r))
        {
            if to_idx != from_idx {
                if vertical {
                    let y = if to_idx < from_idx { target_r.min.y } else { target_r.max.y };
                    ui.painter().line_segment([pos2(target_r.min.x, y), pos2(target_r.max.x, y)], Stroke::new(2.0, t.accent));
                } else {
                    let x = if to_idx < from_idx { target_r.min.x } else { target_r.max.x };
                    ui.painter().line_segment([pos2(x, target_r.min.y), pos2(x, target_r.max.y)], Stroke::new(2.0, t.accent));
                }
            }
            if !pointer_down && to_idx != from_idx && from_idx < app.ui.lumetri_tool_order.len() && to_idx < app.ui.lumetri_tool_order.len() {
                let moved = app.ui.lumetri_tool_order.remove(from_idx);
                app.ui.lumetri_tool_order.insert(to_idx, moved);
            }
        }
        if !pointer_down {
            dragging_idx = None;
        }
    }
    ui.data_mut(|d| {
        if let Some(idx) = dragging_idx {
            d.insert_temp(drag_key, idx);
        } else {
            d.remove::<usize>(drag_key);
        }
    });
}

pub(super) fn render_palette_toolbar(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    e: &EffectInstance,
    current_panel: crate::dock::PanelKind,
    vertical: bool,
    sections: &mut Vec<&'static str>,
) {
    for id in &mut app.ui.lumetri_tool_order {
        *id = super::normalize_tool_id(id).to_string();
    }
    let mut seen = std::collections::HashSet::new();
    app.ui.lumetri_tool_order.retain(|id| ALL_TOOLS.iter().any(|def| def.id == id) && seen.insert(id.clone()));
    for def in &ALL_TOOLS {
        if !app.ui.lumetri_tool_order.iter().any(|id| id == def.id) {
            app.ui.lumetri_tool_order.push(def.id.to_string());
        }
    }

    if vertical {
        ui.vertical(|ui| {
            render_tool_buttons(ui, app, e, current_panel, true, sections);
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let cur_pos = app.ui.lumetri_bar_pos.clone();
                let r_top = ui.selectable_label(cur_pos == "top", tl!("Top")).on_hover_text(tl!("Top bar"));
                app.auto.add("lumetri.bar.top", r_top.rect, "Top");
                if r_top.clicked() {
                    app.ui.lumetri_bar_pos = "top".into();
                }
                let r_bot = ui.selectable_label(cur_pos == "bottom", tl!("Bottom")).on_hover_text(tl!("Bottom bar"));
                app.auto.add("lumetri.bar.bottom", r_bot.rect, "Bottom");
                if r_bot.clicked() {
                    app.ui.lumetri_bar_pos = "bottom".into();
                }
                let r_left = ui.selectable_label(cur_pos == "left", tl!("Left")).on_hover_text(tl!("Left sidebar"));
                app.auto.add("lumetri.bar.left", r_left.rect, "Left");
                if r_left.clicked() {
                    app.ui.lumetri_bar_pos = "left".into();
                }
                let dual_text = if app.ui.lumetri_dual_view { tl!("Dual ✓") } else { tl!("Dual") };
                let r_dual = ui.selectable_label(app.ui.lumetri_dual_view, dual_text).on_hover_text(tl!("Dual view (two tools)"));
                app.auto.add("lumetri.bar.dual", r_dual.rect, "Dual");
                if r_dual.clicked() {
                    app.ui.lumetri_dual_view = !app.ui.lumetri_dual_view;
                }
            });
        });
    } else {
        let avail_w = ui.available_width().max(120.0);
        if avail_w < 1100.0 {
            ui.vertical(|ui| {
                ui.set_max_width(avail_w);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let dual_text = if app.ui.lumetri_dual_view { tl!("Dual ✓") } else { tl!("Dual") };
                        let r_dual = ui.selectable_label(app.ui.lumetri_dual_view, dual_text).on_hover_text(tl!("Dual view (two tools)"));
                        app.auto.add("lumetri.bar.dual", r_dual.rect, "Dual");
                        if r_dual.clicked() {
                            app.ui.lumetri_dual_view = !app.ui.lumetri_dual_view;
                        }
                        let mut cur_pos = app.ui.lumetri_bar_pos.clone();
                        let r_left = ui.selectable_label(cur_pos == "left", tl!("Left")).on_hover_text(tl!("Left sidebar"));
                        app.auto.add("lumetri.bar.left", r_left.rect, "Left");
                        if r_left.clicked() {
                            cur_pos = "left".into();
                        }
                        let r_bot = ui.selectable_label(cur_pos == "bottom", tl!("Bottom")).on_hover_text(tl!("Bottom bar"));
                        app.auto.add("lumetri.bar.bottom", r_bot.rect, "Bottom");
                        if r_bot.clicked() {
                            cur_pos = "bottom".into();
                        }
                        let r_top = ui.selectable_label(cur_pos == "top", tl!("Top")).on_hover_text(tl!("Top bar"));
                        app.auto.add("lumetri.bar.top", r_top.rect, "Top");
                        if r_top.clicked() {
                            cur_pos = "top".into();
                        }
                        app.ui.lumetri_bar_pos = cur_pos;
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    render_tool_buttons(ui, app, e, current_panel, false, sections);
                });
            });
            return;
        }
        ui.allocate_ui_with_layout(vec2(avail_w, 30.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_max_width(avail_w);
            let dual_text = if app.ui.lumetri_dual_view { tl!("Dual ✓") } else { tl!("Dual") };
            let r_dual = ui.selectable_label(app.ui.lumetri_dual_view, dual_text).on_hover_text(tl!("Dual view (two tools)"));
            app.auto.add("lumetri.bar.dual", r_dual.rect, "Dual");
            if r_dual.clicked() {
                app.ui.lumetri_dual_view = !app.ui.lumetri_dual_view;
            }
            let mut cur_pos = app.ui.lumetri_bar_pos.clone();
            let r_left = ui.selectable_label(cur_pos == "left", tl!("Left")).on_hover_text(tl!("Left sidebar"));
            app.auto.add("lumetri.bar.left", r_left.rect, "Left");
            if r_left.clicked() {
                cur_pos = "left".into();
            }
            let r_bot = ui.selectable_label(cur_pos == "bottom", tl!("Bottom")).on_hover_text(tl!("Bottom bar"));
            app.auto.add("lumetri.bar.bottom", r_bot.rect, "Bottom");
            if r_bot.clicked() {
                cur_pos = "bottom".into();
            }
            let r_top = ui.selectable_label(cur_pos == "top", tl!("Top")).on_hover_text(tl!("Top bar"));
            app.auto.add("lumetri.bar.top", r_top.rect, "Top");
            if r_top.clicked() {
                cur_pos = "top".into();
            }
            app.ui.lumetri_bar_pos = cur_pos;
            ui.separator();
            let strip_w = ui.available_width().max(40.0);
            ui.allocate_ui_with_layout(vec2(strip_w, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_max_width(strip_w);
                egui::ScrollArea::horizontal().auto_shrink([false, true]).max_width(strip_w).max_height(28.0).id_salt(("tool_strip", current_panel)).show(
                    ui,
                    |ui| {
                        ui.horizontal(|ui| {
                            render_tool_buttons(ui, app, e, current_panel, false, sections);
                        });
                    },
                );
            });
        });
    }
}
