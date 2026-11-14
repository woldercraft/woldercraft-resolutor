//! Color Groups panel (`PanelKind::ColorGroups`) for the Color workspace.
//!
//! Displays all `ColorGroup`s in the active sequence. Selecting a group sets
//! `app.ui.active_color_group = Some(group.id)` so Color Grading edits (wheels, primaries,
//! curves, nodes) apply to the selected group and all its member clips.

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::ColorGroupId;
use serde_json::json;

use crate::FilmcraftApp;
use crate::theme::Tokens;

#[derive(Clone, Debug)]
struct GroupCardInfo {
    id: ColorGroupId,
    name: String,
    color: [u8; 3],
    clip_count: usize,
    has_grade: bool,
    all_bypassed: bool,
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().clone();

    p.rect_filled(rect, 0.0, Color32::from_rgb(20, 23, 28));
    p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, Color32::from_rgb(34, 38, 46)));
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, Color32::from_rgb(14, 16, 20)));

    let Some(seq) = app.session.active_sequence() else {
        crate::dock::placeholder(ui, rect, &t, tl!("(no active sequence)"));
        return;
    };

    // Prune stale active_color_group
    if let Some(ag) = app.ui.active_color_group
        && seq.color_group(ag).is_none()
    {
        app.ui.active_color_group = None;
    }

    let groups: Vec<GroupCardInfo> = seq
        .color_groups
        .iter()
        .map(|g| {
            let has_grade = !g.effects.is_empty();
            let all_bypassed = has_grade && g.effects.iter().all(|e| !e.enabled);
            GroupCardInfo {
                id: g.id,
                name: g.name.clone(),
                color: g.color,
                clip_count: g.clips.len(),
                has_grade,
                all_bypassed,
            }
        })
        .collect();

    let header_h = 24.0;
    let header_rect = Rect::from_min_size(rect.min, vec2(rect.width(), header_h));
    p.rect_filled(header_rect, 0.0, Color32::from_rgb(24, 28, 34));

    let header_text = if let Some(ag) = app.ui.active_color_group
        && let Some(g) = groups.iter().find(|g| g.id == ag)
    {
        format!("GROUPS ({}) · Editing: {}", groups.len(), g.name)
    } else {
        format!("GROUPS ({})", groups.len())
    };
    p.text(
        pos2(header_rect.min.x + 10.0, header_rect.center().y),
        Align2::LEFT_CENTER,
        &header_text,
        Tokens::semibold(10.0),
        Color32::from_rgb(165, 180, 200),
    );

    let sel_clips: Vec<u64> = if !app.ui.selected_color_clips.is_empty() {
        app.ui.selected_color_clips.iter().map(|c| c.0).collect()
    } else if let Some((cid, _)) = super::lumetri::active_color_clip(app) {
        vec![cid.0]
    } else {
        Vec::new()
    };

    let mut pending_cmd: Option<(&'static str, serde_json::Value)> = None;
    let mut select_new_group = false;

    let mut rx = header_rect.max.x - 6.0;
    rx -= 94.0;
    let new_r = Rect::from_min_size(pos2(rx, header_rect.min.y + 3.0), vec2(90.0, 18.0));
    let new_resp = ui.interact(new_r, egui::Id::unique("cg_new_group"), Sense::click());
    app.auto.add("color_groups.new", new_r, "New Group");
    p.rect_filled(
        new_r,
        3.0,
        if new_resp.hovered() { Color32::from_rgb(80, 130, 200) } else { Color32::from_rgb(60, 100, 165) },
    );
    p.text(new_r.center(), Align2::CENTER_CENTER, tl!("+ New Group"), Tokens::ui(9.5), Color32::WHITE);
    if new_resp.clicked() {
        pending_cmd = Some(("color.groupCreate", json!({ "clips": sel_clips })));
        select_new_group = true;
    }

    if app.ui.active_color_group.is_some() {
        rx -= 104.0;
        let clip_mode_r = Rect::from_min_size(pos2(rx, header_rect.min.y + 3.0), vec2(98.0, 18.0));
        let cm_resp = ui.interact(clip_mode_r, egui::Id::unique("cg_edit_clip_mode"), Sense::click());
        app.auto.add("color_groups.exitGroup", clip_mode_r, "Edit Individual Clip");
        p.rect_filled(
            clip_mode_r,
            3.0,
            if cm_resp.hovered() { Color32::from_rgb(55, 62, 75) } else { Color32::from_rgb(38, 43, 52) },
        );
        p.text(clip_mode_r.center(), Align2::CENTER_CENTER, tl!("Edit Clip"), Tokens::ui(9.5), Color32::from_rgb(220, 228, 240));
        if cm_resp.clicked() {
            app.ui.active_color_group = None;
            app.ui.active_color_node = 0;
        }
    }

    let strip_rect = Rect::from_min_max(pos2(rect.min.x, rect.min.y + header_h), rect.max);

    if groups.is_empty() {
        let msg = tl!("No color groups yet. Select clips in Clips or click '+ New Group'.");
        p.text(strip_rect.center(), Align2::CENTER_CENTER, msg, Tokens::ui(11.0), t.text_dim);
    } else {
        let card_h = (strip_rect.height() - 12.0).clamp(44.0, 82.0);
        let card_w = 152.0;

        let rename_id_key = egui::Id::unique("cg_rename_target");
        let rename_buf_key = egui::Id::unique("cg_rename_buf");
        let mut renaming_gid: Option<ColorGroupId> = ui.ctx().data(|d| d.get_temp(rename_id_key));
        let mut rename_buf: String = ui.ctx().data(|d| d.get_temp(rename_buf_key)).unwrap_or_default();
        let mut commit_rename: Option<(ColorGroupId, String)> = None;

        let mut scroll_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(strip_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );

        let mut activate_group: Option<ColorGroupId> = None;

        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(&mut scroll_ui, |ui| {
                ui.add_space(8.0);
                for (i, grp) in groups.iter().enumerate() {
                    let (card_r, resp) = ui.allocate_exact_size(vec2(card_w, card_h), Sense::click());
                    let cp = ui.painter().clone();

                    let is_active = app.ui.active_color_group == Some(grp.id);
                    let is_hovered = resp.hovered();

                    let bg = if is_active {
                        Color32::from_rgb(34, 40, 52)
                    } else if is_hovered {
                        Color32::from_rgb(28, 33, 42)
                    } else {
                        Color32::from_rgb(23, 26, 33)
                    };
                    let gcol = Color32::from_rgb(grp.color[0], grp.color[1], grp.color[2]);
                    let border = if is_active {
                        Stroke::new(2.0, Color32::from_rgb(255, 175, 40))
                    } else if is_hovered {
                        Stroke::new(1.0, gcol)
                    } else {
                        Stroke::new(1.0, Color32::from_rgb(44, 50, 62))
                    };

                    cp.rect_filled(card_r, 4.0, bg);

                    // Top color bar
                    let bar_r = Rect::from_min_size(card_r.min, vec2(card_r.width(), 5.0));
                    cp.rect_filled(bar_r, CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, gcol);

                    if renaming_gid == Some(grp.id) {
                        let edit_r = Rect::from_min_size(pos2(card_r.min.x + 6.0, card_r.min.y + 10.0), vec2(card_w - 12.0, 20.0));
                        let te_resp = ui.put(edit_r, egui::TextEdit::singleline(&mut rename_buf).desired_width(card_w - 16.0));
                        te_resp.request_focus();
                        if te_resp.lost_focus() || ui.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                            commit_rename = Some((grp.id, rename_buf.clone()));
                            renaming_gid = None;
                        }
                    } else {
                        cp.text(
                            pos2(card_r.min.x + 8.0, card_r.min.y + 19.0),
                            Align2::LEFT_CENTER,
                            &grp.name,
                            Tokens::semibold(11.0),
                            if is_active { Color32::from_rgb(255, 195, 80) } else { Color32::WHITE },
                        );
                    }

                    let clip_label = format!("{} clips", grp.clip_count);
                    cp.text(
                        pos2(card_r.min.x + 8.0, card_r.max.y - 12.0),
                        Align2::LEFT_CENTER,
                        &clip_label,
                        Tokens::ui(9.5),
                        Color32::from_rgb(155, 168, 188),
                    );

                    let status_text = if !grp.has_grade {
                        tl!("Empty")
                    } else if grp.all_bypassed {
                        tl!("Bypassed")
                    } else {
                        tl!("Graded")
                    };
                    let status_col = if !grp.has_grade {
                        Color32::from_rgb(115, 125, 145)
                    } else if grp.all_bypassed {
                        Color32::from_rgb(220, 160, 55)
                    } else {
                        Color32::from_rgb(80, 185, 110)
                    };
                    cp.text(
                        pos2(card_r.max.x - 8.0, card_r.max.y - 12.0),
                        Align2::RIGHT_CENTER,
                        status_text,
                        Tokens::semibold(9.0),
                        status_col,
                    );

                    cp.rect_stroke(card_r, 4.0, border, StrokeKind::Inside);

                    app.auto.add(&format!("color_groups.group.{}", grp.id.0), card_r, &grp.name);
                    app.auto.add(&format!("color_groups.index.{i}"), card_r, &grp.name);

                    if resp.double_clicked() {
                        renaming_gid = Some(grp.id);
                        rename_buf = grp.name.clone();
                    } else if resp.clicked() {
                        activate_group = Some(grp.id);
                    }

                    resp.context_menu(|ui| {
                        if !sel_clips.is_empty() && ui.button(tl!("Assign Selected Clips")).clicked() {
                            pending_cmd = Some(("color.groupAssign", json!({ "clips": sel_clips, "group": grp.id.0 })));
                            ui.close();
                        }
                        if ui.button(tl!("Rename")).clicked() {
                            renaming_gid = Some(grp.id);
                            rename_buf = grp.name.clone();
                            ui.close();
                        }
                        if ui.button(tl!("Toggle Bypass")).clicked() {
                            pending_cmd = Some(("color.groupToggleBypass", json!({ "group": grp.id.0 })));
                            ui.close();
                        }
                        if ui.button(tl!("Reset Grade")).clicked() {
                            pending_cmd = Some(("color.groupReset", json!({ "group": grp.id.0 })));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button(tl!("Delete Group")).clicked() {
                            pending_cmd = Some(("color.groupDelete", json!({ "group": grp.id.0 })));
                            if app.ui.active_color_group == Some(grp.id) {
                                app.ui.active_color_group = None;
                            }
                            ui.close();
                        }
                    });

                    ui.add_space(6.0);
                }
                ui.add_space(8.0);
            });

        ui.ctx().data_mut(|d| {
            if let Some(gid) = renaming_gid {
                d.insert_temp(rename_id_key, gid);
                d.insert_temp(rename_buf_key, rename_buf);
            } else {
                d.remove::<ColorGroupId>(rename_id_key);
                d.remove::<String>(rename_buf_key);
            }
        });

        if let Some((gid, new_name)) = commit_rename
            && !new_name.trim().is_empty()
        {
            pending_cmd = Some(("color.groupRename", json!({ "group": gid.0, "name": new_name.trim() })));
        }

        if let Some(gid) = activate_group {
            app.ui.active_color_group = Some(gid);
            app.ui.active_color_node = 0;
        }
    }

    if let Some((cmd, args)) = pending_cmd {
        match app.session.execute(cmd, args) {
            Ok(val) => {
                if select_new_group
                    && let Some(gid) = val.get("group").and_then(|v| v.as_u64())
                {
                    app.ui.active_color_group = Some(ColorGroupId(gid));
                    app.ui.active_color_node = 0;
                }
            }
            Err(e) => {
                app.ui.status = format!("{e}");
            }
        }
    }
}
