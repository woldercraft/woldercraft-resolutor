//! Non-linear VFX Compositing Node Graph (`VfxGraph` DAG), modeled after DaVinci Resolve Fusion.
//!
//! Supports `Effect`, `Merge`, `Mask`, `PipeRouter`, and `Instance` nodes with typed/colored ports
//! (`Main` yellow / background, `Foreground` green, `EffectMask` blue, `Output` white), per-node
//! universal `VfxNodeSettings` (animatable `blend`, mask inversion, channel flags), visual
//! `VfxUnderlay` boxes, acyclic cycle detection, topological sorting, and bidirectional sync with
//! `TrackItem::effects`.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use filmcraft_geom::Vec2;
use serde::{Deserialize, Serialize};

use crate::keyframe::{Param, ParamValue};
use crate::mask::Mask;
use crate::{EffectInstance, LayerBlendMode};

/// Maximum nodes permitted in a single [`VfxGraph`] (`Never Crash`).
pub const MAX_VFX_NODES: usize = 128;
/// Maximum directed edges permitted in a single [`VfxGraph`].
pub const MAX_VFX_EDGES: usize = 256;

fn yes() -> bool {
    true
}

fn one_u64() -> u64 {
    1
}

fn default_channels() -> [bool; 4] {
    [true, true, true, true]
}

fn default_blend_param() -> Param {
    Param::new(ParamValue::Float(1.0))
}

fn default_center_param() -> Param {
    Param::new(ParamValue::Vec2(Vec2::new(0.5, 0.5)))
}

fn default_size_param() -> Param {
    Param::new(ParamValue::Float(1.0))
}

fn default_angle_param() -> Param {
    Param::new(ParamValue::Float(0.0))
}

/// Identifier of a node within a [`VfxGraph`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VfxNodeId(pub u64);

/// Signal carried by a port or cable in a [`VfxGraph`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VfxPortKind {
    #[default]
    Image,
    Mask,
}

/// Target input slot on a [`VfxEndpoint`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VfxInputSlot {
    /// Yellow triangle: primary input / Merge Background.
    #[default]
    Main,
    /// Green triangle: Merge Foreground.
    Foreground,
    /// Blue triangle: Effect Mask matte.
    EffectMask,
}

/// Source or destination endpoint of a [`VfxEdge`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum VfxEndpoint {
    MediaIn,
    Node(VfxNodeId),
    MediaOut,
}

/// Compositing operator on a [`VfxNodeKind::Merge`] node (Fusion-style).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeOperator {
    #[default]
    Over,
    In,
    Out,
    Atop,
    Xor,
}

impl MergeOperator {
    pub const ALL: [MergeOperator; 5] = [Self::Over, Self::In, Self::Out, Self::Atop, Self::Xor];

    pub fn label(self) -> &'static str {
        match self {
            Self::Over => "Over",
            Self::In => "In",
            Self::Out => "Out",
            Self::Atop => "Atop",
            Self::Xor => "Xor",
        }
    }
}

/// Universal Fusion-style Settings tab attached to every node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VfxNodeSettings {
    /// Master effect mix `0.0..=1.0` (`lerp(main_input, node_output, blend)`). Animatable.
    #[serde(default = "default_blend_param")]
    pub blend: Param,
    /// Invert incoming blue `EffectMask` matte (`1.0 - alpha`).
    #[serde(default)]
    pub invert_mask: bool,
    /// Multiply output alpha by mask matte when applying `EffectMask`.
    #[serde(default)]
    pub multiply_by_mask: bool,
    /// Active RGBA channels `[R, G, B, A]`.
    #[serde(default = "default_channels")]
    pub channels: [bool; 4],
}

impl Default for VfxNodeSettings {
    fn default() -> Self {
        Self { blend: default_blend_param(), invert_mask: false, multiply_by_mask: false, channels: default_channels() }
    }
}

/// Specialized kind of a [`VfxNode`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum VfxNodeKind {
    Effect(EffectInstance),
    Merge {
        #[serde(default)]
        blend_mode: LayerBlendMode,
        #[serde(default)]
        operator: MergeOperator,
        #[serde(default = "default_center_param")]
        center: Param,
        #[serde(default = "default_size_param")]
        size: Param,
        #[serde(default = "default_angle_param")]
        angle: Param,
    },
    Mask(Mask),
    PipeRouter,
    Instance {
        source: VfxNodeId,
        #[serde(default)]
        overrides: BTreeMap<String, Param>,
    },
}

