//! Non-linear Color Grading Node Graph (`ColorGraph` DAG).
//!
//! Models DaVinci Resolve-style node graphs with separate `Rgb` and `Key` (matte/alpha) ports,
//! `Corrector`, `Outside`, `ParallelMixer`, `LayerMixer`, `Splitter`/`Combiner` and `Shared` nodes,
//! attached at four pipeline scopes:
//! 1. `Group Pre-Clip` (`ColorGroup::pre_clip`)
//! 2. `Clip` (`TrackItem::color_graph`)
//! 3. `Group Post-Clip` (`ColorGroup::post_clip`)
//! 4. `Timeline` (`Sequence::timeline_color`)
//!
//! Every graph mutation validates acyclic topological ordering via Kahn's algorithm and enforces
//! bounded node/edge counts (`Never Crash`).

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{EffectInstance, find_effect};

/// Maximum nodes permitted in a single [`ColorGraph`].
pub const MAX_COLOR_NODES: usize = 128;
/// Maximum directed edges permitted in a single [`ColorGraph`].
pub const MAX_COLOR_EDGES: usize = 512;
/// Maximum input branches on a [`ColorNodeKind::ParallelMixer`] or [`ColorNodeKind::LayerMixer`].
pub const MAX_MIXER_INPUTS: usize = 8;

fn yes() -> bool {
    true
}
fn one_f32() -> f32 {
    1.0
}
fn one_u64() -> u64 {
    1
}
fn two_inputs() -> usize {
    2
}

/// Identifier of a node within a [`ColorGraph`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColorNodeId(pub u64);

/// Identifier of a project-wide shared color node in [`crate::Project::shared_color_nodes`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SharedNodeId(pub u64);

/// Pipeline level where a [`ColorGraph`] is evaluated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorScopeKind {
    PreClip,
    #[default]
    Clip,
    PostClip,
    Timeline,
}

impl ColorScopeKind {
    pub const ALL: [ColorScopeKind; 4] = [ColorScopeKind::PreClip, ColorScopeKind::Clip, ColorScopeKind::PostClip, ColorScopeKind::Timeline];

    pub fn id(self) -> &'static str {
        match self {
            ColorScopeKind::PreClip => "pre_clip",
            ColorScopeKind::Clip => "clip",
            ColorScopeKind::PostClip => "post_clip",
            ColorScopeKind::Timeline => "timeline",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "pre_clip" | "preClip" | "pre" => Some(ColorScopeKind::PreClip),
            "clip" => Some(ColorScopeKind::Clip),
            "post_clip" | "postClip" | "post" | "group" => Some(ColorScopeKind::PostClip),
            "timeline" | "sequence" => Some(ColorScopeKind::Timeline),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ColorScopeKind::PreClip => "Pre-Clip",
            ColorScopeKind::Clip => "Clip",
            ColorScopeKind::PostClip => "Post-Clip",
            ColorScopeKind::Timeline => "Timeline",
        }
    }
}

/// Type of signal carried by a port and cable in a [`ColorGraph`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorPortKind {
    /// Color image stream (green connectors in Resolve).
    Rgb,
    /// Alpha / Qualifier / Power Window matte stream in `0.0..=1.0` (blue connectors in Resolve).
    Key,
}

/// Source or destination endpoint of a [`ColorEdge`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorEndpoint {
    /// Graph input (`IN`).
    Source,
    /// An internal graph node.
    Node(ColorNodeId),
    /// Graph output (`OUT`).
    Output,
}

/// Channel decomposition space for [`ColorNodeKind::Splitter`] and [`ColorNodeKind::Combiner`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SplitSpace {
    #[default]
    Rgb,
    YCbCr,
    Hsl,
}

impl SplitSpace {
    pub fn channel_labels(self) -> [&'static str; 3] {
        match self {
            SplitSpace::Rgb => ["Red", "Green", "Blue"],
            SplitSpace::YCbCr => ["Y", "Cb", "Cr"],
            SplitSpace::Hsl => ["Hue", "Sat", "Luma"],
        }
    }
}

/// Composite blend mode used by [`ColorNodeKind::LayerMixer`] per input branch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LayerBlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
    HardLight,
    Add,
    Subtract,
    Darken,
    Lighten,
    Color,
    Luminosity,
}

impl LayerBlendMode {
    pub const ALL: [LayerBlendMode; 12] = [
        LayerBlendMode::Normal,
        LayerBlendMode::Multiply,
        LayerBlendMode::Screen,
        LayerBlendMode::Overlay,
        LayerBlendMode::SoftLight,
        LayerBlendMode::HardLight,
        LayerBlendMode::Add,
        LayerBlendMode::Subtract,
        LayerBlendMode::Darken,
        LayerBlendMode::Lighten,
        LayerBlendMode::Color,
        LayerBlendMode::Luminosity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayerBlendMode::Normal => "Normal",
            LayerBlendMode::Multiply => "Multiply",
            LayerBlendMode::Screen => "Screen",
            LayerBlendMode::Overlay => "Overlay",
            LayerBlendMode::SoftLight => "Soft Light",
            LayerBlendMode::HardLight => "Hard Light",
            LayerBlendMode::Add => "Add",
            LayerBlendMode::Subtract => "Subtract",
            LayerBlendMode::Darken => "Darken",
            LayerBlendMode::Lighten => "Lighten",
            LayerBlendMode::Color => "Color",
            LayerBlendMode::Luminosity => "Luminosity",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "normal" => Some(LayerBlendMode::Normal),
            "multiply" => Some(LayerBlendMode::Multiply),
            "screen" => Some(LayerBlendMode::Screen),
            "overlay" => Some(LayerBlendMode::Overlay),
            "softlight" => Some(LayerBlendMode::SoftLight),
            "hardlight" => Some(LayerBlendMode::HardLight),
            "add" | "linearDodge" => Some(LayerBlendMode::Add),
            "subtract" => Some(LayerBlendMode::Subtract),
            "darken" => Some(LayerBlendMode::Darken),
            "lighten" => Some(LayerBlendMode::Lighten),
            "color" => Some(LayerBlendMode::Color),
            "luminosity" | "luma" => Some(LayerBlendMode::Luminosity),
            _ => None,
        }
    }
}

