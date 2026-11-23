//! Effects panel: searchable folder tree of presets, audio/video effects and transitions (from the
//! effect definitions). Drag onto a clip or edit point; double-click applies to the selection.
//!
//! Color Grading Presets: each preset row has a thumbnail (our Color Grading on the procedural preview
//! picture, `filmcraft_render::lumetri_presets`); double-click (or right-click ▸ Apply to Selected
//! Clips, `effects.presetMenu.apply`) applies it (`lumetri.applyPreset`), and dragging it onto a
//! Timeline clip applies it to that clip (rows and grid cells alike).
//! When the panel is wide (maximized) and a Color Grading Presets folder is active, its presets
//! appear as a scrollable thumbnail grid to the right of the tree. Automation ids:
//! `effects.lumetriPreset.<name>`, `effects.presetGrid`, `effects.presetGrid.<name>`.

use std::collections::HashMap;

use egui::{Align2, Color32, CursorIcon, Key, Rect, Sense, pos2, vec2};
use serde_json::json;

use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::icons::{self, Icon};
use crate::panels::presets::HoveredPreset;
use crate::state::EffectsKindFilter;
use crate::theme::Tokens;
use filmcraft_project::EffectKind;

const FOOTER_H: f32 = 32.0;

#[derive(Clone)]
enum HoveredItem {
    Effect(&'static filmcraft_project::EffectDef),
    Preset(HoveredPreset),
}

#[derive(Clone)]
pub(crate) enum VisibleNode {
    Folder { key: String, open: bool, parent: Option<String>, rect: Rect },
    Effect { id: &'static str, parent: Option<String>, rect: Rect },
    Preset { name: String, parent: Option<String>, rect: Rect },
    LumetriPreset { name: String, parent: Option<String>, rect: Rect },
}

/// Actions collected from effect rows during a frame and dispatched after the tree pass.
#[derive(Default)]
struct EffectRowActions {
    apply: Option<String>,
    apply_preset: Option<String>,
    apply_lumetri: Option<String>,
    set_default: Option<String>,
    toggle_fav: Option<String>,
    record_recent: Option<String>,
    clear_recent: bool,
    hovered: Option<HoveredItem>,
    visible_nodes: Vec<VisibleNode>,
    scroll_to_rect: Option<Rect>,
}

#[derive(Clone, PartialEq, Eq)]
struct CatalogCacheKey {
    filter: String,
    filter_gpu: bool,
    filter_32: bool,
    filter_fav: bool,
    kind_filter: EffectsKindFilter,
    lang: crate::i18n::Language,
    favorites: Vec<String>,
}

#[derive(Clone)]
struct CachedTopGroup {
    top: &'static str,
    indices: Vec<usize>,
    subs: Vec<(&'static str, Vec<usize>)>,
}

#[derive(Clone, Default)]
struct EffectsCatalogCache {
    key: Option<CatalogCacheKey>,
    groups: Vec<CachedTopGroup>,
}

fn domain_color(d: &filmcraft_project::EffectDef, is_default_tr: bool, t: &Tokens) -> Color32 {
    if is_default_tr {
        return t.accent;
    }
    if d.id.starts_with("ec.") {
        return Color32::from_rgb(0x4C, 0xC2, 0xD8);
    }
    match d.kind {
        EffectKind::Audio => Color32::from_rgb(0x4E, 0xB8, 0x9C),
        EffectKind::VideoTransition | EffectKind::AudioTransition => t.text_dim,
        EffectKind::Video => {
            let sub = d.category.get(1).copied().unwrap_or("");
            match sub {
                "Color Correction" | "Color" | "Grading" => Color32::from_rgb(0xE0, 0xA4, 0x45),
                "Keying" | "Matte" | "Channel" => Color32::from_rgb(0x52, 0xB8, 0x8A),
                "Blur & Sharpen" | "Stylize" | "Distort" | "Perspective" | "Generate" => Color32::from_rgb(0x7B, 0x9E, 0xE8),
                _ => t.text_dim,
            }
        }
    }
}

fn draw_effect_icon(ui: &mut egui::Ui, x: f32, cy: f32, d: &filmcraft_project::EffectDef, is_default_tr: bool, t: &Tokens) {
    let icon_r = Rect::from_center_size(pos2(x - 12.0, cy), vec2(13.0, 13.0));
    let fg = domain_color(d, is_default_tr, t);
    let stroke = egui::Stroke::new(if is_default_tr { 1.5 } else { 1.0 }, fg);
    ui.painter().rect_stroke(icon_r, 2.0, stroke, egui::StrokeKind::Inside);
    match d.kind {
        EffectKind::VideoTransition => {
            ui.painter().line_segment([icon_r.left_bottom(), icon_r.right_top()], egui::Stroke::new(1.0, fg));
        }
        EffectKind::AudioTransition => {
            ui.painter().line_segment([icon_r.left_bottom(), icon_r.right_top()], egui::Stroke::new(1.0, fg));
            ui.painter().line_segment([icon_r.left_top(), icon_r.right_bottom()], egui::Stroke::new(1.0, fg));
        }
        EffectKind::Audio => {
            icons::paint(ui.painter(), icon_r.shrink(2.0), Icon::Audio, fg);
        }
        EffectKind::Video => {
            let sub = d.category.get(1).copied().unwrap_or("");
            match sub {
                "Color Correction" | "Color" | "Grading" => icons::paint(ui.painter(), icon_r.shrink(2.0), Icon::Adjust, fg),
                "Keying" | "Matte" | "Channel" => icons::paint(ui.painter(), icon_r.shrink(2.0), Icon::Grid, fg),
                "Generate" => icons::paint(ui.painter(), icon_r.shrink(2.0), Icon::Sparkle, fg),
                "Blur & Sharpen" | "Stylize" | "Distort" | "Perspective" => icons::paint(ui.painter(), icon_r.shrink(2.0), Icon::Fx, fg),
                _ => {
                    ui.painter().text(icon_r.center(), Align2::CENTER_CENTER, "fx", Tokens::ui(8.0), fg);
                }
            }
        }
    }
}

fn is_default_transition(app: &FilmcraftApp, d: &filmcraft_project::EffectDef) -> bool {
    match d.kind {
        EffectKind::VideoTransition => d.id == app.session.state.default_video_transition,
        EffectKind::AudioTransition => d.id == app.session.state.default_audio_transition,
        _ => false,
    }
}

fn paint_highlighted_label(
    ui: &egui::Ui,
    clip: Rect,
    pos: egui::Pos2,
    (text, filter, alias_hint): (&str, &str, Option<&str>),
    base_color: Color32,
    t: &Tokens,
) {
    let painter = ui.painter().with_clip_rect(clip);
    let font = Tokens::ui(12.0);
    let tokens: Vec<&str> = filter.split_whitespace().collect();
    if !tokens.is_empty() {
        let lower_text = text.to_lowercase();
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for tok in &tokens {
            let lower_q = tok.to_lowercase();
            if lower_q.is_empty() {
                continue;
            }
            if let Some(byte_start) = lower_text.find(&lower_q)
                && text.is_char_boundary(byte_start)
                && text.is_char_boundary(byte_start + lower_q.len())
            {
                ranges.push((byte_start, byte_start + lower_q.len()));
            }
        }
        if !ranges.is_empty() {
            ranges.sort_by_key(|&(s, _)| s);
            let mut merged: Vec<(usize, usize)> = Vec::new();
            for (s, e) in ranges {
                if let Some(last) = merged.last_mut()
                    && s <= last.1
                {
                    last.1 = last.1.max(e);
                } else {
                    merged.push((s, e));
                }
            }
            let mut cur_x = pos.x;
            let mut cursor = 0usize;
            for (s, e) in merged {
                if s > cursor
                    && let Some(before) = text.get(cursor..s)
                    && !before.is_empty()
                {
                    let g_before = painter.layout_no_wrap(before.to_string(), font.clone(), base_color);
                    let w = g_before.size().x;
                    painter.galley(pos2(cur_x, pos.y - g_before.size().y * 0.5), g_before, base_color);
                    cur_x += w;
                }
                if let Some(matched) = text.get(s..e)
                    && !matched.is_empty()
                {
                    let g_match = painter.layout_no_wrap(matched.to_string(), font.clone(), Color32::WHITE);
                    let mw = g_match.size().x;
                    let mh = g_match.size().y;
                    let top_y = pos.y - mh * 0.5;
                    painter.rect_filled(Rect::from_min_size(pos2(cur_x - 1.0, top_y), vec2(mw + 2.0, mh)), 2.0, t.accent.gamma_multiply(0.45));
                    painter.galley(pos2(cur_x, top_y), g_match, Color32::WHITE);
                    cur_x += mw;
                }
                cursor = e;
            }
            if cursor < text.len()
                && let Some(after) = text.get(cursor..)
                && !after.is_empty()
            {
                painter.text(pos2(cur_x, pos.y), Align2::LEFT_CENTER, after, font, base_color);
            }
            return;
        }
    }
    let g_main = painter.layout_no_wrap(text.to_string(), font.clone(), base_color);
    let main_w = g_main.size().x;
    painter.galley(pos2(pos.x, pos.y - g_main.size().y * 0.5), g_main, base_color);
    if let Some(alias) = alias_hint {
        painter.text(pos2(pos.x + main_w + 5.0, pos.y), Align2::LEFT_CENTER, format!("({alias})"), Tokens::ui(10.5), t.text_dim);
    }
}

#[allow(clippy::too_many_arguments)]
fn show_effect_row(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    d: &'static filmcraft_project::EffectDef,
    depth: usize,
    parent_folder: Option<&str>,
    auto_id_prefix: &str,
    show_ec_badge: bool,
    filter: &str,
    actions: &mut EffectRowActions,
) {
    let t = app.tokens;
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click_and_drag());
    actions.visible_nodes.push(VisibleNode::Effect { id: d.id, parent: parent_folder.map(str::to_string), rect: r });
    let visible = ui.is_rect_visible(r);
    let is_selected = app.ui.selected_effect.as_deref() == Some(d.id);
    let hovered = visible && (resp.hovered() || ui.rect_contains_pointer(r));
    if is_selected {
        ui.painter().rect_filled(r, 0.0, t.row_selected);
    } else if hovered {
        ui.painter().rect_filled(r, 0.0, t.hover);
    }
    if hovered {
        actions.hovered = Some(HoveredItem::Effect(d));
    }

    let x = r.min.x + tree_x(depth) + 30.0;
    let is_tr = matches!(d.kind, EffectKind::VideoTransition | EffectKind::AudioTransition);
    let is_default_tr = is_default_transition(app, d);
    draw_effect_icon(ui, x, r.center().y, d, is_default_tr, &t);

    let is_fav = app.ui.favorite_effects.iter().any(|f| f == d.id);
    let mut bx = r.max.x - 6.0;

    // Quick Apply (+) button on hover
    if hovered {
        let plus_r = Rect::from_min_size(pos2(bx - 16.0, r.min.y + 2.0), vec2(16.0, 16.0));
        let plus_resp = ui.interact(plus_r, ui.scope_id().with((auto_id_prefix, "apply", d.id)), Sense::click()).on_hover_text(tl!("Apply to Selected"));
        if plus_resp.hovered() {
            ui.painter().rect_filled(plus_r, 2.0, t.accent);
        }
        icons::paint(ui.painter(), plus_r.shrink(3.0), Icon::Plus, if plus_resp.hovered() { Color32::WHITE } else { t.text_dim });
        app.auto.add(&format!("effects.item.apply.{}", d.id), plus_r, "Apply");
        if plus_resp.clicked() {
            actions.apply = Some(d.id.to_string());
            actions.record_recent = Some(d.id.to_string());
        }
        bx -= 18.0;
    }

    // Interactive Favorite button when starred or on hover
    if visible && (is_fav || hovered) {
        let star_r = Rect::from_min_size(pos2(bx - 16.0, r.min.y + 2.0), vec2(16.0, 16.0));
        let star_resp = ui.interact(star_r, ui.scope_id().with((auto_id_prefix, "fav", d.id)), Sense::click()).on_hover_text(if is_fav {
            tl!("Remove from Favorites")
        } else {
            tl!("Add to Favorites")
        });
        let star_fg = if is_fav {
            t.accent
        } else if star_resp.hovered() {
            t.text
        } else {
            t.text_dim
        };
        icons::paint(ui.painter(), star_r.shrink(2.5), Icon::Star, star_fg);
        app.auto.add(&format!("effects.item.fav.{}", d.id), star_r, "Favorite");
        if star_resp.clicked() {
            actions.toggle_fav = Some(d.id.to_string());
        }
        bx -= 18.0;
    }

