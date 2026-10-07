//! Catalog-derived contribution evidence; the browser supplies identities only.
use super::*;
use crate::astrometry::{wcs_from_response, AstrometrySolutionResponse};
use collaboration::{FrameEvidence, MeasuredFootprint, PreparedImport};
use std::collections::BTreeSet;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    pub import_id: Uuid,
    pub catalog: String,
    pub panel: u32,
    pub image_guids: Vec<Uuid>,
}
fn held() -> Failure {
    Failure(StatusCode::UNPROCESSABLE_ENTITY,"Report held: select accepted, saved images with stable GUIDs, matching exposures and fresh pixel solves; required calibration evidence must be present")
}
pub(super) async fn inputs(
    state: &AppState,
    service: Arc<Service>,
    b: &ConnectionBinding,
) -> Result<Value, Failure> {
    let catalogs = state
        .databases
        .read()
        .map_err(|_| invalid())?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let instance = service.instance_id;
    let identified = blocking(move || identified_catalogs(&catalogs, instance)).await?;
    let id = b.id;
    let rig = b.rig_id;
    service.query(move|s|{
        let mut catalogs=Vec::new();
        for (id,(_,catalog)) in identified.by_id {
            if s.catalog_rig(id)?.is_some_and(|r|r.rig.id==rig) {catalogs.push(json!({"id":catalog.id,"name":catalog.name}));}
        }
        let imports=s.collaboration_imports_for_connection(id)?.into_iter().map(|i|json!({"id":i.plan.import_id(),"name":i.plan.share().name,"night":i.plan.night(),"panels":i.plan.share().panel_order})).collect::<Vec<_>>();
        Ok(json!({"catalogs":catalogs,"imports":imports}))
    }).await.map_err(Into::into)
}
pub(super) async fn candidates(
    state: &AppState,
    service: Arc<Service>,
    b: &ConnectionBinding,
    import_id: Uuid,
    slug: &str,
) -> Result<Value, Failure> {
    let catalog = state.get_database(slug).ok_or(Error::Missing)?;
    let path = catalog.database_path.clone();
    let instance = service.instance_id;
    let identity = blocking(move || identity_of(instance, &path)).await?;
    let source = b.source().map_err(Error::from)?;
    let rig = b.rig_id;
    let import = service
        .query(move |s| {
            if s.catalog_rig(identity.id)?.is_none_or(|r| r.rig.id != rig) {
                return Err(StoreError::Conflict);
            }
            let import = s
                .collaboration_import(import_id)?
                .ok_or(StoreError::NotFound)?;
            if import.rig_id != rig || import.plan.source() != &source {
                return Err(StoreError::Conflict);
            }
            Ok(import.plan)
        })
        .await?;
    blocking(move||{
        let conn=crate::server::database_context::open_scheduler_connection_with_flags(&catalog.database_path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_|held())?;
        conn.busy_timeout(Duration::from_secs(2)).map_err(|_|held())?;
        let date=chrono::NaiveDate::parse_from_str(import.night(),"%Y-%m-%d").map_err(|_|held())?.and_hms_opt(0,0,0).ok_or_else(held)?.and_utc().timestamp();
        let mut statement=conn.prepare("SELECT ai.guid,ai.filtername,ai.acquireddate,ai.metadata,t.name FROM acquiredimage ai JOIN target t ON t.Id=ai.targetId WHERE ai.gradingStatus=1 AND ai.guid IS NOT NULL AND ai.acquireddate>=?1 AND ai.acquireddate<?2 ORDER BY ai.acquireddate,ai.Id LIMIT 4097").map_err(|_|held())?;
        let mut result=Vec::new();
        for row in statement.query_map([date-12*3600,date+60*3600],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).map_err(|_|held())? {
            let (guid,filter,time,metadata,target)=row.map_err(|_|held())?;
            let Ok(guid)=Uuid::parse_str(&guid) else {continue;};
            let metadata:Value=serde_json::from_str(&metadata).unwrap_or(Value::Null);
            let name=metadata["FileName"].as_str().and_then(|s|std::path::Path::new(s).file_name()).and_then(|s|s.to_str()).unwrap_or("Unknown file");
            result.push(json!({"guid":guid,"filter":filter,"captured_at":time,"target":target,"file":name}));
        }
        if result.len()>4096 {return Err(Failure(StatusCode::UNPROCESSABLE_ENTITY,"Too many candidate images; narrow this catalog before preparing a report"));}
        Ok(json!({"images":result}))
    }).await?
}
pub(super) async fn review(
    state: &AppState,
    service: Arc<Service>,
    b: &ConnectionBinding,
    selection: Selection,
    expected: Option<String>,
) -> Result<Value, Failure> {
    if selection.image_guids.is_empty()
        || selection.image_guids.len() > collaboration::MAX_REPORT_FRAMES
        || selection.image_guids.iter().any(Uuid::is_nil)
        || selection.image_guids.iter().collect::<BTreeSet<_>>().len()
            != selection.image_guids.len()
    {
        return Err(invalid());
    }
    let catalog = state
        .get_database(&selection.catalog)
        .ok_or(Error::Missing)?;
    let instance = service.instance_id;
    let path = catalog.database_path.clone();
    let identity = blocking(move || identity_of(instance, &path)).await?;
    let import_id = selection.import_id;
    let rig = b.rig_id;
    let source = b.source().map_err(Error::from)?;
    let import = service
        .clone()
        .query(move |s| {
            if s.catalog_rig(identity.id)?.is_none_or(|r| r.rig.id != rig) {
                return Err(StoreError::Conflict);
            }
            let import = s
                .collaboration_import(import_id)?
                .ok_or(StoreError::NotFound)?;
            if import.rig_id != rig || import.plan.source() != &source {
                return Err(StoreError::Conflict);
            }
            Ok(import.plan)
        })
        .await?;
    let (finalized, review_digest) = blocking(move || {
        let frames = frames(&catalog, &import, &selection)?;
        let finalized =
            collaboration::finalize_contribution(&import, &frames).map_err(|_| held())?;
        let digest = collaboration::digest(&serde_json::to_vec(&frames).map_err(|_| held())?);
        Ok::<_, Failure>((finalized, digest))
    })
    .await??;
    let payload = json!(finalized.report());
    let images = finalized.images().to_vec();
    if let Some(expected) = expected {
        if expected != review_digest {
            return Err(Failure(
                StatusCode::CONFLICT,
                "Image evidence changed; review the contribution again",
            ));
        }
        let now = now_ms()?;
        let queued = service
            .run(move |s| s.queue_collaboration_report(&finalized, now))
            .await?;
        return Ok(json!({"queued":queued,"report":payload,"review_digest":review_digest}));
    }
    Ok(json!({"report":payload,"image_guids":images,"review_digest":review_digest}))
}
fn frames(
    catalog: &DatabaseContext,
    import: &PreparedImport,
    selection: &Selection,
) -> Result<Vec<FrameEvidence>, Failure> {
    let conn = crate::server::database_context::open_scheduler_connection_with_flags(
        &catalog.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| held())?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|_| held())?;
    let mut rows = Vec::new();
    let mut guids = selection.image_guids.clone();
    guids.sort();
    for guid in &guids {
        let mut statement=conn.prepare("SELECT Id,gradingStatus,filtername,metadata,acquireddate FROM acquiredimage WHERE guid=?1 COLLATE NOCASE LIMIT 2").map_err(|_|held())?;
        let mut matches = statement
            .query_map([guid.to_string()], |r| {
                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, i32>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            })
            .map_err(|_| held())?;
        let row = matches.next().ok_or_else(held)?.map_err(|_| held())?;
        if matches.next().is_some() {
            return Err(held());
        }
        rows.push((*guid, row));
    }
    drop(conn);
    let mut solutions = Vec::new();
    let mut frames = Vec::new();
    for (guid, (id, grade, filter, metadata, captured)) in rows {
        if grade != 1 {
            return Err(held());
        }
        let metadata: Value = serde_json::from_str(&metadata).map_err(|_| held())?;
        let path = catalog.get_image_path(metadata["FileName"].as_str().ok_or_else(held)?);
        let path = dunce::canonicalize(path).map_err(|_| held())?;
        let analysis = catalog
            .astrometry_evidence
            .evidence_for_source(&catalog.cache_dir_path, id, None)
            .ok_or_else(held)?;
        if dunce::canonicalize(&analysis.source_fingerprint.canonical_path).map_err(|_| held())?
            != path
        {
            return Err(held());
        }
        let solution = analysis.solution.ok_or_else(held)?;
        let fingerprint = collaboration::digest(
            &serde_json::to_vec(&analysis.source_fingerprint).map_err(|_| held())?,
        );
        let headers = crate::astrometry_headers::FitsAstrometryHeaders::from_path(&path)
            .map_err(|_| held())?;
        let frame_header = crate::image_io::read_frame_header(&path, &path).map_err(|_| held())?;
        let class = crate::image_io::classify_frame(&path, &path, &frame_header);
        if class.kind == crate::image_io::FrameKind::Integration {
            return Err(held());
        }
        let calibrated = class.evidence == crate::image_io::KindEvidence::Header
            && class
                .steps
                .contains(crate::image_io::ProcessingSteps::CALIBRATED);
        // Describe the saved pixels, not the rig's current camera configuration.
        let frame_colour = frame_is_colour(&frame_header);
        if catalog
            .astrometry_evidence
            .evidence_for_source(&catalog.cache_dir_path, id, None)
            .is_none_or(|current| current.source_fingerprint != analysis.source_fingerprint)
        {
            return Err(held());
        }
        let exposure = headers
            .exposure_seconds
            .as_ref()
            .map(|v| v.value)
            .filter(|v| v.is_finite() && *v > 0.0 && *v <= 86400.0)
            .ok_or_else(held)?;
        // The named night comes from explicit operator review, not UTC-date
        // guessing. Reject unrelated captures while allowing time-zone offsets.
        let date = chrono::NaiveDate::parse_from_str(import.night(), "%Y-%m-%d")
            .map_err(|_| held())?
            .and_hms_opt(0, 0, 0)
            .ok_or_else(held)?
            .and_utc()
            .timestamp();
        if captured.is_none_or(|time| !(date - 12 * 3600..date + 60 * 3600).contains(&time)) {
            return Err(held());
        }
        let number = |key: &str| {
            metadata[key]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)
        };
        frames.push(FrameEvidence {
            capture_id: Uuid::new_v5(&Uuid::NAMESPACE_OID, guid.as_bytes()),
            image_guid: guid,
            source_digest: import.digest().into(),
            panel_index: selection.panel,
            filter,
            exposure_ms: (exposure * 1000.0).round() as u64,
            saved: true,
            accepted: true,
            finalized: true,
            image_fingerprint: fingerprint.clone(),
            solve_fingerprint: fingerprint,
            solved_footprint: MeasuredFootprint {
                ra: solution.center_ra_deg,
                dec: solution.center_dec_deg,
                width: 1.0,
                height: 1.0,
                rotation: 0.0,
            },
            scale_arcsec: Some(solution.pixel_scale_arcsec_per_pixel),
            focal_length_mm: headers.focal_length_mm.map(|v| v.value),
            hfr_arcsec: if class.kind.is_derivative() {
                None
            } else {
                number("HFR").map(|hfr| hfr * solution.pixel_scale_arcsec_per_pixel)
            },
            guide_rms_arcsec: None,
            moon_illumination: None,
            moon_separation_degrees: None,
            calibrated,
            bandpass_nm: None,
            colour: frame_colour,
        });
        solutions.push(solution);
    }
    let footprint = common_footprint(&solutions).ok_or_else(held)?;
    for frame in &mut frames {
        frame.solved_footprint = footprint.clone();
    }
    frames.sort_by_key(|f| f.image_guid);
    Ok(frames)
}
fn frame_is_colour(header: &crate::image_io::FrameHeader) -> bool {
    header
        .cards
        .iter()
        .any(|(key, value)| match (key.as_str(), value) {
            ("BAYERPAT", seiza_fits::HeaderValue::String(pattern)) => !pattern.trim().is_empty(),
            ("NAXIS3", seiza_fits::HeaderValue::Integer(channels)) => *channels >= 3,
            _ => false,
        })
}
/// Find a conservative north-up square contained in every fresh TAN solution.
/// Seiza owns the projection. A fixed one-percent margin keeps numerical and
/// image-boundary errors out of reported coverage; disjoint fields are held.
fn common_footprint(solutions: &[AstrometrySolutionResponse]) -> Option<MeasuredFootprint> {
    let first = solutions.first()?;
    let ra = first.center_ra_deg;
    let dec = first.center_dec_deg;
    if !ra.is_finite() || !dec.is_finite() || dec.abs() > 85.0 {
        return None;
    }
    let wcs: Vec<_> = solutions
        .iter()
        .map(|s| (wcs_from_response(&s.wcs), s.image_width, s.image_height))
        .collect();
    let contains = |half: f64| {
        // Sample edges as well as corners: constant-declination edges curve in TAN.
        (-8..=8).all(|ix| {
            (-8..=8).all(|iy| {
                let x = f64::from(ix) / 8.0;
                let y = f64::from(iy) / 8.0;
                let corner_ra = (ra + x * half / dec.to_radians().cos()).rem_euclid(360.0);
                let corner_dec = dec + y * half;
                wcs.iter().all(|(w, width, height)| {
                    w.world_to_pixel(corner_ra, corner_dec)
                        .is_some_and(|(px, py)| {
                            px.is_finite()
                                && py.is_finite()
                                && px >= 1.0
                                && py >= 1.0
                                && px < *width as f64 - 2.0
                                && py < *height as f64 - 2.0
                        })
                })
            })
        })
    };
    if !contains(0.0) {
        return None;
    }
    let (mut low, mut high) = (0.0, 15.0);
    for _ in 0..48 {
        let middle = (low + high) / 2.0;
        if contains(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    let side = low * 1.98;
    if side < 0.00001 {
        return None;
    }
    Some(MeasuredFootprint {
        ra,
        dec,
        width: side,
        height: side,
        rotation: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn solution(ra: f64) -> AstrometrySolutionResponse {
        serde_json::from_value(json!({"center_ra_deg":ra,"center_dec_deg":20.0,"pixel_scale_arcsec_per_pixel":3.6,"matched_stars":30,"rms_arcsec":0.2,"image_width":1000,"image_height":1000,
            "wcs":{"crval":[ra,20.0],"crpix":[500.0,500.0],"cd":[[-0.001,0.0],[0.0,0.001]],"ctype":["RA---TAN","DEC--TAN"],"cunit":["deg","deg"],"radesys":"ICRS","equinox":2000.0},"footprint":[],"objects":[]})).unwrap()
    }
    #[test]
    fn common_coverage_shrinks_with_offsets_and_holds_disjoint_fields() {
        let one = common_footprint(&[solution(359.9)]).unwrap();
        let two = common_footprint(&[solution(359.9), solution(0.05)]).unwrap();
        assert!(two.width < one.width);
        assert!(common_footprint(&[solution(10.0), solution(30.0)]).is_none());
    }
    #[test]
    fn colour_requires_saved_pixel_evidence() {
        use crate::image_io::FrameHeader;
        use seiza_fits::HeaderValue;
        assert!(!frame_is_colour(&FrameHeader::default()));
        for (key, value) in [
            ("BAYERPAT", HeaderValue::String("RGGB".into())),
            ("NAXIS3", HeaderValue::Integer(3)),
        ] {
            assert!(frame_is_colour(&FrameHeader {
                cards: vec![(key.into(), value)],
                ..Default::default()
            }));
        }
    }
    #[test]
    fn catalog_evidence_uses_actual_headers_and_holds_duplicate_or_stale_sources() {
        use seiza_fits::{F32ImageData, HeaderValue, WriteHeaderCard};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved.fits");
        seiza_fits::write_f32_image(
            &path,
            20,
            20,
            F32ImageData::Mono(&vec![1.0; 400]),
            &[
                WriteHeaderCard::new("EXPTIME", HeaderValue::Float(300.0)),
                WriteHeaderCard::new("IMAGETYP", HeaderValue::String("LIGHT".into())),
            ],
        )
        .unwrap();
        let db_path = dir.path().join("catalog.sqlite");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE acquiredimage(Id INTEGER PRIMARY KEY,guid TEXT,gradingStatus INTEGER,filtername TEXT,metadata TEXT,acquireddate INTEGER)").unwrap();
        let guid = Uuid::new_v4();
        conn.execute(
            "INSERT INTO acquiredimage VALUES(1,?1,1,'Ha',?2,1791171000)",
            rusqlite::params![
                guid.to_string(),
                json!({"FileName":"saved.fits","HFR":2.0}).to_string()
            ],
        )
        .unwrap();
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
        let source = path.canonicalize().unwrap();
        let metadata = std::fs::metadata(&source).unwrap();
        let modified = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let mut solved = solution(10.0);
        solved.image_width = 20;
        solved.image_height = 20;
        solved.wcs.crpix = [10.0, 10.0];
        let analysis = serde_json::from_value(json!({"image_id":1,"status":"solved","mode":"hinted","solution":solved,
            "source_fingerprint":{"canonical_path":source.to_string_lossy(),"size_bytes":metadata.len(),"modified_unix_seconds":modified.as_secs(),"modified_subsec_nanos":modified.subsec_nanos()},
            "solve_attempt":{"outcome":"solved","modes_attempted":["hinted"],"detected_stars":30,"duration_ms":5,"image_quality_evidence":true,"cacheable":true},"computed_at":1791171000})).unwrap();
        crate::astrometry::persist_pixel_analysis(&catalog.cache_dir_path, &analysis).unwrap();
        let source = Source::new("https://collab.example/", "000000000001", false).unwrap();
        let import = collaboration::prepare_import(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/crates/director-interop/tests/fixtures/starfront-tonight.json"
            )),
            &source,
            "2026-10-05",
            "000000000004",
        )
        .unwrap();
        let selection = Selection {
            import_id: import.import_id(),
            catalog: "catalog".into(),
            panel: 0,
            image_guids: vec![guid],
        };
        let extracted = frames(&catalog, &import, &selection).unwrap();
        assert_eq!(extracted[0].exposure_ms, 300_000);
        assert_eq!(extracted[0].hfr_arcsec, Some(7.2));
        assert!(!extracted[0].calibrated);
        assert!(!extracted[0].colour);
        conn.execute("INSERT INTO acquiredimage SELECT 2,guid,gradingStatus,filtername,metadata,acquireddate FROM acquiredimage WHERE Id=1", []).unwrap();
        assert!(frames(&catalog, &import, &selection).is_err());
        conn.execute("DELETE FROM acquiredimage WHERE Id=2", [])
            .unwrap();
        std::fs::write(&path, b"replaced image").unwrap();
        assert!(frames(&catalog, &import, &selection).is_err());
    }
}