/// Functional kind of a [`ColorNode`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ColorNodeKind {
    /// Standard color grading corrector node with all Lumetri / Resolve palettes, Power Windows,
    /// Qualifier, RGB input/output and Key input/output.
    Corrector {
        effect: Box<EffectInstance>,
        #[serde(default = "one_f32")]
        key_input_gain: f32,
        #[serde(default = "one_f32")]
        key_output_gain: f32,
        #[serde(default)]
        key_invert: bool,
    },
    /// Outside corrector node: grades the inverse of the upstream node's key matte.
    Outside {
        effect: Box<EffectInstance>,
        #[serde(default = "one_f32")]
        key_input_gain: f32,
        #[serde(default = "one_f32")]
        key_output_gain: f32,
        #[serde(default)]
        key_invert: bool,
    },
    /// Parallel mixer combining `inputs` branches simultaneously in uniform balance.
    ParallelMixer {
        #[serde(default = "two_inputs")]
        inputs: usize,
    },
    /// Layer mixer compositing `inputs` branches in stack order (higher index over lower index)
    /// using each branch's Key matte and blend mode.
    LayerMixer {
        #[serde(default = "two_inputs")]
        inputs: usize,
        #[serde(default)]
        blend_modes: Vec<LayerBlendMode>,
        #[serde(default)]
        gains: Vec<f32>,
    },
    /// Splits an RGB input into 3 monochrome channel outputs (`from_port = 0, 1, 2`).
    Splitter {
        #[serde(default)]
        space: SplitSpace,
    },
    /// Recombines 3 channel inputs (`to_port = 0, 1, 2`) back into an RGB stream.
    Combiner {
        #[serde(default)]
        space: SplitSpace,
    },
    /// Reference to a project-wide [`SharedColorNode`].
    Shared {
        shared_id: SharedNodeId,
        #[serde(default = "one_f32")]
        key_input_gain: f32,
        #[serde(default = "one_f32")]
        key_output_gain: f32,
        #[serde(default)]
        key_invert: bool,
    },
}

impl ColorNodeKind {
    pub fn default_corrector() -> Self {
        let effect = default_lumetri_instance();
        ColorNodeKind::Corrector { effect: Box::new(effect), key_input_gain: 1.0, key_output_gain: 1.0, key_invert: false }
    }

    pub fn rgb_input_ports(&self) -> u8 {
        match self {
            ColorNodeKind::Corrector { .. } | ColorNodeKind::Outside { .. } | ColorNodeKind::Splitter { .. } | ColorNodeKind::Shared { .. } => 1,
            ColorNodeKind::ParallelMixer { inputs } | ColorNodeKind::LayerMixer { inputs, .. } => (*inputs).clamp(2, MAX_MIXER_INPUTS) as u8,
            ColorNodeKind::Combiner { .. } => 3,
        }
    }

    pub fn rgb_output_ports(&self) -> u8 {
        match self {
            ColorNodeKind::Splitter { .. } => 3,
            _ => 1,
        }
    }

    pub fn key_input_ports(&self) -> u8 {
        match self {
            ColorNodeKind::Corrector { .. } | ColorNodeKind::Outside { .. } | ColorNodeKind::Shared { .. } => 1,
            ColorNodeKind::LayerMixer { inputs, .. } => (*inputs).clamp(2, MAX_MIXER_INPUTS) as u8,
            ColorNodeKind::ParallelMixer { .. } | ColorNodeKind::Splitter { .. } | ColorNodeKind::Combiner { .. } => 0,
        }
    }

    pub fn key_output_ports(&self) -> u8 {
        match self {
            ColorNodeKind::Corrector { .. }
            | ColorNodeKind::Outside { .. }
            | ColorNodeKind::Shared { .. }
            | ColorNodeKind::ParallelMixer { .. }
            | ColorNodeKind::LayerMixer { .. } => 1,
            ColorNodeKind::Splitter { .. } | ColorNodeKind::Combiner { .. } => 0,
        }
    }

    pub fn effect(&self) -> Option<&EffectInstance> {
        match self {
            ColorNodeKind::Corrector { effect, .. } | ColorNodeKind::Outside { effect, .. } => Some(effect),
            _ => None,
        }
    }

    pub fn effect_mut(&mut self) -> Option<&mut EffectInstance> {
        match self {
            ColorNodeKind::Corrector { effect, .. } | ColorNodeKind::Outside { effect, .. } => Some(effect),
            _ => None,
        }
    }

    pub fn key_params(&self) -> Option<(f32, f32, bool)> {
        match self {
            ColorNodeKind::Corrector { key_input_gain, key_output_gain, key_invert, .. }
            | ColorNodeKind::Outside { key_input_gain, key_output_gain, key_invert, .. }
            | ColorNodeKind::Shared { key_input_gain, key_output_gain, key_invert, .. } => Some((*key_input_gain, *key_output_gain, *key_invert)),
            _ => None,
        }
    }