    // Responsive technical badges: keep at least 100px for the effect label.
    let mut badge_budget = (bx - x - 100.0).max(0.0);
    if is_default_tr && badge_budget >= 24.0 {
        let br = Rect::from_min_size(pos2(bx - 24.0, r.min.y + 3.0), vec2(22.0, 14.0));
        ui.painter().rect_filled(br, 2.0, t.accent);
        ui.painter().text(br.center(), Align2::CENTER_CENTER, "DEF", Tokens::ui(8.0), Color32::WHITE);
        bx -= 26.0;
        badge_budget -= 26.0;
    }
    if d.accelerated && badge_budget >= 22.0 {
        let br = Rect::from_min_size(pos2(bx - 22.0, r.min.y + 3.0), vec2(20.0, 14.0));
        ui.painter().rect_filled(br, 2.0, t.field_border);
        let bp = ui.painter().with_clip_rect(br);
        icons::paint(&bp, br.shrink(2.5), Icon::Bolt, t.text_dim);
        bx -= 24.0;
        badge_budget -= 24.0;
    }
    for (on, label) in [(show_ec_badge, "EC"), (d.float32, "32"), (d.yuv, "YUV")] {
        if on && badge_budget >= 22.0 {
            let br = Rect::from_min_size(pos2(bx - 22.0, r.min.y + 3.0), vec2(20.0, 14.0));
            ui.painter().rect_filled(br, 2.0, t.field_border);
            ui.painter().text(br.center(), Align2::CENTER_CENTER, label, Tokens::ui(8.5), t.text_dim);
            bx -= 24.0;
            badge_budget -= 24.0;
        }
    }

    let label_max_x = (bx - 4.0).max(x);
    let label_clip = Rect::from_min_max(pos2(x, r.min.y), pos2(label_max_x, r.max.y));
    let translated = crate::i18n::t(d.name);
    let alias_hint = if !filter.is_empty() && !matches_tokens(translated, filter) {
        filmcraft_project::search_aliases(d).iter().copied().find(|a| matches_tokens(a, filter))
    } else {
        None
    };
    paint_highlighted_label(ui, label_clip, pos2(x, r.center().y), (translated, filter, alias_hint), if is_selected { Color32::WHITE } else { t.text }, &t);

    app.auto.add(&format!("{auto_id_prefix}.{}", d.id), r, d.name);

    if resp.clicked() {
        app.ui.selected_effect = Some(d.id.to_string());
    }
    if resp.drag_started_by(egui::PointerButton::Primary) {
        app.ui.selected_effect = Some(d.id.to_string());
        crate::panels::start_drag_effect(ui, d.id);
        actions.record_recent = Some(d.id.to_string());
    }
    if resp.double_clicked() {
        app.ui.selected_effect = Some(d.id.to_string());
        actions.apply = Some(d.id.to_string());
        actions.record_recent = Some(d.id.to_string());
    }

