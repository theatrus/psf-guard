//! Where a stack build's lights are, and their headers, from what the
//! catalog keeps ([`crate::light_headers`]).
//!
//! Preparing a build used to look every light up in the image folders and
//! open its header. A light whose file stands as it was when a build last
//! found it is now taken from the catalog: its path, and its header. What
//! had to be looked up or read is kept after preparation, by
//! [`crate::server::light_header_writes`], so the next build need not.

use crate::calibration::LightMatcher;
use crate::commands::import::headers::FrameMeta;
use crate::light_headers::{FileStamp, LightHeaders};
use crate::models::AcquiredImage;
use crate::server::database_context::DatabaseContext;
use crate::server::handlers::AppError;
use crate::server::remote_upload::MappedLightSources;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The lights of one build request.
pub(crate) struct KeptLights {
    headers: LightHeaders,
    /// Each light file's size and time as found, by image.
    stamps: HashMap<i32, Option<FileStamp>>,
    /// The lights a stack's own lookup found, rather than a remote upload's
    /// record: only those may be gone to straight next time.
    found_by_lookup: HashSet<i32>,
}

impl KeptLights {
    /// What the catalog keeps of a project's lights.
    pub(crate) fn load(conn: &Connection, project_id: i32) -> Self {
        Self {
            headers: LightHeaders::load(conn, project_id),
            stamps: HashMap::new(),
            found_by_lookup: HashSet::new(),
        }
    }

    /// Where a light's file is, and its source fingerprint, or `None` when it
    /// is not found. A light is not looked for again where a stack's own
    /// lookup last found it, its file as it was then: the layout probes stat
    /// every candidate folder on the share. A remote upload's record, not
    /// that lookup, finds a delayed or mapped upload, so those are always
    /// looked up.
    pub(crate) fn locate(
        &mut self,
        ctx: &DatabaseContext,
        image: &AcquiredImage,
        target_name: &str,
        filename: &str,
        mapped_sources: &MappedLightSources,
    ) -> Result<Option<(PathBuf, String)>, AppError> {
        let by_lookup = !ctx.requires_delayed_file_identity(image.id)
            && mapped_sources.quality_revision(image.id).is_none();
        let located = by_lookup
            .then(|| self.located(ctx, image.id, filename))
            .flatten();
        let (path, metadata) = match located {
            Some((path, metadata)) => {
                ctx.record_resolved_image_file(image, &path);
                (path, Some(metadata))
            }
            None => {
                match crate::server::handlers::find_fits_file(ctx, image, target_name, filename) {
                    Ok(path) => {
                        let metadata = path.metadata().ok();
                        (path, metadata)
                    }
                    Err(AppError::NotFound) => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
        };
        if by_lookup {
            self.found_by_lookup.insert(image.id);
        }
        self.stamps.insert(
            image.id,
            metadata.as_ref().and_then(FileStamp::from_metadata),
        );
        let fingerprint = super::source_fingerprint_from(&path, metadata.as_ref());
        Ok(Some((path, fingerprint)))
    }

    /// The path a stack's lookup last found this light at, with the file's
    /// metadata, while it stands as it was then under one of the image
    /// folders and by the name the catalog gives.
    fn located(
        &self,
        ctx: &DatabaseContext,
        image_id: i32,
        filename: &str,
    ) -> Option<(PathBuf, std::fs::Metadata)> {
        let path = self.headers.located(image_id)?;
        let in_place = path.file_name().and_then(|name| name.to_str()) == Some(filename)
            && ctx
                .image_dir_paths
                .iter()
                .any(|root| path.starts_with(root));
        if !in_place {
            return None;
        }
        let metadata = path.metadata().ok()?;
        self.headers
            .holds(image_id, path, FileStamp::from_metadata(&metadata))
            .then(|| (path.to_path_buf(), metadata))
    }

    /// The header of a light [`Self::locate`] found at `path`: the kept one
    /// while it holds the file, else read from it.
    pub(crate) fn meta(&mut self, image_id: i32, path: &Path) -> FrameMeta {
        self.headers.meta(
            image_id,
            path,
            self.stamps.get(&image_id).copied().flatten(),
            self.found_by_lookup.contains(&image_id),
        )
    }

    /// Keep what preparation had to look up or read, after it ends and
    /// off the shared connection, with the calibration files `matcher` read.
    pub(crate) fn keep(mut self, database_path: &str, matcher: Option<&mut LightMatcher>) {
        crate::server::light_header_writes::queue(
            database_path,
            self.headers.take_fresh(),
            matcher
                .map(LightMatcher::take_file_checks)
                .unwrap_or_default(),
        );
    }
}
