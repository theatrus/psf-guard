use super::*;
use crate::calibration::{self, CalibrationKind, LibrarySnapshot};
use crate::server::director::collaboration_activation::{associations, matching};
use psf_guard_director_interop::collaboration::PreparedImport;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

fn held(message: &'static str) -> Failure {
    Failure(StatusCode::UNPROCESSABLE_ENTITY, message)
}

pub(super) async fn context(
    state: &Arc<AppState>,
    service: Arc<Service>,
    b: &ConnectionBinding,
    selection: &Selection,
) -> Result<(Arc<DatabaseContext>, PreparedImport), Failure> {
    let catalog = state
        .get_database(&selection.catalog)
        .ok_or(Error::Missing)?;
    let path = catalog.database_path.clone();
    let instance = service.instance_id;
    let identity = blocking(move || identity_of(instance, &path)).await?;
    let (rig, source, import_id, digest) = (
        b.rig_id,
        b.source().map_err(Error::from)?,
        selection.import_id,
        selection.source_digest.clone(),
    );
    let import = service
        .query(move |s| {
            if s.catalog_rig(identity.id)?.is_none_or(|r| r.rig.id != rig) {
                return Err(StoreError::Conflict);
            }
            let import = if let Some(digest) = digest {
                s.collaboration_import_revision(import_id, &digest)?
            } else {
                s.collaboration_import(import_id)?
            }
            .ok_or(StoreError::NotFound)?;
            if import.rig_id != rig || import.plan.source() != &source {
                return Err(StoreError::Conflict);
            }
            Ok(import.plan)
        })
        .await?;
    Ok((catalog, import))
}

fn rows(
    catalog: &DatabaseContext,
    import: &PreparedImport,
    selection: &Selection,
) -> Result<Vec<evidence::SelectedRow>, Failure> {
    let conn = crate::server::database_context::open_scheduler_connection_with_flags(
        &catalog.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| invalid())?;
    let rows = evidence::selected_rows(&conn, &selection.image_guids)?;
    let links = associations(&conn, import.import_id()).map_err(|_| invalid())?;
    if links.is_empty() {
        return Err(held(
            "Activate this visit before uploading its files; target identity is required",
        ));
    }
    let night = selection
        .observing_night
        .as_deref()
        .unwrap_or(import.night());
    psf_guard_director_interop::astrocollab::validate_night(night).map_err(|_| invalid())?;
    for (_, (image, _, target_guid)) in &rows {
        if image.grading_status != 1 {
            return Err(held(
                "A selected image is no longer accepted; review the package again",
            ));
        }
        let matched = matching(
            &links,
            target_guid.as_deref().unwrap_or(""),
            &image.filter_name,
            image.acquired_date.ok_or_else(invalid)?,
        );
        let (start, end) = super::super::workflows::evidence::night_bounds(night)?;
        if !(start..end).contains(&image.acquired_date.ok_or_else(invalid)?) {
            return Err(held(
                "A selected image is outside the reviewed observing night",
            ));
        }
        if matched.len() != 1
            || matched[0].source_digest != import.digest()
            || matched[0].panel != selection.panel
        {
            return Err(held(
                "A selected image does not belong to this assignment and panel",
            ));
        }
    }
    if rows
        .iter()
        .map(|(_, r)| r.0.target_id)
        .collect::<BTreeSet<_>>()
        .len()
        != 1
        || rows
            .iter()
            .map(|(_, r)| r.0.filter_name.clone())
            .collect::<BTreeSet<_>>()
            .len()
            != 1
    {
        return Err(held("Select one target and filter per transfer package"));
    }
    Ok(rows)
}

pub(super) fn outputs(
    catalog: &DatabaseContext,
    import: &PreparedImport,
    selection: &Selection,
) -> Result<Value, Failure> {
    let rows = rows(catalog, import, selection)?;
    let image = &rows.first().ok_or_else(invalid)?.1 .0;
    let ids = rows.iter().map(|(_, r)| r.0.id).collect::<BTreeSet<_>>();
    let entries = crate::server::stack_preview::contribution_outputs(
        catalog,
        image.project_id,
        image.target_id,
    );
    Ok(
        json!({"outputs":entries.into_iter().filter(|(e,_)|e.group.filter_name.eq_ignore_ascii_case(&image.filter_name)).map(|(e,_)|{
        let membership=e.group.frames.iter().filter(|f|matches!(f.disposition.as_str(),"reference"|"accepted")).map(|f|f.image_id).collect::<BTreeSet<_>>();
        json!({"job_id":e.job_id,"group_index":e.group.index,"revision":e.artifact_revision,"label":format!("{} · {} · {}",if e.wbpp.is_some(){"WBPP"}else{"PSF Guard"},e.group.target_name,e.group.filter_name),"wbpp":e.wbpp.is_some(),"matches_selection":membership==ids})}).collect::<Vec<_>>()}),
    )
}

