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
    /// The flats it combines that could not be read, and why. The session
    /// is read again only when one whose file was away is back.
    pub unread: Vec<UnreadFlat>,
}

/// A flat a session's response should have combined but could not read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadFlat {
    pub frame_uuid: String,
    /// No file at its path, as on a share that is away or a copy moved
    /// elsewhere; false when the file was there but would not read.
    pub missing: bool,
    pub reason: String,
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

/// What some of an imaging session's lights said about which response
/// family their flats come from: the lights of one filter, one target and
/// one recorded angle (lights within the rig's rotation tolerance of each
/// other), so a background never mixes two fields or two angles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyEvidenceRow {
    pub session_key: String,
    /// Canonical filter name.
    pub filter: String,
    pub target: String,
    /// The lights' middle recorded angle, and the lowest and highest.
    pub angle: f64,
    pub angle_low: f64,
    pub angle_high: f64,
    /// The first and last of the lights, Unix seconds.
    pub start: i64,
    pub end: i64,
    /// The candidates, the sessions standing for them, which of those
    /// measured alike, and the choice algorithm, digested
    /// (`Book::family_evidence_key`): a row whose key is not the current
    /// one is not read.
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

impl FamilyEvidenceRow {
    /// Where it is kept, besides the session and filter: its target and
    /// lowest angle.
    pub fn angle_key(&self) -> String {
        format!("{:.1}", self.angle_low)
    }
}