    resp.context_menu(|ui| {
        let apply_label = tl!("Apply to Selected");
        let b_apply = ui.button(apply_label);
        app.auto.add("effects.itemMenu.apply", b_apply.rect, apply_label);
        if b_apply.clicked() {
            actions.apply = Some(d.id.to_string());
            actions.record_recent = Some(d.id.to_string());
            ui.close();
        }
        if is_tr {
            let b_def = ui.button(tl!("Set as Default Transition"));
            app.auto.add("effects.itemMenu.setDefaultTransition", b_def.rect, "Set as Default Transition");
            if b_def.clicked() {
                actions.set_default = Some(d.id.to_string());
                ui.close();
            }
        }
        let fav_label = if is_fav { tl!("Remove from Favorites") } else { tl!("Add to Favorites") };
        let b_fav = ui.button(fav_label);
        app.auto.add("effects.menu.toggleFavorite", b_fav.rect, fav_label);
        if b_fav.clicked() {
            actions.toggle_fav = Some(d.id.to_string());
            ui.close();
        }
        ui.separator();
        if ui.button(tl!("Copy Effect Name")).clicked() {
            ui.ctx().copy_text(crate::i18n::t(d.name).to_string());
            ui.close();
        }
        if ui.button(tl!("Copy Effect ID")).clicked() {
            ui.ctx().copy_text(d.id.to_string());
            ui.close();
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn show_favorite_effects(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    defs: &'static [filmcraft_project::EffectDef],
    filter: &str,
    effective_filter: &str,
    filter_gpu: bool,
    filter_32: bool,
    kind_filter: EffectsKindFilter,
    actions: &mut EffectRowActions,
) -> usize {
    let fav_defs: Vec<_> = app
        .ui
        .favorite_effects
        .iter()
        .filter_map(|id| defs.iter().find(|d| d.id == id))
        .filter(|d| kind_matches_effect(kind_filter, d.kind) && (!filter_gpu || d.accelerated) && (!filter_32 || d.float32) && matches_effect(d, filter))
        .collect();
    if fav_defs.is_empty() {
        return 0;
    }
    let count = fav_defs.len();
    if !folder_row(app, ui, "Favorites", 0, effective_filter, Some(count), None, actions) {
        return 1;
    }
    for d in fav_defs {
        show_effect_row(app, ui, d, 1, Some("Favorites"), "effects.favorites", false, filter, actions);
    }
    count
}

#[allow(clippy::too_many_arguments)]
fn show_recent_effects(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    defs: &'static [filmcraft_project::EffectDef],
    filter: &str,
    effective_filter: &str,
    filter_gpu: bool,
    filter_32: bool,
    kind_filter: EffectsKindFilter,
    actions: &mut EffectRowActions,
) -> usize {
    let recent_defs: Vec<_> = app
        .ui
        .recent_effects
        .iter()
        .filter_map(|id| defs.iter().find(|d| d.id == id))
        .filter(|d| kind_matches_effect(kind_filter, d.kind) && (!filter_gpu || d.accelerated) && (!filter_32 || d.float32) && matches_effect(d, filter))
        .collect();
    if recent_defs.is_empty() {
        return 0;
    }
    let count = recent_defs.len();
    let mut clear_req = false;
    let open = folder_row_with_menu(app, ui, "Recent", 0, effective_filter, Some(count), None, actions, |app, ui| {
        let b = ui.button(tl!("Clear Recent Effects"));
        app.auto.add("effects.action.clearRecent", b.rect, "Clear Recent Effects");
        if b.clicked() {
            clear_req = true;
            ui.close();
        }
    });
    if clear_req {
        actions.clear_recent = true;
    }
    if !open {
        return 1;
    }
    for d in recent_defs {
        show_effect_row(app, ui, d, 1, Some("Recent"), "effects.recent", false, filter, actions);
    }
    count
}

fn record_recent_effect(recent: &mut Vec<String>, id: &str) {
    recent.retain(|x| x != id);
    recent.insert(0, id.to_string());
    recent.truncate(10);
}

fn toggle_favorite_effect(favs: &mut Vec<String>, id: &str) {
    if let Some(pos) = favs.iter().position(|x| x == id) {
        favs.remove(pos);
    } else {
        favs.push(id.to_string());
    }
}

fn kind_matches_effect(kind_filter: EffectsKindFilter, kind: EffectKind) -> bool {
    match kind_filter {
        EffectsKindFilter::All => true,
        EffectsKindFilter::Video => kind == EffectKind::Video,
        EffectsKindFilter::Audio => kind == EffectKind::Audio,
        EffectsKindFilter::Transitions => matches!(kind, EffectKind::VideoTransition | EffectKind::AudioTransition),
        EffectsKindFilter::Presets => false,
    }
}

fn kind_matches_top(kind_filter: EffectsKindFilter, top: &str) -> bool {
    match kind_filter {
        EffectsKindFilter::All => true,
        EffectsKindFilter::Video => top == "Video Effects" || top == "Legacy" || top == filmcraft_project::EFFECTCRAFT_TOP_FOLDER,
        EffectsKindFilter::Audio => top == "Audio Effects",
        EffectsKindFilter::Transitions => top == "Video Transitions" || top == "Audio Transitions",
        EffectsKindFilter::Presets => top == "Presets" || top == filmcraft_project::vtransition::COLOR_PRESETS_FOLDER_KEY,
    }
}

/// Header with search box, clear-search button, quick-filter toggles, collapse-all, inspector toggle, and wrapping kind pills.
/// Returns `(search_focused, search_jumped_down, search_submitted, body_top_y)`.
fn show_header(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) -> (bool, bool, bool, f32) {
    let t = app.tokens;
    let has_query = !app.ui.effects_search.is_empty();
    let show_inspector_btn = rect.width() > 480.0;
    let clear_w = if has_query { 20.0 } else { 0.0 };
    // 3 filter toggles + 1 collapse-all button (+ optional inspector toggle)
    let btn_count = if show_inspector_btn { 5.0 } else { 4.0 };
    let actions_w = btn_count * 24.0 + clear_w;
    let sr = Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2((rect.width() - 16.0 - actions_w).max(60.0), 22.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sr));
    let mut q = app.ui.effects_search.clone();
    let search_resp = crate::widgets::search_field(&mut child, &mut q, tl!("Search effects"), sr.width(), &t);
    if app.ui.focused == PanelKind::Effects && ui.input(|i| i.modifiers.command && i.key_pressed(Key::F)) {
        search_resp.request_focus();
    }
    let mut search_focused = search_resp.has_focus();
    let mut search_down = false;
    let mut search_enter = false;
    if search_focused && ui.input(|i| i.key_pressed(Key::Escape)) {
        q.clear();
        search_resp.surrender_focus();
        search_focused = false;
    } else if search_focused && ui.input(|i| i.key_pressed(Key::ArrowDown)) {
        search_resp.surrender_focus();
        search_focused = false;
        search_down = true;
    } else if search_focused && !q.trim().is_empty() && ui.input(|i| i.key_pressed(Key::Enter)) {
        search_enter = true;
    }
    app.ui.effects_search = q;

    let mut bx = sr.max.x + 4.0;
    if has_query {
        let cr = Rect::from_min_size(pos2(bx, sr.min.y + 2.0), vec2(18.0, 18.0));
        let resp = ui.interact(cr, ui.scope_id().with("effects.action.clearSearch"), Sense::click()).on_hover_text(tl!("Clear Search"));
        ui.painter().rect_filled(cr, 3.0, if resp.hovered() { t.hover } else { t.field_border });
        icons::paint(ui.painter(), cr.shrink(3.5), Icon::Close, t.text_dim);
        app.auto.add("effects.action.clearSearch", cr, "Clear Search");
        if resp.clicked() {
            app.ui.effects_search.clear();
        }
        bx += 20.0;
    }

    for (i, (id, icon_opt, label, tip, on)) in [
        ("effects.filter.gpu", Some(Icon::Bolt), "⚡", tl!("Accelerated Effects (GPU)"), &mut app.ui.effects_filter_gpu),
        ("effects.filter.float32", None, "32", tl!("32-bit Float Color Effects"), &mut app.ui.effects_filter_32bit),
        ("effects.filter.favorites", Some(Icon::Star), "★", tl!("Favorite Effects Only"), &mut app.ui.effects_filter_favorites),
    ]
    .into_iter()
    .enumerate()
    {
        let br = Rect::from_min_size(pos2(bx + i as f32 * 24.0, sr.min.y + 2.0), vec2(22.0, 18.0));
        let resp = ui.interact(br, ui.scope_id().with(id), Sense::click()).on_hover_text(tip);
        let bg = if *on {
            t.accent
        } else if resp.hovered() {
            t.hover
        } else {
            t.field_border
        };
        let fg = if *on { Color32::WHITE } else { t.text_dim };
        ui.painter().rect_filled(br, 3.0, bg);
        if let Some(icon) = icon_opt {
            icons::paint(ui.painter(), br.shrink(3.5), icon, fg);
        } else {
            ui.painter().text(br.center(), Align2::CENTER_CENTER, label, Tokens::ui(9.5), fg);
        }
        app.auto.add(id, br, label);
        if resp.clicked() {
            *on = !*on;
        }
    }

    // Collapse All folders button
    let col_r = Rect::from_min_size(pos2(bx + 3.0 * 24.0, sr.min.y + 2.0), vec2(22.0, 18.0));
    let col_resp = ui.interact(col_r, ui.scope_id().with("effects.action.collapseAll"), Sense::click()).on_hover_text(tl!("Collapse All Folders"));
    ui.painter().rect_filled(col_r, 3.0, if col_resp.hovered() { t.hover } else { t.field_border });
    icons::paint(ui.painter(), col_r.shrink(3.5), Icon::CollapseAll, t.text_dim);
    app.auto.add("effects.action.collapseAll", col_r, "Collapse All");
    if col_resp.clicked() {
        app.ui.expanded_fx.clear();
        app.ui.lumetri_grid_folder = None;
    }

    if show_inspector_btn {
        let ins_r = Rect::from_min_size(pos2(bx + 4.0 * 24.0, sr.min.y + 2.0), vec2(22.0, 18.0));
        let active = app.ui.effects_inspector_open && app.ui.lumetri_grid_folder.is_none();
        let ins_resp = ui.interact(ins_r, ui.scope_id().with("effects.action.toggleInspector"), Sense::click()).on_hover_text(tl!("Effect Inspector"));
        let bg = if active {
            t.accent
        } else if ins_resp.hovered() {
            t.hover
        } else {
            t.field_border
        };
        let fg = if active { Color32::WHITE } else { t.text_dim };
        ui.painter().rect_filled(ins_r, 3.0, bg);
        icons::paint(ui.painter(), ins_r.shrink(3.5), Icon::Info, fg);
        app.auto.add("effects.action.toggleInspector", ins_r, "Effect Inspector");
        if ins_resp.clicked() {
            app.ui.effects_inspector_open = !app.ui.effects_inspector_open;
            if app.ui.effects_inspector_open {
                app.ui.lumetri_grid_folder = None;
            }
        }
    }

    let bottom_y = show_kind_pills(app, ui, rect, sr.max.y + 4.0) + 22.0;
    ui.painter().line_segment([pos2(rect.min.x, bottom_y), pos2(rect.max.x, bottom_y)], egui::Stroke::new(1.0, t.separator));
    (search_focused, search_down, search_enter, bottom_y + 2.0)
}

fn show_kind_pills(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, start_y: f32) -> f32 {
    let t = app.tokens;
    let mut pills_y = start_y;
    let mut px = rect.min.x + 8.0;
    let pills = [
        (EffectsKindFilter::All, "effects.kind.all", tl!("All")),
        (EffectsKindFilter::Video, "effects.kind.video", tl!("Video")),
        (EffectsKindFilter::Audio, "effects.kind.audio", tl!("Audio")),
        (EffectsKindFilter::Transitions, "effects.kind.transitions", tl!("Transitions")),
        (EffectsKindFilter::Presets, "effects.kind.presets", tl!("Presets")),
    ];
    for (kind, id, label) in pills {
        let galley = ui.painter().layout_no_wrap(label.to_string(), Tokens::ui(10.5), t.text);
        let pw = (galley.size().x + 12.0).max(28.0);
        if px > rect.min.x + 8.0 && px + pw > rect.max.x - 8.0 {
            px = rect.min.x + 8.0;
            pills_y += 20.0;
        }
        let pr = Rect::from_min_size(pos2(px, pills_y), vec2(pw, 18.0));
        let resp = ui.interact(pr, ui.scope_id().with(id), Sense::click());
        let active = app.ui.effects_kind_filter == kind;
        let bg = if active {
            t.pill_active_bg
        } else if resp.hovered() {
            t.hover
        } else {
            Color32::TRANSPARENT
        };
        let fg = if active { Color32::WHITE } else { t.text_dim };
        if bg != Color32::TRANSPARENT {
            ui.painter().rect_filled(pr, 3.0, bg);
        }
        ui.painter().text(pr.center(), Align2::CENTER_CENTER, label, Tokens::ui(10.5), fg);
        app.auto.add(id, pr, label);
        if resp.clicked() {
            app.ui.effects_kind_filter = kind;
        }
        px += pw + 4.0;
    }
    pills_y
}

fn show_empty_state(app: &mut FilmcraftApp, ui: &mut egui::Ui, area: Rect, presets_hidden_by_tech_filter: bool) {
    let t = app.tokens;
    let cy = area.center().y.min(area.min.y + 110.0);
    let icon_r = Rect::from_center_size(pos2(area.center().x, cy - 22.0), vec2(20.0, 20.0));
    icons::paint(ui.painter(), icon_r, Icon::Search, t.text_dim);
    let msg = if presets_hidden_by_tech_filter { tl!("Presets do not use GPU/32-bit filters") } else { tl!("No effects match the current filter") };
    ui.painter().text(pos2(area.center().x, cy + 4.0), Align2::CENTER_CENTER, msg, Tokens::ui(12.0), t.text_dim);
    let btn_label = if presets_hidden_by_tech_filter { tl!("Show Presets") } else { tl!("Clear Filters") };
    let btn_id = if presets_hidden_by_tech_filter { "effects.action.clearTechFilters" } else { "effects.action.clearFilters" };
    let btn_r = Rect::from_center_size(pos2(area.center().x, cy + 32.0), vec2(120.0, 22.0));
    let resp = ui.interact(btn_r, ui.scope_id().with(btn_id), Sense::click());
    ui.painter().rect_filled(btn_r, 3.0, if resp.hovered() { t.accent_hover } else { t.field_border });
    ui.painter().text(btn_r.center(), Align2::CENTER_CENTER, btn_label, Tokens::ui(11.0), t.text);
    app.auto.add(btn_id, btn_r, btn_label);
    if resp.clicked() {
        app.ui.effects_filter_gpu = false;
        app.ui.effects_filter_32bit = false;
        if !presets_hidden_by_tech_filter {
            app.ui.effects_search.clear();
            app.ui.effects_filter_favorites = false;
            app.ui.effects_kind_filter = EffectsKindFilter::All;
        }
    }
}

fn selected_target_label(app: &FilmcraftApp) -> Option<String> {
    let seq = app.session.active_sequence()?;
    let id = app.session.state.selection.first()?;
    let (_, it) = seq.find_item(*id)?;
    let name = if it.name.is_empty() { format!("Clip #{}", id.0) } else { it.name.clone() };
    Some(name)
}

fn show_info_footer(app: &mut FilmcraftApp, ui: &mut egui::Ui, footer: Rect, item: Option<&HoveredItem>, visible_rows: usize) {
    let t = app.tokens;
    ui.painter().rect_filled(footer, 0.0, t.header_bg);
    ui.painter().line_segment([footer.left_top(), footer.right_top()], egui::Stroke::new(1.0, t.separator));
    app.auto.add("effects.infoFooter", footer, "Effects Info Footer");
    let clip = footer.shrink2(vec2(8.0, 2.0));
    let target_suffix = match selected_target_label(app) {
        Some(name) => format!(" • {}: {name}", tl!("Target")),
        None => format!(" • {}", tl!("No clip selected")),
    };
    match item {
        Some(HoveredItem::Effect(d)) => {
            let cat = d.category.iter().map(|c| crate::i18n::t(c)).collect::<Vec<_>>().join(" ▸ ");
            let mut meta = Vec::new();
            if d.accelerated {
                meta.push("GPU");
            }
            if d.float32 {
                meta.push("32-bit");
            }
            if d.yuv {
                meta.push("YUV");
            }
            if is_default_transition(app, d) {
                meta.push("Default");
            }
            let meta_str = if meta.is_empty() { format!("{} params", d.params.len()) } else { format!("{} • {} params", meta.join(" · "), d.params.len()) };
            ui.painter().with_clip_rect(clip).text(pos2(clip.min.x, clip.min.y + 8.0), Align2::LEFT_CENTER, crate::i18n::t(d.name), Tokens::ui(11.5), t.text);
            ui.painter().with_clip_rect(clip).text(
                pos2(clip.min.x, clip.min.y + 21.0),
                Align2::LEFT_CENTER,
                format!("{cat}  —  {meta_str}{target_suffix}"),
                Tokens::ui(10.0),
                t.text_dim,
            );
        }
        Some(HoveredItem::Preset(p)) => {
            let kind_lbl = if p.builtin { tl!("Built-in Preset") } else { tl!("User Preset") };
            let desc = if p.description.is_empty() { kind_lbl.to_string() } else { crate::i18n::t(&p.description).to_string() };
            ui.painter().with_clip_rect(clip).text(pos2(clip.min.x, clip.min.y + 8.0), Align2::LEFT_CENTER, crate::i18n::t(&p.name), Tokens::ui(11.5), t.text);
            ui.painter().with_clip_rect(clip).text(
                pos2(clip.min.x, clip.min.y + 21.0),
                Align2::LEFT_CENTER,
                format!("{desc}{target_suffix}"),
                Tokens::ui(10.0),
                t.text_dim,
            );
        }
        None => {
            let hint = format!("{visible_rows} {} • {}{target_suffix}", tl!("items"), tl!("Drag onto a clip or double-click to apply"));
            ui.painter().with_clip_rect(clip).text(clip.center(), Align2::CENTER_CENTER, hint, Tokens::ui(10.5), t.text_dim);
        }
    }
}

fn handle_keyboard_nav(
    app: &mut FilmcraftApp,
    ui: &egui::Ui,
    (search_focused, search_down, search_enter): (bool, bool, bool),
    filter: &str,
    actions: &mut EffectRowActions,
) {
    if actions.visible_nodes.is_empty() {
        return;
    }
    if search_enter {
        if let Some(first_leaf) = actions.visible_nodes.iter().find(|n| !matches!(n, VisibleNode::Folder { .. })) {
            match first_leaf {
                VisibleNode::Effect { id, .. } => {
                    app.ui.selected_effect = Some((*id).to_string());
                    actions.apply = Some((*id).to_string());
                    actions.record_recent = Some((*id).to_string());
                }
                VisibleNode::Preset { name, .. } => {
                    actions.apply_preset = Some(name.clone());
                }
                VisibleNode::LumetriPreset { name, .. } => {
                    actions.apply_lumetri = Some(name.clone());
                }
                VisibleNode::Folder { .. } => {}
            }
        }
        return;
    }

    let sel_folder_id = egui::Id::unique("effects-selected-folder");
    let sel_item_id = egui::Id::unique("effects-selected-preset");
    let mut sel_folder: Option<String> = ui.ctx().data(|d| d.get_temp(sel_folder_id)).unwrap_or(None);
    let mut sel_item: Option<String> = ui.ctx().data(|d| d.get_temp(sel_item_id)).unwrap_or(None);

    let select_node =
        |app: &mut FilmcraftApp, sel_folder: &mut Option<String>, sel_item: &mut Option<String>, node: &VisibleNode, actions: &mut EffectRowActions| match node
        {
            VisibleNode::Effect { id, rect, .. } => {
                app.ui.selected_effect = Some((*id).to_string());
                *sel_folder = None;
                *sel_item = None;
                actions.scroll_to_rect = Some(*rect);
            }
            VisibleNode::Folder { key, rect, .. } => {
                app.ui.selected_effect = None;
                *sel_folder = Some(key.clone());
                *sel_item = None;
                actions.scroll_to_rect = Some(*rect);
            }
            VisibleNode::Preset { name, rect, .. } => {
                app.ui.selected_effect = None;
                *sel_folder = None;
                *sel_item = Some(format!("preset:{name}"));
                actions.scroll_to_rect = Some(*rect);
            }
            VisibleNode::LumetriPreset { name, rect, .. } => {
                app.ui.selected_effect = None;
                *sel_folder = None;
                *sel_item = Some(format!("lumetri:{name}"));
                actions.scroll_to_rect = Some(*rect);
            }
        };

    if search_down {
        let target = actions.visible_nodes.iter().find(|n| !matches!(n, VisibleNode::Folder { .. })).or_else(|| actions.visible_nodes.first()).cloned();
        if let Some(node) = target {
            select_node(app, &mut sel_folder, &mut sel_item, &node, actions);
            ui.ctx().data_mut(|d| {
                d.insert_temp(sel_folder_id, sel_folder);
                d.insert_temp(sel_item_id, sel_item);
            });
        }
        return;
    }

    if app.ui.focused != PanelKind::Effects || search_focused {
        return;
    }

    let down = ui.input(|i| i.key_pressed(Key::ArrowDown));
    let up = ui.input(|i| i.key_pressed(Key::ArrowUp));
    let right = ui.input(|i| i.key_pressed(Key::ArrowRight));
    let left = ui.input(|i| i.key_pressed(Key::ArrowLeft));
    let home = ui.input(|i| i.key_pressed(Key::Home));
    let end = ui.input(|i| i.key_pressed(Key::End));
    let enter = ui.input(|i| i.key_pressed(Key::Enter));
    let esc = ui.input(|i| i.key_pressed(Key::Escape));
    let fav_key_pressed = ui.input(|i| i.modifiers.is_none() && i.key_pressed(Key::F));

    if esc && !app.ui.effects_search.is_empty() {
        app.ui.effects_search.clear();
        return;
    }

    let current_idx = if let Some(eid) = app.ui.selected_effect.as_deref() {
        actions.visible_nodes.iter().position(|n| matches!(n, VisibleNode::Effect { id, .. } if *id == eid))
    } else if let Some(ref item_k) = sel_item {
        actions.visible_nodes.iter().position(|n| match n {
            VisibleNode::Preset { name, .. } => item_k == &format!("preset:{name}"),
            VisibleNode::LumetriPreset { name, .. } => item_k == &format!("lumetri:{name}"),
            _ => false,
        })
    } else if let Some(ref fkey) = sel_folder {
        actions.visible_nodes.iter().position(|n| matches!(n, VisibleNode::Folder { key, .. } if key == fkey))
    } else {
        None
    };

    if home || end {
        let idx = if home { 0 } else { actions.visible_nodes.len().saturating_sub(1) };
        if let Some(node) = actions.visible_nodes.get(idx).cloned() {
            select_node(app, &mut sel_folder, &mut sel_item, &node, actions);
        }
    } else if down || up {
        let len = actions.visible_nodes.len();
        let next_idx = match current_idx {
            Some(idx) if down => (idx + 1).min(len.saturating_sub(1)),
            Some(idx) if up => idx.saturating_sub(1),
            None if down => 0,
            None if up => len.saturating_sub(1),
            _ => 0,
        };
        if let Some(node) = actions.visible_nodes.get(next_idx).cloned() {
            select_node(app, &mut sel_folder, &mut sel_item, &node, actions);
        }
    } else if let Some(idx) = current_idx
        && let Some(cur) = actions.visible_nodes.get(idx).cloned()
    {
        handle_selected_node_key(app, ui, idx, cur, (left, right, enter, fav_key_pressed), filter, &mut sel_folder, &mut sel_item, actions);
    }

    ui.ctx().data_mut(|d| {
        d.insert_temp(sel_folder_id, sel_folder);
        d.insert_temp(sel_item_id, sel_item);
    });
}

#[allow(clippy::too_many_arguments)]
fn handle_selected_node_key(
    app: &mut FilmcraftApp,
    ui: &egui::Ui,
    idx: usize,
    cur: VisibleNode,
    (left, right, enter, fav_key_pressed): (bool, bool, bool, bool),
    filter: &str,
    sel_folder: &mut Option<String>,
    sel_item: &mut Option<String>,
    actions: &mut EffectRowActions,
) {
    let jump_parent = |app: &mut FilmcraftApp, parent: Option<String>, sel_folder: &mut Option<String>, sel_item: &mut Option<String>, actions: &mut EffectRowActions| {
        if let Some(p) = parent
            && let Some(VisibleNode::Folder { key, rect, .. }) = actions.visible_nodes.iter().find(|n| matches!(n, VisibleNode::Folder { key: k, .. } if k == &p)).cloned()
        {
            app.ui.selected_effect = None;
            *sel_folder = Some(key);
            *sel_item = None;
            actions.scroll_to_rect = Some(rect);
        }
    };

    match cur {
        VisibleNode::Folder { key, open, parent, .. } => {
            if right && !open || left && open || enter {
                toggle_folder_open(app, ui, &key, filter, open);
            } else if right
                && let Some(next) = actions.visible_nodes.get(idx + 1).cloned()
            {
                match next {
                    VisibleNode::Effect { id, rect, .. } => {
                        app.ui.selected_effect = Some(id.to_string());
                        *sel_folder = None;
                        *sel_item = None;
                        actions.scroll_to_rect = Some(rect);
                    }
                    VisibleNode::Folder { key: k, rect, .. } => {
                        app.ui.selected_effect = None;
                        *sel_folder = Some(k);
                        *sel_item = None;
                        actions.scroll_to_rect = Some(rect);
                    }
                    VisibleNode::Preset { name, rect, .. } => {
                        app.ui.selected_effect = None;
                        *sel_folder = None;
                        *sel_item = Some(format!("preset:{name}"));
                        actions.scroll_to_rect = Some(rect);
                    }
                    VisibleNode::LumetriPreset { name, rect, .. } => {
                        app.ui.selected_effect = None;
                        *sel_folder = None;
                        *sel_item = Some(format!("lumetri:{name}"));
                        actions.scroll_to_rect = Some(rect);
                    }
                }
            } else if left {
                jump_parent(app, parent, sel_folder, sel_item, actions);
            }
        }
        VisibleNode::Effect { id, parent, .. } => {
            if left {
                jump_parent(app, parent, sel_folder, sel_item, actions);
            } else if enter {
                actions.apply = Some(id.to_string());
                actions.record_recent = Some(id.to_string());
            } else if fav_key_pressed {
                actions.toggle_fav = Some(id.to_string());
            }
        }
        VisibleNode::Preset { name, parent, .. } => {
            if left {
                jump_parent(app, parent, sel_folder, sel_item, actions);
            } else if enter {
                actions.apply_preset = Some(name);
            } else if fav_key_pressed {
                actions.toggle_fav = Some(crate::panels::presets::preset_fav_key(&name));
            }
        }
        VisibleNode::LumetriPreset { name, parent, .. } => {
            if left {
                jump_parent(app, parent, sel_folder, sel_item, actions);
            } else if enter {
                actions.apply_lumetri = Some(name);
            } else if fav_key_pressed {
                actions.toggle_fav = Some(format!("lumetri:{name}"));
            }
        }
    }
}

fn build_catalog_groups(defs: &'static [filmcraft_project::EffectDef], tops: &[&'static str], key: &CatalogCacheKey) -> Vec<CachedTopGroup> {
    let mut groups = Vec::new();
    for &top in tops {
        if !kind_matches_top(key.kind_filter, top) || top == "Presets" || top == filmcraft_project::vtransition::COLOR_PRESETS_FOLDER_KEY {
            continue;
        }
        let mut indices: Vec<usize> = defs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.category.first() == Some(&top))
            .filter(|(_, d)| {
                kind_matches_effect(key.kind_filter, d.kind)
                    && (!key.filter_gpu || d.accelerated)
                    && (!key.filter_32 || d.float32)
                    && (!key.filter_fav || key.favorites.iter().any(|f| f == d.id))
                    && matches_effect(d, &key.filter)
            })
            .map(|(i, _)| i)
            .collect();
        if top == "Video Effects" || top == "Legacy" || top == filmcraft_project::EFFECTCRAFT_TOP_FOLDER || !key.filter.is_empty() {
            indices.sort_by_key(|&i| {
                let d = &defs[i];
                let rank = if key.filter.is_empty() { 0 } else { effect_match_rank(d, &key.filter, &key.favorites) };
                (
                    d.category.get(1) == Some(&"Obsolete"),
                    d.category.get(1).copied().unwrap_or(""),
                    rank,
                    !key.filter.is_empty() && !d.accelerated,
                    d.name.to_ascii_lowercase(),
                )
            });
        }
        let sub_of = |i: usize| defs[i].category.get(1).copied().unwrap_or("");
        let mut sub_map: HashMap<&'static str, Vec<usize>> = HashMap::new();
        let mut sub_order: Vec<&'static str> = Vec::new();
        for &idx in &indices {
            let s = sub_of(idx);
            if !sub_map.contains_key(s) {
                sub_order.push(s);
            }
            sub_map.entry(s).or_default().push(idx);
        }
        sub_order.sort_by_key(|s| s.is_empty());
        let subs = sub_order.into_iter().filter_map(|s| sub_map.remove(s).map(|v| (s, v))).collect();
        groups.push(CachedTopGroup { top, indices, subs });
    }
    groups
}

fn cached_catalog_groups(
    ctx: &egui::Context,
    defs: &'static [filmcraft_project::EffectDef],
    tops: &[&'static str],
    key: CatalogCacheKey,
) -> Vec<CachedTopGroup> {
    let cache_id = egui::Id::unique("effects-catalog-cache");
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<EffectsCatalogCache>(cache_id);
        if cache.key.as_ref() != Some(&key) {
            cache.groups = build_catalog_groups(defs, tops, &key);
            cache.key = Some(key);
        }
        cache.groups.clone()
    })
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let (search_focused, search_down, search_enter, body_top) = show_header(app, ui, rect);

