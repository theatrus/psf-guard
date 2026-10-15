//! The numbers a pass keeps for each flat session, pair and imaging
//! session, as plain data: what the grouping reads and the store writes.

use super::metric::Spot;
use serde::{Deserialize, Serialize};

/// One measured flat session as the catalog keeps it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResponseRow {
    pub response_key: String,
    pub session_key: String,
    pub rig_uuid: String,
    pub family_key: String,
    pub night: String,
    pub filter: Option<String>,
    pub rotation: Option<f64>,
    /// Every flat of the session in use, in capture order.
    pub frame_uuids: Vec<String>,
    /// How many of them the response combines.
    pub frames: usize,
    pub captured_first: Option<i64>,
    pub captured_last: Option<i64>,
    pub level_adu: Option<f64>,
    pub pedestal_adu: Option<f64>,
    pub noise: Option<f64>,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub bin: Option<usize>,
    pub algorithm_version: u32,
    /// Why the session could not be measured.
    pub error: Option<String>,
}

/// Two sessions' metrics as the catalog keeps them.
#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonRow {
    pub earlier_key: String,
    pub later_key: String,
    pub broad: f64,
    pub dust: f64,
    pub sigma: f64,
    pub spots: Vec<Spot>,
    pub broad_x: f64,
    pub broad_y: f64,
}

/// What an imaging session's own lights say about one change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The lights lack the change: shot before it.
    Before,
    /// The lights carry the change: shot after it.
    After,
    /// The lights cannot tell, as over a nebula.
    Undecided,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Before => "before",
            Verdict::After => "after",
            Verdict::Undecided => "undecided",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "before" => Some(Verdict::Before),
            "after" => Some(Verdict::After),
            "undecided" => Some(Verdict::Undecided),
            _ => None,
        }
    }
}

/// One filter's fit within an imaging session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceFit {
    pub filter: String,
    /// Targets the lights were of.
    pub targets: Vec<String>,
    pub lights: usize,
    /// How much of the change the lights carry: 0 none, 1 all of it.
    pub carried: f64,
    /// Spread of that estimate over placebo positions; none when too few
    /// placebos fit in the frame.
    pub spread: Option<f64>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceRow {
    pub session_key: String,
    pub event_key: String,
    pub verdict: Verdict,
    pub fits: Vec<EvidenceFit>,
}
