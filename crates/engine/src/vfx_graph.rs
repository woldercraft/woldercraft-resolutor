//! Engine commands for the non-linear VFX Compositing Node Graph (`vfxGraph.*`).
//!
//! Operates on `TrackItem::vfx_graph`, keeping `TrackItem::effects` synchronized for legacy views
//! and GPU fast-path evaluation. Every mutation validates acyclic DAG structure, port compatibility,
//! and Never-Crash bounds (`MAX_VFX_NODES`, `MAX_VFX_EDGES`) before committing to undo history.

use std::collections::BTreeMap;

use filmcraft_geom::Vec2;
use filmcraft_project::{
    ClipId, EffectKind, LayerBlendMode, MAX_VFX_NODES, Mask, MaskPath, MergeOperator, Param, ParamKind, ParamValue, TrackKind, VfxEdge, VfxEndpoint,
    VfxGraph, VfxInputSlot, VfxNode, VfxNodeId, VfxNodeKind, VfxNodeSettings, VfxPortKind, VfxUnderlay, find_effect,
};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, clip_p, f64_p, has_seq, json_to_param, str_p, u64_p};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;

fn spec(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: has_seq, run, journal: true, named: crate::commands::NamedKeys::NONE }
}

fn query(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: has_seq, run, journal: false, named: crate::commands::NamedKeys::NONE }
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        query("vfxGraph.get", "Get VFX Node Graph", r#"{"clip":id?}"#, get_graph),
        spec("vfxGraph.ensure", "Initialize VFX Node Graph", r#"{"clip":id?}"#, ensure_graph),
        spec(
            "vfxGraph.addNode",
            "Add VFX Node",
            r#"{"clip":id?,"kind":"effect"|"merge"|"mask"|"pipeRouter","effect":str?,"maskKind":"rect"|"ellipse"|"path"?,"after":id?,"connectTo":id?,"slot":"main"|"fg"|"mask"?,"pos":[x,y]?}"#,
            add_node,
        ),
        spec(
            "vfxGraph.insertOnEdge",
            "Insert VFX Node on Edge",
            r#"{"clip":id?,"node":id,"from":"in"|id,"to":"out"|id,"slot":"main"|"fg"|"mask"?}"#,
            insert_on_edge,
        ),
        spec("vfxGraph.extractNode", "Extract VFX Node", r#"{"clip":id?,"node":id}"#, extract_node),
        spec(
            "vfxGraph.connect",
            "Connect VFX Nodes",
            r#"{"clip":id?,"from":"in"|id,"to":"out"|id,"slot":"main"|"fg"|"mask"?,"kind":"image"|"mask"?}"#,
            connect_ports,
        ),
        spec(
            "vfxGraph.disconnect",
            "Disconnect VFX Node Input",
            r#"{"clip":id?,"to":"out"|id,"slot":"main"|"fg"|"mask"?}"#,
            disconnect_port,
        ),
        spec("vfxGraph.swapInputs", "Swap Merge Inputs", r#"{"clip":id?,"node":id}"#, swap_inputs),
        spec("vfxGraph.togglePassThrough", "Toggle VFX Node Pass-Through", r#"{"clip":id?,"node":id}"#, toggle_pass_through),
        spec("vfxGraph.removeNode", "Remove VFX Node", r#"{"clip":id?,"node":id}"#, remove_node),
        spec("vfxGraph.duplicateNode", "Duplicate VFX Node", r#"{"clip":id?,"node":id}"#, duplicate_node),
        spec("vfxGraph.createInstance", "Create VFX Node Instance", r#"{"clip":id?,"source":id}"#, create_instance),
        spec(
            "vfxGraph.deinstanceParam",
            "De-instance VFX Parameter",
            r#"{"clip":id?,"node":id,"param":str,"reinstance":bool?}"#,
            deinstance_param,
        ),
        spec(
            "vfxGraph.setNodeSetting",
            "Set VFX Node Settings",
            r#"{"clip":id?,"node":id,"blend":f?,"blendKeyframe":bool?,"blendToggleAnimation":bool?,"invertMask":bool?,"multiplyByMask":bool?,"channels":[bool,bool,bool,bool]?,"label":str?,"pos":[x,y]?,"merge":str|bool?}"#,
            set_node_setting,
        ),
        spec(
            "vfxGraph.setNodeParam",
            "Set VFX Node Parameter",
            r#"{"clip":id?,"node":id,"param":str,"value":val,"keyframe":bool?,"toggleAnimation":bool?,"merge":str|bool?}"#,
            set_node_param,
        ),
        spec(
            "vfxGraph.addUnderlay",
            "Add VFX Underlay",
            r#"{"clip":id?,"title":str?,"color":[r,g,b]?,"rect":[x,y,w,h]}"#,
            add_underlay,
        ),
        spec("vfxGraph.removeUnderlay", "Remove VFX Underlay", r#"{"clip":id?,"underlay":id}"#, remove_underlay),
        spec("vfxGraph.autoLayout", "Auto-Layout VFX Graph", r#"{"clip":id?}"#, auto_layout),
    ]
}

fn resolve_target_clip(s: &Session, p: &Value) -> Option<ClipId> {
    if let Some(c) = clip_p(p, "clip") {
        return Some(c);
    }
    let seq = s.active_sequence()?;
    if let Some(c) = s
        .state
        .selection
        .iter()
        .copied()
        .find(|&c| seq.find_item(c).and_then(|(tid, _)| seq.track(tid)).is_some_and(|t| t.kind == TrackKind::Video))
    {
        return Some(c);
    }
    let ph = s.playhead();
    for track in seq.video_tracks.iter().rev() {
        if !track.locked && !track.muted {
            for item in &track.items {
                if item.start <= ph && ph < item.end() {
                    return Some(item.id);
                }
            }
        }
    }
    seq.video_tracks.iter().flat_map(|t| &t.items).next().map(|i| i.id)
}

fn parse_endpoint(v: Option<&Value>, cmd: &str, field: &str) -> Result<VfxEndpoint> {
    let Some(v) = v else {
        return Err(bad(cmd, format!("need `{field}`")));
    };
    if let Some(s) = v.as_str() {
        return match s.to_ascii_lowercase().as_str() {
            "in" | "mediain" | "media_in" | "source" => Ok(VfxEndpoint::MediaIn),
            "out" | "mediaout" | "media_out" | "output" => Ok(VfxEndpoint::MediaOut),
            _ => s.parse::<u64>().map(|n| VfxEndpoint::Node(VfxNodeId(n))).map_err(|_| bad(cmd, format!("invalid endpoint `{s}`"))),
        };
    }
    if let Some(n) = v.as_u64() {
        return Ok(VfxEndpoint::Node(VfxNodeId(n)));
    }
    Err(bad(cmd, format!("invalid `{field}` endpoint")))
}

fn parse_slot(v: Option<&str>, cmd: &str) -> Result<VfxInputSlot> {
    match v.unwrap_or("main").to_ascii_lowercase().as_str() {
        "main" | "bg" | "background" | "input" => Ok(VfxInputSlot::Main),
        "fg" | "foreground" => Ok(VfxInputSlot::Foreground),
        "mask" | "effectmask" | "effect_mask" => Ok(VfxInputSlot::EffectMask),
        other => Err(bad(cmd, format!("unknown slot `{other}` (expected `main`, `fg`, or `mask`)"))),
    }
}

fn parse_port_kind(v: Option<&str>, slot: VfxInputSlot, cmd: &str) -> Result<VfxPortKind> {
    match v {
        None => Ok(if slot == VfxInputSlot::EffectMask { VfxPortKind::Mask } else { VfxPortKind::Image }),
        Some(raw) => match raw.to_ascii_lowercase().as_str() {
            "image" | "rgb" | "color" => Ok(VfxPortKind::Image),
            "mask" | "matte" | "alpha" => Ok(VfxPortKind::Mask),
            other => Err(bad(cmd, format!("unknown port kind `{other}`"))),
        },
    }
}

fn parse_merge_operator(raw: &str) -> Option<MergeOperator> {
    match raw.to_ascii_lowercase().as_str() {
        "over" => Some(MergeOperator::Over),
        "in" => Some(MergeOperator::In),
        "out" => Some(MergeOperator::Out),
        "atop" => Some(MergeOperator::Atop),
        "xor" => Some(MergeOperator::Xor),
        _ => None,
    }
}

fn merge_key_from(p: &Value, prefix: &str, id: u64) -> Option<String> {
    match p.get("merge") {
        Some(Value::String(k)) => Some(format!("{prefix}:{id}:{k}")),
        _ => bool_p(p, "merge").unwrap_or(false).then(|| format!("{prefix}:{id}")),
    }
}

fn mutate_vfx_graph<R, F>(s: &mut Session, p: &Value, cmd: &str, label: &str, merge_key: Option<&str>, f: F) -> Result<R>
where
    F: FnOnce(&mut VfxGraph, Tick, (u32, u32)) -> Result<R>,
{
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let clip_id = resolve_target_clip(s, p).ok_or_else(|| bad(cmd, "no video clip selected"))?;
    let ph = s.playhead();
    let seq_size = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).unwrap_or((1920, 1080));
    let body = move |pr: &mut filmcraft_project::Project, _: &mut crate::EditorState| -> Result<R> {
        let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let (_, it) = q.find_item_mut(clip_id).ok_or(filmcraft_edit::EditError::NoItem(clip_id))?;
        let mt = it.effect_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
        if it.vfx_graph.is_none() {
            it.vfx_graph = Some(Box::new(VfxGraph::from_serial_effects(&it.effects)));
        }
        let Some(vg) = it.vfx_graph.as_mut() else {
            return Err(bad(cmd, "missing clip VFX graph"));
        };
        let out = f(vg, mt, seq_size)?;
        it.sync_vfx_effects_from_graph();
        Ok(out)
    };
    match merge_key {
        Some(k) => s.edit_merged(label, k, body),
        None => s.edit(label, body),
    }
}

fn get_graph(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.get";
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let clip_id = resolve_target_clip(s, p).ok_or_else(|| bad(CMD, "no video clip selected"))?;
    let (_, it) = seq.find_item(clip_id).ok_or(filmcraft_edit::EditError::NoItem(clip_id))?;
    let graph = it.effective_vfx_graph().into_owned();
    let topo = graph.topo_order().unwrap_or_default();
    Ok(json!({ "clip": clip_id.0, "graph": graph, "topoOrder": topo }))
}

fn ensure_graph(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.ensure";
    mutate_vfx_graph(s, p, CMD, "Initialize VFX Graph", None, |g, _, _| Ok(json!({ "nodes": g.nodes.len() })))
}

fn add_node(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.addNode";
    let kind_raw = str_p(p, "kind").unwrap_or("effect").to_ascii_lowercase();
    let effect_name = str_p(p, "effect").map(str::to_string);
    let mask_kind = str_p(p, "maskKind").unwrap_or("ellipse").to_ascii_lowercase();
    let after = u64_p(p, "after").map(VfxNodeId);
    let connect_to = u64_p(p, "connectTo").map(VfxNodeId);
    let slot = parse_slot(str_p(p, "slot"), CMD)?;
    let custom_pos = p.get("pos").and_then(Value::as_array).and_then(|a| {
        let x = a.first()?.as_f64()? as f32;
        let y = a.get(1)?.as_f64()? as f32;
        (x.is_finite() && y.is_finite()).then_some([x.clamp(-50_000.0, 50_000.0), y.clamp(-50_000.0, 50_000.0)])
    });

    mutate_vfx_graph(s, p, CMD, "Add VFX Node", None, move |g, _, (fw, fh)| {
        if g.nodes.len() >= MAX_VFX_NODES {
            return Err(bad(CMD, "maximum VFX nodes exceeded"));
        }
        let (node_kind, default_label, is_mask) = match kind_raw.as_str() {
            "effect" => {
                let req = effect_name.as_deref().ok_or_else(|| bad(CMD, "need `effect` for effect node"))?;
                let def = find_effect(req)
                    .or_else(|| filmcraft_project::find_effect_by_name(req))
                    .ok_or_else(|| bad(CMD, format!("unknown effect `{req}`")))?;
                if def.intrinsic || def.kind != EffectKind::Video || def.id == "lumetri" || filmcraft_project::graphic::is_layer_id(def.id) {
                    return Err(bad(CMD, format!("`{}` cannot be added as a VFX node", def.name)));
                }
                (VfxNodeKind::Effect(def.instance()), None, false)
            }
            "merge" => (
                VfxNodeKind::Merge {
                    blend_mode: LayerBlendMode::Normal,
                    operator: MergeOperator::Over,
                    center: Param::new(ParamValue::Vec2(Vec2::new(0.5, 0.5))),
                    size: Param::new(ParamValue::Float(1.0)),
                    angle: Param::new(ParamValue::Float(0.0)),
                },
                Some("Merge".to_string()),
                false,
            ),
            "mask" => {
                let (w, h) = (fw.max(1) as f64, fh.max(1) as f64);
                let path = match mask_kind.as_str() {
                    "rect" | "rectangle" => MaskPath::rect(w * 0.25, h * 0.25, w * 0.75, h * 0.75),
                    "path" | "polygon" => MaskPath::regular_polygon(Vec2::new(w * 0.5, h * 0.5), Vec2::new(w * 0.25, h * 0.25), 5),
                    _ => MaskPath::ellipse(Vec2::new(w * 0.5, h * 0.5), Vec2::new(w * 0.25, h * 0.25)),
                };
                (VfxNodeKind::Mask(Mask::new("Mask", path)), Some("Mask".to_string()), true)
            }
            "piperouter" | "pipe_router" | "router" => (VfxNodeKind::PipeRouter, Some("Router".to_string()), false),
            other => return Err(bad(CMD, format!("unknown VFX node kind `{other}`"))),
        };

        let id = g.alloc_id();
        let pos = custom_pos.unwrap_or_else(|| {
            if let Some(aid) = after
                && let Some(an) = g.node(aid)
            {
                [an.pos[0] + 204.0, an.pos[1] + if is_mask { -110.0 } else { 0.0 }]
            } else {
                [120.0 + g.nodes.len() as f32 * 204.0, if is_mask { 20.0 } else { 120.0 }]
            }
        });

        g.nodes.push(VfxNode { id, kind: node_kind, enabled: true, label: default_label, settings: VfxNodeSettings::default(), pos });
        let ep = VfxEndpoint::Node(id);

        if is_mask {
            if let Some(target_id) = connect_to.or(after) {
                let _ = g.connect(VfxEdge {
                    from: ep,
                    to: VfxEndpoint::Node(target_id),
                    to_slot: VfxInputSlot::EffectMask,
                    kind: VfxPortKind::Mask,
                });
            }
        } else if let Some(target_id) = connect_to {
            let _ = g.connect(VfxEdge { from: ep, to: VfxEndpoint::Node(target_id), to_slot: slot, kind: VfxPortKind::Image });
        } else {
            // Splice after `after` (or before MediaOut if no `after` specified).
            let src_ep = after.map(VfxEndpoint::Node).unwrap_or_else(|| {
                g.edges.iter().find(|e| e.to == VfxEndpoint::MediaOut && e.to_slot == VfxInputSlot::Main).map_or(VfxEndpoint::MediaIn, |e| e.from)
            });
            let existing_out: Option<(VfxEndpoint, VfxInputSlot)> = g
                .edges
                .iter()
                .find(|e| e.from == src_ep && e.to_slot == VfxInputSlot::Main)
                .map(|e| (e.to, e.to_slot));
            g.connect(VfxEdge { from: src_ep, to: ep, to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).map_err(|e| bad(CMD, e))?;
            let next_target = existing_out.unwrap_or((VfxEndpoint::MediaOut, VfxInputSlot::Main));
            g.connect(VfxEdge { from: ep, to: next_target.0, to_slot: next_target.1, kind: VfxPortKind::Image }).map_err(|e| bad(CMD, e))?;
        }

        Ok(json!({ "node": id.0, "kind": kind_raw }))
    })
}

fn insert_on_edge(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.insertOnEdge";
    let id = VfxNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    let from = parse_endpoint(p.get("from"), CMD, "from")?;
    let to = parse_endpoint(p.get("to"), CMD, "to")?;
    let slot = parse_slot(str_p(p, "slot"), CMD)?;
    mutate_vfx_graph(s, p, CMD, "Insert VFX Node on Wire", None, move |g, _, _| {
        g.insert_on_edge(id, from, to, slot).map_err(|e| bad(CMD, e))?;
        Ok(json!({ "node": id.0 }))
    })
}

fn extract_node(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.extractNode";
    let id = VfxNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    mutate_vfx_graph(s, p, CMD, "Extract VFX Node", None, move |g, _, _| {
        if g.node(id).is_none() {
            return Err(bad(CMD, format!("unknown VFX node {}", id.0)));
        }
        g.extract_node(id);
        Ok(json!({ "node": id.0 }))
    })
}

fn connect_ports(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.connect";
    let from = parse_endpoint(p.get("from"), CMD, "from")?;
    let to = parse_endpoint(p.get("to"), CMD, "to")?;
    let slot = parse_slot(str_p(p, "slot"), CMD)?;
    let kind = parse_port_kind(str_p(p, "kind"), slot, CMD)?;
    mutate_vfx_graph(s, p, CMD, "Connect VFX Nodes", None, move |g, _, _| {
        g.connect(VfxEdge { from, to, to_slot: slot, kind }).map_err(|e| bad(CMD, e))?;
        Ok(json!({ "ok": true }))
    })
}

fn disconnect_port(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.disconnect";
    let to = parse_endpoint(p.get("to"), CMD, "to")?;
    let slot = parse_slot(str_p(p, "slot"), CMD)?;
    mutate_vfx_graph(s, p, CMD, "Disconnect VFX Wire", None, move |g, _, _| {
        let before = g.edges.len();
        g.disconnect(to, slot);
        Ok(json!({ "removed": before.saturating_sub(g.edges.len()) }))
    })
}

fn swap_inputs(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.swapInputs";
    let id = VfxNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    mutate_vfx_graph(s, p, CMD, "Swap Merge Inputs", None, move |g, _, _| {
        g.swap_merge_inputs(id).map_err(|e| bad(CMD, e))?;
        Ok(json!({ "node": id.0 }))
    })
}

fn toggle_pass_through(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.togglePassThrough";
    let id = VfxNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    mutate_vfx_graph(s, p, CMD, "Toggle VFX Pass-Through", None, move |g, _, _| {
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown VFX node {}", id.0)))?;
        n.enabled = !n.enabled;
        if let VfxNodeKind::Effect(fx) = &mut n.kind {
            fx.enabled = n.enabled;
        }
        Ok(json!({ "node": id.0, "enabled": n.enabled }))
    })
}

fn remove_node(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.removeNode";
    let id = VfxNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    mutate_vfx_graph(s, p, CMD, "Remove VFX Node", None, move |g, _, _| {
        if g.node(id).is_none() {
            return Err(bad(CMD, format!("unknown VFX node {}", id.0)));
        }
        g.extract_node(id);
        g.nodes.retain(|n| n.id != id);
        // Convert any Instance nodes referencing `id` into standalone disabled PipeRouters or keep safe.
        for n in &mut g.nodes {
            if matches!(n.kind, VfxNodeKind::Instance { source, .. } if source == id) {
                n.kind = VfxNodeKind::PipeRouter;
            }
        }
        Ok(json!({ "removed": id.0, "remaining": g.nodes.len() }))
    })
}

fn duplicate_node(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.duplicateNode";
    let id = VfxNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    mutate_vfx_graph(s, p, CMD, "Duplicate VFX Node", None, move |g, _, _| {
        if g.nodes.len() >= MAX_VFX_NODES {
            return Err(bad(CMD, "maximum VFX nodes exceeded"));
        }
        let mut copy = g.node(id).cloned().ok_or_else(|| bad(CMD, format!("unknown VFX node {}", id.0)))?;
        let new_id = g.alloc_id();
        copy.id = new_id;
        copy.pos = [copy.pos[0] + 40.0, copy.pos[1] + 92.0];
        g.nodes.push(copy);
        Ok(json!({ "node": new_id.0 }))
    })
}

fn create_instance(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.createInstance";
    let source = VfxNodeId(u64_p(p, "source").ok_or_else(|| bad(CMD, "need `source`"))?);
    mutate_vfx_graph(s, p, CMD, "Create VFX Instance", None, move |g, _, _| {
        if g.nodes.len() >= MAX_VFX_NODES {
            return Err(bad(CMD, "maximum VFX nodes exceeded"));
        }
        let src = g.node(source).ok_or_else(|| bad(CMD, format!("unknown source VFX node {}", source.0)))?;
        let root_source = match &src.kind {
            VfxNodeKind::Effect(_) => source,
            VfxNodeKind::Instance { source: s2, .. } => *s2,
            _ => return Err(bad(CMD, "only Effect nodes can be instanced")),
        };
        let pos = [src.pos[0] + 40.0, src.pos[1] + 104.0];
        let base_label = src.label.clone();
        let new_id = g.alloc_id();
        g.nodes.push(VfxNode {
            id: new_id,
            kind: VfxNodeKind::Instance { source: root_source, overrides: BTreeMap::new() },
            enabled: true,
            label: base_label.map(|l| format!("{l} (Inst)")),
            settings: VfxNodeSettings::default(),
            pos,
        });
        Ok(json!({ "node": new_id.0, "source": root_source.0 }))
    })
}

fn deinstance_param(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.deinstanceParam";
    let id = VfxNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    let param = str_p(p, "param").ok_or_else(|| bad(CMD, "need `param`"))?.to_string();
    let reinstance = bool_p(p, "reinstance").unwrap_or(false);
    mutate_vfx_graph(s, p, CMD, "De-instance VFX Parameter", None, move |g, _, _| {
        let resolved_val = g.resolve_effect(id).and_then(|fx| fx.param(&param).cloned());
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown VFX node {}", id.0)))?;
        let VfxNodeKind::Instance { overrides, .. } = &mut n.kind else {
            return Err(bad(CMD, "node is not an Instance node"));
        };
        if reinstance {
            overrides.remove(&param);
        } else {
            let cur = resolved_val.ok_or_else(|| bad(CMD, format!("unknown parameter `{param}`")))?;
            overrides.insert(param.clone(), cur);
        }
        Ok(json!({ "node": id.0, "param": param, "deinstanced": !reinstance }))
    })
}

fn set_node_setting(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.setNodeSetting";
    let id = VfxNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    let blend = f64_p(p, "blend");
    let blend_kf = bool_p(p, "blendKeyframe").unwrap_or(false);
    let blend_toggle_kf = bool_p(p, "blendToggleKeyframe").unwrap_or(false);
    let blend_toggle_anim = bool_p(p, "blendToggleAnimation").unwrap_or(false);
    let invert_mask = bool_p(p, "invertMask");
    let multiply_by_mask = bool_p(p, "multiplyByMask");
    let channels = p.get("channels").and_then(Value::as_array).and_then(|arr| {
        if arr.len() != 4 {
            return None;
        }
        Some([arr[0].as_bool()?, arr[1].as_bool()?, arr[2].as_bool()?, arr[3].as_bool()?])
    });
    let label_opt = p.get("label").map(|v| v.as_str().unwrap_or("").trim().to_string());
    let pos_opt = p.get("pos").and_then(Value::as_array).and_then(|a| {
        let x = a.first()?.as_f64()? as f32;
        let y = a.get(1)?.as_f64()? as f32;
        (x.is_finite() && y.is_finite()).then_some([x.clamp(-50_000.0, 50_000.0), y.clamp(-50_000.0, 50_000.0)])
    });
    if blend.is_some_and(|v| !v.is_finite()) {
        return Err(bad(CMD, "`blend` must be finite"));
    }
    let merge = merge_key_from(p, "vfxGraph.setting", id.0);
    mutate_vfx_graph(s, p, CMD, "Edit VFX Node Settings", merge.as_deref(), move |g, mt, _| {
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown VFX node {}", id.0)))?;
        if blend_toggle_anim {
            n.settings.blend.toggle_animation(mt);
        }
        if blend_toggle_kf && !n.settings.blend.remove_keyframe_at(mt) {
            n.settings.blend.add_keyframe(mt);
        }
        if let Some(b) = blend {
            let v = ParamValue::Float(b.clamp(0.0, 1.0));
            if blend_kf {
                n.settings.blend.put_keyframe(mt, v);
            } else {
                n.settings.blend.set_at(mt, v);
            }
        }
        if let Some(inv) = invert_mask {
            n.settings.invert_mask = inv;
        }
        if let Some(mul) = multiply_by_mask {
            n.settings.multiply_by_mask = mul;
        }
        if let Some(ch) = channels {
            n.settings.channels = ch;
        }
        if let Some(lbl) = label_opt {
            n.label = (!lbl.is_empty()).then_some(lbl);
            if let VfxNodeKind::Effect(fx) = &mut n.kind {
                fx.label = n.label.clone();
            }
        }
        if let Some(pos) = pos_opt {
            n.pos = pos;
        }
        Ok(json!({ "node": id.0 }))
    })
}

fn apply_param_mutation(
    prm: &mut Param,
    param: &str,
    def_kind: Option<&ParamKind>,
    val_opt: Option<&Value>,
    mt: Tick,
    (keyframe, toggle_anim, toggle_kf): (bool, bool, bool),
) -> Result<()> {
    if toggle_anim {
        prm.toggle_animation(mt);
    }
    if toggle_kf && !prm.remove_keyframe_at(mt) {
        prm.add_keyframe(mt);
    }
    if let Some(val) = val_opt {
        let pv = json_to_param(&prm.value, val).ok_or_else(|| bad("vfxGraph.setNodeParam", format!("invalid value for `{param}`")))?;
        if let (Some(ParamKind::Float { min, max, .. }), ParamValue::Float(x)) = (def_kind, &pv)
            && (!x.is_finite() || *x < *min || *x > *max)
        {
            return Err(bad("vfxGraph.setNodeParam", format!("`{param}` must be between {min} and {max}, got {x}")));
        }
        if keyframe {
            prm.put_keyframe(mt, pv);
        } else {
            prm.set_at(mt, pv);
        }
    }
    Ok(())
}

fn set_node_param(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.setNodeParam";
    let id = VfxNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    let param = str_p(p, "param").ok_or_else(|| bad(CMD, "need `param`"))?.to_string();
    let val_opt = p.get("value").cloned();
    let keyframe = bool_p(p, "keyframe").unwrap_or(false);
    let toggle_anim = bool_p(p, "toggleAnimation").unwrap_or(false);
    let toggle_kf = bool_p(p, "toggleKeyframe").unwrap_or(false);
    let merge = merge_key_from(p, "vfxGraph.param", id.0);

    mutate_vfx_graph(s, p, CMD, "Adjust VFX Node Parameter", merge.as_deref(), move |g, mt, _| {
        // Resolve Instance target: if parameter is overridden edit override, else edit source node!
        let target_id = match g.node(id).map(|n| &n.kind) {
            Some(VfxNodeKind::Instance { source, overrides }) if !overrides.contains_key(&param) => *source,
            Some(_) => id,
            None => return Err(bad(CMD, format!("unknown VFX node {}", id.0))),
        };
        let n = g.node_mut(target_id).ok_or_else(|| bad(CMD, format!("unknown VFX node {}", target_id.0)))?;
        match &mut n.kind {
            VfxNodeKind::Effect(fx) => {
                if !fx.params.contains_key(&param)
                    && let Some(d) = fx.def().and_then(|d| d.param(&param))
                {
                    fx.params.insert(param.clone(), Param::new(d.default.clone()));
                }
                let def_kind = fx.def().and_then(|d| d.param(&param)).map(|d| d.kind.clone());
                let prm = fx.param_mut(&param).ok_or_else(|| bad(CMD, format!("unknown param `{param}`")))?;
                apply_param_mutation(prm, &param, def_kind.as_ref(), val_opt.as_ref(), mt, (keyframe, toggle_anim, toggle_kf))?;
            }
            VfxNodeKind::Instance { overrides, .. } => {
                let prm = overrides.get_mut(&param).ok_or_else(|| bad(CMD, format!("unknown overridden param `{param}`")))?;
                apply_param_mutation(prm, &param, None, val_opt.as_ref(), mt, (keyframe, toggle_anim, toggle_kf))?;
            }
            VfxNodeKind::Merge { blend_mode, operator, center, size, angle } => match param.as_str() {
                "blend_mode" | "blendMode" => {
                    let s = val_opt.as_ref().and_then(Value::as_str).ok_or_else(|| bad(CMD, "blendMode expects string"))?;
                    *blend_mode = LayerBlendMode::from_name(s).ok_or_else(|| bad(CMD, format!("unknown blend mode `{s}`")))?;
                }
                "operator" => {
                    let s = val_opt.as_ref().and_then(Value::as_str).ok_or_else(|| bad(CMD, "operator expects string"))?;
                    *operator = parse_merge_operator(s).ok_or_else(|| bad(CMD, format!("unknown merge operator `{s}`")))?;
                }
                "center" | "size" | "angle" => {
                    let prm = match param.as_str() {
                        "center" => center,
                        "size" => size,
                        _ => angle,
                    };
                    apply_param_mutation(prm, &param, None, val_opt.as_ref(), mt, (keyframe, toggle_anim, toggle_kf))?;
                }
                _ => return Err(bad(CMD, format!("unknown Merge param `{param}`"))),
            },
            VfxNodeKind::Mask(mask) => match param.as_str() {
                "inverted" => {
                    mask.inverted = val_opt.as_ref().and_then(Value::as_bool).ok_or_else(|| bad(CMD, "inverted expects bool"))?;
                }
                "feather" | "opacity" | "expansion" | "feather_balance" => {
                    let prm = match param.as_str() {
                        "feather" => &mut mask.feather,
                        "opacity" => &mut mask.opacity,
                        "expansion" => &mut mask.expansion,
                        _ => &mut mask.feather_balance,
                    };
                    apply_param_mutation(prm, &param, None, val_opt.as_ref(), mt, (keyframe, toggle_anim, toggle_kf))?;
                }
                _ => return Err(bad(CMD, format!("unknown Mask param `{param}`"))),
            },
            VfxNodeKind::PipeRouter => return Err(bad(CMD, "PipeRouter has no parameters")),
        }
        Ok(json!({ "node": id.0, "param": param }))
    })
}

fn add_underlay(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.addUnderlay";
    let title = str_p(p, "title").unwrap_or("Group").to_string();
    let color = p
        .get("color")
        .and_then(Value::as_array)
        .and_then(|a| Some([a.first()?.as_u64()?.min(255) as u8, a.get(1)?.as_u64()?.min(255) as u8, a.get(2)?.as_u64()?.min(255) as u8]))
        .unwrap_or([68, 92, 128]);
    let rect = p
        .get("rect")
        .and_then(Value::as_array)
        .and_then(|a| {
            let x = a.first()?.as_f64()? as f32;
            let y = a.get(1)?.as_f64()? as f32;
            let w = a.get(2)?.as_f64()? as f32;
            let h = a.get(3)?.as_f64()? as f32;
            (x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()).then_some([x, y, w.max(80.0), h.max(60.0)])
        })
        .ok_or_else(|| bad(CMD, "need `rect: [x, y, w, h]`"))?;
    mutate_vfx_graph(s, p, CMD, "Add VFX Underlay", None, move |g, _, _| {
        let next_id = g.underlays.iter().map(|u| u.id).max().unwrap_or(0).saturating_add(1);
        g.underlays.push(VfxUnderlay { id: next_id, title: title.clone(), color, rect });
        Ok(json!({ "underlay": next_id }))
    })
}

fn remove_underlay(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.removeUnderlay";
    let uid = u64_p(p, "underlay").ok_or_else(|| bad(CMD, "need `underlay`"))?;
    mutate_vfx_graph(s, p, CMD, "Remove VFX Underlay", None, move |g, _, _| {
        let before = g.underlays.len();
        g.underlays.retain(|u| u.id != uid);
        Ok(json!({ "removed": before.saturating_sub(g.underlays.len()) }))
    })
}

fn auto_layout(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "vfxGraph.autoLayout";
    mutate_vfx_graph(s, p, CMD, "Auto-Layout VFX Graph", None, |g, _, _| {
        g.auto_layout();
        Ok(json!({ "nodes": g.nodes.len() }))
    })
}

#[cfg(test)]
mod tests {
    use filmcraft_project::{VfxEndpoint, VfxInputSlot, VfxNodeId, VfxNodeKind};
    use serde_json::json;

    use crate::Session;

    fn session_with_clip() -> (Session, u64) {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let clip_id = s
            .active_sequence()
            .and_then(|q| q.video_tracks.iter().flat_map(|t| &t.items).next())
            .expect("demo has a video clip")
            .id
            .0;
        (s, clip_id)
    }

    #[test]
    fn vfx_graph_add_splice_extract_merge_swap_instance_settings_and_undo() {
        let (mut s, clip) = session_with_clip();

        // Add two effects + a Merge node
        let r1 = s.execute("vfxGraph.addNode", json!({"clip": clip, "kind": "effect", "effect": "invert"})).unwrap();
        let n1 = r1["node"].as_u64().unwrap();
        let r2 = s.execute("vfxGraph.addNode", json!({"clip": clip, "kind": "effect", "effect": "gaussian_blur", "after": n1})).unwrap();
        let n2 = r2["node"].as_u64().unwrap();
        let rm = s.execute("vfxGraph.addNode", json!({"clip": clip, "kind": "merge", "after": n2})).unwrap();
        let nm = rm["node"].as_u64().unwrap();

        // Connect MediaIn -> Merge:Foreground and swap BG/FG (`Ctrl+T`)
        s.execute("vfxGraph.connect", json!({"clip": clip, "from": "in", "to": nm, "slot": "fg"})).unwrap();
        s.execute("vfxGraph.swapInputs", json!({"clip": clip, "node": nm})).unwrap();
        {
            let seq = s.active_sequence().unwrap();
            let (_, it) = seq.find_item(filmcraft_project::ClipId(clip)).unwrap();
            let g = it.vfx_graph.as_ref().unwrap();
            assert!(g.edges.iter().any(|e| e.from == VfxEndpoint::MediaIn && e.to == VfxEndpoint::Node(VfxNodeId(nm)) && e.to_slot == VfxInputSlot::Main));
            assert!(g.edges.iter().any(|e| e.from == VfxEndpoint::Node(VfxNodeId(n2)) && e.to == VfxEndpoint::Node(VfxNodeId(nm)) && e.to_slot == VfxInputSlot::Foreground));
        }

        // Create Instance of n2 (gaussian_blur) and de-instance `blurriness`
        let ri = s.execute("vfxGraph.createInstance", json!({"clip": clip, "source": n2})).unwrap();
        let ni = ri["node"].as_u64().unwrap();
        s.execute("vfxGraph.deinstanceParam", json!({"clip": clip, "node": ni, "param": "blurriness"})).unwrap();
        s.execute("vfxGraph.setNodeParam", json!({"clip": clip, "node": ni, "param": "blurriness", "value": 42.0})).unwrap();

        // Edit universal Settings (blend + channels + pass-through)
        s.execute("vfxGraph.setNodeSetting", json!({"clip": clip, "node": n1, "blend": 0.5, "channels": [true, false, true, true]})).unwrap();
        s.execute("vfxGraph.togglePassThrough", json!({"clip": clip, "node": n1})).unwrap();
        {
            let seq = s.active_sequence().unwrap();
            let (_, it) = seq.find_item(filmcraft_project::ClipId(clip)).unwrap();
            let g = it.vfx_graph.as_ref().unwrap();
            let node1 = g.node(VfxNodeId(n1)).unwrap();
            assert!(!node1.enabled);
            assert_eq!(node1.settings.channels, [true, false, true, true]);
            let node_i = g.node(VfxNodeId(ni)).unwrap();
            assert!(matches!(node_i.kind, VfxNodeKind::Instance { ref overrides, .. } if overrides.contains_key("blurriness")));
        }

        // Extract n2 cleanly and undo restores it
        s.execute("vfxGraph.extractNode", json!({"clip": clip, "node": n2})).unwrap();
        s.undo().unwrap();
        let seq = s.active_sequence().unwrap();
        let (_, it) = seq.find_item(filmcraft_project::ClipId(clip)).unwrap();
        let g = it.vfx_graph.as_ref().unwrap();
        assert!(g.edges.iter().any(|e| e.from == VfxEndpoint::Node(VfxNodeId(n1)) && e.to == VfxEndpoint::Node(VfxNodeId(n2))));
    }

    #[test]
    fn vfx_graph_rejects_hostile_params_and_cycles() {
        let (mut s, clip) = session_with_clip();
        let r1 = s.execute("vfxGraph.addNode", json!({"clip": clip, "kind": "effect", "effect": "invert"})).unwrap();
        let n1 = r1["node"].as_u64().unwrap();
        // Self-loop must fail cleanly
        assert!(s.execute("vfxGraph.connect", json!({"clip": clip, "from": n1, "to": n1})).is_err());
        // Unknown node id must fail cleanly
        assert!(s.execute("vfxGraph.swapInputs", json!({"clip": clip, "node": 99999})).is_err());
        // Swapping non-Merge node must fail cleanly
        assert!(s.execute("vfxGraph.swapInputs", json!({"clip": clip, "node": n1})).is_err());
        // Unknown effect name must fail cleanly
        assert!(s.execute("vfxGraph.addNode", json!({"clip": clip, "kind": "effect", "effect": "nonexistent_fx_xyz"})).is_err());
    }
}