    let defs = filmcraft_project::effect_defs();
    let body = Rect::from_min_max(pos2(rect.min.x, body_top), pos2(rect.max.x, (rect.max.y - FOOTER_H).max(body_top)));

    // Wide panel: split with a draggable divider when a Color Grading Presets grid folder or the Effect Inspector is active.
    let grid_active = app.ui.lumetri_grid_folder.is_some();
    let inspector_active = !grid_active && app.ui.effects_inspector_open;
    let split_active = body.width() > 480.0 && (grid_active || inspector_active);
    let (tree_rect, splitter_rect, right_area) = if split_active {
        let min_tree = 200.0;
        let max_tree = (body.width() - 220.0).max(min_tree);
        let tree_w = app.ui.effects_tree_width.clamp(min_tree, max_tree);
        let t_rect = Rect::from_min_max(body.min, pos2(body.min.x + tree_w, body.max.y));
        let s_rect = Rect::from_min_max(pos2(t_rect.max.x, body.min.y), pos2(t_rect.max.x + 6.0, body.max.y));
        let r_rect = Rect::from_min_max(pos2(s_rect.max.x, body.min.y), body.max);
        (t_rect, Some(s_rect), Some(r_rect))
    } else {
        (body, None, None)
    };

    if let Some(sr) = splitter_rect {
        let s_resp = ui.interact(sr, ui.scope_id().with("effects.presetGrid.splitter"), Sense::click_and_drag());
        if s_resp.hovered() || s_resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        let line_color = if s_resp.hovered() || s_resp.dragged() { app.tokens.accent } else { app.tokens.separator };
        ui.painter().line_segment([pos2(sr.center().x, sr.min.y), pos2(sr.center().x, sr.max.y)], egui::Stroke::new(1.0, line_color));
        app.auto.add("effects.presetGrid.splitter", sr, "Preset Grid Splitter");
        if s_resp.dragged() {
            let min_tree = 200.0;
            let max_tree = (body.width() - 220.0).max(min_tree);
            app.ui.effects_tree_width = (app.ui.effects_tree_width + s_resp.drag_delta().x).clamp(min_tree, max_tree);
        }
        if s_resp.double_clicked() {
            app.ui.effects_tree_width = 300.0;
        }
    }

    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(tree_rect).id_salt("fx-body"));
    bui.set_clip_rect(tree_rect);
    let q = app.ui.effects_search.clone();
    let filter = q.trim();
    sync_search_collapse_state(ui.ctx(), filter);
    let filter_gpu = app.ui.effects_filter_gpu;
    let filter_32 = app.ui.effects_filter_32bit;
    let filter_fav = app.ui.effects_filter_favorites;
    let kind_filter = app.ui.effects_kind_filter;
    let active_filter = !filter.is_empty() || filter_gpu || filter_32 || filter_fav || kind_filter != EffectsKindFilter::All;

    let tops = filmcraft_project::vtransition::EFFECT_TOP_FOLDERS;
    let cache_key = CatalogCacheKey {
        filter: filter.to_string(),
        filter_gpu,
        filter_32,
        filter_fav,
        kind_filter,
        lang: app.ui.language,
        favorites: app.ui.favorite_effects.clone(),
    };
    let groups = cached_catalog_groups(ui.ctx(), defs, tops, cache_key);

    let mut actions = EffectRowActions::default();
    let mut lumetri_apply: Option<String> = None;
    let mut preset_action: Option<(String, String)> = None;
    let mut visible_rows: usize = 0;