/// Metadata-only revalidation uses the same selection as preparation, without
/// hashing every source again for each file in a large package.
type Manifest = (Value, Vec<(String, PathBuf)>, Vec<String>);

pub(super) fn manifest(
    catalog: &DatabaseContext,
    import: &PreparedImport,
    request: &Request,
    stopping: impl Fn() -> bool,
) -> Result<Manifest, Failure> {
    let rows = rows(catalog, import, &request.selection)?;
    let conn = crate::server::database_context::open_scheduler_connection_with_flags(
        &catalog.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| invalid())?;
    let library = LibrarySnapshot::load(&conn).map_err(|_| invalid())?;
    let links = associations(&conn, import.import_id()).map_err(|_| invalid())?;
    let tree = catalog
        .get_directory_tree()
        .map_err(|_| held("Image folders are unavailable"))?;
    let nights = calibration::Nights::load(&conn);
    let mut associations = Vec::new();
    let mut calibration_sets = BTreeMap::new();
    let mut files = BTreeMap::new();
    let mut review_reasons = BTreeSet::new();
    let mut warnings=vec!["Starfront stores these files but does not receive the per-light calibration manifest yet. The manifest is retained locally.".to_owned()];
    for (guid, (image, target, target_guid)) in &rows {
        if stopping() {
            return Err(Failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "Package preparation stopped for application shutdown",
            ));
        }
        let path = evidence::saved_path(catalog, image, target).map_err(|_| {
            Failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "A selected image has not arrived; retry scheduled",
            )
        })?;
        let light = crate::commands::import::headers::read_frame_meta(&path);
        if !light.readable {
            return Err(held("A selected image header is unreadable"));
        }
        let matched = matching(
            &links,
            target_guid.as_deref().unwrap_or(""),
            &image.filter_name,
            image.acquired_date.ok_or_else(invalid)?,
        );
        let exposure_ms = light
            .exposure_s
            .filter(|v| v.is_finite() && *v > 0.0 && *v <= 86400.0)
            .map(|v| (v * 1000.0).round() as u64);
        if matched.len() != 1
            || exposure_ms.is_none_or(|ms| {
                !psf_guard_director_interop::collaboration::exposure_matches(
                    matched[0].exposure_ms,
                    ms,
                )
            })
        {
            return Err(held(
                "A selected frame's exposure does not match its assignment",
            ));
        }
        let selected = library.select(&light);
        let gap = calibration::calibration_gap(&conn, &light, &selected, &nights)
            .map_err(|_| invalid())?;
        if request.output.is_none() && gap.is_some() && calibration::exclude_uncalibrated_enabled()
        {
            return Err(held(
                "A selected light would be left out by the calibration export policy",
            ));
        }
        if request.output.is_none() && gap.is_some() {
            review_reasons.insert("Some lights have incomplete calibration. Review the selected files before uploading.".to_owned());
        }
        if request.output.is_none() && selected.flat_basis.correction().is_some() {
            return Err(held("This package needs corrected flats. Select an existing PSF Guard or WBPP stack instead of raw files"));
        }
        let mut selected = selected;
        if calibration::remap_missing_sources(&mut selected, Some(&tree)) > 0
            && request.output.is_none()
        {
            return Err(Failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "Matched calibration files are missing; retry scheduled",
            ));
        }
        let selected = calibration::delivery_selection(selected);
        let mut kinds = BTreeMap::<String, Vec<String>>::new();
        for (kind, frames) in [
            (CalibrationKind::Bias, &selected.bias),
            (CalibrationKind::Dark, &selected.dark),
            (CalibrationKind::DarkFlat, &selected.dark_flat),
            (CalibrationKind::Flat, &selected.flat),
        ] {
            let wire = if kind == CalibrationKind::DarkFlat {
                "darkflat"
            } else {
                kind.as_str()
            };
            for frame in frames {
                kinds
                    .entry(wire.into())
                    .or_default()
                    .push(frame.frame_uuid.clone());
                if request.output.is_none() {
                    files.insert((wire.to_owned(), frame.source_path.clone()), ());
                }
            }
        }
        let calibration_set = json!({"calibration":kinds,"flat_basis":selected.flat_basis,"rules_digest":selected.rules.matching_digest(),"rules":selected.rules.notes(),"gap":gap});
        let key = psf_guard_director_interop::collaboration::digest(
            &serde_json::to_vec(&calibration_set).map_err(|_| invalid())?,
        );
        calibration_sets
            .entry(key.clone())
            .or_insert(calibration_set);
        associations.push(json!({"image_guid":guid,"image_id":image.id,"grade":image.grading_status,
            "target_guid":target_guid,"target_id":image.target_id,"filter":image.filter_name,
            "captured_at":image.acquired_date,"profile_id":image.profile_id,"exposure_seconds":light.exposure_s,
            "source":stamp(&path)?,"calibration_set":key}));
        if request.output.is_none() {
            files.insert(("light".to_owned(), path), ());
        }
    }
    let mut output_manifest = Value::Null;
    if let Some(output) = &request.output {
        let image = &rows[0].1 .0;
        let (entry, path) = crate::server::stack_preview::contribution_outputs(
            catalog,
            image.project_id,
            image.target_id,
        )
        .into_iter()
        .find(|(e, _)| {
            e.job_id == output.job_id
                && e.group.index == output.group_index
                && e.artifact_revision == output.revision
                && e.group.filter_name.eq_ignore_ascii_case(&image.filter_name)
        })
        .ok_or_else(|| held("The selected stack changed or is unavailable; select it again"))?;
        if entry.wbpp.is_some() {
            if !request.confirm_wbpp_membership {
                return Err(held("Confirm the WBPP stack contains the selected contribution images. WBPP does not record exact input membership"));
            }
            warnings.push("WBPP input membership is operator-confirmed, not verified. Uploading this stack grants no additional exposure credit.".into());
        } else {
            let integrated = entry
                .group
                .frames
                .iter()
                .filter(|f| matches!(f.disposition.as_str(), "reference" | "accepted"))
                .map(|f| f.image_id)
                .collect::<BTreeSet<_>>();
            if integrated != rows.iter().map(|(_, r)| r.0.id).collect() {
                return Err(held(
                    "Select exactly the accepted images integrated in this PSF Guard stack",
                ));
            }
            for (_, (image, target, _)) in &rows {
                let path = evidence::saved_path(catalog, image, target)?;
                let fingerprint = crate::server::stack_preview::source_fingerprint(&path);
                if entry
                    .group
                    .frames
                    .iter()
                    .filter(|f| f.image_id == image.id)
                    .all(|f| f.source_fingerprint.as_deref() != Some(&fingerprint))
                {
                    return Err(held("A stack input changed or has no saved fingerprint; rebuild the stack before uploading"));
                }
            }
        }
        output_manifest = json!({"job_id":entry.job_id,"revision":entry.artifact_revision,"group":entry.group,"wbpp":entry.wbpp,"source":stamp(&path)?});
        files.insert(("light".to_owned(), path), ());
        warnings.push("This is an integrated stack, not a raw exposure. File delivery does not create a contribution report.".into());
    }
    let paths = files.into_keys().collect::<Vec<_>>();
    warnings.extend(review_reasons.iter().cloned());
    let stamps = paths
        .iter()
        .map(|(kind, path)| Ok(json!({"kind":kind,"source":stamp(path)?})))
        .collect::<Result<Vec<_>, Failure>>()?;
    Ok((
        json!({"version":1,"task":import.share().task_id,"source_digest":import.digest(),"night":request.selection.observing_night,
        "panel":request.selection.panel,"lights":associations,"calibration_sets":calibration_sets,"output":output_manifest,"files":stamps,"review_reasons":review_reasons}),
        paths,
        warnings,
    ))
}

