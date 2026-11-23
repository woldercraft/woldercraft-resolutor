//! Engine commands for the non-linear Color Grading Node Graph (`color.graph.*`).
//!
//! Operates on any of the four grading scopes:
//! - `"pre_clip"` (`ColorGroup::pre_clip`)
//! - `"clip"` (`TrackItem::color_graph`, synchronized with `TrackItem::effects` `"lumetri"` nodes)
//! - `"post_clip"` (`ColorGroup::post_clip`, synchronized with `ColorGroup::effects`)
//! - `"timeline"` (`Sequence::timeline_color`)
//!
//! All mutations validate acyclic DAG structure and bounds before committing to undo history.

use filmcraft_project::{
    ClipId, ColorEdge, ColorEndpoint, ColorGraph, ColorGroupId, ColorNodeId, ColorNodeKind, ColorPortKind, ColorScopeKind, LayerBlendMode, Param,
    SharedColorNode, SharedNodeId, SplitSpace, TrackKind, default_lumetri_instance, find_effect,
};
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
        query("color.graph.get", "Get Color Node Graph", r#"{"scope":"pre_clip"|"clip"|"post_clip"|"timeline"?,"clip":id?,"group":id?}"#, get_graph),
        spec(
            "color.graph.addNode",
            "Add Color Graph Node",
            r#"{"scope":str?,"clip":id?,"group":id?,"kind":"serial"|"outside"|"parallel"|"layer"|"splitter"|"shared"?,"after":id?,"space":"rgb"|"ycbcr"|"hsl"?,"shared":id?}"#,
            add_node,
        ),
        spec("color.graph.deleteNode", "Delete Color Graph Node", r#"{"scope":str?,"clip":id?,"group":id?,"node":id}"#, delete_node),
        spec(
            "color.graph.connect",
            "Connect Color Graph Ports",
            r#"{"scope":str?,"clip":id?,"group":id?,"kind":"rgb"|"key"?,"from":"source"|id,"fromPort":n?,"to":"output"|id,"toPort":n?,"invertKey":bool?}"#,
            connect_ports,
        ),
        spec(
            "color.graph.disconnect",
            "Disconnect Color Graph Port",
            r#"{"scope":str?,"clip":id?,"group":id?,"kind":"rgb"|"key"?,"to":"output"|id,"toPort":n?}"#,
            disconnect_port,
        ),
        spec("color.graph.moveNode", "Move Color Graph Node", r#"{"scope":str?,"clip":id?,"group":id?,"node":id,"pos":[x,y],"merge":str|bool?}"#, move_node),
        spec(
            "color.graph.setNodeParam",
            "Set Color Graph Node Parameter",
            r#"{"scope":str?,"clip":id?,"group":id?,"node":id,"param":str,"value":val,"merge":str|bool?,"begin":bool?}"#,
            set_node_param,
        ),
        spec(
            "color.graph.setKeyParams",
            "Set Color Node Key Parameters",
            r#"{"scope":str?,"clip":id?,"group":id?,"node":id,"inputGain":f?,"outputGain":f?,"invert":bool?,"merge":str|bool?}"#,
            set_key_params,
        ),
        spec(
            "color.graph.setLayerMixer",
            "Configure Layer Mixer Branch",
            r#"{"scope":str?,"clip":id?,"group":id?,"node":id,"port":n,"blend":str?,"gain":f?}"#,
            set_layer_mixer,
        ),
        spec("color.graph.toggleBypass", "Toggle Color Graph Node Bypass", r#"{"scope":str?,"clip":id?,"group":id?,"node":id}"#, toggle_bypass),
        spec("color.graph.resetNode", "Reset Color Graph Node", r#"{"scope":str?,"clip":id?,"group":id?,"node":id}"#, reset_node),
        spec("color.graph.shareNode", "Save Color Node as Shared Node", r#"{"scope":str?,"clip":id?,"group":id?,"node":id,"name":str?}"#, share_node),
        spec("color.graph.unlinkShared", "Unlink Shared Color Node to Local Corrector", r#"{"scope":str?,"clip":id?,"group":id?,"node":id}"#, unlink_shared),
    ]
}

fn resolve_target_clip(s: &Session, p: &Value) -> Option<ClipId> {
    if let Some(c) = clip_p(p, "clip") {
        return Some(c);
    }
    let seq = s.active_sequence()?;
    if let Some(c) =
        s.state.selection.iter().copied().find(|&c| seq.find_item(c).and_then(|(tid, _)| seq.track(tid)).is_some_and(|t| t.kind == TrackKind::Video))
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

#[derive(Clone, Copy, Debug)]
enum ResolvedScope {
    PreClip(ColorGroupId),
    Clip(ClipId),
    PostClip(ColorGroupId),
    Timeline,
}

fn resolve_scope(s: &Session, p: &Value, cmd: &str) -> Result<ResolvedScope> {
    let scope_kind = match str_p(p, "scope") {
        Some(raw) => ColorScopeKind::from_id(raw).ok_or_else(|| bad(cmd, format!("unknown color graph scope `{raw}`")))?,
        None => {
            if p.get("group").is_some_and(|v| !v.is_null()) {
                ColorScopeKind::PostClip
            } else {
                ColorScopeKind::Clip
            }
        }
    };
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    match scope_kind {
        ColorScopeKind::Timeline => Ok(ResolvedScope::Timeline),
        ColorScopeKind::Clip => {
            let cid = resolve_target_clip(s, p).ok_or_else(|| bad(cmd, "no video clip selected"))?;
            if seq.find_item(cid).is_none() {
                return Err(bad(cmd, format!("unknown clip {}", cid.0)));
            }
            Ok(ResolvedScope::Clip(cid))
        }
        ColorScopeKind::PreClip | ColorScopeKind::PostClip => {
            let gid = if let Some(g) = u64_p(p, "group") {
                ColorGroupId(g)
            } else if let Some(cid) = resolve_target_clip(s, p)
                && let Some(g) = seq.color_group_of(cid)
            {
                g.id
            } else {
                return Err(bad(cmd, "clip does not belong to a Color Group (specify `group` or assign the clip to a group first)"));
            };
            if seq.color_group(gid).is_none() {
                return Err(bad(cmd, format!("unknown color group {}", gid.0)));
            }
            Ok(if scope_kind == ColorScopeKind::PreClip { ResolvedScope::PreClip(gid) } else { ResolvedScope::PostClip(gid) })
        }
    }
}

/// Mutate the resolved [`ColorGraph`], automatically initializing `TrackItem::color_graph` or
/// `ColorGroup::post_clip` from legacy `"lumetri"` effects on first edit and syncing them back afterwards.
fn edit_graph<R>(
    s: &mut Session,
    p: &Value,
    cmd: &str,
    label: &str,
    merge_key: Option<&str>,
    f: impl FnOnce(&mut ColorGraph, &mut std::collections::BTreeMap<SharedNodeId, SharedColorNode>) -> Result<R>,
) -> Result<R> {
    let scope = resolve_scope(s, p, cmd)?;
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let body = move |pr: &mut filmcraft_project::Project, _: &mut crate::EditorState| -> Result<R> {
        let mut shared = std::mem::take(&mut pr.shared_color_nodes);
        let res = (|| -> Result<R> {
            let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
            match scope {
                ResolvedScope::Timeline => f(&mut q.timeline_color, &mut shared),
                ResolvedScope::PreClip(gid) => {
                    let g = q.color_group_mut(gid).ok_or_else(|| bad(cmd, "unknown color group"))?;
                    f(&mut g.pre_clip, &mut shared)
                }
                ResolvedScope::PostClip(gid) => {
                    let g = q.color_group_mut(gid).ok_or_else(|| bad(cmd, "unknown color group"))?;
                    if g.post_clip.is_empty() && !g.effects.is_empty() {
                        g.post_clip = ColorGraph::from_serial_effects(&g.effects);
                    }
                    let out = f(&mut g.post_clip, &mut shared)?;
                    g.sync_effects_from_post_clip();
                    Ok(out)
                }
                ResolvedScope::Clip(cid) => {
                    let (_, it) = q.find_item_mut(cid).ok_or(filmcraft_edit::EditError::NoItem(cid))?;
                    if it.color_graph.is_none() {
                        it.color_graph = Some(Box::new(ColorGraph::from_serial_effects(&it.effects)));
                    }
                    let Some(cg) = it.color_graph.as_mut() else {
                        return Err(bad(cmd, "missing clip color graph"));
                    };
                    let out = f(cg, &mut shared)?;
                    it.sync_lumetri_effects_from_graph();
                    Ok(out)
                }
            }
        })();
        pr.shared_color_nodes = shared;
        res
    };
    match merge_key {
        Some(k) => s.edit_merged(label, k, body),
        None => s.edit(label, body),
    }
}

fn get_graph(s: &mut Session, p: &Value) -> Result<Value> {
    let scope = resolve_scope(s, p, "color.graph.get")?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let graph: ColorGraph = match scope {
        ResolvedScope::Timeline => seq.timeline_color.clone(),
        ResolvedScope::PreClip(gid) => seq.color_group(gid).ok_or_else(|| bad("color.graph.get", "unknown color group"))?.pre_clip.clone(),
        ResolvedScope::PostClip(gid) => seq.color_group(gid).ok_or_else(|| bad("color.graph.get", "unknown color group"))?.effective_post_clip().into_owned(),
        ResolvedScope::Clip(cid) => {
            let (_, it) = seq.find_item(cid).ok_or(filmcraft_edit::EditError::NoItem(cid))?;
            it.effective_color_graph().into_owned()
        }
    };
    let topo = graph.topo_order().unwrap_or_default();
    Ok(json!({
        "graph": graph,
        "topoOrder": topo,
    }))
}

fn parse_endpoint(v: Option<&Value>, cmd: &str, field: &str) -> Result<ColorEndpoint> {
    let Some(v) = v else {
        return Err(bad(cmd, format!("need `{field}`")));
    };
    if let Some(s) = v.as_str() {
        return match s.to_ascii_lowercase().as_str() {
            "source" | "in" => Ok(ColorEndpoint::Source),
            "output" | "out" => Ok(ColorEndpoint::Output),
            _ => s.parse::<u64>().map(|n| ColorEndpoint::Node(ColorNodeId(n))).map_err(|_| bad(cmd, format!("invalid endpoint `{s}`"))),
        };
    }
    if let Some(n) = v.as_u64() {
        return Ok(ColorEndpoint::Node(ColorNodeId(n)));
    }
    Err(bad(cmd, format!("invalid `{field}` endpoint")))
}

fn parse_port_kind(p: &Value, cmd: &str) -> Result<ColorPortKind> {
    match str_p(p, "kind").unwrap_or("rgb").to_ascii_lowercase().as_str() {
        "rgb" | "color" => Ok(ColorPortKind::Rgb),
        "key" | "alpha" | "matte" => Ok(ColorPortKind::Key),
        other => Err(bad(cmd, format!("unknown port kind `{other}` (expected `rgb` or `key`)"))),
    }
}

fn add_node(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.addNode";
    let kind = str_p(p, "kind").unwrap_or("serial").to_ascii_lowercase();
    let after = u64_p(p, "after").map(ColorNodeId);
    let space = match str_p(p, "space").unwrap_or("rgb").to_ascii_lowercase().as_str() {
        "rgb" => SplitSpace::Rgb,
        "ycbcr" | "yuv" => SplitSpace::YCbCr,
        "hsl" => SplitSpace::Hsl,
        other => return Err(bad(CMD, format!("unknown split space `{other}`"))),
    };
    let shared_req = u64_p(p, "shared").map(SharedNodeId);

    edit_graph(s, p, CMD, "Add Color Node", None, move |g, shared| {
        if g.nodes.is_empty() && matches!(kind.as_str(), "outside" | "parallel" | "layer") {
            let _ = g.add_serial_node(None, default_lumetri_instance()).map_err(|e| bad(CMD, e))?;
        }
        let anchor = after.or_else(|| g.nodes.last().map(|n| n.id));
        match kind.as_str() {
            "serial" | "corrector" => {
                let id = g.add_serial_node(after, default_lumetri_instance()).map_err(|e| bad(CMD, e))?;
                Ok(json!({"node": id.0, "kind": "serial"}))
            }
            "outside" => {
                let from = anchor.ok_or_else(|| bad(CMD, "need a base node for Outside node"))?;
                let id = g.add_outside_node(from, default_lumetri_instance()).map_err(|e| bad(CMD, e))?;
                Ok(json!({"node": id.0, "kind": "outside"}))
            }
            "parallel" => {
                let from = anchor.ok_or_else(|| bad(CMD, "need a base node for Parallel node"))?;
                let (branch, mixer) = g.add_branched_node(from, default_lumetri_instance(), false).map_err(|e| bad(CMD, e))?;
                Ok(json!({"node": branch.0, "mixer": mixer.0, "kind": "parallel"}))
            }
            "layer" => {
                let from = anchor.ok_or_else(|| bad(CMD, "need a base node for Layer Mixer node"))?;
                let (branch, mixer) = g.add_branched_node(from, default_lumetri_instance(), true).map_err(|e| bad(CMD, e))?;
                Ok(json!({"node": branch.0, "mixer": mixer.0, "kind": "layer"}))
            }
            "splitter" | "splitter_combiner" | "splittercombiner" => {
                let (spl, chs, cmb) = g.add_splitter_combiner(after, space).map_err(|e| bad(CMD, e))?;
                Ok(json!({
                    "node": chs[0].0,
                    "splitter": spl.0,
                    "channels": [chs[0].0, chs[1].0, chs[2].0],
                    "combiner": cmb.0,
                    "kind": "splitter"
                }))
            }
            "shared" => {
                let sid = if let Some(sid) = shared_req {
                    if !shared.contains_key(&sid) {
                        return Err(bad(CMD, format!("unknown shared color node {}", sid.0)));
                    }
                    sid
                } else {
                    let next_sid = SharedNodeId(shared.keys().map(|k| k.0).max().unwrap_or(0).saturating_add(1));
                    shared.insert(
                        next_sid,
                        SharedColorNode { id: next_sid, name: format!("Shared {}", next_sid.0), effect: default_lumetri_instance(), locked: false },
                    );
                    next_sid
                };
                let id = g.add_serial_node(after, default_lumetri_instance()).map_err(|e| bad(CMD, e))?;
                if let Some(n) = g.node_mut(id) {
                    n.kind = ColorNodeKind::Shared { shared_id: sid, key_input_gain: 1.0, key_output_gain: 1.0, key_invert: false };
                    n.label = shared.get(&sid).map(|sn| sn.name.clone());
                }
                Ok(json!({"node": id.0, "shared": sid.0, "kind": "shared"}))
            }
            other => Err(bad(CMD, format!("unknown node kind `{other}`"))),
        }
    })
}

fn delete_node(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.deleteNode";
    let id = ColorNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    edit_graph(s, p, CMD, "Delete Color Node", None, move |g, _| {
        g.remove_node(id).map_err(|e| bad(CMD, e))?;
        Ok(json!({"deleted": id.0, "remaining": g.nodes.len()}))
    })
}

fn connect_ports(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.connect";
    let kind = parse_port_kind(p, CMD)?;
    let from = parse_endpoint(p.get("from"), CMD, "from")?;
    let to = parse_endpoint(p.get("to"), CMD, "to")?;
    let from_port = u64_p(p, "fromPort").unwrap_or(0).min(255) as u8;
    let to_port = u64_p(p, "toPort").unwrap_or(0).min(255) as u8;
    let invert_key = bool_p(p, "invertKey").unwrap_or(false) && kind == ColorPortKind::Key;
    edit_graph(s, p, CMD, "Connect Color Nodes", None, move |g, _| {
        g.connect(ColorEdge { kind, from, from_port, to, to_port, invert_key }).map_err(|e| bad(CMD, e))?;
        Ok(json!({"ok": true}))
    })
}

fn disconnect_port(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.disconnect";
    let kind = parse_port_kind(p, CMD)?;
    let to = parse_endpoint(p.get("to"), CMD, "to")?;
    let to_port = u64_p(p, "toPort").unwrap_or(0).min(255) as u8;
    edit_graph(s, p, CMD, "Disconnect Color Nodes", None, move |g, _| {
        let removed = g.disconnect(to, kind, to_port);
        Ok(json!({"removed": removed}))
    })
}

fn move_node(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.moveNode";
    let id = ColorNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    let pos_arr = p.get("pos").and_then(Value::as_array).ok_or_else(|| bad(CMD, "need `pos: [x, y]`"))?;
    let x = pos_arr.first().and_then(Value::as_f64).ok_or_else(|| bad(CMD, "invalid `pos[0]`"))? as f32;
    let y = pos_arr.get(1).and_then(Value::as_f64).ok_or_else(|| bad(CMD, "invalid `pos[1]`"))? as f32;
    if !x.is_finite() || !y.is_finite() {
        return Err(bad(CMD, "node position must be finite"));
    }
    let merge = match p.get("merge") {
        Some(Value::String(k)) => Some(format!("color.graph.move:{k}")),
        _ => bool_p(p, "merge").unwrap_or(false).then(|| format!("color.graph.move:{}", id.0)),
    };
    edit_graph(s, p, CMD, "Move Color Node", merge.as_deref(), move |g, _| {
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown color node {}", id.0)))?;
        n.pos = [x.clamp(-50_000.0, 50_000.0), y.clamp(-50_000.0, 50_000.0)];
        Ok(json!({"node": id.0, "pos": n.pos}))
    })
}

fn set_node_param(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.setNodeParam";
    let id = ColorNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    let param = str_p(p, "param").ok_or_else(|| bad(CMD, "need `param`"))?.to_string();
    let val = p.get("value").cloned().ok_or_else(|| bad(CMD, "need `value`"))?;
    let def = find_effect("lumetri").ok_or_else(|| bad(CMD, "lumetri definition missing"))?;
    let pd = def.param(&param).ok_or_else(|| bad(CMD, format!("unknown lumetri param `{param}`")))?;
    let pv = json_to_param(&pd.default, &val).ok_or_else(|| bad(CMD, format!("invalid value for `{param}`")))?;
    if let (filmcraft_project::ParamKind::Float { min, max, .. }, filmcraft_project::ParamValue::Float(x)) = (&pd.kind, &pv)
        && (!x.is_finite() || *x < *min || *x > *max)
    {
        return Err(bad(CMD, format!("`{param}` must be between {min} and {max}, got {x}")));
    }
    if bool_p(p, "begin").unwrap_or(false) {
        s.history.merge_key = None;
    }
    let merge = match p.get("merge") {
        Some(Value::String(k)) => Some(format!("color.graph.param:{}:{k}", id.0)),
        _ => bool_p(p, "merge").unwrap_or(false).then(|| format!("color.graph.param:{}:{param}", id.0)),
    };
    edit_graph(s, p, CMD, "Adjust Color Node", merge.as_deref(), move |g, shared| {
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown color node {}", id.0)))?;
        match &mut n.kind {
            ColorNodeKind::Corrector { effect, .. } | ColorNodeKind::Outside { effect, .. } => {
                effect.params.entry(param.clone()).or_insert_with(|| Param::new(pv.clone())).value = pv.clone();
            }
            ColorNodeKind::Shared { shared_id, .. } => {
                let sn = shared.get_mut(shared_id).ok_or_else(|| bad(CMD, "missing shared node"))?;
                if sn.locked {
                    return Err(bad(CMD, "shared node is locked"));
                }
                sn.effect.params.entry(param.clone()).or_insert_with(|| Param::new(pv.clone())).value = pv.clone();
            }
            _ => return Err(bad(CMD, "node does not accept color grading parameters")),
        }
        Ok(json!({"node": id.0, "param": param}))
    })
}

fn set_key_params(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.setKeyParams";
    let id = ColorNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    let in_gain = f64_p(p, "inputGain").map(|v| v as f32);
    let out_gain = f64_p(p, "outputGain").map(|v| v as f32);
    let invert = bool_p(p, "invert");
    if in_gain.is_some_and(|v| !v.is_finite()) || out_gain.is_some_and(|v| !v.is_finite()) {
        return Err(bad(CMD, "key gain must be finite"));
    }
    let merge = match p.get("merge") {
        Some(Value::String(k)) => Some(format!("color.graph.key:{}:{k}", id.0)),
        _ => bool_p(p, "merge").unwrap_or(false).then(|| format!("color.graph.key:{}", id.0)),
    };
    edit_graph(s, p, CMD, "Adjust Node Key", merge.as_deref(), move |g, _| {
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown color node {}", id.0)))?;
        if !n.kind.set_key_params(in_gain, out_gain, invert) {
            return Err(bad(CMD, "node does not have key input/output controls"));
        }
        let (ig, og, inv) = n.kind.key_params().unwrap_or((1.0, 1.0, false));
        Ok(json!({"node": id.0, "inputGain": ig, "outputGain": og, "invert": inv}))
    })
}

fn set_layer_mixer(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.setLayerMixer";
    let id = ColorNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    let port = u64_p(p, "port").unwrap_or(1) as usize;
    let blend = match str_p(p, "blend") {
        Some(b) => Some(LayerBlendMode::from_name(b).ok_or_else(|| bad(CMD, format!("unknown blend mode `{b}`")))?),
        None => None,
    };
    let gain = f64_p(p, "gain").map(|v| (v as f32).clamp(0.0, 1.0));
    edit_graph(s, p, CMD, "Configure Layer Mixer", None, move |g, _| {
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown color node {}", id.0)))?;
        let ColorNodeKind::LayerMixer { inputs, blend_modes, gains } = &mut n.kind else {
            return Err(bad(CMD, "target node is not a Layer Mixer"));
        };
        if port >= *inputs {
            return Err(bad(CMD, format!("port {port} out of bounds for Layer Mixer with {inputs} inputs")));
        }
        blend_modes.resize(*inputs, LayerBlendMode::Normal);
        gains.resize(*inputs, 1.0);
        if let Some(bm) = blend {
            blend_modes[port] = bm;
        }
        if let Some(gn) = gain.filter(|v| v.is_finite()) {
            gains[port] = gn;
        }
        Ok(json!({"node": id.0, "port": port}))
    })
}

fn toggle_bypass(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.toggleBypass";
    let id = ColorNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    edit_graph(s, p, CMD, "Toggle Color Node Bypass", None, move |g, _| {
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown color node {}", id.0)))?;
        n.enabled = !n.enabled;
        if let Some(fx) = n.kind.effect_mut() {
            fx.enabled = n.enabled;
        }
        Ok(json!({"node": id.0, "enabled": n.enabled}))
    })
}

fn reset_node(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.resetNode";
    let id = ColorNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    edit_graph(s, p, CMD, "Reset Color Node", None, move |g, shared| {
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown color node {}", id.0)))?;
        match &mut n.kind {
            ColorNodeKind::Corrector { effect, key_input_gain, key_output_gain, key_invert }
            | ColorNodeKind::Outside { effect, key_input_gain, key_output_gain, key_invert } => {
                **effect = default_lumetri_instance();
                *key_input_gain = 1.0;
                *key_output_gain = 1.0;
                *key_invert = false;
            }
            ColorNodeKind::Shared { shared_id, key_input_gain, key_output_gain, key_invert } => {
                if let Some(sn) = shared.get_mut(shared_id)
                    && !sn.locked
                {
                    sn.effect = default_lumetri_instance();
                }
                *key_input_gain = 1.0;
                *key_output_gain = 1.0;
                *key_invert = false;
            }
            _ => {}
        }
        Ok(json!({"node": id.0}))
    })
}

fn share_node(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.shareNode";
    let id = ColorNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    let custom_name = str_p(p, "name").map(str::to_string);
    edit_graph(s, p, CMD, "Save Shared Color Node", None, move |g, shared| {
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown color node {}", id.0)))?;
        let Some(fx) = n.kind.effect().cloned() else {
            return Err(bad(CMD, "only Corrector or Outside nodes can be converted to Shared nodes"));
        };
        let (ig, og, inv) = n.kind.key_params().unwrap_or((1.0, 1.0, false));
        let next_sid = SharedNodeId(shared.keys().map(|k| k.0).max().unwrap_or(0).saturating_add(1));
        let name = custom_name.or_else(|| n.label.clone()).unwrap_or_else(|| format!("Shared {}", next_sid.0));
        shared.insert(next_sid, SharedColorNode { id: next_sid, name: name.clone(), effect: fx, locked: false });
        n.kind = ColorNodeKind::Shared { shared_id: next_sid, key_input_gain: ig, key_output_gain: og, key_invert: inv };
        n.label = Some(name);
        Ok(json!({"node": id.0, "shared": next_sid.0}))
    })
}

fn unlink_shared(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "color.graph.unlinkShared";
    let id = ColorNodeId(u64_p(p, "node").ok_or_else(|| bad(CMD, "need `node`"))?);
    edit_graph(s, p, CMD, "Unlink Shared Color Node", None, move |g, shared| {
        let n = g.node_mut(id).ok_or_else(|| bad(CMD, format!("unknown color node {}", id.0)))?;
        let ColorNodeKind::Shared { shared_id, key_input_gain, key_output_gain, key_invert } = n.kind else {
            return Err(bad(CMD, "node is not a Shared node"));
        };
        let fx = shared.get(&shared_id).map_or_else(default_lumetri_instance, |sn| sn.effect.clone());
        n.kind = ColorNodeKind::Corrector { effect: Box::new(fx), key_input_gain, key_output_gain, key_invert };
        Ok(json!({"node": id.0}))
    })
}