    egui::ScrollArea::vertical().id_salt("fx-scroll").auto_shrink([false, false]).show(&mut bui, |ui| {
        let effective_filter = if active_filter && filter.is_empty() { "*" } else { filter };
        if !app.ui.favorite_effects.is_empty() && !filter_fav && kind_filter != EffectsKindFilter::Presets {
            visible_rows += show_favorite_effects(app, ui, defs, filter, effective_filter, filter_gpu, filter_32, kind_filter, &mut actions);
        }
        if !app.ui.recent_effects.is_empty() && !filter_fav && kind_filter != EffectsKindFilter::Presets {
            visible_rows += show_recent_effects(app, ui, defs, filter, effective_filter, filter_gpu, filter_32, kind_filter, &mut actions);
        }
        for &top in tops {
            if !kind_matches_top(kind_filter, top) {
                continue;
            }
            if top == "Presets" {
                if filter_gpu || filter_32 {
                    continue;
                }
                let presets_all = app.session.presets.all();
                let matching_presets = presets_all
                    .iter()
                    .filter(|p| {
                        (!filter_fav || app.ui.favorite_effects.iter().any(|f| f == &crate::panels::presets::preset_fav_key(&p.name)))
                            && matches_tokens(&p.name, filter)
                    })
                    .count();
                if matching_presets > 0 {
                    visible_rows += 1;
                }
                if matching_presets > 0 && presets_folder_row(app, ui, top, 0, filter, Some(matching_presets), &mut preset_action, &mut actions.visible_nodes) {
                    // Scan the presets directory once per session when first opened; users can
                    // still trigger a manual re-scan from the action row or context menu.
                    let scanned_id = egui::Id::unique("effects-presets-scanned-once");
                    let already_scanned: bool = ui.ctx().data(|d| d.get_temp(scanned_id)).unwrap_or(false);
                    if !already_scanned {
                        ui.ctx().data_mut(|d| d.insert_temp(scanned_id, true));
                        match app.session.execute("presets.scan", json!({})) {
                            Ok(v) => {
                                let imported = v["imported"].as_u64().unwrap_or(0);
                                let removed = v["removed"].as_u64().unwrap_or(0);
                                if imported > 0 || removed > 0 {
                                    app.ui.status =
                                        tlf!("Preset library updated: {imported} loaded, {removed} removed", imported = imported, removed = removed);
                                } else if let Some(e) = v["errors"].as_array().and_then(|a| a.first()).and_then(|v| v.as_str()) {
                                    app.ui.status = e.to_string();
                                }
                            }
                            Err(e) => app.ui.status = e.to_string(),
                        }
                    }
                    let (pa, ph, pfav) = crate::panels::presets::folder_rows(app, ui, filter, &mut actions.visible_nodes);
                    preset_action = pa.or(preset_action.take());
                    if let Some(hp) = ph {
                        actions.hovered = Some(HoveredItem::Preset(hp));
                    }
                    if let Some(fk) = pfav {
                        actions.toggle_fav = Some(fk);
                    }
                }
                continue;
            }
            if top == filmcraft_project::vtransition::COLOR_PRESETS_FOLDER_KEY {
                if filter_gpu || filter_32 {
                    continue;
                }
                let (applied, count) = lumetri_rows(app, ui, filter, filter_fav, &mut actions);
                visible_rows += count;
                if let Some(name) = applied {
                    lumetri_apply = Some(name);
                }
                continue;
            }

            let Some(group) = groups.iter().find(|g| g.top == top) else {
                continue;
            };
            if active_filter && group.indices.is_empty() {
                continue;
            }
            visible_rows += group.indices.len().max(1);
            let open = folder_row(app, ui, top, 0, effective_filter, Some(group.indices.len()), None, &mut actions);
            if !open {
                continue;
            }
            for (s, sub_indices) in &group.subs {
                let key = format!("{top}/{s}");
                if !s.is_empty() && !folder_row(app, ui, &key, 1, effective_filter, Some(sub_indices.len()), Some(top), &mut actions) {
                    continue;
                }
                for &idx in sub_indices {
                    if let Some(d) = defs.get(idx) {
                        let depth = if s.is_empty() { 1 } else { 2 };
                        let parent = if s.is_empty() { top } else { key.as_str() };
                        let show_ec = !filter.is_empty() && d.id.starts_with("ec.");
                        show_effect_row(app, ui, d, depth, Some(parent), "effects.item", show_ec, filter, &mut actions);
                    }
                }
            }
        }
    });

    handle_keyboard_nav(app, ui, (search_focused, search_down, search_enter), filter, &mut actions);
    if let Some(target_r) = actions.scroll_to_rect {
        bui.scroll_to_rect(target_r, None);
    }

    if visible_rows == 0 && active_filter {
        let presets_hidden_by_tech = kind_filter == EffectsKindFilter::Presets && (filter_gpu || filter_32);
        show_empty_state(app, ui, tree_rect, presets_hidden_by_tech);
    }

    if let Some(area) = right_area {
        if grid_active {
            if let Some(name) = preset_grid(app, ui, area, filter, &mut actions) {
                lumetri_apply = Some(name);
            }
        } else if inspector_active {
            show_effect_inspector(app, ui, area, defs, &mut actions);
        }
    }

    let selected_item = app.ui.selected_effect.as_deref().and_then(|id| defs.iter().find(|d| d.id == id)).map(HoveredItem::Effect);
    let info_item = actions.hovered.as_ref().or(selected_item.as_ref());
    let footer = Rect::from_min_max(pos2(rect.min.x, body.max.y), rect.max);
    show_info_footer(app, ui, footer, info_item, visible_rows);

    if let Some(name) = lumetri_apply.or(actions.apply_lumetri)
        && let Err(e) = app.session.execute("lumetri.applyPreset", json!({"name": name}))
    {
        app.ui.status = e.to_string();
    }
    if let Some(name) = actions.apply_preset {
        preset_action = Some(("presets.apply".to_string(), name));
    }
    if let Some((cmd, name)) = preset_action {
        crate::panels::presets::run(app, ui.ctx(), &cmd, &name);
    }
    if let Some(id) = actions.set_default
        && let Err(e) = app.session.execute("effects.setDefaultTransition", json!({"effect": id}))
    {
        app.ui.status = e.to_string();
    }
    if actions.clear_recent {
        app.ui.recent_effects.clear();
    }
    if let Some(id) = actions.toggle_fav {
        toggle_favorite_effect(&mut app.ui.favorite_effects, &id);
    }
    if let Some(id) = actions.record_recent {
        record_recent_effect(&mut app.ui.recent_effects, &id);
    }
    if let Some(id) = actions.apply {
        let r = app.session.execute("effects.apply", json!({"effect": id}));
        if let Err(e) = r {
            app.ui.status = e.to_string();
        }
    }
}

/// Where a tree row at `depth` starts (its chevron): folders at 0 (top) and 1 (sub-folders), and
/// what a folder holds one level deeper than the folder (#2329), so an effect is never level with,
/// or left of, the folder it is in. A row's icon is centred 18 points in, its label 30.
pub(crate) fn tree_x(depth: usize) -> f32 {
    8.0 + depth as f32 * 16.0
}

fn sync_search_collapse_state(ctx: &egui::Context, filter: &str) {
    let last_id = egui::Id::unique("effects-last-search");
    let collapsed_id = egui::Id::unique("effects-search-collapsed");
    ctx.data_mut(|d| {
        let prev: String = d.get_temp(last_id).unwrap_or_default();
        if prev != filter {
            d.insert_temp(last_id, filter.to_string());
            d.insert_temp(collapsed_id, Vec::<String>::new());
        }
    });
}

pub(crate) fn is_folder_open(app: &FilmcraftApp, ui: &egui::Ui, key: &str, filter: &str) -> bool {
    if !filter.is_empty() {
        let collapsed_id = egui::Id::unique("effects-search-collapsed");
        let collapsed: Vec<String> = ui.ctx().data(|d| d.get_temp(collapsed_id)).unwrap_or_default();
        !collapsed.iter().any(|saved| folder_key_matches(saved, key))
    } else {
        app.ui.expanded_fx.iter().any(|saved| folder_key_matches(saved, key))
    }
}

pub(crate) fn toggle_folder_open(app: &mut FilmcraftApp, ui: &egui::Ui, key: &str, filter: &str, currently_open: bool) {
    if !filter.is_empty() {
        let collapsed_id = egui::Id::unique("effects-search-collapsed");
        ui.ctx().data_mut(|d| {
            let collapsed = d.get_temp_mut_or_default::<Vec<String>>(collapsed_id);
            if currently_open {
                if !collapsed.iter().any(|saved| folder_key_matches(saved, key)) {
                    collapsed.push(key.to_string());
                }
            } else {
                collapsed.retain(|saved| !folder_key_matches(saved, key));
            }
        });
    } else if currently_open {
        app.ui.expanded_fx.retain(|saved| !folder_key_matches(saved, key));
    } else {
        app.ui.expanded_fx.push(key.to_string());
    }
}

#[allow(clippy::too_many_arguments)]
fn presets_folder_row(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    key: &str,
    depth: usize,
    filter: &str,
    count: Option<usize>,
    action: &mut Option<(String, String)>,
    visible_nodes: &mut Vec<VisibleNode>,
) -> bool {
    let t = app.tokens;
    let open = is_folder_open(app, ui, key, filter);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click());
    visible_nodes.push(VisibleNode::Folder { key: key.to_string(), open, parent: None, rect: r });
    if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, t.hover);
    }
    let x = r.min.x + tree_x(depth);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(x + 4.0, r.center().y), vec2(10.0, 10.0)),
        if open { Icon::ChevronDown } else { Icon::ChevronRight },
        t.text_dim,
    );
    icons::paint(ui.painter(), Rect::from_center_size(pos2(x + 18.0, r.center().y), vec2(13.0, 13.0)), Icon::Folder, t.text_dim);
    let name = if depth == 0 { key } else { key.split_once('/').map_or(key, |(_, folder)| folder) };
    let translated = crate::i18n::t(name);
    let g = ui.painter().layout_no_wrap(translated.to_string(), Tokens::ui(12.0), t.text);
    let text_w = g.size().x;
    ui.painter().galley(pos2(x + 30.0, r.center().y - g.size().y * 0.5), g, t.text);
    if let Some(n) = count {
        ui.painter().text(pos2(x + 30.0 + text_w + 6.0, r.center().y), Align2::LEFT_CENTER, format!("({n})"), Tokens::ui(10.5), t.text_dim);
    }
    let name = if depth == 0 { key } else { key.split_once('/').map_or(key, |(_, folder)| folder) };
    app.auto.add(&format!("effects.folder.{key}"), r, name);

    resp.context_menu(|ui| {
        if ui.button(tl!("Import Presets…")).clicked() {
            *action = Some(("presets.import".to_string(), String::new()));
            ui.close();
        }
        if ui.button(tl!("Open Presets Folder")).clicked() {
            *action = Some(("presets.revealFolder".to_string(), String::new()));
            ui.close();
        }
        if ui.button(tl!("Scan Presets Folder")).clicked() {
            *action = Some(("presets.scan".to_string(), String::new()));
            ui.close();
        }
    });

    if resp.clicked() {
        toggle_folder_open(app, ui, key, filter, open);
        return is_folder_open(app, ui, key, filter);
    }
    open
}

pub(crate) fn matches_tokens(text: &str, filter: &str) -> bool {
    let trimmed = filter.trim();
    if trimmed.is_empty() {
        return true;
    }
    trimmed.split_whitespace().all(|tok| crate::i18n::matches_query(text, tok))
}

/// Whether the Effects panel's search shows `d` for `filter`: every whitespace-separated token must
/// match its label in the interface language, a legacy/alias name (`"Lumetri"`, `"CC *"`), its
/// technical id (`ec.*`), or its sub-category.
fn matches_effect(d: &filmcraft_project::EffectDef, filter: &str) -> bool {
    let trimmed = filter.trim();
    if trimmed.is_empty() {
        return true;
    }
    trimmed.split_whitespace().all(|tok| {
        crate::i18n::matches_query(d.name, tok)
            || filmcraft_project::search_aliases(d).iter().any(|alias| crate::i18n::matches_query(alias, tok))
            || crate::i18n::matches_query(d.id, tok)
            || d.category.get(1).is_some_and(|cat| crate::i18n::matches_query(cat, tok))
    })
}

fn effect_match_rank(d: &filmcraft_project::EffectDef, filter: &str, favorites: &[String]) -> u8 {
    let q = filter.trim().to_ascii_lowercase();
    let name_l = crate::i18n::t(d.name).to_ascii_lowercase();
    let is_fav = favorites.iter().any(|f| f == d.id);
    let base = if name_l == q {
        0
    } else if name_l.starts_with(&q) {
        2
    } else if name_l.contains(&q) {
        4
    } else if filmcraft_project::search_aliases(d).iter().any(|a| crate::i18n::matches_query(a, &q)) {
        6
    } else {
        8
    };
    if is_fav { base } else { base + 1 }
}

