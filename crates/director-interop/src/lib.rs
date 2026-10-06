//! Read-only external demand, never a Director allocation or dispatch permit.
//! Hosts own HTTP, credentials, persistence, reviewed admission and execution.

pub mod astrocollab;
pub mod collaboration;
mod json;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    TooLarge,
    InvalidJson,
    LimitExceeded,
    InvalidSource,
    UnsupportedProtocol,
    InvalidIdentity,
    InvalidNight,
    InvalidValue,
    InvalidRegion,
    AmbiguousFilter,
    AmbiguousTask,
    NeedsReview,
    InvalidEvidence,
    InvalidReply,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never echo untrusted response bodies, URLs or field values.
        write!(f, "Director interoperability validation failed: {self:?}")
    }
}

impl std::error::Error for Error {}
