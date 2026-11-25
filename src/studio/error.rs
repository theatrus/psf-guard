use crate::image_steps::{DomainError, MaskError, RegionError};
use thiserror::Error;

/// Why the scaffolding could not do what was asked.
#[derive(Debug, Error)]
pub enum StudioError {
    /// A region does not fit an image.
    #[error(transparent)]
    Region(#[from] RegionError),
    /// A mask could not be made or used.
    #[error(transparent)]
    Mask(#[from] MaskError),
    /// A step was given samples it does not work on.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// A step could not be applied.
    #[error("{step} could not be applied: {reason}")]
    Step {
        /// The step's name.
        step: String,
        /// What went wrong.
        reason: String,
    },
    /// A read needs more memory than the budget allows, even as one tile of
    /// the smallest size.
    #[error(
        "a {tile} x {tile} pixel tile needs {needed} bytes and the memory budget is {limit} bytes"
    )]
    OverBudget {
        /// The side of the smallest tile that was tried.
        tile: usize,
        /// The peak bytes that tile needs.
        needed: u64,
        /// The budget.
        limit: u64,
    },
    /// A document could not be read, or is not one this build understands.
    #[error("the document cannot be used: {0}")]
    Document(String),
    /// A file could not be read or written.
    #[error("{path}: {source}")]
    File {
        /// The file.
        path: std::path::PathBuf,
        /// The system's error.
        #[source]
        source: std::io::Error,
    },
}
