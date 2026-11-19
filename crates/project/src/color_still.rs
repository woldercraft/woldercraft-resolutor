//! Gallery stills & PowerGrades snapshots for the Color workspace (`color_stills`).

use serde::{Deserialize, Serialize};

use crate::{ColorGraph, EffectInstance};

/// Maximum still entries permitted per project (`Never Crash` bounded storage).
pub const MAX_COLOR_STILLS: usize = 512;

/// Maximum thumbnail edge dimension stored on a [`ColorStill`].
pub const MAX_STILL_THUMB_SIDE: u32 = 320;

/// Saved Color Gallery Still / PowerGrade containing both a small RGBA8 thumbnail and the full
/// [`ColorGraph`] snapshot (plus fallback flat Lumetri effects when copied from a flat clip).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorStill {
    pub id: u64,
    pub label: String,
    #[serde(default)]
    pub clip_name: String,
    #[serde(default)]
    pub timecode: String,
    #[serde(default)]
    pub graph: ColorGraph,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectInstance>,
    #[serde(default)]
    pub thumb_w: u32,
    #[serde(default)]
    pub thumb_h: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thumb_rgba: Vec<u8>,
}

impl ColorStill {
    /// Sanitize thumbnail dimensions against corrupted / hostile project files.
    pub fn has_valid_thumb(&self) -> bool {
        let w = self.thumb_w;
        let h = self.thumb_h;
        if w == 0 || h == 0 || w > MAX_STILL_THUMB_SIDE || h > MAX_STILL_THUMB_SIDE {
            return false;
        }
        (w as usize).checked_mul(h as usize).and_then(|px| px.checked_mul(4)) == Some(self.thumb_rgba.len())
    }
}