    pub fn set_key_params(&mut self, in_gain: Option<f32>, out_gain: Option<f32>, invert: Option<bool>) -> bool {
        match self {
            ColorNodeKind::Corrector { key_input_gain, key_output_gain, key_invert, .. }
            | ColorNodeKind::Outside { key_input_gain, key_output_gain, key_invert, .. }
            | ColorNodeKind::Shared { key_input_gain, key_output_gain, key_invert, .. } => {
                if let Some(g) = in_gain.filter(|v| v.is_finite()) {
                    *key_input_gain = g.clamp(0.0, 2.0);
                }
                if let Some(g) = out_gain.filter(|v| v.is_finite()) {
                    *key_output_gain = g.clamp(0.0, 1.0);
                }
                if let Some(inv) = invert {
                    *key_invert = inv;
                }
                true
            }
            _ => false,
        }
    }

    pub fn type_badge(&self) -> &'static str {
        match self {
            ColorNodeKind::Corrector { .. } => "COR",
            ColorNodeKind::Outside { .. } => "OUT",
            ColorNodeKind::ParallelMixer { .. } => "PAR",
            ColorNodeKind::LayerMixer { .. } => "LAY",
            ColorNodeKind::Splitter { .. } => "SPL",
            ColorNodeKind::Combiner { .. } => "CMB",
            ColorNodeKind::Shared { .. } => "SHR",
        }
    }
}

/// A node in a [`ColorGraph`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorNode {
    pub id: ColorNodeId,
    pub kind: ColorNodeKind,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Canvas position `[x, y]` in unscaled graph coordinates.
    #[serde(default)]
    pub pos: [f32; 2],
}

/// A directed connection between two ports of the same [`ColorPortKind`] in a [`ColorGraph`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ColorEdge {
    pub kind: ColorPortKind,
    pub from: ColorEndpoint,
    #[serde(default)]
    pub from_port: u8,
    pub to: ColorEndpoint,
    #[serde(default)]
    pub to_port: u8,
    /// When `true` on a `Key` edge, the transferred matte is inverted (`1.0 - key`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invert_key: bool,
}

/// A project-wide shared color node definition stored in [`crate::Project::shared_color_nodes`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedColorNode {
    pub id: SharedNodeId,
    pub name: String,
    pub effect: EffectInstance,
    #[serde(default)]
    pub locked: bool,
}

/// Directed acyclic graph of color grading nodes for a clip, group (`Pre-Clip`/`Post-Clip`), or sequence (`Timeline`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorGraph {
    #[serde(default)]
    pub nodes: Vec<ColorNode>,
    #[serde(default)]
    pub edges: Vec<ColorEdge>,
    #[serde(default = "one_u64")]
    pub next_node_id: u64,
}

impl Default for ColorGraph {
    fn default() -> Self {
        Self { nodes: Vec::new(), edges: Vec::new(), next_node_id: 1 }
    }
}

pub fn default_lumetri_instance() -> EffectInstance {
    find_effect("lumetri").map(|d| d.instance()).unwrap_or_else(|| EffectInstance {
        effect: "lumetri".into(),
        enabled: true,
        params: BTreeMap::new(),
        masks: Vec::new(),
        post_fader: false,
        essential: false,
        layer: None,
        label: None,
    })
}

