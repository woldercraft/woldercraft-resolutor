//! Color Gallery panel (`PanelKind::ColorGallery`): project Stills and user-level PowerGrades
//! with Grab Still, Apply/Append Grade, and interactive Image Wipe toggle in the Program monitor.

use std::io::Read as _;

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, pos2, vec2};
use filmcraft_project::ColorStill;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::FilmcraftApp;

const POWERGRADES_FILE: &str = "powergrades.json";
const MAX_POWERGRADES_BYTES: u64 = 16 * 1024 * 1024;
const MAX_POWERGRADES: usize = 256;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct PowerGradeStore {
    stills: Vec<ColorStill>,
}

fn powergrades_path(app: &FilmcraftApp) -> Option<std::path::PathBuf> {
    app.session.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(POWERGRADES_FILE))
}

fn load_powergrades(path: &std::path::Path) -> Vec<ColorStill> {
    let Some(bytes) = std::fs::File::open(path).ok().and_then(|f| {
        let mut buf = Vec::new();
        f.take(MAX_POWERGRADES_BYTES + 1).read_to_end(&mut buf).ok()?;
        (buf.len() as u64 <= MAX_POWERGRADES_BYTES).then_some(buf)
    }) else {
        return Vec::new();
    };
    let mut store: PowerGradeStore = serde_json::from_slice(&bytes).unwrap_or_default();
    store.stills.truncate(MAX_POWERGRADES);
    store.stills
}

