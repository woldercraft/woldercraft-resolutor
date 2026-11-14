//! Timeline Clip Selector Strip (DaVinci Resolve style).
//!
//! Provides a dedicated adaptive horizontal strip of clip cards from the sequence's video tracks,
//! with real thumbnails, Color Group badges, multi-selection for batch organization/grading
//! operations, and quick filtering.
//!
//! Strict multi-clip rule: selecting multiple clips without a group only enables batch/group
//! operations; live Color Grading applies to >1 clip only when editing a Color Group.

use egui::{Align2, Color32, CornerRadius, Key, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{ClipId, ColorGroupId, ItemId};
use filmcraft_time::Tick;
use serde_json::json;

use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::frames::{FrameKey, Target};
use crate::icons::{self, Icon};
use crate::theme::Tokens;

#[derive(Clone, Debug)]
pub struct ColorClipInfo {
    pub id: ClipId,
    pub source: Option<ItemId>,
    pub source_mid: Tick,
    pub name: String,
    pub track_name: String,
    pub start: Tick,
    pub duration: Tick,
    pub node_count: usize,
    pub vfx_count: usize,
    pub is_graded: bool,
    pub is_bypassed: bool,
    pub is_active: bool,
    pub group: Option<(ColorGroupId, String, [u8; 3])>,
}

pub fn collect_video_clips(app: &FilmcraftApp) -> Vec<ColorClipInfo> {
    let Some(seq) = app.session.active_sequence() else {
        return Vec::new();
    };
    let active_clip_id = super::lumetri::active_color_clip(app).map(|(c, _)| c);
    let mut clips = Vec::new();

    for track in &seq.video_tracks {
        if track.locked {
            continue;
        }
        for item in &track.items {
            let lumetri_fx: Vec<_> = item.effects.iter().filter(|e| e.effect == "lumetri").collect();
            let node_count = lumetri_fx.len();
            let is_graded = node_count > 0;
            let is_bypassed = is_graded && lumetri_fx.iter().all(|e| !e.enabled);
            let vfx_count = item
                .effects
                .iter()
                .filter(|e| e.effect != "lumetri" && e.def().is_some_and(|d| !d.intrinsic && d.kind == filmcraft_project::EffectKind::Video))
                .count();
            let is_active = active_clip_id == Some(item.id);
            let group = seq.color_group_of(item.id).map(|g| (g.id, g.name.clone(), g.color));
            let source = Some(item.item);
            let source_mid = item.source_in + Tick(item.duration.0 / 2);
            clips.push(ColorClipInfo {
                id: item.id,
                source,
                source_mid,
                name: item.name.clone(),
                track_name: track.name.clone(),
                start: item.start,
                duration: item.duration,
                node_count,
                vfx_count,
                is_graded,
                is_bypassed,
                is_active,
                group,
            });
        }
    }

    clips.sort_by_key(|c| c.start);
    clips
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

fn draw_header_btn(
    ui: &mut egui::Ui,
    p: &egui::Painter,
    r: Rect,
    id: &str,
    label: &str,
    active: bool,
    accent: Option<Color32>,
) -> egui::Response {
    let resp = ui.interact(r, egui::Id::unique(id), Sense::click());
    let bg = if active {
        accent.unwrap_or(Color32::from_rgb(55, 95, 155))
    } else if resp.hovered() {
        Color32::from_rgb(45, 52, 65)
    } else {
        Color32::from_rgb(30, 34, 42)
    };
    p.rect_filled(r, 3.0, bg);
    p.text(
        r.center(),
        Align2::CENTER_CENTER,
        label,
        Tokens::ui(9.5),
        if active { Color32::WHITE } else { Color32::from_rgb(205, 215, 230) },
    );
    resp
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().clone();

    p.rect_filled(rect, 0.0, Color32::from_rgb(20, 23, 28));
    p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, Color32::from_rgb(34, 38, 46)));
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, Color32::from_rgb(14, 16, 20)));

    let all_clips = collect_video_clips(app);
    if all_clips.is_empty() {
        crate::dock::placeholder(ui, rect, &t, tl!("(no video clips in active sequence)"));
        return;
    }

    // Prune stale selected_color_clips
    app.ui.selected_color_clips.retain(|id| all_clips.iter().any(|c| c.id == *id));

    // Filter clips according to app.ui.color_clip_filter
    let filter = app.ui.color_clip_filter.clone();
    let clips: Vec<ColorClipInfo> = all_clips
        .iter()
        .filter(|c| match filter.as_str() {
            "ungrouped" => c.group.is_none(),
            "graded" => c.is_graded,
            "bypassed" => c.is_bypassed,
            _ => true,
        })
        .cloned()
        .collect();

    // Ctrl/Cmd + A selects all filtered clips when ColorClips is focused
    if app.ui.focused == PanelKind::ColorClips
        && ui.input(|i| i.modifiers.command && i.key_pressed(Key::A))
    {
        app.ui.selected_color_clips = clips.iter().map(|c| c.id).collect();
    }

    // Top mini-toolbar
    let header_h = 24.0;
    let header_rect = Rect::from_min_size(rect.min, vec2(rect.width(), header_h));
    p.rect_filled(header_rect, 0.0, Color32::from_rgb(24, 28, 34));

    let sel_count = app.ui.selected_color_clips.len();
    let header_text = if sel_count > 1 {
        format!("CLIPS ({}) · {} sel", clips.len(), sel_count)
    } else {
        format!("CLIPS ({})", clips.len())
    };
    p.text(
        pos2(header_rect.min.x + 10.0, header_rect.center().y),
        Align2::LEFT_CENTER,
        &header_text,
        Tokens::semibold(10.0),
        Color32::from_rgb(165, 180, 200),
    );

    // Filter pills on the left after header text
    let mut lx = header_rect.min.x + 132.0;
    let filters = [
        ("", tl!("All")),
        ("ungrouped", tl!("Ungrouped")),
        ("graded", tl!("Graded")),
        ("bypassed", tl!("Bypassed")),
    ];
    for (key, label) in filters {
        let w = (label.chars().count() as f32 * 5.8 + 14.0).max(42.0);
        if lx + w < header_rect.max.x - 320.0 {
            let r = Rect::from_min_size(pos2(lx, header_rect.min.y + 3.0), vec2(w, 18.0));
            let resp = draw_header_btn(ui, &p, r, &format!("cc_filter_{key}"), label, filter == key, None);
            app.auto.add(&format!("color_clips.filter.{}", if key.is_empty() { "all" } else { key }), r, label);
            if resp.clicked() {
                app.ui.color_clip_filter = key.to_string();
            }
            lx += w + 4.0;
        }
    }

    // Right side: Prev / Next clip buttons + Batch actions
    let mut rx = header_rect.max.x - 6.0;

    rx -= 62.0;
    let next_r = Rect::from_min_size(pos2(rx, header_rect.min.y + 3.0), vec2(58.0, 18.0));
    let next_resp = draw_header_btn(ui, &p, next_r, "color_clips_next", "Sig ▶", false, None);
    app.auto.add("color_clips.next", next_r, "Siguiente Clip");

    rx -= 62.0;
    let prev_r = Rect::from_min_size(pos2(rx, header_rect.min.y + 3.0), vec2(58.0, 18.0));
    let prev_resp = draw_header_btn(ui, &p, prev_r, "color_clips_prev", "◀ Ant", false, None);
    app.auto.add("color_clips.prev", prev_r, "Anterior Clip");

    // Batch actions when >= 1 clip selected in multi-selection
    let batch_ids: Vec<u64> = if !app.ui.selected_color_clips.is_empty() {
        app.ui.selected_color_clips.iter().map(|c| c.0).collect()
    } else if let Some((active_id, _)) = super::lumetri::active_color_clip(app) {
        vec![active_id.0]
    } else {
        Vec::new()
    };

    let mut pending_cmd: Option<(&'static str, serde_json::Value)> = None;
    let mut select_new_group = false;

    if !batch_ids.is_empty() && rx - 190.0 > lx {
        rx -= 58.0;
        let reset_r = Rect::from_min_size(pos2(rx, header_rect.min.y + 3.0), vec2(54.0, 18.0));
        let reset_resp = draw_header_btn(ui, &p, reset_r, "cc_batch_reset", tl!("Reset"), false, None);
        app.auto.add("color_clips.batchReset", reset_r, "Reset Grade");
        if reset_resp.clicked() {
            pending_cmd = Some(("color.batchResetGrade", json!({ "clips": batch_ids })));
        }

        rx -= 62.0;
        let byp_r = Rect::from_min_size(pos2(rx, header_rect.min.y + 3.0), vec2(58.0, 18.0));
        let byp_resp = draw_header_btn(ui, &p, byp_r, "cc_batch_bypass", tl!("Bypass"), false, None);
        app.auto.add("color_clips.batchBypass", byp_r, "Toggle Bypass");
        if byp_resp.clicked() {
            pending_cmd = Some(("color.batchToggleBypass", json!({ "clips": batch_ids })));
        }

        rx -= 76.0;
        let grp_r = Rect::from_min_size(pos2(rx, header_rect.min.y + 3.0), vec2(72.0, 18.0));
        let grp_resp = draw_header_btn(
            ui,
            &p,
            grp_r,
            "cc_batch_group",
            tl!("+ Group"),
            false,
            Some(Color32::from_rgb(70, 115, 180)),
        );
        app.auto.add("color_clips.groupSelection", grp_r, "Group Selection");
        if grp_resp.clicked() {
            pending_cmd = Some(("color.groupCreate", json!({ "clips": batch_ids })));
            select_new_group = true;
        }
    }

    let active_idx = clips.iter().position(|c| c.is_active).unwrap_or(0);
    let mut nav_clip: Option<ColorClipInfo> = None;
    if prev_resp.clicked() && active_idx > 0 {
        nav_clip = Some(clips[active_idx - 1].clone());
    }
    if next_resp.clicked() && active_idx + 1 < clips.len() {
        nav_clip = Some(clips[active_idx + 1].clone());
    }

    // Scrollable horizontal clips strip with adaptive 16:9 sizing
    let strip_rect = Rect::from_min_max(pos2(rect.min.x, rect.min.y + header_h), rect.max);
    let avail_h = (strip_rect.height() - 10.0).max(52.0);
    let banner_h = 16.0;
    let footer_h = 16.0;
    let thumb_h = (avail_h - banner_h - footer_h - 6.0).clamp(24.0, 96.0);
    let thumb_w = (thumb_h * 16.0 / 9.0).clamp(76.0, 172.0);
    let card_w = thumb_w + 8.0;
    let card_h = banner_h + thumb_h + footer_h + 6.0;

    let playhead = app.session.playhead();
    let project = app.session.project.clone();
    let groups_snapshot: Vec<(ColorGroupId, String, [u8; 3])> = app
        .session
        .active_sequence()
        .map(|s| s.color_groups.iter().map(|g| (g.id, g.name.clone(), g.color)).collect())
        .unwrap_or_default();

    let mut scroll_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(strip_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );

    let mut clicked_clip: Option<(usize, ColorClipInfo, bool, bool)> = None;
    let mut copy_grade_from: Option<ClipId> = None;

    egui::ScrollArea::horizontal()
        .auto_shrink([false, false])
        .show(&mut scroll_ui, |ui| {
            ui.add_space(8.0);
            for (i, clip) in clips.iter().enumerate() {
                let (card_r, resp) = ui.allocate_exact_size(vec2(card_w, card_h), Sense::click());
                let cp = ui.painter();

                let is_hovered = resp.hovered();
                let is_multi_selected = app.ui.selected_color_clips.contains(&clip.id);
                let is_active_clip = clip.is_active && app.ui.active_color_group.is_none();
                let is_active_group_member = app
                    .ui
                    .active_color_group
                    .is_some_and(|ag| clip.group.as_ref().is_some_and(|(gid, _, _)| *gid == ag));

                let bg_color = if is_active_clip || is_active_group_member {
                    Color32::from_rgb(32, 38, 48)
                } else if is_multi_selected {
                    Color32::from_rgb(28, 36, 50)
                } else if is_hovered {
                    Color32::from_rgb(28, 33, 42)
                } else {
                    Color32::from_rgb(22, 25, 32)
                };

                let border_stroke = if is_active_clip {
                    Stroke::new(2.0, Color32::from_rgb(255, 175, 40))
                } else if is_active_group_member {
                    let rgb = clip.group.as_ref().map(|(_, _, c)| *c).unwrap_or([255, 175, 40]);
                    Stroke::new(2.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]))
                } else if is_multi_selected {
                    Stroke::new(2.0, Color32::from_rgb(80, 155, 255))
                } else if is_hovered {
                    Stroke::new(1.0, Color32::from_rgb(70, 140, 230))
                } else {
                    Stroke::new(1.0, Color32::from_rgb(42, 48, 58))
                };

                cp.rect_filled(card_r, 4.0, bg_color);

                // Left group stripe if clip belongs to a group
                if let Some((_, _, rgb)) = &clip.group {
                    let stripe_r = Rect::from_min_size(card_r.min, vec2(3.5, card_r.height()));
                    cp.rect_filled(
                        stripe_r,
                        CornerRadius { nw: 4, ne: 0, sw: 4, se: 0 },
                        Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                    );
                }

                // Top banner: Number and Track badge
                let banner_r = Rect::from_min_size(card_r.min, vec2(card_r.width(), banner_h));
                cp.rect_filled(
                    banner_r,
                    CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 },
                    Color32::from_rgb(18, 20, 26),
                );

                let num_str = format!("{:02}", i + 1);
                cp.text(
                    pos2(banner_r.min.x + 7.0, banner_r.center().y),
                    Align2::LEFT_CENTER,
                    &num_str,
                    Tokens::semibold(10.0),
                    if is_active_clip { Color32::from_rgb(255, 185, 60) } else { Color32::from_rgb(190, 200, 215) },
                );

                cp.text(
                    pos2(banner_r.max.x - 6.0, banner_r.center().y),
                    Align2::RIGHT_CENTER,
                    &clip.track_name,
                    Tokens::ui(9.0),
                    Color32::from_rgb(120, 135, 155),
                );

                // Middle area: Thumbnail box
                let thumb_r = Rect::from_min_max(
                    pos2(card_r.min.x + 4.0, card_r.min.y + banner_h + 2.0),
                    pos2(card_r.max.x - 4.0, card_r.max.y - footer_h - 2.0),
                );
                cp.rect_filled(thumb_r, 2.0, Color32::from_rgb(14, 16, 20));

                let mut drew_thumb = false;
                if let Some(src_id) = clip.source {
                    let scale = crate::panels::monitor::quantize_scale(
                        (thumb_r.width() * ui.ctx().pixels_per_point() / 1920.0).clamp(1.0 / 32.0, 0.25),
                    );
                    let key = FrameKey {
                        target: Target::Item(src_id),
                        frame: 0,
                        size: (scale * 1000.0) as u32,
                        revision: 0,
                        draft: true,
                        bypass: false,
                    };
                    app.frames.request(key, clip.source_mid, scale, &project, 5);
                    if let Some(img) = app.frames.get(&key) {
                        let tex = app.texture_for(ui.ctx(), &format!("color-clip-{}", clip.id.0), key, &img);
                        cp.image(tex, thumb_r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        drew_thumb = true;
                    }
                }
                if !drew_thumb {
                    let icon_r = Rect::from_center_size(thumb_r.center(), vec2(16.0, 16.0));
                    icons::paint(cp, icon_r, Icon::Film, Color32::from_rgba_unmultiplied(120, 140, 170, 60));
                }

                // Badges on thumbnail: Group pill, Node badge, VFX badge
                let mut badge_x = thumb_r.min.x + 3.0;
                if let Some((_, gname, rgb)) = &clip.group {
                    let short = truncate_chars(gname, 8);
                    let bw = (short.chars().count() as f32 * 5.0 + 8.0).clamp(24.0, 56.0);
                    let badge_r = Rect::from_min_size(pos2(badge_x, thumb_r.min.y + 3.0), vec2(bw, 12.0));
                    cp.rect_filled(badge_r, 2.0, Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], 215));
                    cp.text(badge_r.center(), Align2::CENTER_CENTER, &short, Tokens::semibold(8.0), Color32::from_rgb(16, 18, 22));
                    badge_x += bw + 3.0;
                }
                if clip.is_graded {
                    let badge_r = Rect::from_min_size(pos2(badge_x, thumb_r.min.y + 3.0), vec2(32.0, 12.0));
                    let col = if clip.is_bypassed {
                        Color32::from_rgba_unmultiplied(165, 115, 35, 215)
                    } else {
                        Color32::from_rgba_unmultiplied(40, 140, 70, 215)
                    };
                    cp.rect_filled(badge_r, 2.0, col);
                    cp.text(badge_r.center(), Align2::CENTER_CENTER, format!("{} nod", clip.node_count), Tokens::semibold(8.0), Color32::WHITE);
                    badge_x += 35.0;
                }
                if clip.vfx_count > 0 && badge_x + 30.0 < thumb_r.max.x {
                    let badge_r = Rect::from_min_size(pos2(badge_x, thumb_r.min.y + 3.0), vec2(30.0, 12.0));
                    cp.rect_filled(badge_r, 2.0, Color32::from_rgba_unmultiplied(115, 75, 225, 220));
                    cp.text(badge_r.center(), Align2::CENTER_CENTER, format!("{} FX", clip.vfx_count), Tokens::semibold(8.0), Color32::WHITE);
                }

                // Playhead needle across thumbnail
                if playhead >= clip.start && playhead < clip.start + clip.duration && clip.duration.0 > 0 {
                    let prog = (playhead.0 - clip.start.0) as f32 / clip.duration.0 as f32;
                    let nx = thumb_r.min.x + prog.clamp(0.0, 1.0) * thumb_r.width();
                    cp.line_segment([pos2(nx, thumb_r.min.y), pos2(nx, thumb_r.max.y)], Stroke::new(1.5, Color32::from_rgb(255, 170, 30)));
                }

                // Bottom footer: Clip Name (clipped cleanly) and Duration
                let footer_y = card_r.max.y - footer_h * 0.5;
                let dur_str = format!("{:.1}s", clip.duration.seconds());
                let dur_reserved = 36.0;
                let max_name_chars = ((card_w - dur_reserved - 12.0) / 5.5).max(5.0) as usize;
                let display_name = truncate_chars(&clip.name, max_name_chars);
                let name_clip_r = Rect::from_min_max(
                    pos2(card_r.min.x + 6.0, card_r.max.y - footer_h),
                    pos2(card_r.max.x - dur_reserved - 4.0, card_r.max.y),
                );
                cp.with_clip_rect(name_clip_r).text(
                    pos2(card_r.min.x + 6.0, footer_y),
                    Align2::LEFT_CENTER,
                    &display_name,
                    Tokens::ui(9.5),
                    Color32::from_rgb(210, 220, 235),
                );
                cp.text(
                    pos2(card_r.max.x - 6.0, footer_y),
                    Align2::RIGHT_CENTER,
                    &dur_str,
                    Tokens::ui(8.5),
                    Color32::from_rgb(130, 140, 160),
                );

                cp.rect_stroke(card_r, 4.0, border_stroke, StrokeKind::Inside);

                // Automation IDs
                app.auto.add(&format!("color_clips.clip.{}", clip.id.0), card_r, &clip.name);
                app.auto.add(&format!("color_clips.index.{i}"), card_r, &format!("Clip {}", i + 1));

                if resp.clicked() {
                    let mods = ui.input(|inp| inp.modifiers);
                    clicked_clip = Some((i, clip.clone(), mods.command || mods.ctrl, mods.shift));
                }

                resp.context_menu(|ui| {
                    let target_ids: Vec<u64> = if app.ui.selected_color_clips.contains(&clip.id) && !app.ui.selected_color_clips.is_empty() {
                        app.ui.selected_color_clips.iter().map(|c| c.0).collect()
                    } else {
                        vec![clip.id.0]
                    };

                    let create_lbl = if target_ids.len() > 1 {
                        tlf!("Create New Group ({n})", n = target_ids.len())
                    } else {
                        tl!("Create New Group").to_string()
                    };
                    if ui.button(create_lbl).clicked() {
                        pending_cmd = Some(("color.groupCreate", json!({ "clips": target_ids })));
                        select_new_group = true;
                        ui.close();
                    }

                    ui.menu_button(tl!("Add to Group"), |ui| {
                        if groups_snapshot.is_empty() {
                            ui.add_enabled(false, egui::Button::new(tl!("(No groups yet)")));
                            if ui.button(tl!("+ Create New Group")).clicked() {
                                pending_cmd = Some(("color.groupCreate", json!({ "clips": target_ids })));
                                select_new_group = true;
                                ui.close();
                            }
                        } else {
                            for (gid, gname, rgb) in &groups_snapshot {
                                let label = egui::RichText::new(format!("● {gname}")).color(Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                                if ui.button(label).clicked() {
                                    pending_cmd = Some(("color.groupAssign", json!({ "clips": target_ids, "group": gid.0 })));
                                    ui.close();
                                }
                            }
                        }
                    });

                    if (clip.group.is_some() || target_ids.len() > 1) && ui.button(tl!("Remove from Group")).clicked() {
                        pending_cmd = Some(("color.groupUnassign", json!({ "clips": target_ids })));
                        ui.close();
                    }

                    ui.separator();
                    if ui.button(tl!("Copy Grade")).clicked() {
                        copy_grade_from = Some(clip.id);
                        ui.close();
                    }
                    if let Some(src_clip) = app.ui.copied_color_clip
                        && ui.button(tl!("Paste Grade")).clicked()
                    {
                        pending_cmd = Some(("color.batchPasteGrade", json!({ "from": src_clip.0, "clips": target_ids })));
                        ui.close();
                    }
                    if ui.button(tl!("Toggle Bypass")).clicked() {
                        pending_cmd = Some(("color.batchToggleBypass", json!({ "clips": target_ids })));
                        ui.close();
                    }
                    if ui.button(tl!("Reset Grade")).clicked() {
                        pending_cmd = Some(("color.batchResetGrade", json!({ "clips": target_ids })));
                        ui.close();
                    }
                });

                ui.add_space(6.0);
            }
            ui.add_space(8.0);
        });

    if let Some(src_id) = copy_grade_from {
        app.ui.copied_color_clip = Some(src_id);
        app.ui.status = tl!("Grade copied").to_string();
    }

    if let Some((cmd, args)) = pending_cmd {
        match app.session.execute(cmd, args) {
            Ok(val) => {
                if select_new_group
                    && let Some(gid) = val.get("group").and_then(|v| v.as_u64())
                {
                    app.ui.active_color_group = Some(ColorGroupId(gid));
                }
            }
            Err(e) => {
                app.ui.status = format!("{e}");
            }
        }
    }

    if let Some((idx, target, ctrl, shift)) = clicked_clip {
        if ctrl {
            if let Some(pos) = app.ui.selected_color_clips.iter().position(|id| *id == target.id) {
                app.ui.selected_color_clips.remove(pos);
            } else {
                app.ui.selected_color_clips.push(target.id);
            }
        } else if shift {
            let anchor_idx = app
                .ui
                .selected_color_clips
                .last()
                .and_then(|last_id| clips.iter().position(|c| c.id == *last_id))
                .unwrap_or(active_idx);
            let (lo, hi) = if anchor_idx <= idx { (anchor_idx, idx) } else { (idx, anchor_idx) };
            app.ui.selected_color_clips = clips[lo..=hi].iter().map(|c| c.id).collect();
        } else {
            app.ui.selected_color_clips = vec![target.id];
            app.session.state.selection = vec![target.id];
            app.session.set_playhead(target.start);
            app.ui.active_color_group = None;
            app.ui.active_color_node = 0;
            app.ui.active_vfx_node = 0;
        }
    } else if let Some(target) = nav_clip {
        app.ui.selected_color_clips = vec![target.id];
        app.session.state.selection = vec![target.id];
        app.session.set_playhead(target.start);
        app.ui.active_color_group = None;
        app.ui.active_color_node = 0;
        app.ui.active_vfx_node = 0;
    }
}
