//! Where a stack build's lights are, and their headers, from what the
//! catalog keeps ([`crate::light_headers`]).
//!
//! Preparing a build used to look every light up in the image folders and
//! open its header. A light whose file stands as it was when a build last
//! found it is now taken from the catalog: its path, and its header. What
//! had to be looked up or read is handed to
//! [`crate::server::light_header_writes`], so the next build need not.

use crate::calibration::LightMatcher;
use crate::commands::import::headers::FrameMeta;
use crate::light_headers::{FileStamp, LightHeaders};
use crate::models::AcquiredImage;
use crate::server::database_context::DatabaseContext;
use crate::server::handlers::AppError;
use crate::server::remote_upload::MappedLightSources;
use rusqlite::OpenFlags;
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
    /// What the catalog keeps of a project's lights, read on a read-only
    /// connection of its own: a thousand headers are a few megabytes of
    /// JSON, which the shared connection should not wait on. A catalog that
    /// cannot be opened keeps nothing, and every light is looked up.
    pub(crate) fn load(ctx: &DatabaseContext, project_id: i32) -> Self {
        let headers = crate::server::database_context::open_scheduler_connection_with_flags(
            &ctx.database_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map(|conn| LightHeaders::load(&conn, project_id))
        .unwrap_or_else(|error| {
            tracing::debug!("kept light headers unavailable: {error}");
            LightHeaders::default()
        });
        Self {
            headers,
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

    /// Hand over what preparation had to look up or read, with the
    /// calibration files `matcher` read, to be kept once the build has its
    /// place in the line.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light_headers::KeptHeader;

    struct Fixture {
        dir: tempfile::TempDir,
        images: PathBuf,
        ctx: DatabaseContext,
        image: AcquiredImage,
    }

    /// One light, `LIGHT_0001.fits`, in the only image folder.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let images = dir.path().join("images");
        std::fs::create_dir(&images).unwrap();
        std::fs::write(images.join("LIGHT_0001.fits"), b"a light").unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','p');
             INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
                 VALUES(1,'Target',1,1,10,20,0,0,100,'t');
             INSERT INTO acquiredimage(Id,projectId,targetId,gradingStatus,metadata,
                 acquireddate,filtername)
                 VALUES(1,1,1,1,'{\"FileName\":\"LIGHT_0001.fits\"}',1,'Ha');",
        )
        .unwrap();
        let image = crate::db::Database::new(&conn)
            .get_images_by_project_id(1)
            .unwrap()
            .remove(0)
            .0;
        let mut ctx = DatabaseContext::new_for_test(conn);
        ctx.image_dir_paths = vec![images.clone()];
        Fixture {
            dir,
            images,
            ctx,
            image,
        }
    }

    /// Lights whose kept header says a lookup found light 1 at `path`, as
    /// the file there stands now.
    fn located_at(path: &Path) -> KeptLights {
        let header = KeptHeader {
            located: true,
            ..KeptHeader::new(
                path,
                FileStamp::of(path).unwrap(),
                FrameMeta {
                    path: path.to_path_buf(),
                    readable: true,
                    ..FrameMeta::default()
                },
            )
        };
        KeptLights {
            headers: LightHeaders::with_kept([(1, header)]),
            stamps: HashMap::new(),
            found_by_lookup: HashSet::new(),
        }
    }

    #[test]
    fn a_light_is_gone_to_straight_only_where_it_still_stands() {
        let fixture = fixture();
        let ctx = &fixture.ctx;
        let found = fixture.images.join("LIGHT_0001.fits");
        let lights = located_at(&found);
        assert_eq!(
            lights
                .located(ctx, 1, "LIGHT_0001.fits")
                .map(|(path, _)| path),
            Some(found.clone())
        );

        // Not by another name than the catalog gives the light.
        let renamed = fixture.images.join("LIGHT_0002.fits");
        std::fs::copy(&found, &renamed).unwrap();
        assert!(located_at(&renamed)
            .located(ctx, 1, "LIGHT_0001.fits")
            .is_none());

        // Not outside the image folders.
        let outside = fixture.dir.path().join("LIGHT_0001.fits");
        std::fs::copy(&found, &outside).unwrap();
        assert!(located_at(&outside)
            .located(ctx, 1, "LIGHT_0001.fits")
            .is_none());

        // Not once the file there changed.
        let changed = located_at(&found);
        std::fs::write(&found, b"another light").unwrap();
        assert!(changed.located(ctx, 1, "LIGHT_0001.fits").is_none());
    }

    #[test]
    fn a_light_gone_from_where_it_was_found_is_looked_up() {
        let fixture = fixture();
        let found = fixture.images.join("LIGHT_0001.fits");
        let old = fixture.images.join("old");
        std::fs::create_dir(&old).unwrap();
        std::fs::copy(&found, old.join("LIGHT_0001.fits")).unwrap();
        let mut lights = located_at(&old.join("LIGHT_0001.fits"));
        std::fs::remove_file(old.join("LIGHT_0001.fits")).unwrap();
        let (path, fingerprint) = lights
            .locate(
                &fixture.ctx,
                &fixture.image,
                "Target",
                "LIGHT_0001.fits",
                &MappedLightSources::default(),
            )
            .unwrap()
            .expect("the lookup finds it");
        assert_eq!(path, found);
        assert_eq!(fingerprint, super::super::source_fingerprint(&found));
        assert_eq!(lights.stamps[&1], FileStamp::of(&found));
        assert!(lights.found_by_lookup.contains(&1));

        // A light nowhere to be found is missing, not an error.
        std::fs::remove_file(&found).unwrap();
        let mut lights = KeptLights {
            headers: LightHeaders::default(),
            stamps: HashMap::new(),
            found_by_lookup: HashSet::new(),
        };
        assert!(lights
            .locate(
                &fixture.ctx,
                &fixture.image,
                "Target",
                "LIGHT_0001.fits",
                &MappedLightSources::default(),
            )
            .unwrap()
            .is_none());
    }
}