/// A node in a [`VfxGraph`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VfxNode {
    pub id: VfxNodeId,
    pub kind: VfxNodeKind,
    /// When `false`, node operates in Pass-Through mode (`Ctrl+P`).
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub settings: VfxNodeSettings,
    #[serde(default)]
    pub pos: [f32; 2],
}

/// A directed connection in a [`VfxGraph`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VfxEdge {
    pub from: VfxEndpoint,
    pub to: VfxEndpoint,
    #[serde(default)]
    pub to_slot: VfxInputSlot,
    #[serde(default)]
    pub kind: VfxPortKind,
}

/// A colored grouping rectangle drawn behind nodes (`Ctrl+Shift+U` in Fusion).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VfxUnderlay {
    pub id: u64,
    pub title: String,
    pub color: [u8; 3],
    pub rect: [f32; 4],
}

/// Directed acyclic compositing graph for clip-level VFX.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VfxGraph {
    #[serde(default)]
    pub nodes: Vec<VfxNode>,
    #[serde(default)]
    pub edges: Vec<VfxEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub underlays: Vec<VfxUnderlay>,
    #[serde(default = "one_u64")]
    pub next_node_id: u64,
}

impl Default for VfxGraph {
    fn default() -> Self {
        Self { nodes: Vec::new(), edges: Vec::new(), underlays: Vec::new(), next_node_id: 1 }
    }
}