impl ColorGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn has_enabled_nodes(&self) -> bool {
        self.nodes.iter().any(|n| n.enabled)
    }

    pub fn node(&self, id: ColorNodeId) -> Option<&ColorNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: ColorNodeId) -> Option<&mut ColorNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    fn alloc_node_id(&mut self) -> ColorNodeId {
        let max_used = self.nodes.iter().map(|n| n.id.0).max().unwrap_or(0);
        if self.next_node_id <= max_used {
            self.next_node_id = max_used.saturating_add(1);
        }
        let id = ColorNodeId(self.next_node_id);
        self.next_node_id = self.next_node_id.saturating_add(1);
        id
    }

    /// Construct a serial `Source -> Node 1 -> ... -> Node N -> Output` graph from `"lumetri"` effects.
    pub fn from_serial_effects(effects: &[EffectInstance]) -> Self {
        let mut g = Self::new();
        let mut prev = ColorEndpoint::Source;
        for (idx, fx) in effects.iter().filter(|e| e.effect == "lumetri").enumerate() {
            if g.nodes.len() >= MAX_COLOR_NODES {
                break;
            }
            let id = g.alloc_node_id();
            let col = idx % 4;
            let row = idx / 4;
            let pos = [110.0 + col as f32 * 192.0, 40.0 + row as f32 * 114.0];
            g.nodes.push(ColorNode {
                id,
                kind: ColorNodeKind::Corrector { effect: Box::new(fx.clone()), key_input_gain: 1.0, key_output_gain: 1.0, key_invert: false },
                enabled: fx.enabled,
                label: fx.label.clone(),
                pos,
            });
            g.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: prev, from_port: 0, to: ColorEndpoint::Node(id), to_port: 0, invert_key: false });
            prev = ColorEndpoint::Node(id);
        }
        if prev != ColorEndpoint::Source {
            g.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: prev, from_port: 0, to: ColorEndpoint::Output, to_port: 0, invert_key: false });
        }
        g
    }

    /// Extract the ordered `"lumetri"` effects of `Corrector` and `Outside` nodes in topological order
    /// (for syncing back to legacy `effects: Vec<EffectInstance>` inspection).
    pub fn corrector_effects(&self) -> Vec<EffectInstance> {
        let order = self.topo_order().unwrap_or_else(|_| self.nodes.iter().map(|n| n.id).collect());
        let mut out = Vec::new();
        for id in order {
            if let Some(n) = self.node(id)
                && let Some(fx) = n.kind.effect()
            {
                let mut cloned = fx.clone();
                cloned.enabled = n.enabled;
                cloned.label = n.label.clone();
                out.push(cloned);
            }
        }
        out
    }

    /// If this graph is a pure serial RGB chain of `Corrector` (or `Shared`) nodes without Key cables,
    /// `Outside`, `ParallelMixer`, `LayerMixer`, or `Splitter`/`Combiner`, returns the ordered
    /// `EffectInstance` list so the fast linear pipeline can evaluate it directly.
    pub fn as_linear_effects(&self, shared: &BTreeMap<SharedNodeId, SharedColorNode>) -> Option<Vec<EffectInstance>> {
        if self.nodes.is_empty() {
            return Some(Vec::new());
        }
        if self.edges.iter().any(|e| e.kind == ColorPortKind::Key) {
            return None;
        }
        for n in &self.nodes {
            match &n.kind {
                ColorNodeKind::Corrector { key_input_gain, key_output_gain, key_invert, .. }
                | ColorNodeKind::Shared { key_input_gain, key_output_gain, key_invert, .. } => {
                    if (*key_input_gain - 1.0).abs() > 1e-4 || (*key_output_gain - 1.0).abs() > 1e-4 || *key_invert {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        // Walk from Source -> ... -> Output following unique Rgb edges
        let mut cur = ColorEndpoint::Source;
        let mut chain = Vec::with_capacity(self.nodes.len());
        let mut steps = 0usize;
        while cur != ColorEndpoint::Output {
            steps += 1;
            if steps > self.nodes.len() + 2 {
                return None;
            }
            let mut outgoing = self.edges.iter().filter(|e| e.kind == ColorPortKind::Rgb && e.from == cur);
            let edge = outgoing.next()?;
            if outgoing.next().is_some() {
                // Branching fan-out -> not a simple linear chain
                return None;
            }
            match edge.to {
                ColorEndpoint::Source => return None,
                ColorEndpoint::Output => {
                    cur = ColorEndpoint::Output;
                }
                ColorEndpoint::Node(id) => {
                    let node = self.node(id)?;
                    let mut fx = match &node.kind {
                        ColorNodeKind::Corrector { effect, .. } => (**effect).clone(),
                        ColorNodeKind::Shared { shared_id, .. } => shared.get(shared_id)?.effect.clone(),
                        _ => return None,
                    };
                    fx.enabled = node.enabled;
                    fx.label = node.label.clone();
                    chain.push(fx);
                    cur = ColorEndpoint::Node(id);
                }
            }
        }
        Some(chain)
    }

    /// Topological ordering of all nodes using Kahn's algorithm.
    /// Fails with an error if a cycle exists or node count exceeds [`MAX_COLOR_NODES`].
    pub fn topo_order(&self) -> Result<Vec<ColorNodeId>, String> {
        if self.nodes.len() > MAX_COLOR_NODES {
            return Err(format!("color graph has {} nodes (maximum is {MAX_COLOR_NODES})", self.nodes.len()));
        }
        let mut index_of = BTreeMap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if index_of.insert(n.id, i).is_some() {
                return Err(format!("duplicate color node id {}", n.id.0));
            }
        }
        let n_len = self.nodes.len();
        let mut in_deg = vec![0usize; n_len];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n_len];
        for e in &self.edges {
            if let (ColorEndpoint::Node(u), ColorEndpoint::Node(v)) = (e.from, e.to) {
                let Some(&ui) = index_of.get(&u) else {
                    return Err(format!("edge references unknown source node {}", u.0));
                };
                let Some(&vi) = index_of.get(&v) else {
                    return Err(format!("edge references unknown target node {}", v.0));
                };
                if ui == vi {
                    return Err(format!("self-loop on color node {}", u.0));
                }
                if !adj[ui].contains(&vi) {
                    adj[ui].push(vi);
                    in_deg[vi] += 1;
                }
            }
        }
        let mut queue = VecDeque::new();
        for (i, &deg) in in_deg.iter().enumerate() {
            if deg == 0 {
                queue.push_back(i);
            }
        }
        let mut order = Vec::with_capacity(n_len);
        while let Some(u) = queue.pop_front() {
            order.push(self.nodes[u].id);
            for &v in &adj[u] {
                in_deg[v] = in_deg[v].saturating_sub(1);
                if in_deg[v] == 0 {
                    queue.push_back(v);
                }
            }
        }
        if order.len() != n_len {
            return Err("color graph contains a cycle".into());
        }
        Ok(order)
    }

    /// Validate structural invariants, port bounds, finite coordinates, and acyclicity.
    pub fn validate(&self) -> Result<(), String> {
        if self.nodes.len() > MAX_COLOR_NODES {
            return Err(format!("color graph exceeds {MAX_COLOR_NODES} nodes"));
        }
        if self.edges.len() > MAX_COLOR_EDGES {
            return Err(format!("color graph exceeds {MAX_COLOR_EDGES} edges"));
        }
        for n in &self.nodes {
            if !n.pos[0].is_finite() || !n.pos[1].is_finite() {
                return Err(format!("color node {} has non-finite position", n.id.0));
            }
        }
        let _ = self.topo_order()?;
        let mut incoming_seen = BTreeMap::new();
        for e in &self.edges {
            if e.from == ColorEndpoint::Output {
                return Err("graph Output cannot be an edge source".into());
            }
            if e.to == ColorEndpoint::Source {
                return Err("graph Source cannot be an edge destination".into());
            }
            if e.from == e.to {
                return Err("self-loop edge is not allowed".into());
            }
            let max_out = match (e.from, e.kind) {
                (ColorEndpoint::Source, ColorPortKind::Rgb | ColorPortKind::Key) => 1,
                (ColorEndpoint::Node(id), ColorPortKind::Rgb) => self.node(id).ok_or("unknown source node")?.kind.rgb_output_ports(),
                (ColorEndpoint::Node(id), ColorPortKind::Key) => self.node(id).ok_or("unknown source node")?.kind.key_output_ports(),
                (ColorEndpoint::Output, _) => 0,
            };
            if e.from_port >= max_out {
                return Err(format!("source port {} out of bounds (max {})", e.from_port, max_out));
            }
            let max_in = match (e.to, e.kind) {
                (ColorEndpoint::Output, ColorPortKind::Rgb | ColorPortKind::Key) => 1,
                (ColorEndpoint::Node(id), ColorPortKind::Rgb) => self.node(id).ok_or("unknown destination node")?.kind.rgb_input_ports(),
                (ColorEndpoint::Node(id), ColorPortKind::Key) => self.node(id).ok_or("unknown destination node")?.kind.key_input_ports(),
                (ColorEndpoint::Source, _) => 0,
            };
            if e.to_port >= max_in {
                return Err(format!("destination port {} out of bounds (max {})", e.to_port, max_in));
            }
            if incoming_seen.insert((e.to, e.kind, e.to_port), e.from).is_some() {
                return Err("multiple edges connected to the same input port".into());
            }
        }
        Ok(())
    }

    /// Connect two ports, replacing any existing cable on `(edge.to, edge.kind, edge.to_port)`.
    /// Rejects the edit without mutating `self` if it would introduce a cycle or invalid port.
    pub fn connect(&mut self, edge: ColorEdge) -> Result<(), String> {
        let snapshot = self.edges.clone();
        self.edges.retain(|e| !(e.to == edge.to && e.kind == edge.kind && e.to_port == edge.to_port));
        self.edges.push(edge);
        if let Err(err) = self.validate() {
            self.edges = snapshot;
            return Err(err);
        }
        Ok(())
    }

    /// Disconnect any cable feeding `(to, kind, to_port)`.
    pub fn disconnect(&mut self, to: ColorEndpoint, kind: ColorPortKind, to_port: u8) -> bool {
        let before = self.edges.len();
        self.edges.retain(|e| !(e.to == to && e.kind == kind && e.to_port == to_port));
        self.edges.len() != before
    }

    /// Add a serial `Corrector` node after `after` (or at the end of the main chain if `None`).
    pub fn add_serial_node(&mut self, after: Option<ColorNodeId>, effect: EffectInstance) -> Result<ColorNodeId, String> {
        if self.nodes.len() >= MAX_COLOR_NODES {
            return Err(format!("color graph already has {MAX_COLOR_NODES} nodes"));
        }
        let anchor = after.and_then(|id| self.node(id).map(|n| (n.id, n.pos))).or_else(|| {
            // Find the node currently feeding Output, or the last node
            self.edges
                .iter()
                .find_map(|e| {
                    if e.kind == ColorPortKind::Rgb
                        && e.to == ColorEndpoint::Output
                        && let ColorEndpoint::Node(id) = e.from
                    {
                        self.node(id).map(|n| (n.id, n.pos))
                    } else {
                        None
                    }
                })
                .or_else(|| self.nodes.last().map(|n| (n.id, n.pos)))
        });

        let id = self.alloc_node_id();
        let pos = match anchor {
            Some((_, [ax, ay])) => [ax + 192.0, ay],
            None => [110.0, 40.0],
        };
        let enabled = effect.enabled;
        let label = effect.label.clone();
        self.nodes.push(ColorNode {
            id,
            kind: ColorNodeKind::Corrector { effect: Box::new(effect), key_input_gain: 1.0, key_output_gain: 1.0, key_invert: false },
            enabled,
            label,
            pos,
        });

        match anchor {
            None => {
                self.edges.push(ColorEdge {
                    kind: ColorPortKind::Rgb,
                    from: ColorEndpoint::Source,
                    from_port: 0,
                    to: ColorEndpoint::Node(id),
                    to_port: 0,
                    invert_key: false,
                });
                self.edges.push(ColorEdge {
                    kind: ColorPortKind::Rgb,
                    from: ColorEndpoint::Node(id),
                    from_port: 0,
                    to: ColorEndpoint::Output,
                    to_port: 0,
                    invert_key: false,
                });
            }
            Some((prev_id, _)) => {
                let prev_ep = ColorEndpoint::Node(prev_id);
                let new_ep = ColorEndpoint::Node(id);
                let mut rewired = false;
                for e in &mut self.edges {
                    if e.kind == ColorPortKind::Rgb && e.from == prev_ep && e.from_port == 0 {
                        e.from = new_ep;
                        rewired = true;
                    }
                }
                self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: prev_ep, from_port: 0, to: new_ep, to_port: 0, invert_key: false });
                if !rewired {
                    self.edges.retain(|e| !(e.kind == ColorPortKind::Rgb && e.to == ColorEndpoint::Output));
                    self.edges.push(ColorEdge {
                        kind: ColorPortKind::Rgb,
                        from: new_ep,
                        from_port: 0,
                        to: ColorEndpoint::Output,
                        to_port: 0,
                        invert_key: false,
                    });
                }
            }
        }
        self.validate()?;
        Ok(id)
    }

    /// Add an `Outside` node (`Alt+O`) after `from_node`: receives `from_node`'s RGB output and its
    /// **inverted Key output** (`invert_key = true`).
    pub fn add_outside_node(&mut self, from_node: ColorNodeId, effect: EffectInstance) -> Result<ColorNodeId, String> {
        if self.nodes.len() >= MAX_COLOR_NODES {
            return Err(format!("color graph already has {MAX_COLOR_NODES} nodes"));
        }
        let Some(src) = self.node(from_node) else {
            return Err(format!("unknown source node {}", from_node.0));
        };
        let pos = [src.pos[0] + 192.0, src.pos[1] + 54.0];
        let id = self.alloc_node_id();
        let enabled = effect.enabled;
        let label = effect.label.clone().or_else(|| Some("Outside".into()));
        self.nodes.push(ColorNode {
            id,
            kind: ColorNodeKind::Outside { effect: Box::new(effect), key_input_gain: 1.0, key_output_gain: 1.0, key_invert: false },
            enabled,
            label,
            pos,
        });
        let prev_ep = ColorEndpoint::Node(from_node);
        let new_ep = ColorEndpoint::Node(id);
        let mut rewired = false;
        for e in &mut self.edges {
            if e.kind == ColorPortKind::Rgb && e.from == prev_ep && e.from_port == 0 {
                e.from = new_ep;
                rewired = true;
            }
        }
        self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: prev_ep, from_port: 0, to: new_ep, to_port: 0, invert_key: false });
        self.edges.push(ColorEdge { kind: ColorPortKind::Key, from: prev_ep, from_port: 0, to: new_ep, to_port: 0, invert_key: true });
        if !rewired {
            self.edges.retain(|e| !(e.kind == ColorPortKind::Rgb && e.to == ColorEndpoint::Output));
            self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: new_ep, from_port: 0, to: ColorEndpoint::Output, to_port: 0, invert_key: false });
        }
        self.validate()?;
        Ok(id)
    }

    /// Add a parallel branch (`Alt+P`, `layer = false`) or layer branch (`Alt+L`, `layer = true`)
    /// alongside `base_node`, creating or expanding a `ParallelMixer` / `LayerMixer`.
    /// Returns `(new_branch_corrector_id, mixer_node_id)`.
    pub fn add_branched_node(&mut self, base_node: ColorNodeId, effect: EffectInstance, layer: bool) -> Result<(ColorNodeId, ColorNodeId), String> {
        if self.nodes.len() + 2 > MAX_COLOR_NODES {
            return Err(format!("color graph exceeds {MAX_COLOR_NODES} nodes"));
        }
        let Some(base) = self.node(base_node) else {
            return Err(format!("unknown base node {}", base_node.0));
        };
        let base_pos = base.pos;
        let base_ep = ColorEndpoint::Node(base_node);
        // Find the upstream RGB source feeding `base_node`
        let (upstream, upstream_port) = self
            .edges
            .iter()
            .find(|e| e.kind == ColorPortKind::Rgb && e.to == base_ep && e.to_port == 0)
            .map(|e| (e.from, e.from_port))
            .unwrap_or((ColorEndpoint::Source, 0));

        // Check if `base_node` already feeds an existing ParallelMixer / LayerMixer of matching kind
        let existing_mixer = self.edges.iter().find_map(|e| {
            if e.kind == ColorPortKind::Rgb
                && e.from == base_ep
                && let ColorEndpoint::Node(mid) = e.to
                && let Some(mn) = self.node(mid)
            {
                let matches_kind = matches!((&mn.kind, layer), (ColorNodeKind::ParallelMixer { .. }, false) | (ColorNodeKind::LayerMixer { .. }, true));
                if matches_kind {
                    return Some(mid);
                }
            }
            None
        });

        let branch_id = self.alloc_node_id();
        let enabled = effect.enabled;
        let label = effect.label.clone();

        if let Some(mixer_id) = existing_mixer {
            let new_port = {
                let Some(mn) = self.node_mut(mixer_id) else {
                    return Err("missing mixer node".into());
                };
                match &mut mn.kind {
                    ColorNodeKind::ParallelMixer { inputs } => {
                        if *inputs >= MAX_MIXER_INPUTS {
                            return Err(format!("mixer already has {MAX_MIXER_INPUTS} inputs"));
                        }
                        let p = *inputs as u8;
                        *inputs += 1;
                        p
                    }
                    ColorNodeKind::LayerMixer { inputs, blend_modes, gains } => {
                        if *inputs >= MAX_MIXER_INPUTS {
                            return Err(format!("mixer already has {MAX_MIXER_INPUTS} inputs"));
                        }
                        let p = *inputs as u8;
                        *inputs += 1;
                        blend_modes.resize(*inputs, LayerBlendMode::Normal);
                        gains.resize(*inputs, 1.0);
                        p
                    }
                    _ => return Err("target is not a mixer".into()),
                }
            };
            let branch_pos = [base_pos[0], base_pos[1] + f32::from(new_port) * 110.0];
            self.nodes.push(ColorNode {
                id: branch_id,
                kind: ColorNodeKind::Corrector { effect: Box::new(effect), key_input_gain: 1.0, key_output_gain: 1.0, key_invert: false },
                enabled,
                label,
                pos: branch_pos,
            });
            self.edges.push(ColorEdge {
                kind: ColorPortKind::Rgb,
                from: upstream,
                from_port: upstream_port,
                to: ColorEndpoint::Node(branch_id),
                to_port: 0,
                invert_key: false,
            });
            self.edges.push(ColorEdge {
                kind: ColorPortKind::Rgb,
                from: ColorEndpoint::Node(branch_id),
                from_port: 0,
                to: ColorEndpoint::Node(mixer_id),
                to_port: new_port,
                invert_key: false,
            });
            if layer {
                self.edges.push(ColorEdge {
                    kind: ColorPortKind::Key,
                    from: ColorEndpoint::Node(branch_id),
                    from_port: 0,
                    to: ColorEndpoint::Node(mixer_id),
                    to_port: new_port,
                    invert_key: false,
                });
            }
            self.validate()?;
            return Ok((branch_id, mixer_id));
        }

        // Create both the new parallel branch node and a new mixer node
        let mixer_id = self.alloc_node_id();
        let branch_pos = [base_pos[0], base_pos[1] + 110.0];
        let mixer_pos = [base_pos[0] + 192.0, base_pos[1] + 55.0];
        self.nodes.push(ColorNode {
            id: branch_id,
            kind: ColorNodeKind::Corrector { effect: Box::new(effect), key_input_gain: 1.0, key_output_gain: 1.0, key_invert: false },
            enabled,
            label,
            pos: branch_pos,
        });
        let mixer_kind = if layer {
            ColorNodeKind::LayerMixer { inputs: 2, blend_modes: vec![LayerBlendMode::Normal, LayerBlendMode::Normal], gains: vec![1.0, 1.0] }
        } else {
            ColorNodeKind::ParallelMixer { inputs: 2 }
        };
        self.nodes.push(ColorNode {
            id: mixer_id,
            kind: mixer_kind,
            enabled: true,
            label: Some(if layer { "Layer Mixer".into() } else { "Parallel".into() }),
            pos: mixer_pos,
        });

        let mixer_ep = ColorEndpoint::Node(mixer_id);
        let branch_ep = ColorEndpoint::Node(branch_id);
        let mut rewired = false;
        for e in &mut self.edges {
            if e.kind == ColorPortKind::Rgb && e.from == base_ep && e.from_port == 0 {
                e.from = mixer_ep;
                rewired = true;
            }
        }
        self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: upstream, from_port: upstream_port, to: branch_ep, to_port: 0, invert_key: false });
        self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: base_ep, from_port: 0, to: mixer_ep, to_port: 0, invert_key: false });
        self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: branch_ep, from_port: 0, to: mixer_ep, to_port: 1, invert_key: false });
        if layer {
            self.edges.push(ColorEdge { kind: ColorPortKind::Key, from: base_ep, from_port: 0, to: mixer_ep, to_port: 0, invert_key: false });
            self.edges.push(ColorEdge { kind: ColorPortKind::Key, from: branch_ep, from_port: 0, to: mixer_ep, to_port: 1, invert_key: false });
        }
        if !rewired {
            self.edges.retain(|e| !(e.kind == ColorPortKind::Rgb && e.to == ColorEndpoint::Output));
            self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: mixer_ep, from_port: 0, to: ColorEndpoint::Output, to_port: 0, invert_key: false });
        }
        self.validate()?;
        Ok((branch_id, mixer_id))
    }

    /// Add a `Splitter` -> 3 channel `Corrector`s -> `Combiner` group (`Alt+Y`).
    pub fn add_splitter_combiner(&mut self, after: Option<ColorNodeId>, space: SplitSpace) -> Result<(ColorNodeId, [ColorNodeId; 3], ColorNodeId), String> {
        if self.nodes.len() + 5 > MAX_COLOR_NODES {
            return Err(format!("color graph exceeds {MAX_COLOR_NODES} nodes"));
        }
        let anchor = after.and_then(|id| self.node(id).map(|n| (n.id, n.pos))).or_else(|| self.nodes.last().map(|n| (n.id, n.pos)));
        let base_pos = anchor.map_or([110.0, 40.0], |(_, [x, y])| [x + 192.0, y]);

        let split_id = self.alloc_node_id();
        let ch0 = self.alloc_node_id();
        let ch1 = self.alloc_node_id();
        let ch2 = self.alloc_node_id();
        let comb_id = self.alloc_node_id();

        self.nodes.push(ColorNode {
            id: split_id,
            kind: ColorNodeKind::Splitter { space },
            enabled: true,
            label: Some("Splitter".into()),
            pos: [base_pos[0], base_pos[1]],
        });
        let labels = space.channel_labels();
        let ch_ids = [ch0, ch1, ch2];
        for (i, &cid) in ch_ids.iter().enumerate() {
            let offset_y = (i as f32 - 1.0) * 100.0;
            self.nodes.push(ColorNode {
                id: cid,
                kind: ColorNodeKind::default_corrector(),
                enabled: true,
                label: Some(labels[i].into()),
                pos: [base_pos[0] + 165.0, base_pos[1] + offset_y],
            });
        }
        self.nodes.push(ColorNode {
            id: comb_id,
            kind: ColorNodeKind::Combiner { space },
            enabled: true,
            label: Some("Combiner".into()),
            pos: [base_pos[0] + 330.0, base_pos[1]],
        });

        let split_ep = ColorEndpoint::Node(split_id);
        let comb_ep = ColorEndpoint::Node(comb_id);

        match anchor {
            None => {
                self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: ColorEndpoint::Source, from_port: 0, to: split_ep, to_port: 0, invert_key: false });
                self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: comb_ep, from_port: 0, to: ColorEndpoint::Output, to_port: 0, invert_key: false });
            }
            Some((prev_id, _)) => {
                let prev_ep = ColorEndpoint::Node(prev_id);
                let mut rewired = false;
                for e in &mut self.edges {
                    if e.kind == ColorPortKind::Rgb && e.from == prev_ep && e.from_port == 0 {
                        e.from = comb_ep;
                        rewired = true;
                    }
                }
                self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: prev_ep, from_port: 0, to: split_ep, to_port: 0, invert_key: false });
                if !rewired {
                    self.edges.retain(|e| !(e.kind == ColorPortKind::Rgb && e.to == ColorEndpoint::Output));
                    self.edges.push(ColorEdge {
                        kind: ColorPortKind::Rgb,
                        from: comb_ep,
                        from_port: 0,
                        to: ColorEndpoint::Output,
                        to_port: 0,
                        invert_key: false,
                    });
                }
            }
        }
        for (i, &cid) in ch_ids.iter().enumerate() {
            let cep = ColorEndpoint::Node(cid);
            self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: split_ep, from_port: i as u8, to: cep, to_port: 0, invert_key: false });
            self.edges.push(ColorEdge { kind: ColorPortKind::Rgb, from: cep, from_port: 0, to: comb_ep, to_port: i as u8, invert_key: false });
        }
        self.validate()?;
        Ok((split_id, ch_ids, comb_id))
    }

    /// Delete a node from the graph, healing the RGB connection if it had a primary incoming RGB source.
    pub fn remove_node(&mut self, id: ColorNodeId) -> Result<(), String> {
        let Some(pos) = self.nodes.iter().position(|n| n.id == id) else {
            return Err(format!("unknown color node {}", id.0));
        };
        let ep = ColorEndpoint::Node(id);
        let upstream_rgb = self.edges.iter().find(|e| e.kind == ColorPortKind::Rgb && e.to == ep && e.to_port == 0).map(|e| (e.from, e.from_port));
        let downstream_rgb: Vec<(ColorEndpoint, u8)> =
            self.edges.iter().filter(|e| e.kind == ColorPortKind::Rgb && e.from == ep && e.from_port == 0).map(|e| (e.to, e.to_port)).collect();

        self.nodes.remove(pos);
        self.edges.retain(|e| e.from != ep && e.to != ep);

        if let Some((up_from, up_port)) = upstream_rgb {
            for (down_to, down_port) in downstream_rgb {
                if up_from != down_to && !(up_from == ColorEndpoint::Source && down_to == ColorEndpoint::Output) {
                    let already = self.edges.iter().any(|e| e.kind == ColorPortKind::Rgb && e.to == down_to && e.to_port == down_port);
                    if !already {
                        self.edges.push(ColorEdge {
                            kind: ColorPortKind::Rgb,
                            from: up_from,
                            from_port: up_port,
                            to: down_to,
                            to_port: down_port,
                            invert_key: false,
                        });
                    }
                }
            }
        }
        if self.nodes.is_empty() {
            self.edges.clear();
        }
        self.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ParamValue;

    #[test]
    fn serial_chain_constructs_and_round_trips_linear_effects() {
        let mut fx1 = default_lumetri_instance();
        fx1.params.insert("exposure".into(), crate::Param::new(ParamValue::Float(1.25)));
        let mut fx2 = default_lumetri_instance();
        fx2.params.insert("contrast".into(), crate::Param::new(ParamValue::Float(20.0)));

        let g = ColorGraph::from_serial_effects(&[fx1.clone(), fx2.clone()]);
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 3);
        g.validate().unwrap();

        let linear = g.as_linear_effects(&BTreeMap::new()).expect("pure serial chain");
        assert_eq!(linear.len(), 2);
        assert_eq!(linear[0].param("exposure").unwrap().value, ParamValue::Float(1.25));
        assert_eq!(linear[1].param("contrast").unwrap().value, ParamValue::Float(20.0));
    }

    #[test]
    fn cycle_detection_rejects_back_edge_without_mutating_graph() {
        let mut g = ColorGraph::new();
        let n1 = g.add_serial_node(None, default_lumetri_instance()).unwrap();
        let n2 = g.add_serial_node(Some(n1), default_lumetri_instance()).unwrap();
        let before = g.edges.clone();

        // Attempt to connect n2 -> n1 (would create n1 -> n2 -> n1 cycle)
        let res = g.connect(ColorEdge {
            kind: ColorPortKind::Rgb,
            from: ColorEndpoint::Node(n2),
            from_port: 0,
            to: ColorEndpoint::Node(n1),
            to_port: 0,
            invert_key: false,
        });
        assert!(res.is_err());
        assert_eq!(g.edges, before, "failed cyclic connect must leave edges untouched");
    }

    #[test]
    fn outside_parallel_layer_and_splitter_nodes_validate_and_remove_cleanly() {
        let mut g = ColorGraph::new();
        let n1 = g.add_serial_node(None, default_lumetri_instance()).unwrap();
        let out_id = g.add_outside_node(n1, default_lumetri_instance()).unwrap();
        assert!(g.as_linear_effects(&BTreeMap::new()).is_none(), "Outside node uses Key cable, not linear");

        let (par_branch, par_mixer) = g.add_branched_node(out_id, default_lumetri_instance(), false).unwrap();
        let (lay_branch, lay_mixer) = g.add_branched_node(par_mixer, default_lumetri_instance(), true).unwrap();
        let (spl, chs, cmb) = g.add_splitter_combiner(Some(lay_mixer), SplitSpace::YCbCr).unwrap();
        g.validate().unwrap();

        let order = g.topo_order().unwrap();
        let pos = |id: ColorNodeId| order.iter().position(|x| *x == id).unwrap();
        assert!(pos(n1) < pos(out_id));
        assert!(pos(out_id) < pos(par_mixer));
        assert!(pos(par_branch) < pos(par_mixer));
        assert!(pos(lay_branch) < pos(lay_mixer));
        assert!(pos(spl) < pos(chs[0]) && pos(chs[0]) < pos(cmb));

        g.remove_node(out_id).unwrap();
        g.validate().unwrap();
    }
}