fn save_powergrades(path: &std::path::Path, stills: &[ColorStill]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(&PowerGradeStore { stills: stills.to_vec() }).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, path)
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 4.0))).id_salt("color_gallery"));
    bui.set_clip_rect(rect.intersect(ui.clip_rect()));

    // Ensure PowerGrades are cached from disk once per session.
    let pg_loaded_id = egui::Id::unique("powergrades-loaded");
    if !ui.ctx().data(|d| d.get_temp::<bool>(pg_loaded_id).unwrap_or(false)) {
        if let Some(path) = powergrades_path(app) {
            app.ui.powergrades = load_powergrades(&path);
        }
        ui.ctx().data_mut(|d| d.insert_temp(pg_loaded_id, true));
    }

    // Fixed top toolbar
    bui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let is_stills = app.ui.gallery_tab == 0;
        let tab_stills = ui.selectable_label(is_stills, egui::RichText::new(tl!("Stills")).small().strong());
        app.auto.add("color_gallery.tab.stills", tab_stills.rect, "Stills");
        if tab_stills.clicked() {
            app.ui.gallery_tab = 0;
        }

        let tab_pg = ui.selectable_label(!is_stills, egui::RichText::new(tl!("PowerGrades")).small().strong());
        app.auto.add("color_gallery.tab.powergrades", tab_pg.rect, "PowerGrades");
        if tab_pg.clicked() {
            app.ui.gallery_tab = 1;
        }

        ui.separator();

        let grab = ui.small_button(tl!("Grab Still")).on_hover_text(tl!("Capture frame and color grade (Ctrl+Alt+G)"));
        app.auto.add("color_gallery.grab", grab.rect, "Grab Still");
        if grab.clicked() {
            if let Err(err) = app.session.execute("color.stillGrab", json!({})) {
                app.ui.status = err.to_string();
            } else if let Some(last) = app.session.project.color_stills.last() {
                app.ui.selected_still_id = Some(last.id);
            }
        }

        let wipe_on = app.ui.program_wipe.enabled;
        let wipe_btn = ui
            .selectable_label(wipe_on, egui::RichText::new(tl!("Wipe")).small().color(if wipe_on { t.accent } else { t.text }))
            .on_hover_text(tl!("Compare selected still in Program monitor with interactive Wipe"));
        app.auto.add("color_gallery.wipe_toggle", wipe_btn.rect, "Toggle Image Wipe");
        if wipe_btn.clicked() {
            app.ui.program_wipe.enabled = !wipe_on;
            if app.ui.program_wipe.enabled && app.ui.program_wipe.still_id.is_none() {
                app.ui.program_wipe.still_id = app.ui.selected_still_id.or_else(|| app.session.project.color_stills.first().map(|s| s.id));
            }
        }
    });

    // Determine currently active album items
    let active_tab = app.ui.gallery_tab;
    let selected_id = app.ui.selected_still_id;
    let items: Vec<ColorStill> = if active_tab == 0 { app.session.project.color_stills.clone() } else { app.ui.powergrades.clone() };
    let selected_still = selected_id.and_then(|id| items.iter().find(|s| s.id == id)).or_else(|| items.first()).cloned();
    if let Some(sel) = &selected_still
        && app.ui.selected_still_id != Some(sel.id)
    {
        app.ui.selected_still_id = Some(sel.id);
    }

    // Secondary compact action bar for selected still
    bui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let has_sel = selected_still.is_some();
        let apply_btn = ui.add_enabled(has_sel, egui::Button::new(egui::RichText::new(tl!("Apply")).small()));
        app.auto.add("color_gallery.apply", apply_btn.rect, "Apply Still");
        if apply_btn.clicked()
            && let Some(sel) = &selected_still
        {
            apply_still_to_selection(app, sel, false);
        }

        let app_btn = ui.add_enabled(has_sel, egui::Button::new(egui::RichText::new(tl!("Append")).small()));
        app.auto.add("color_gallery.append", app_btn.rect, "Append Still");
        if app_btn.clicked()
            && let Some(sel) = &selected_still
        {
            apply_still_to_selection(app, sel, true);
        }

        if active_tab == 0 {
            let save_pg = ui.add_enabled(has_sel, egui::Button::new(egui::RichText::new(tl!("Save PowerGrade")).small()));
            app.auto.add("color_gallery.save_pg", save_pg.rect, "Save PowerGrade");
            if save_pg.clicked()
                && let Some(sel) = &selected_still
            {
                let mut copy = sel.clone();
                copy.id = app.ui.powergrades.iter().map(|s| s.id).max().unwrap_or(0) + 1;
                if app.ui.powergrades.len() < MAX_POWERGRADES {
                    app.ui.powergrades.push(copy);
                    if let Some(path) = powergrades_path(app)
                        && let Err(err) = save_powergrades(&path, &app.ui.powergrades)
                    {
                        app.ui.status = err.to_string();
                    }
                }
            }
        }

        let del_btn = ui.add_enabled(has_sel, egui::Button::new(egui::RichText::new(tl!("Delete")).small()));
        app.auto.add("color_gallery.delete", del_btn.rect, "Delete Still");
        if del_btn.clicked()
            && let Some(sel) = &selected_still
        {
            if active_tab == 0 {
                if let Err(err) = app.session.execute("color.stillDelete", json!({"id": sel.id})) {
                    app.ui.status = err.to_string();
                } else if app.ui.program_wipe.still_id == Some(sel.id) {
                    app.ui.program_wipe.still_id = None;
                }
            } else {
                app.ui.powergrades.retain(|s| s.id != sel.id);
                if let Some(path) = powergrades_path(app)
                    && let Err(err) = save_powergrades(&path, &app.ui.powergrades)
                {
                    app.ui.status = err.to_string();
                }
            }
            app.ui.selected_still_id = None;
        }
    });

    bui.separator();

    if items.is_empty() {
        bui.vertical_centered(|ui| {
            ui.add_space(16.0);
            ui.label(egui::RichText::new(tl!("No stills captured yet — click Grab Still (Ctrl+Alt+G)")).small().color(t.text_dim));
        });
        return;
    }

    let rename_key = egui::Id::unique("gallery-rename-id");
    let rename_buf_key = egui::Id::unique("gallery-rename-buf");
    let mut renaming: Option<u64> = ui.ctx().data(|d| d.get_temp(rename_key));
    let mut rename_buf: String = ui.ctx().data(|d| d.get_temp(rename_buf_key)).unwrap_or_default();

    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("color_gallery_scroll").show(&mut bui, |ui| {
        let avail_w = ui.available_width().max(120.0);
        let cols = ((avail_w / 138.0).floor() as usize).clamp(1, 6);
        let gap = 6.0;
        let card_w = ((avail_w - gap * (cols - 1) as f32) / cols as f32).floor().max(100.0);

        for row in items.chunks(cols) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for st in row {
                    let is_sel = selected_still.as_ref().is_some_and(|s| s.id == st.id);
                    let is_wiping = app.ui.program_wipe.enabled && app.ui.program_wipe.still_id == Some(st.id);
                    let frame = egui::Frame::new()
                        .fill(if is_sel { Color32::from_rgb(34, 42, 56) } else { Color32::from_rgb(23, 26, 32) })
                        .stroke(Stroke::new(
                            if is_sel || is_wiping { 1.5 } else { 1.0 },
                            if is_wiping {
                                Color32::from_rgb(235, 185, 70)
                            } else if is_sel {
                                t.accent
                            } else {
                                Color32::from_rgb(38, 43, 52)
                            },
                        ))
                        .corner_radius(5.0)
                        .inner_margin(egui::Margin::same(4));

                    frame.show(ui, |ui| {
                        ui.set_width((card_w - 8.0).max(88.0));
                        let thumb_w = ui.available_width().max(64.0);
                        let thumb_h = (thumb_w * 9.0 / 16.0).round();
                        let (t_rect, t_resp) = ui.allocate_exact_size(vec2(thumb_w, thumb_h), Sense::click());
                        ui.painter().rect_filled(t_rect, 3.0, Color32::from_rgb(12, 14, 18));

                        if let Some(tex) = still_texture(ui.ctx(), st) {
                            ui.painter().image(tex.id(), t_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        } else {
                            ui.painter().rect_stroke(t_rect, 3.0, Stroke::new(1.0, Color32::from_rgb(46, 52, 64)), StrokeKind::Inside);
                        }
                        if t_resp.clicked() {
                            app.ui.selected_still_id = Some(st.id);
                            if app.ui.program_wipe.enabled {
                                app.ui.program_wipe.still_id = Some(st.id);
                            }
                        }
                        if t_resp.double_clicked() {
                            app.ui.selected_still_id = Some(st.id);
                            apply_still_to_selection(app, st, false);
                        }
                        app.auto.add(&format!("color_gallery.still.{}", st.id), t_rect, &st.label);

                        if renaming == Some(st.id) {
                            let ed = ui.add(egui::TextEdit::singleline(&mut rename_buf).desired_width(thumb_w));
                            ed.request_focus();
                            if ed.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                let clean = rename_buf.trim().to_string();
                                if !clean.is_empty() {
                                    if active_tab == 0 {
                                        let _ = app.session.execute("color.stillRename", json!({"id": st.id, "name": clean}));
                                    } else if let Some(pg) = app.ui.powergrades.iter_mut().find(|g| g.id == st.id) {
                                        pg.label = clean;
                                        if let Some(path) = powergrades_path(app) {
                                            let _ = save_powergrades(&path, &app.ui.powergrades);
                                        }
                                    }
                                }
                                renaming = None;
                            }
                        } else {
                            let lbl_resp = ui.add(egui::Label::new(egui::RichText::new(&st.label).strong().size(10.0)).truncate().sense(Sense::click()));
                            if lbl_resp.double_clicked() {
                                renaming = Some(st.id);
                                rename_buf = st.label.clone();
                            }
                        }
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("{} · {} nodes", st.timecode, st.graph.nodes.len().max(1))).size(8.5).color(t.text_dim),
                            )
                            .truncate(),
                        );
                    });
                }
            });
            ui.add_space(6.0);
        }
    });

    ui.ctx().data_mut(|d| {
        if let Some(id) = renaming {
            d.insert_temp(rename_key, id);
            d.insert_temp(rename_buf_key, rename_buf);
        } else {
            d.remove::<u64>(rename_key);
        }
    });
}