pub(super) fn source_version(source: &Value) -> Result<String, Failure> {
    Ok(psf_guard_director_interop::collaboration::digest(
        &serde_json::to_vec(source).map_err(|_| invalid())?,
    ))
}

/// Admission uses cheap filesystem stamps, never a read of every old frame.
/// Delivery still hashes the bytes and records that content revision.
pub(super) fn source_versions(
    catalog: &DatabaseContext,
    selection: &Selection,
) -> Result<BTreeMap<Uuid, String>, Failure> {
    let conn = crate::server::database_context::open_scheduler_connection_with_flags(
        &catalog.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| invalid())?;
    let mut versions = BTreeMap::new();
    for ids in selection
        .image_guids
        .chunks(psf_guard_director_interop::collaboration::MAX_REPORT_FRAMES)
    {
        for (guid, (image, target, _)) in evidence::selected_rows(&conn, ids)? {
            if let Ok(source) =
                evidence::saved_path(catalog, &image, &target).and_then(|path| stamp(&path))
            {
                versions.insert(guid, source_version(&source)?);
            }
        }
    }
    Ok(versions)
}

fn stamp(path: &std::path::Path) -> Result<Value, Failure> {
    let meta = std::fs::metadata(path)
        .map_err(|_| held("A package source file is unavailable; originals were not changed"))?;
    if !meta.is_file() || meta.len() == 0 {
        return Err(held("A package source is not a completed file"));
    }
    Ok(
        json!({"path":path,"bytes":meta.len(),"modified":meta.modified().ok().and_then(|m|m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d|d.as_nanos().to_string())}),
    )
}