fn folder_key_matches(saved: &str, key: &str) -> bool {
    if saved == key {
        return true;
    }
    let Some(tail) = key.strip_prefix(filmcraft_project::vtransition::COLOR_PRESETS_FOLDER_KEY) else { return false };
    (tail.is_empty() || tail.starts_with('/')) && ["Color Grade Presets", "Color Grading Presets"].iter().any(|alias| saved.strip_prefix(alias) == Some(tail))
}

#[allow(clippy::too_many_arguments)]
fn folder_row(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    key: &str,
    depth: usize,
    filter: &str,
    count: Option<usize>,
    parent: Option<&str>,
    actions: &mut EffectRowActions,
) -> bool {
    folder_row_with_menu(app, ui, key, depth, filter, count, parent, actions, |_, _| {})
}

#[allow(clippy::too_many_arguments)]
fn folder_row_with_menu(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    key: &str,
    depth: usize,
    filter: &str,
    count: Option<usize>,
    parent: Option<&str>,
    actions: &mut EffectRowActions,
    menu: impl FnOnce(&mut FilmcraftApp, &mut egui::Ui),
) -> bool {
    let t = app.tokens;
    let open = is_folder_open(app, ui, key, filter);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click());
    actions.visible_nodes.push(VisibleNode::Folder { key: key.to_string(), open, parent: parent.map(str::to_string), rect: r });

    let sel_folder_id = egui::Id::unique("effects-selected-folder");
    let is_sel = app.ui.selected_effect.is_none() && ui.ctx().data(|d| d.get_temp::<Option<String>>(sel_folder_id)).flatten().as_deref() == Some(key);
    let name = if depth == 0 { filmcraft_project::vtransition::effect_folder_label(key) } else { key.split_once('/').map_or(key, |(_, folder)| folder) };
    if is_sel {
        ui.painter().rect_filled(r, 0.0, t.row_selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, t.hover);
    }
    let x = r.min.x + tree_x(depth);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(x + 4.0, r.center().y), vec2(10.0, 10.0)),
        if open { Icon::ChevronDown } else { Icon::ChevronRight },
        t.text_dim,
    );
    icons::paint(ui.painter(), Rect::from_center_size(pos2(x + 18.0, r.center().y), vec2(13.0, 13.0)), Icon::Folder, t.text_dim);
    // a sub-folder key is `<top>/<folder>`; folder names may themselves contain a slash
    // (Audio Effects ▸ Noise Reduction/Restoration), so split once, after the top folder
    let translated = crate::i18n::t(name);
    let fg = if is_sel { Color32::WHITE } else { t.text };
    let g = ui.painter().layout_no_wrap(translated.to_string(), Tokens::ui(12.0), fg);
    let text_w = g.size().x;
    ui.painter().galley(pos2(x + 30.0, r.center().y - g.size().y * 0.5), g, fg);
    if let Some(n) = count {
        ui.painter().text(pos2(x + 30.0 + text_w + 6.0, r.center().y), Align2::LEFT_CENTER, format!("({n})"), Tokens::ui(10.5), t.text_dim);
    }
    app.auto.add(&format!("effects.folder.{key}"), r, name);
    if let Some(tail) = key.strip_prefix(filmcraft_project::vtransition::COLOR_PRESETS_FOLDER_KEY) {
        for alias in ["Color Grade Presets", "Color Grading Presets"] {
            app.auto.add(&format!("effects.folder.{alias}{tail}"), r, name);
        }
    }
    resp.context_menu(|ui| menu(app, ui));
    if resp.clicked() {
        app.ui.selected_effect = None;
        ui.ctx().data_mut(|d| d.insert_temp(sel_folder_id, Some(key.to_string())));
        toggle_folder_open(app, ui, key, filter, open);
    }
    open
}

/// Right-click menu of a Color Grading preset: Apply to Selected Clips (automation id
/// `effects.presetMenu.apply`). True when chosen.
fn apply_menu(app: &mut FilmcraftApp, resp: &egui::Response) -> bool {
    let mut chosen = false;
    resp.context_menu(|ui| {
        let b = ui.button(tl!("Apply to Selected Clips"));
        app.auto.add("effects.presetMenu.apply", b.rect, "Apply to Selected Clips");
        if b.clicked() {
            chosen = true;
            ui.close();
        }
    });
    chosen
}

/// A preset's thumbnail texture (rendered once per size, cached by name).
fn preset_texture(app: &mut FilmcraftApp, ctx: &egui::Context, p: &filmcraft_render::lumetri_presets::LumetriPreset, w: usize) -> egui::TextureId {
    let name = format!("lumetri-preset-{}-{w}", p.name);
    if let Some((id, _)) = app.texture_existing(&name) {
        return id;
    }
    let h = (w * 9 / 16).max(1);
    let img = filmcraft_render::lumetri_presets::thumbnail(p, w, h);
    let rgba = crate::frames::Rgba { w: img.w, h: img.h, px: img.over_black_rgba8() };
    let key = crate::frames::FrameKey {
        target: crate::frames::Target::Item(filmcraft_project::ItemId(u64::MAX)),
        frame: 0,
        size: w as u32,
        revision: 0,
        draft: false,
        bypass: false,
    };
    app.texture_for(ctx, &name, key, &rgba)
}

/// Ungraded procedural preview thumbnail for Before/After hover wipe comparison.
fn neutral_preview_texture(app: &mut FilmcraftApp, ctx: &egui::Context, w: usize) -> egui::TextureId {
    let name = format!("lumetri-preview-neutral-{w}");
    if let Some((id, _)) = app.texture_existing(&name) {
        return id;
    }
    let h = (w * 9 / 16).max(1);
    let img = filmcraft_render::lumetri_presets::preview_image(w, h);
    let rgba = crate::frames::Rgba { w: img.w, h: img.h, px: img.over_black_rgba8() };
    let key = crate::frames::FrameKey {
        target: crate::frames::Target::Item(filmcraft_project::ItemId(u64::MAX)),
        frame: 1,
        size: w as u32,
        revision: 0,
        draft: false,
        bypass: false,
    };
    app.texture_for(ctx, &name, key, &rgba)
}

