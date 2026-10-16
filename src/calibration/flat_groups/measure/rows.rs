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
pub use super::evidence::Verdict;

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

/// What an imaging session's own lights said, in one filter, about which
/// response family their flats come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyEvidenceRow {
    pub session_key: String,
    /// Canonical filter name.
    pub filter: String,
    /// The candidates and the sessions standing for them, digested
    /// (`Book::family_evidence_key`).
    pub key: String,
    /// The candidate families' keys.
    pub candidates: Vec<String>,
    /// The family the lights follow, when one stood clear.
    pub chosen: Option<String>,
    pub lights: usize,
    pub targets: Vec<String>,
    /// Each pair of candidates, as the lights' background fitted it.
    pub pairs: Vec<FamilyPairRow>,
}

/// One pair of candidate families against a session's background.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyPairRow {
    pub first: String,
    pub second: String,
    /// `alike` (nothing compact tells them apart), `unfitted` (the
    /// background could not be weighed against them) or `fit`.
    #[serde(default)]
    pub outcome: String,
    /// 0 follows `first`, 1 follows `second`; absent when nothing compact
    /// tells the two apart.
    pub carried: Option<f64>,
    pub spread: Option<f64>,
    pub verdict: Option<Verdict>,
}