#[cfg(test)]
pub(super) fn hash_file(path: &std::path::Path) -> Result<(String, u64, u128), Failure> {
    hash_with_stop(path, || false)
}

fn hash_with_stop(
    path: &std::path::Path,
    stopping: impl Fn() -> bool,
) -> Result<(String, u64, u128), Failure> {
    crate::image_io::read_header(path)
        .map_err(|_| held("A package source is not a readable FITS/XISF frame"))?;
    let before = stamp(path)?;
    let mut file =
        std::fs::File::open(path).map_err(|_| held("A package source file is unavailable"))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        if stopping() {
            return Err(Failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "File preparation stopped for application shutdown",
            ));
        }
        let n = file
            .read(&mut buffer)
            .map_err(|_| held("Could not read a package file"))?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    if stamp(path)? != before {
        return Err(held("A source file changed while preparing the transfer"));
    }
    let digest = hash
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Ok((
        digest,
        before["bytes"].as_u64().ok_or_else(invalid)?,
        before["modified"]
            .as_str()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    ))
}

pub(super) fn build(
    catalog: &DatabaseContext,
    import: &PreparedImport,
    request: &Request,
    catalog_id: Uuid,
    rig_id: Uuid,
    stopping: impl Fn() -> bool,
) -> Result<Package, Failure> {
    let (manifest, paths, warnings) = manifest(catalog, import, request, &stopping)?;
    let guard = psf_guard_director_interop::collaboration::digest(
        &serde_json::to_vec(&manifest).map_err(|_| invalid())?,
    );
    let mut artifacts = Vec::new();
    let mut names = BTreeSet::new();
    for (kind, path) in paths {
        let (hash, bytes, modified) = hash_with_stop(&path, &stopping)?;
        // Content-addressed names prevent Starfront's same-name/same-size
        // shortcut from confusing different rigs or different file revisions.
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(invalid)?
            .to_ascii_lowercase();
        let role = if request.output.is_some() {
            "stack"
        } else {
            &kind
        };
        let name = format!("psfg-{role}-{hash}.{extension}");
        if names.insert(name.clone()) {
            artifacts.push(Artifact {
                name,
                kind,
                path,
                bytes,
                modified,
                hash,
            });
        }
    }
    Ok(Package {
        staging_root: catalog.cache_dir_path.join("transfers"),
        owner: None,
        catalog_id,
        rig_id,
        base_url: import.source().base_url().into(),
        agent_id: import.source().agent_id().into(),
        project: import.share().project_id.clone(),
        label: import
            .share()
            .name
            .clone()
            .unwrap_or_else(|| import.share().project_id.clone()),
        catalog: catalog.id.clone(),
        manifest,
        guard,
        artifacts,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza_fits::{F32ImageData, HeaderValue, WriteHeaderCard};
    fn frame(path: &std::path::Path, kind: &str, filter: &str, exposure: f64, date: &str) {
        seiza_fits::write_f32_image(
            path,
            20,
            20,
            F32ImageData::Mono(&vec![1000.0; 400]),
            &[
                WriteHeaderCard::new("IMAGETYP", HeaderValue::String(kind.into())),
                WriteHeaderCard::new("FILTER", HeaderValue::String(filter.into())),
                WriteHeaderCard::new("EXPTIME", HeaderValue::Float(exposure)),
                WriteHeaderCard::new("DATE-OBS", HeaderValue::String(date.into())),
                WriteHeaderCard::new("INSTRUME", HeaderValue::String("Test camera".into())),
                WriteHeaderCard::new("GAIN", HeaderValue::Integer(100)),
                WriteHeaderCard::new("OFFSET", HeaderValue::Integer(30)),
                WriteHeaderCard::new("XBINNING", HeaderValue::Integer(1)),
                WriteHeaderCard::new("YBINNING", HeaderValue::Integer(1)),
            ],
        )
        .unwrap();
    }

    #[test]
    fn transfer_package_matches_export_inputs_and_rechecks_grades() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("catalog.sqlite");
        let mut conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE acquiredimage(Id INTEGER PRIMARY KEY,guid TEXT,gradingStatus INTEGER,filtername TEXT,metadata TEXT,acquireddate INTEGER,targetId INTEGER DEFAULT 1,projectId INTEGER DEFAULT 1,profileId TEXT); CREATE TABLE target(Id INTEGER PRIMARY KEY,guid TEXT,name TEXT DEFAULT 'Target'); INSERT INTO target(Id,guid) VALUES(1,'11111111-1111-4111-8111-111111111111')").unwrap();
        let source = Source::new("https://example.com/", "000000000001", false).unwrap();
        let import = psf_guard_director_interop::collaboration::prepare_import(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/crates/director-interop/tests/fixtures/starfront-tonight.json"
            )),
            &source,
            "2026-10-05",
            "000000000004",
        )
        .unwrap();
        conn.execute_batch(crate::server::director::collaboration_activation::DDL)
            .unwrap();
        conn.execute("INSERT INTO psf_guard_collaboration_plan VALUES('plan','11111111-1111-4111-8111-111111111111',?1,?2,0,'O',300000,1791170000000,NULL,11)",rusqlite::params![import.import_id().to_string(),import.digest()]).unwrap();
        let guid = Uuid::new_v4();
        let light = dir.path().join("light.fits");
        frame(&light, "LIGHT", "OIII", 300.0, "2026-10-05T04:50:00");
        conn.execute("INSERT INTO acquiredimage(Id,guid,gradingStatus,filtername,metadata,acquireddate) VALUES(1,?1,1,'OIII',?2,1791171000)",rusqlite::params![guid.to_string(),json!({"FileName":"light.fits"}).to_string()]).unwrap();
        let mut calibration_frames = Vec::new();
        for (name, kind, filter, exposure) in [
            ("flat", "FLAT", "OIII", 1.0),
            ("unrelated", "FLAT", "R", 1.0),
            ("darkflat", "DARKFLAT", "OIII", 1.0),
            ("dark", "DARK", "OIII", 300.0),
            ("bias", "BIAS", "OIII", 0.0),
        ] {
            let path = dir.path().join(format!("{name}.fits"));
            frame(&path, kind, filter, exposure, "2026-10-05T12:00:00");
            calibration_frames.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let tx = conn.transaction().unwrap();
        calibration::import_calibration_frames(&tx, &calibration_frames, Some("profile")).unwrap();
        tx.commit().unwrap();
        let catalog = DatabaseContext::new(
            "catalog".into(),
            "Catalog".into(),
            db_path.to_string_lossy().into(),
            vec![dir.path().to_string_lossy().into()],
            None,
            None,
            None,
            dir.path().join("cache"),
        )
        .unwrap();
        let request = Request {
            selection: Selection {
                import_id: import.import_id(),
                catalog: "catalog".into(),
                panel: 0,
                image_guids: vec![guid],
                source_digest: None,
                observing_night: None,
            },
            output: None,
            confirm_wbpp_membership: false,
            automatic: false,
            upload_pending: false,
            attempt: None,
            source_versions: BTreeMap::new(),
        };
        let mut pending = request.clone();
        pending.upload_pending = true;
        assert!(!pending.review_required(&json!({"review_reasons":[]})));
        assert!(!pending.review_required(&json!({"warnings":["Calibration manifest stays local"]})));
        assert!(pending.review_required(&json!({"review_reasons":["Incomplete calibration"]})));
        assert!(request.review_required(&json!({"review_reasons":[]}))); // old Prepare requests retain their consent boundary
        let mut automatic = request.clone();
        automatic.automatic = true;
        assert!(!automatic.review_required(&json!({"review_reasons":[]})));
        assert!(automatic.review_required(&json!({"review_reasons":["Incomplete calibration"]})));
        let (snapshot, paths, _) = manifest(&catalog, &import, &request, || false).unwrap();
        let meta = crate::commands::import::headers::read_frame_meta(&light);
        let export = calibration::export_destinations(
            &conn,
            &meta,
            "Target",
            None,
            crate::commands::export::ExportLayout::Standard,
            &calibration::Nights::load(&conn),
            false,
        )
        .unwrap();
        let expected = export
            .items
            .iter()
            .map(|(_, frame, _)| frame.source_path.clone())
            .collect::<BTreeSet<_>>();
        let actual = paths
            .iter()
            .filter(|(kind, _)| kind != "light")
            .map(|(_, path)| path.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);
        assert!(!actual.contains(&dir.path().join("unrelated.fits")));
        assert!(!actual.is_empty());
        assert_eq!(snapshot["lights"][0]["image_guid"], guid.to_string());
        let second_guid = Uuid::new_v4();
        std::fs::copy(&light, dir.path().join("second.fits")).unwrap();
        conn.execute("INSERT INTO acquiredimage(Id,guid,gradingStatus,filtername,metadata,acquireddate) VALUES(2,?1,1,'OIII',?2,1791171000)", rusqlite::params![second_guid.to_string(), json!({"FileName":"second.fits","UnrelatedQualityCache":"x".repeat(100000)}).to_string()]).unwrap();
        let mut combined = request.clone();
        combined.selection.image_guids.push(second_guid);
        let (compact, _, _) = manifest(&catalog, &import, &combined, || false).unwrap();
        assert_eq!(compact["calibration_sets"].as_object().unwrap().len(), 1);
        assert_eq!(
            compact["lights"][0]["calibration_set"],
            compact["lights"][1]["calibration_set"]
        );
        assert!(serde_json::to_vec(&compact).unwrap().len() < 20000);
        let mut large = request.selection.clone();
        let tx = conn.transaction().unwrap();
        for id in 3..5003 {
            let guid = Uuid::new_v4();
            large.image_guids.push(guid);
            tx.execute("INSERT INTO acquiredimage(Id,guid,gradingStatus,filtername,metadata,acquireddate) VALUES(?1,?2,1,'OIII',?3,1791171000)", rusqlite::params![id,guid.to_string(),json!({"FileName":"light.fits"}).to_string()]).unwrap();
        }
        tx.commit().unwrap();
        assert_eq!(source_versions(&catalog, &large).unwrap().len(), 5001);
        let before = source_versions(&catalog, &request.selection).unwrap();
        let package = build(
            &catalog,
            &import,
            &request,
            Uuid::new_v4(),
            Uuid::new_v4(),
            || false,
        )
        .unwrap();
        assert!(package.artifacts.iter().any(|a| a.kind == "light"));
        assert!(package.artifacts.iter().all(|a| a.name.contains(&a.hash)));
        assert!(build(
            &catalog,
            &import,
            &request,
            Uuid::new_v4(),
            Uuid::new_v4(),
            || true
        )
        .is_err());
        conn.execute("UPDATE acquiredimage SET gradingStatus=2", [])
            .unwrap();
        assert!(manifest(&catalog, &import, &request, || false).is_err());
        conn.execute("UPDATE acquiredimage SET gradingStatus=1", [])
            .unwrap();
        frame(&light, "LIGHT", "OIII", 150.0, "2026-10-05T04:50:00");
        assert_ne!(
            source_versions(&catalog, &request.selection).unwrap(),
            before
        );
        let error = manifest(&catalog, &import, &request, || false).unwrap_err();
        assert!(error.1.contains("exposure does not match"));
    }
}