/// Cached GPU texture for a [`ColorStill`] thumbnail (bounded to valid RGBA dimensions).
pub(crate) fn still_texture(ctx: &egui::Context, still: &ColorStill) -> Option<TextureHandle> {
    if !still.has_valid_thumb() {
        return None;
    }
    let key = egui::Id::unique(("color-still-tex", still.id, still.thumb_w, still.thumb_h, still.thumb_rgba.len()));
    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(key)) {
        return Some(tex);
    }
    let img = egui::ColorImage::from_rgba_unmultiplied([still.thumb_w as usize, still.thumb_h as usize], &still.thumb_rgba);
    let tex = ctx.load_texture(format!("still-{}", still.id), img, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    Some(tex)
}

fn apply_still_to_selection(app: &mut FilmcraftApp, still: &ColorStill, append: bool) {
    let clips: Vec<u64> = if !app.ui.selected_color_clips.is_empty() {
        app.ui.selected_color_clips.iter().map(|c| c.0).collect()
    } else if !app.session.state.selection.is_empty() {
        app.session.state.selection.iter().map(|c| c.0).collect()
    } else if let Some((cid, _)) = crate::panels::lumetri::active_color_clip(app) {
        vec![cid.0]
    } else {
        Vec::new()
    };
    if clips.is_empty() {
        app.ui.status = tl!("Select a clip to apply the still grade").to_string();
        return;
    }
    if app.ui.gallery_tab == 0 {
        if let Err(err) = app.session.execute("color.stillApply", json!({"id": still.id, "clips": clips, "mode": if append { "append" } else { "replace" }})) {
            app.ui.status = err.to_string();
        }
    } else {
        // Apply PowerGrade directly via project edit
        let st = still.clone();
        let res = app.session.edit(if append { "Append PowerGrade" } else { "Apply PowerGrade" }, move |pr, ed| {
            let Some(seq_id) = ed.active_sequence else { return Ok(()) };
            let Some(seq) = pr.sequence_mut(seq_id) else { return Ok(()) };
            for raw in &clips {
                let Some((_, it)) = seq.find_item_mut(filmcraft_project::ClipId(*raw)) else { continue };
                if append {
                    for fx in &st.effects {
                        it.effects.push(fx.clone());
                    }
                    it.color_graph = Some(Box::new(filmcraft_project::ColorGraph::from_serial_effects(&it.effects)));
                } else {
                    it.effects.retain(|e| e.effect != "lumetri");
                    it.color_graph = Some(Box::new(st.graph.clone()));
                    it.sync_lumetri_effects_from_graph();
                    if it.effects.iter().all(|e| e.effect != "lumetri") {
                        it.effects.extend(st.effects.iter().cloned());
                    }
                }
            }
            Ok(())
        });
        if let Err(err) = res {
            app.ui.status = err.to_string();
        }
    }
}