impl VfxGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn has_enabled_nodes(&self) -> bool {
        self.nodes.iter().any(|n| n.enabled)
    }

    /// True when the graph uses non-linear topology (`Merge`, `Mask`, `PipeRouter`, `Instance`),
    /// branches, shared masks, or custom `Settings` that the legacy flat GPU chain cannot represent.
    pub fn requires_dag_eval(&self) -> bool {
        if self.nodes.iter().any(|n| {
            !matches!(n.kind, VfxNodeKind::Effect(_))
                || n.settings.invert_mask
                || n.settings.multiply_by_mask
                || n.settings.channels != [true, true, true, true]
                || n.settings.blend.is_animated()
                || (n.settings.blend.value.as_f64().unwrap_or(1.0) - 1.0).abs() > 1e-4
        }) {
            return true;
        }
        // Check if any edge targets Foreground/EffectMask or branches away from a single linear chain.
        self.edges.iter().any(|e| e.to_slot != VfxInputSlot::Main)
    }

    pub fn node(&self, id: VfxNodeId) -> Option<&VfxNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: VfxNodeId) -> Option<&mut VfxNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn alloc_id(&mut self) -> VfxNodeId {
        let id = VfxNodeId(self.next_node_id.max(1));
        self.next_node_id = id.0.saturating_add(1);
        id
    }

    /// Build a linear `[MediaIn] -> [N1] -> ... -> [Nk] -> [MediaOut]` graph from serial non-intrinsic
    /// video effects (excluding `"lumetri"` and graphic layers).
    pub fn from_serial_effects(effects: &[EffectInstance]) -> Self {
        let mut g = Self::new();
        let vfx: Vec<&EffectInstance> = effects
            .iter()
            .filter(|e| {
                e.effect != "lumetri"
                    && !crate::graphic::is_layer(e)
                    && e.def().is_some_and(|d| !d.intrinsic && d.kind == crate::EffectKind::Video)
            })
            .collect();
        if vfx.is_empty() {
            return g;
        }
        let mut prev = VfxEndpoint::MediaIn;
        for (i, e) in vfx.into_iter().enumerate() {
            let id = g.alloc_id();
            g.nodes.push(VfxNode {
                id,
                kind: VfxNodeKind::Effect(e.clone()),
                enabled: e.enabled,
                label: e.label.clone(),
                settings: VfxNodeSettings::default(),
                pos: [120.0 + i as f32 * 204.0, 120.0],
            });
            let ep = VfxEndpoint::Node(id);
            g.edges.push(VfxEdge { from: prev, to: ep, to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image });
            prev = ep;
        }
        g.edges.push(VfxEdge { from: prev, to: VfxEndpoint::MediaOut, to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image });
        g
    }

    /// Check whether adding `from -> to` would introduce a cycle.
    pub fn would_create_cycle(&self, from: VfxEndpoint, to: VfxEndpoint) -> bool {
        if from == to {
            return true;
        }
        let (VfxEndpoint::Node(from_id), VfxEndpoint::Node(to_id)) = (from, to) else {
            return matches!((from, to), (VfxEndpoint::MediaOut, _) | (_, VfxEndpoint::MediaIn));
        };
        // Reachability from `to_id` following outgoing edges: if `from_id` is reachable, adding `from -> to` closes a cycle.
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        seen.insert(to_id);
        queue.push_back(to_id);
        while let Some(cur) = queue.pop_front() {
            if cur == from_id {
                return true;
            }
            for e in &self.edges {
                if e.from == VfxEndpoint::Node(cur)
                    && let VfxEndpoint::Node(next_id) = e.to
                    && seen.insert(next_id)
                {
                    queue.push_back(next_id);
                }
            }
        }
        false
    }

    /// Validate and insert/replace a connection targeting `(edge.to, edge.to_slot)`.
    pub fn connect(&mut self, edge: VfxEdge) -> Result<(), &'static str> {
        if self.edges.len() >= MAX_VFX_EDGES {
            return Err("maximum VFX edges exceeded");
        }
        if let VfxEndpoint::Node(id) = edge.from
            && self.node(id).is_none()
        {
            return Err("source node does not exist");
        }
        if let VfxEndpoint::Node(id) = edge.to
            && self.node(id).is_none()
        {
            return Err("destination node does not exist");
        }
        match edge.to_slot {
            VfxInputSlot::EffectMask => {
                // EffectMask accepts Mask output or Image luma.
                if let VfxEndpoint::Node(id) = edge.to
                    && matches!(self.node(id).map(|n| &n.kind), Some(VfxNodeKind::Mask(_) | VfxNodeKind::PipeRouter))
                {
                    return Err("node does not accept an effect mask");
                }
            }
            VfxInputSlot::Foreground => {
                if let VfxEndpoint::Node(id) = edge.to
                    && !matches!(self.node(id).map(|n| &n.kind), Some(VfxNodeKind::Merge { .. }))
                {
                    return Err("only Merge nodes have a foreground input");
                }
                if edge.kind == VfxPortKind::Mask {
                    return Err("foreground input requires an image output");
                }
            }
            VfxInputSlot::Main => {
                if edge.kind == VfxPortKind::Mask
                    && !matches!(
                        edge.to,
                        VfxEndpoint::Node(id) if matches!(self.node(id).map(|n| &n.kind), Some(VfxNodeKind::PipeRouter))
                    )
                {
                    return Err("main input requires an image output");
                }
            }
        }
        if self.would_create_cycle(edge.from, edge.to) {
            return Err("connection would create a cycle");
        }
        // Each input slot holds a single incoming wire; replace any previous one.
        self.edges.retain(|e| !(e.to == edge.to && e.to_slot == edge.to_slot));
        self.edges.push(edge);
        Ok(())
    }

    pub fn disconnect(&mut self, to: VfxEndpoint, to_slot: VfxInputSlot) {
        self.edges.retain(|e| !(e.to == to && e.to_slot == to_slot));
    }

    /// Splice `node_id` onto an existing edge `from -> to:to_slot` (`Shift+Drag` onto wire).
    pub fn insert_on_edge(&mut self, node_id: VfxNodeId, from: VfxEndpoint, to: VfxEndpoint, to_slot: VfxInputSlot) -> Result<(), &'static str> {
        let Some(node) = self.node(node_id) else {
            return Err("node does not exist");
        };
        if matches!(node.kind, VfxNodeKind::Mask(_)) {
            return Err("mask nodes cannot be spliced into an image pipe");
        }
        if !self.edges.iter().any(|e| e.from == from && e.to == to && e.to_slot == to_slot) {
            return Err("target edge does not exist");
        }
        if from == VfxEndpoint::Node(node_id) || to == VfxEndpoint::Node(node_id) {
            return Err("node is already an endpoint of that edge");
        }
        // First cleanly extract `node_id` from wherever it was connected so we don't leave a broken chain.
        self.extract_node(node_id);
        self.disconnect(to, to_slot);
        let ep = VfxEndpoint::Node(node_id);
        self.connect(VfxEdge { from, to: ep, to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image })?;
        self.connect(VfxEdge { from: ep, to, to_slot, kind: VfxPortKind::Image })?;
        Ok(())
    }

    /// Unhook `node_id` cleanly from the pipe, bridging its `Main` source to all downstream consumers.
    pub fn extract_node(&mut self, node_id: VfxNodeId) {
        let ep = VfxEndpoint::Node(node_id);
        let main_src = self.edges.iter().find(|e| e.to == ep && e.to_slot == VfxInputSlot::Main).map(|e| (e.from, e.kind));
        let outgoing: Vec<VfxEdge> = self.edges.iter().filter(|e| e.from == ep).copied().collect();
        self.edges.retain(|e| e.from != ep && e.to != ep);
        if let Some((src, kind)) = main_src {
            for out in outgoing {
                let _ = self.connect(VfxEdge { from: src, to: out.to, to_slot: out.to_slot, kind });
            }
        }
    }

    /// Swap `Main` (Background) and `Foreground` wires on a [`VfxNodeKind::Merge`] node (`Ctrl+T`).
    pub fn swap_merge_inputs(&mut self, node_id: VfxNodeId) -> Result<(), &'static str> {
        let Some(n) = self.node(node_id) else {
            return Err("node does not exist");
        };
        if !matches!(n.kind, VfxNodeKind::Merge { .. }) {
            return Err("only Merge nodes can swap Background/Foreground inputs");
        }
        let ep = VfxEndpoint::Node(node_id);
        for e in &mut self.edges {
            if e.to == ep {
                match e.to_slot {
                    VfxInputSlot::Main => e.to_slot = VfxInputSlot::Foreground,
                    VfxInputSlot::Foreground => e.to_slot = VfxInputSlot::Main,
                    VfxInputSlot::EffectMask => {}
                }
            }
        }
        Ok(())
    }

    /// Kahn's topological order over all nodes in the graph (`None` if cyclic).
    pub fn topo_order(&self) -> Option<Vec<VfxNodeId>> {
        let mut indegree: HashMap<VfxNodeId, usize> = self.nodes.iter().map(|n| (n.id, 0usize)).collect();
        let mut adj: HashMap<VfxNodeId, Vec<VfxNodeId>> = HashMap::new();
        for e in &self.edges {
            if let (VfxEndpoint::Node(u), VfxEndpoint::Node(v)) = (e.from, e.to)
                && u != v
            {
                *indegree.entry(v).or_insert(0) += 1;
                adj.entry(u).or_default().push(v);
            }
        }
        let mut q: VecDeque<VfxNodeId> = self.nodes.iter().filter(|n| indegree.get(&n.id).copied().unwrap_or(0) == 0).map(|n| n.id).collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(u) = q.pop_front() {
            order.push(u);
            if let Some(vs) = adj.get(&u) {
                for &v in vs {
                    if let Some(deg) = indegree.get_mut(&v) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            q.push_back(v);
                        }
                    }
                }
            }
        }
        if order.len() == self.nodes.len() { Some(order) } else { None }
    }

    /// Compute clean non-overlapping left-to-right topological positions for all nodes.
    pub fn auto_layout(&mut self) {
        let order = self.topo_order().unwrap_or_else(|| self.nodes.iter().map(|n| n.id).collect());
        let mut depth_map: HashMap<VfxNodeId, usize> = HashMap::new();
        for &id in &order {
            let mut d = 0usize;
            for e in &self.edges {
                if e.to == VfxEndpoint::Node(id)
                    && let VfxEndpoint::Node(src) = e.from
                {
                    d = d.max(depth_map.get(&src).copied().unwrap_or(0) + 1);
                }
            }
            depth_map.insert(id, d);
        }
        let mut col_counts: HashMap<usize, usize> = HashMap::new();
        for &id in &order {
            let col = depth_map.get(&id).copied().unwrap_or(0);
            let row = *col_counts.get(&col).unwrap_or(&0);
            col_counts.insert(col, row + 1);
            if let Some(n) = self.node_mut(id) {
                let y_off = if matches!(n.kind, VfxNodeKind::Mask(_)) { -110.0 } else { 0.0 };
                n.pos = [120.0 + col as f32 * 204.0, 120.0 + row as f32 * 116.0 + y_off];
            }
        }
    }

    /// Resolve an [`EffectInstance`] for a node (resolving [`VfxNodeKind::Instance`] overrides).
    pub fn resolve_effect(&self, id: VfxNodeId) -> Option<EffectInstance> {
        let node = self.node(id)?;
        match &node.kind {
            VfxNodeKind::Effect(inst) => {
                let mut out = inst.clone();
                out.enabled = node.enabled;
                if node.label.is_some() {
                    out.label = node.label.clone();
                }
                Some(out)
            }
            VfxNodeKind::Instance { source, overrides } => {
                let mut base = self.resolve_effect_non_recursive(*source)?;
                for (k, v) in overrides {
                    base.params.insert(k.clone(), v.clone());
                }
                base.enabled = node.enabled;
                if node.label.is_some() {
                    base.label = node.label.clone();
                }
                Some(base)
            }
            _ => None,
        }
    }

    fn resolve_effect_non_recursive(&self, id: VfxNodeId) -> Option<EffectInstance> {
        let node = self.node(id)?;
        if let VfxNodeKind::Effect(inst) = &node.kind {
            Some(inst.clone())
        } else {
            None
        }
    }

    /// Extract serial [`EffectInstance`]s in topological order for syncing back to `TrackItem::effects`.
    pub fn serial_effects(&self) -> Vec<EffectInstance> {
        let order = self.topo_order().unwrap_or_else(|| self.nodes.iter().map(|n| n.id).collect());
        order.into_iter().filter_map(|id| self.resolve_effect(id)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find_effect;

    fn fx(id: &str) -> EffectInstance {
        find_effect(id).expect("effect must exist").instance()
    }

    #[test]
    fn builds_serial_chain_and_preserves_order() {
        let g = VfxGraph::from_serial_effects(&[fx("ec.blur.gaussian"), fx("ec.stylize.glow")]);
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 3);
        let order = g.topo_order().expect("acyclic");
        assert_eq!(order, vec![VfxNodeId(1), VfxNodeId(2)]);
        let serial = g.serial_effects();
        assert_eq!(serial.len(), 2);
        assert_eq!(serial[0].effect, "ec.blur.gaussian");
        assert_eq!(serial[1].effect, "ec.stylize.glow");
    }

    #[test]
    fn rejects_cycles_and_incompatible_ports() {
        let mut g = VfxGraph::from_serial_effects(&[fx("ec.blur.gaussian"), fx("ec.stylize.glow")]);
        // Closing loop N2 -> N1 must fail
        assert!(g.connect(VfxEdge { from: VfxEndpoint::Node(VfxNodeId(2)), to: VfxEndpoint::Node(VfxNodeId(1)), to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).is_err());
        // Foreground slot on non-Merge node must fail
        assert!(g.connect(VfxEdge { from: VfxEndpoint::MediaIn, to: VfxEndpoint::Node(VfxNodeId(1)), to_slot: VfxInputSlot::Foreground, kind: VfxPortKind::Image }).is_err());
    }

    #[test]
    fn insert_on_edge_extract_and_swap_merge_inputs() {
        let mut g = VfxGraph::from_serial_effects(&[fx("ec.blur.gaussian")]);
        let glow_id = g.alloc_id();
        g.nodes.push(VfxNode {
            id: glow_id,
            kind: VfxNodeKind::Effect(fx("ec.stylize.glow")),
            enabled: true,
            label: None,
            settings: VfxNodeSettings::default(),
            pos: [0.0, 0.0],
        });
        g.insert_on_edge(glow_id, VfxEndpoint::MediaIn, VfxEndpoint::Node(VfxNodeId(1)), VfxInputSlot::Main).unwrap();
        let serial = g.serial_effects();
        assert_eq!(serial[0].effect, "ec.stylize.glow");
        assert_eq!(serial[1].effect, "ec.blur.gaussian");

        g.extract_node(glow_id);
        assert!(g.edges.iter().any(|e| e.from == VfxEndpoint::MediaIn && e.to == VfxEndpoint::Node(VfxNodeId(1))));

        // Merge input swapping
        let merge_id = g.alloc_id();
        g.nodes.push(VfxNode {
            id: merge_id,
            kind: VfxNodeKind::Merge {
                blend_mode: LayerBlendMode::Normal,
                operator: MergeOperator::Over,
                center: default_center_param(),
                size: default_size_param(),
                angle: default_angle_param(),
            },
            enabled: true,
            label: None,
            settings: VfxNodeSettings::default(),
            pos: [300.0, 120.0],
        });
        g.connect(VfxEdge { from: VfxEndpoint::MediaIn, to: VfxEndpoint::Node(merge_id), to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).unwrap();
        g.connect(VfxEdge { from: VfxEndpoint::Node(VfxNodeId(1)), to: VfxEndpoint::Node(merge_id), to_slot: VfxInputSlot::Foreground, kind: VfxPortKind::Image }).unwrap();
        g.swap_merge_inputs(merge_id).unwrap();
        assert!(g.edges.iter().any(|e| e.from == VfxEndpoint::MediaIn && e.to == VfxEndpoint::Node(merge_id) && e.to_slot == VfxInputSlot::Foreground));
        assert!(g.edges.iter().any(|e| e.from == VfxEndpoint::Node(VfxNodeId(1)) && e.to == VfxEndpoint::Node(merge_id) && e.to_slot == VfxInputSlot::Main));
    }
}