/// The Color Grading Presets folder of the tree: sub-folders and preset rows with thumbnails. Returns
/// `(preset_to_apply, visible_count)`.
fn lumetri_rows(app: &mut FilmcraftApp, ui: &mut egui::Ui, filter: &str, filter_fav: bool, actions: &mut EffectRowActions) -> (Option<String>, usize) {
    use filmcraft_render::lumetri_presets as lp;
    let t = app.tokens;
    let top = filmcraft_project::vtransition::COLOR_PRESETS_FOLDER_KEY;
    let presets: Vec<lp::LumetriPreset> = lp::presets()
        .into_iter()
        .filter(|p| (!filter_fav || app.ui.favorite_effects.iter().any(|f| f == &format!("lumetri:{}", p.name))) && matches_tokens(p.name, filter))
        .collect();
    if (!filter.is_empty() || filter_fav) && presets.is_empty() {
        return (None, 0);
    }
    let top_open = folder_row(app, ui, top, 0, filter, Some(presets.len()), None, actions);
    if !top_open {
        return (None, presets.len().max(1));
    }
    let mut apply = None;
    for folder in lp::FOLDERS {
        let items: Vec<&lp::LumetriPreset> = presets.iter().filter(|p| p.folder == folder).collect();
        if items.is_empty() {
            continue;
        }
        let key = format!("{top}/{folder}");
        let was_open = is_folder_open(app, ui, &key, filter);
        let open = folder_row(app, ui, &key, 1, filter, Some(items.len()), Some(top), actions);
        // the folder clicked last is the one the wide panel shows as a grid (`folder_row` toggles
        // the expanded state on a click and returns the state it drew)
        let now = is_folder_open(app, ui, &key, filter);
        if now != was_open {
            app.ui.lumetri_grid_folder = Some(folder.to_string());
        }
        if !open {
            continue;
        }
        for p in items {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click_and_drag());
            actions.visible_nodes.push(VisibleNode::LumetriPreset { name: p.name.to_string(), parent: Some(key.clone()), rect: r });
            let visible = ui.is_rect_visible(r);
            let row_hovered = visible && (resp.hovered() || ui.rect_contains_pointer(r));
            if visible && row_hovered {
                ui.painter().rect_filled(r, 0.0, t.hover);
            }
            if row_hovered {
                actions.hovered = Some(HoveredItem::Preset(HoveredPreset { name: p.name.to_string(), description: p.description.to_string(), builtin: true }));
            }
            let fav_key = format!("lumetri:{}", p.name);
            let is_fav = app.ui.favorite_effects.iter().any(|f| f == &fav_key);
            let mut right_x = r.max.x - 8.0;
            if visible && (is_fav || row_hovered) {
                let star_r = Rect::from_min_size(pos2(right_x - 16.0, r.min.y + 4.0), vec2(16.0, 16.0));
                let star_resp = ui.interact(star_r, ui.scope_id().with(("lumetri-fav", p.name)), Sense::click());
                let star_fg = if is_fav {
                    t.accent
                } else if star_resp.hovered() {
                    t.text
                } else {
                    t.text_dim
                };
                icons::paint(ui.painter(), star_r.shrink(2.5), Icon::Star, star_fg);
                app.auto.add(&format!("effects.lumetriPreset.fav.{}", p.name), star_r, p.name);
                if star_resp.clicked() {
                    actions.toggle_fav = Some(fav_key);
                }
                right_x -= 18.0;
            }
            let tr = Rect::from_min_size(pos2(r.min.x + tree_x(2) + 12.0, r.min.y + 3.0), vec2(32.0, 18.0));
            if visible {
                let tex = preset_texture(app, ui.ctx(), p, 64);
                // inside a Color Grading Presets sub-folder: one level deeper than it
                ui.painter().image(tex, tr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            let label_clip = Rect::from_min_max(pos2(tr.max.x + 8.0, r.min.y), pos2((right_x - 4.0).max(tr.max.x + 8.0), r.max.y));
            ui.painter().with_clip_rect(label_clip).text(
                pos2(tr.max.x + 8.0, r.center().y),
                Align2::LEFT_CENTER,
                crate::i18n::t(p.name),
                Tokens::ui(12.0),
                t.text,
            );
            app.auto.add(&format!("effects.lumetriPreset.{}", p.name), r, p.name);
            let resp = resp.on_hover_text(crate::i18n::t(p.description));
            if resp.drag_started_by(egui::PointerButton::Primary) {
                crate::panels::start_drag_lumetri_preset(ui, p.name);
            }
            if apply_menu(app, &resp) {
                apply = Some(p.name.to_string());
            }
            if resp.double_clicked() {
                apply = Some(p.name.to_string());
            }
        }
    }
    (apply, presets.len().max(1))
}

/// The wide Effects panel's scrollable thumbnail grid of the selected Color Grading Presets folder.
/// Returns the preset to apply (double-click or context menu).
fn preset_grid(app: &mut FilmcraftApp, ui: &mut egui::Ui, area: Rect, filter: &str, actions: &mut EffectRowActions) -> Option<String> {
    use filmcraft_render::lumetri_presets as lp;
    let t = app.tokens;
    let folder = app.ui.lumetri_grid_folder.clone()?;
    app.auto.add("effects.presetGrid", area, &folder);

    let presets: Vec<lp::LumetriPreset> = lp::presets().into_iter().filter(|p| p.folder == folder && crate::i18n::matches_query(p.name, filter)).collect();

    let header_r = Rect::from_min_size(area.min, vec2(area.width(), 22.0));
    ui.painter().rect_filled(header_r, 0.0, t.header_bg);
    ui.painter().text(
        pos2(header_r.min.x + 8.0, header_r.center().y),
        Align2::LEFT_CENTER,
        format!("{} ({})", crate::i18n::t(&folder), presets.len()),
        Tokens::ui(11.5),
        t.text_dim,
    );

    // Close grid button (×) and Thumbnail scale buttons (S / M / L)
    let mut hx = header_r.max.x - 6.0;
    let close_r = Rect::from_min_size(pos2(hx - 18.0, header_r.min.y + 2.0), vec2(18.0, 18.0));
    let close_resp = ui.interact(close_r, ui.scope_id().with("effects.presetGrid.close"), Sense::click()).on_hover_text(tl!("Close Grid"));
    ui.painter().rect_filled(close_r, 3.0, if close_resp.hovered() { t.hover } else { Color32::TRANSPARENT });
    ui.painter().text(close_r.center(), Align2::CENTER_CENTER, "×", Tokens::ui(11.0), t.text_dim);
    app.auto.add("effects.presetGrid.close", close_r, "Close Grid");
    if close_resp.clicked() {
        app.ui.lumetri_grid_folder = None;
    }
    hx -= 22.0;

    for (scale, id, label) in [(2u8, "effects.presetGrid.size.l", "L"), (1u8, "effects.presetGrid.size.m", "M"), (0u8, "effects.presetGrid.size.s", "S")] {
        let sr = Rect::from_min_size(pos2(hx - 18.0, header_r.min.y + 2.0), vec2(18.0, 18.0));
        let resp = ui.interact(sr, ui.scope_id().with(id), Sense::click());
        let active = app.ui.effects_preset_thumb_scale == scale;
        let bg = if active {
            t.pill_active_bg
        } else if resp.hovered() {
            t.hover
        } else {
            t.field_border
        };
        let fg = if active { Color32::WHITE } else { t.text_dim };
        ui.painter().rect_filled(sr, 2.0, bg);
        ui.painter().text(sr.center(), Align2::CENTER_CENTER, label, Tokens::ui(9.5), fg);
        app.auto.add(id, sr, label);
        if resp.clicked() {
            app.ui.effects_preset_thumb_scale = scale;
        }
        hx -= 20.0;
    }

    ui.painter().line_segment([header_r.left_bottom(), header_r.right_bottom()], egui::Stroke::new(1.0, t.separator));

    let grid_body = Rect::from_min_max(pos2(area.min.x, header_r.max.y), area.max);
    let mut gui = ui.new_child(egui::UiBuilder::new().max_rect(grid_body).id_salt("fx-preset-grid"));
    gui.set_clip_rect(grid_body);

    let (cw, ch, tex_w) = match app.ui.effects_preset_thumb_scale {
        0 => (112.0f32, 63.0f32, 128usize),
        2 => (208.0f32, 117.0f32, 224usize),
        _ => (150.0f32, 84.0f32, 160usize),
    };
    let (gap, label_h) = (6.0f32, 18.0f32);
    let cols = (((grid_body.width() - gap) / (cw + gap)).floor() as usize).max(1);
    let rows = presets.len().div_ceil(cols).max(1);
    let total_h = gap + rows as f32 * (ch + label_h + gap);

    let neutral_tex = neutral_preview_texture(app, ui.ctx(), tex_w);
    let mut apply = None;
    egui::ScrollArea::vertical().id_salt("fx-preset-grid-scroll").auto_shrink([false, false]).show(&mut gui, |ui| {
        let (canvas, _) = ui.allocate_exact_size(vec2(grid_body.width().max(1.0), total_h), Sense::hover());
        for (k, p) in presets.iter().enumerate() {
            let x = canvas.min.x + gap + (k % cols) as f32 * (cw + gap);
            let y = canvas.min.y + gap + (k / cols) as f32 * (ch + label_h + gap);
            let cell = Rect::from_min_size(pos2(x, y), vec2(cw, ch + label_h));
            let pic = Rect::from_min_size(cell.min, vec2(cw, ch)).shrink(2.0);
            let tex = preset_texture(app, ui.ctx(), p, tex_w);
            let resp = ui.interact(cell, egui::Id::unique(("lumetri-grid", p.name)), Sense::click_and_drag()).on_hover_text(crate::i18n::t(p.description));
            ui.painter().rect_filled(cell, 3.0, if resp.hovered() { t.hover } else { t.field_bg });

            // Interactive Before/After wipe when hovering over the thumbnail
            if resp.hovered() {
                actions.hovered = Some(HoveredItem::Preset(HoveredPreset { name: p.name.to_string(), description: p.description.to_string(), builtin: true }));
                if let Some(ptr) = ui.input(|i| i.pointer.hover_pos())
                    && pic.contains(ptr)
                {
                    let u = ((ptr.x - pic.min.x) / pic.width().max(1.0)).clamp(0.05, 0.95);
                    let split_x = pic.min.x + pic.width() * u;
                    let left_rect = Rect::from_min_max(pic.min, pos2(split_x, pic.max.y));
                    let right_rect = Rect::from_min_max(pos2(split_x, pic.min.y), pic.max);
                    ui.painter().image(neutral_tex, left_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(u, 1.0)), Color32::WHITE);
                    ui.painter().image(tex, right_rect, Rect::from_min_max(pos2(u, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    ui.painter().line_segment([pos2(split_x, pic.min.y), pos2(split_x, pic.max.y)], egui::Stroke::new(1.0, Color32::WHITE));
                } else {
                    ui.painter().image(tex, pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
            } else {
                ui.painter().image(tex, pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }

            let label = ui.painter().layout(crate::i18n::t(p.name).to_string(), Tokens::ui(11.0), t.text, cw - 6.0);
            ui.painter().galley(pos2(cell.min.x + 4.0, pic.max.y + 3.0), label, t.text);
            app.auto.add(&format!("effects.presetGrid.{}", p.name), cell, p.name);
            if resp.drag_started_by(egui::PointerButton::Primary) {
                crate::panels::start_drag_lumetri_preset(ui, p.name);
            }
            if apply_menu(app, &resp) {
                apply = Some(p.name.to_string());
            }
            if resp.double_clicked() {
                apply = Some(p.name.to_string());
            }
        }
    });
    apply
}

fn param_kind_label(kind: &filmcraft_project::ParamKind) -> &'static str {
    use filmcraft_project::ParamKind;
    match kind {
        ParamKind::Float { .. } => "Float",
        ParamKind::Point => "Point",
        ParamKind::Color => "Color",
        ParamKind::Bool => "Bool",
        ParamKind::Choice(_) => "Choice",
        ParamKind::Angle => "Angle",
        ParamKind::Text => "Text",
        ParamKind::Curve { .. } => "Curve",
        ParamKind::Wheel => "Wheel",
        ParamKind::Path => "Path",
    }
}

/// Right-hand Effect Inspector pane shown on wide panels when `effects_inspector_open` is active.
fn show_effect_inspector(app: &mut FilmcraftApp, ui: &mut egui::Ui, area: Rect, defs: &'static [filmcraft_project::EffectDef], actions: &mut EffectRowActions) {
    let t = app.tokens;
    app.auto.add("effects.inspector", area, "Effect Inspector");

    let header_r = Rect::from_min_size(area.min, vec2(area.width(), 22.0));
    ui.painter().rect_filled(header_r, 0.0, t.header_bg);
    ui.painter().text(pos2(header_r.min.x + 8.0, header_r.center().y), Align2::LEFT_CENTER, tl!("Effect Inspector"), Tokens::ui(11.5), t.text_dim);

    let close_r = Rect::from_min_size(pos2(header_r.max.x - 24.0, header_r.min.y + 2.0), vec2(18.0, 18.0));
    let close_resp = ui.interact(close_r, ui.scope_id().with("effects.inspector.close"), Sense::click()).on_hover_text(tl!("Close Inspector"));
    ui.painter().rect_filled(close_r, 3.0, if close_resp.hovered() { t.hover } else { Color32::TRANSPARENT });
    icons::paint(ui.painter(), close_r.shrink(3.5), Icon::Close, t.text_dim);
    app.auto.add("effects.inspector.close", close_r, "Close Inspector");
    if close_resp.clicked() {
        app.ui.effects_inspector_open = false;
    }
    ui.painter().line_segment([header_r.left_bottom(), header_r.right_bottom()], egui::Stroke::new(1.0, t.separator));

    let body_r = Rect::from_min_max(pos2(area.min.x, header_r.max.y), area.max).shrink2(vec2(10.0, 8.0));
    let Some(d) = app.ui.selected_effect.as_deref().and_then(|id| defs.iter().find(|d| d.id == id)) else {
        ui.painter().text(body_r.center(), Align2::CENTER_CENTER, tl!("Select an effect to inspect its parameters"), Tokens::ui(11.5), t.text_dim);
        return;
    };

    let mut iui = ui.new_child(egui::UiBuilder::new().max_rect(body_r).id_salt("fx-inspector-body"));
    iui.set_clip_rect(body_r);
    egui::ScrollArea::vertical().id_salt("fx-inspector-scroll").auto_shrink([false, false]).show(&mut iui, |ui| {
        ui.label(egui::RichText::new(crate::i18n::t(d.name)).size(14.0).strong().color(t.text));
        let cat = d.category.iter().map(|c| crate::i18n::t(c)).collect::<Vec<_>>().join(" ▸ ");
        ui.label(egui::RichText::new(cat).size(11.0).color(t.text_dim));
        ui.label(egui::RichText::new(d.id).size(10.0).monospace().color(t.text_dim));
        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            let mut badge = |label: &str, active: bool| {
                let bg = if active { t.accent } else { t.field_border };
                let fg = if active { Color32::WHITE } else { t.text_dim };
                egui::Frame::NONE.fill(bg).corner_radius(3.0).inner_margin(egui::Margin::symmetric(6, 2)).show(ui, |ui| {
                    ui.label(egui::RichText::new(label).size(10.0).color(fg));
                });
            };
            if d.accelerated {
                badge("GPU", false);
            }
            if d.float32 {
                badge("32-bit", false);
            }
            if d.yuv {
                badge("YUV", false);
            }
            if d.intrinsic {
                badge("Intrinsic", false);
            }
            if is_default_transition(app, d) {
                badge("Default", true);
            }
        });
        ui.add_space(8.0);

        ui.horizontal_wrapped(|ui| {
            let apply_btn = ui.button(tl!("Apply to Selected"));
            app.auto.add("effects.inspector.apply", apply_btn.rect, "Apply to Selected");
            if apply_btn.clicked() {
                actions.apply = Some(d.id.to_string());
                actions.record_recent = Some(d.id.to_string());
            }
            let is_fav = app.ui.favorite_effects.iter().any(|f| f == d.id);
            let fav_btn = ui.button(if is_fav { tl!("Remove from Favorites") } else { tl!("Add to Favorites") });
            app.auto.add("effects.inspector.fav", fav_btn.rect, "Favorite");
            if fav_btn.clicked() {
                actions.toggle_fav = Some(d.id.to_string());
            }
            if matches!(d.kind, EffectKind::VideoTransition | EffectKind::AudioTransition) {
                let def_btn = ui.button(tl!("Set as Default Transition"));
                if def_btn.clicked() {
                    actions.set_default = Some(d.id.to_string());
                }
            }
            if ui.button(tl!("Copy Effect ID")).clicked() {
                ui.ctx().copy_text(d.id.to_string());
            }
        });

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(egui::RichText::new(format!("{} ({})", tl!("Parameters"), d.params.len())).size(12.0).strong().color(t.text));
        ui.add_space(4.0);

        for p in &d.params {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(crate::i18n::t(p.label)).size(11.5).color(t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(param_kind_label(&p.kind)).size(10.0).color(t.text_dim));
                    if p.animatable {
                        let (sr, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                        icons::paint(ui.painter(), sr, Icon::Stopwatch, t.text_dim);
                    }
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_search_matches_turkish_casing() {
        crate::i18n::set_current(crate::i18n::Language::Tr);
        let ctx = egui::Context::default();
        let session = filmcraft_engine::Session::default();
        let mut app = FilmcraftApp::new(session);
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        for query in ["IŞIK", "ışık", "Işık"] {
            app.ui.effects_search = query.to_string();
            app.auto.begin_frame();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(&mut app, ui, rect);
            });
            out.textures_delta.clear();
            assert!(app.auto.find("effects.item.light_leaks").is_some(), "query `{query}` through effects search caller must find `light_leaks`",);
        }
        crate::i18n::set_current(crate::i18n::Language::En);
    }

    /// #1868: the search still finds Color Grading by the names it used to have, so someone who
    /// types "Lumetri" is not left with an empty panel.
    #[test]
    fn effects_search_finds_grading_by_its_old_names() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        let shows = |app: &mut FilmcraftApp, query: &str| {
            app.ui.effects_search = query.to_string();
            app.auto.begin_frame();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(app, ui, rect);
            });
            out.textures_delta.clear();
            app.auto.find("effects.item.lumetri").is_some()
        };
        for query in ["Lumetri", "lumetri color", "LUMETRI", "Color Grade", "Color Grading"] {
            assert!(shows(&mut app, query), "query `{query}` must find Color Grading");
        }
        for query in ["Gaussian", "Ultra", "Scopes"] {
            assert!(!shows(&mut app, query), "query `{query}` must not show Color Grading");
        }
    }

    #[test]
    fn effects_search_finds_by_id_alias_and_filters() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));

        // Search by technical ID and by CC alias.
        app.ui.effects_search = "ec.sim.ccparticleworld".to_string();
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.item.ec.sim.ccparticleworld").is_some());

        app.ui.effects_search = "CC Kaleida".to_string();
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.item.ec.stylize.cckaleida").is_some());

        // Quick-filter GPU only hides CPU-only effects like ec.sim.ccparticleworld.
        app.ui.effects_search = "ec.sim.ccparticleworld".to_string();
        app.ui.effects_filter_gpu = true;
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.item.ec.sim.ccparticleworld").is_none());

        app.ui.effects_search = "ec.blur.gaussian".to_string();
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.item.ec.blur.gaussian").is_some());
    }

    #[test]
    fn empty_state_appears_and_clear_filters_resets_all() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));

        app.ui.effects_search = "zzzz_nonexistent_effect_xyz".to_string();
        app.ui.effects_filter_gpu = true;
        app.ui.effects_kind_filter = EffectsKindFilter::Video;
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        let clear_el = app.auto.find("effects.action.clearFilters").expect("empty state Clear Filters button must be registered");
        let click_pos = pos2(clear_el.rect[0] + clear_el.rect[2] * 0.5, clear_el.rect[1] + clear_el.rect[3] * 0.5);
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::PointerMoved(click_pos));
        raw.events.push(egui::Event::PointerButton { pos: click_pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        let mut out = ctx.run_ui(raw, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        let mut raw_up = egui::RawInput::default();
        raw_up.events.push(egui::Event::PointerMoved(click_pos));
        raw_up.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut out = ctx.run_ui(raw_up, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        assert!(app.ui.effects_search.is_empty());
        assert!(!app.ui.effects_filter_gpu);
        assert_eq!(app.ui.effects_kind_filter, EffectsKindFilter::All);
    }

    #[test]
    fn wide_panel_only_splits_when_preset_grid_is_active() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        app.ui.lumetri_grid_folder = None;
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.presetGrid").is_none());
        let folder_r = app.auto.find("effects.folder.Video Transitions").expect("Video Transitions folder must be visible");
        assert!(folder_r.rect[2] > 600.0, "tree should use full width when preset grid is inactive");

        app.ui.lumetri_grid_folder = Some("Film Stocks".to_string());
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.presetGrid").is_some());
        let folder_r_split = app.auto.find("effects.folder.Video Transitions").expect("Video Transitions folder must be visible");
        assert!(folder_r_split.rect[2] <= 300.0, "tree should be 300px wide when preset grid is active");
    }

    #[test]
    fn kind_filter_pills_restrict_categories() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));

        app.ui.effects_kind_filter = EffectsKindFilter::Audio;
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.folder.Audio Effects").is_some());
        assert!(app.auto.find("effects.folder.Video Effects").is_none());
        assert!(app.auto.find("effects.folder.Video Transitions").is_none());
    }

    #[test]
    fn single_click_selects_effect() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        app.ui.effects_search = "ec.blur.gaussian".to_string();

        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        let item_r = app.auto.find("effects.item.ec.blur.gaussian").expect("gaussian blur item must be visible");
        let pos = pos2(item_r.rect[0] + item_r.rect[2] * 0.5, item_r.rect[1] + item_r.rect[3] * 0.5);

        let mut down = egui::RawInput::default();
        down.events.push(egui::Event::PointerMoved(pos));
        down.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        let mut out = ctx.run_ui(down, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        let mut up = egui::RawInput::default();
        up.events.push(egui::Event::PointerMoved(pos));
        up.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        let mut out = ctx.run_ui(up, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        assert_eq!(app.ui.selected_effect.as_deref(), Some("ec.blur.gaussian"));
    }

    #[test]
    fn info_footer_shows_on_hover_without_selection() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        app.ui.effects_search = "ec.blur.gaussian".to_string();
        app.ui.selected_effect = None;

        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        let item_r = app.auto.find("effects.item.ec.blur.gaussian").expect("gaussian blur item must be visible");
        let pos = pos2(item_r.rect[0] + item_r.rect[2] * 0.5, item_r.rect[1] + item_r.rect[3] * 0.5);

        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::PointerMoved(pos));
        app.auto.begin_frame();
        let mut out = ctx.run_ui(raw, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        assert!(app.auto.find("effects.infoFooter").is_some(), "info footer must always be rendered");
    }

    #[test]
    fn narrow_panel_wraps_kind_pills_without_clipping() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(220.0, 600.0));

        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        for id in ["effects.kind.all", "effects.kind.video", "effects.kind.audio", "effects.kind.transitions", "effects.kind.presets"] {
            assert!(app.auto.find(id).is_some(), "kind pill `{id}` must not be clipped at 220px width");
        }
    }

    #[test]
    fn inline_star_button_toggles_favorite_and_populates_favorites_folder() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        app.ui.effects_search = "ec.blur.gaussian".to_string();

        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        let item_r = app.auto.find("effects.item.ec.blur.gaussian").expect("gaussian blur item must be visible");
        let pos = pos2(item_r.rect[0] + item_r.rect[2] * 0.5, item_r.rect[1] + item_r.rect[3] * 0.5);

        let mut hover = egui::RawInput::default();
        hover.events.push(egui::Event::PointerMoved(pos));
        app.auto.begin_frame();
        let mut out = ctx.run_ui(hover, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        let fav_btn = app.auto.find("effects.item.fav.ec.blur.gaussian").expect("inline star button must appear on hover");
        let star_pos = pos2(fav_btn.rect[0] + fav_btn.rect[2] * 0.5, fav_btn.rect[1] + fav_btn.rect[3] * 0.5);

        let mut down = egui::RawInput::default();
        down.events.push(egui::Event::PointerMoved(star_pos));
        down.events.push(egui::Event::PointerButton { pos: star_pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(down, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        let mut up = egui::RawInput::default();
        up.events.push(egui::Event::PointerMoved(star_pos));
        up.events.push(egui::Event::PointerButton { pos: star_pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(up, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        assert!(app.ui.favorite_effects.contains(&"ec.blur.gaussian".to_string()));

        app.ui.effects_search.clear();
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.folder.Favorites").is_some(), "Favorites folder must appear once an effect is starred");
    }

    #[test]
    fn keyboard_arrows_expand_collapse_folders_and_select_items() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.ui.focused = PanelKind::Effects;
        app.ui.expanded_fx.clear();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));

        // Initial render
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        // ArrowDown selects the first folder (Video Transitions)
        let mut down = egui::RawInput::default();
        down.events.push(egui::Event::Key { key: Key::ArrowDown, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(down, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        // ArrowRight expands the selected folder
        let mut right = egui::RawInput::default();
        right.events.push(egui::Event::Key { key: Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(right, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(!app.ui.expanded_fx.is_empty(), "ArrowRight should expand the selected folder");

        let expanded_count = app.ui.expanded_fx.len();
        // ArrowLeft collapses it
        let mut left = egui::RawInput::default();
        left.events.push(egui::Event::Key { key: Key::ArrowLeft, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(left, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.ui.expanded_fx.len() < expanded_count, "ArrowLeft should collapse the open folder");
    }

    #[test]
    fn preset_grid_splitter_resizes_and_size_buttons_change_layout() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.ui.lumetri_grid_folder = Some("Film Stocks".to_string());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        assert!(app.auto.find("effects.presetGrid.splitter").is_some());
        let size_l = app.auto.find("effects.presetGrid.size.l").expect("L size button must be present");
        let l_pos = pos2(size_l.rect[0] + size_l.rect[2] * 0.5, size_l.rect[1] + size_l.rect[3] * 0.5);

        let mut down = egui::RawInput::default();
        down.events.push(egui::Event::PointerMoved(l_pos));
        down.events.push(egui::Event::PointerButton { pos: l_pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(down, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        let mut up = egui::RawInput::default();
        up.events.push(egui::Event::PointerMoved(l_pos));
        up.events.push(egui::Event::PointerButton { pos: l_pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(up, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        assert_eq!(app.ui.effects_preset_thumb_scale, 2);
    }

    #[test]
    fn multi_token_search_matches_out_of_order_words() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        app.ui.effects_search = "blur gaus".to_string();

        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.item.ec.blur.gaussian").is_some(), "multi-token query `blur gaus` must find Gaussian Blur");
    }

    #[test]
    fn folder_can_be_collapsed_during_active_search() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        app.ui.effects_search = "ec.blur.gaussian".to_string();

        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.item.ec.blur.gaussian").is_some());

        let ec_folder_id = format!("effects.folder.{}", filmcraft_project::EFFECTCRAFT_TOP_FOLDER);
        let folder = app.auto.find(&ec_folder_id).expect("EffectCraft top folder must be visible");
        let f_pos = pos2(folder.rect[0] + folder.rect[2] * 0.5, folder.rect[1] + folder.rect[3] * 0.5);

        let mut down = egui::RawInput::default();
        down.events.push(egui::Event::PointerMoved(f_pos));
        down.events.push(egui::Event::PointerButton { pos: f_pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(down, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        let mut up = egui::RawInput::default();
        up.events.push(egui::Event::PointerMoved(f_pos));
        up.events.push(egui::Event::PointerButton { pos: f_pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(up, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.item.ec.blur.gaussian").is_none(), "collapsing folder during search must hide its children");
    }

    #[test]
    fn effect_inspector_toggles_and_shows_selected_effect_parameters() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.ui.selected_effect = Some("ec.blur.gaussian".to_string());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();
        assert!(app.auto.find("effects.inspector").is_none());

        let btn = app.auto.find("effects.action.toggleInspector").expect("inspector toggle must exist on wide panel");
        let b_pos = pos2(btn.rect[0] + btn.rect[2] * 0.5, btn.rect[1] + btn.rect[3] * 0.5);

        let mut down = egui::RawInput::default();
        down.events.push(egui::Event::PointerMoved(b_pos));
        down.events.push(egui::Event::PointerButton { pos: b_pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(down, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        let mut up = egui::RawInput::default();
        up.events.push(egui::Event::PointerMoved(b_pos));
        up.events.push(egui::Event::PointerButton { pos: b_pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(up, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        assert!(app.ui.effects_inspector_open);
        assert!(app.auto.find("effects.inspector").is_some());
        assert!(app.auto.find("effects.inspector.apply").is_some());
    }

    #[test]
    fn presets_folder_is_reachable_via_keyboard_navigation() {
        let ctx = egui::Context::default();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.ui.focused = PanelKind::Effects;
        app.ui.effects_kind_filter = EffectsKindFilter::Presets;
        app.ui.expanded_fx.clear();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));

        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        let mut down = egui::RawInput::default();
        down.events.push(egui::Event::Key { key: Key::ArrowDown, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(down, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        let mut right = egui::RawInput::default();
        right.events.push(egui::Event::Key { key: Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        app.auto.begin_frame();
        let mut out = ctx.run_ui(right, |ui| show(&mut app, ui, rect));
        out.textures_delta.clear();

        assert!(app.ui.expanded_fx.iter().any(|k| k == "Presets"), "ArrowDown + ArrowRight must expand Presets folder");
    }
}

#[cfg(test)]
mod naming_tests {
    use super::folder_key_matches;

    #[test]
    fn color_preset_labels_do_not_change_saved_folder_identity() {
        let key = filmcraft_project::vtransition::COLOR_PRESETS_FOLDER_KEY;
        assert_eq!(key, "Lumetri Presets");
        assert_eq!(filmcraft_project::vtransition::effect_folder_label(key), "Color Grading Presets");
        for saved in ["Lumetri Presets", "Color Grade Presets", "Color Grading Presets"] {
            assert!(folder_key_matches(saved, key));
            assert!(folder_key_matches(&format!("{saved}/Cinematic"), &format!("{key}/Cinematic")));
            assert!(!folder_key_matches(&format!("{saved}/Other"), &format!("{key}/Cinematic")));
        }
    }

    #[test]
    fn favorites_and_recent_effects_track_order_and_cap() {
        let mut favs = Vec::new();
        super::toggle_favorite_effect(&mut favs, "tint");
        super::toggle_favorite_effect(&mut favs, "gaussian_blur");
        assert_eq!(favs, vec!["tint", "gaussian_blur"]);
        super::toggle_favorite_effect(&mut favs, "tint");
        assert_eq!(favs, vec!["gaussian_blur"]);

        let mut recent = Vec::new();
        for i in 0..12 {
            super::record_recent_effect(&mut recent, &format!("fx_{i}"));
        }
        assert_eq!(recent.len(), 10);
        assert_eq!(recent[0], "fx_11");
        super::record_recent_effect(&mut recent, "fx_5");
        assert_eq!(recent[0], "fx_5");
        assert_eq!(recent.len(), 10);
    }
}
