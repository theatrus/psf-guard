//! What image steps share, so that the editor can run a step on part of an
//! image and combine results. See "Building blocks" in
//! `docs/design/psf-studio.md`.
//!
//! - [`ImageShape`]: the width, height and channel count of a caller's
//!   samples.
//! - [`Region`]: a rectangle of an image, with the copying and padding a
//!   step needs to work on it.
//! - [`Domain`]: whether samples are linear light or display-referred, and
//!   the refusal a step gives when handed the wrong kind.
//! - [`Mask`]: one plane of values in `[0, 1]` that limits where a step
//!   takes effect.
//! - [`RegionOperation`]: the contract for a step that is resolved once from
//!   a whole image and then applied to any region of it, and
//!   [`region_mismatch`], the check that an operation keeps the contract.
//!
//! This module adds no image type. A step takes row-major,
//! channel-interleaved `f32` samples with their shape, as Seiza's steps do.
//! Pixels count from 0. A region's `x` and `y` are its first column and row.
//!
//! # The region contract
//!
//! The editor shows a full-resolution crop while a slider moves, works
//! through a mosaic in tiles, and caches results by region. All three need
//! one guarantee: the result for a region matches the same pixels of the
//! result for the whole image.
//!
//! A step keeps it by resolving first and applying second. Resolving
//! measures what the step needs from the whole image, such as a sample
//! range, a noise level or a fitted model, and stores it in the operation.
//! Applying uses only those stored values and the block it is given, and
//! states how many pixels of context it needs.
//!
//! # Where this belongs
//!
//! This is written here to settle its shape against real steps. Once it has,
//! it moves to Seiza, because the steps that must keep the contract are
//! Seiza's. No Seiza step keeps it yet: wrap one in a [`RegionOperation`]
//! here and test it with [`region_mismatch`] before relying on its preview.

mod domain;
mod mask;
mod region;
mod shape;
mod step;

#[cfg(test)]
mod contract_tests;

pub use domain::{Accepts, Domain, DomainError, ParseAcceptsError, ParseDomainError};
pub use mask::{Mask, MaskError, MaskRange};
pub use region::{Region, RegionError};
pub use shape::ImageShape;
pub use step::{
    apply_to_region, region_mismatch, Block, Edges, RegionApplyError, RegionMismatch,
    RegionOperation,
};
