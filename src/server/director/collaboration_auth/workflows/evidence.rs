//! Catalog-derived contribution evidence; the browser supplies identities only.
use super::*;
use crate::astrometry::{wcs_from_response, AstrometrySolutionResponse};
use crate::server::director::collaboration_activation::{associations, matching};
use collaboration::{FrameEvidence, MeasuredFootprint, PreparedImport};
use std::collections::{BTreeMap, BTreeSet};
mod automatic;
mod measurements;
pub(in crate::server::director::collaboration_auth) use automatic::{automatic, AutomaticResult};

struct CatalogFrames {
    frames: Vec<FrameEvidence>,
    director: Vec<DirectorCapture>,
}

struct DirectorCapture {
    id: Uuid,
    goal: Uuid,
    assignment_start_ms: u64,
    assignment_end_ms: Option<u64>,
    captured_at_ms: u64,
    exposure_ms: u64,
}

impl DirectorCapture {
    fn matches(&self, receipt: &psf_guard_director_meta::inbox::Receipt) -> bool {
        let attempt = &receipt.payload["attempt"];
        let evidence = &attempt["evidence"];
        let Some(reserved) = attempt["reserved_at_ms"].as_u64() else {
            return false;
        };
        let Some(elapsed) = evidence["elapsed_ms"].as_u64() else {
            return false;
        };
        // Native triggers may run between reservation and exposure. elapsed
        // measures capture/save, not that intervening autofocus or flip.
        receipt.state == "saved"
            && Uuid::parse_str(&receipt.capture_id).ok() == Some(self.id)
            && Uuid::parse_str(&receipt.goal_id).ok() == Some(self.goal)
            && attempt["capture_id"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
                == Some(self.id)
            && attempt["goal_id"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
                == Some(self.goal)
            && evidence["state"] == "saved"
            && evidence["image_id"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
                == Some(self.id)
            && reserved >= self.assignment_start_ms
            && self.assignment_end_ms.is_none_or(|end| reserved < end)
            && self.captured_at_ms >= reserved.saturating_sub(2000)
            && elapsed.saturating_add(2000) >= self.exposure_ms
    }
}

const IMAGE_SELECT: &str = "SELECT ai.Id,ai.projectId,ai.targetId,ai.acquireddate,ai.filtername,ai.gradingStatus,ai.metadata,ai.profileId,ai.guid,t.name,t.guid FROM acquiredimage ai";

fn image_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<crate::models::AcquiredImage> {
    Ok(crate::models::AcquiredImage {
        id: row.get(0)?,
        project_id: row.get(1)?,
        target_id: row.get(2)?,
        acquired_date: row.get(3)?,
        filter_name: row.get(4)?,
        grading_status: row.get(5)?,
        metadata: row.get(6)?,
        profile_id: row.get(7)?,
        guid: row.get(8)?,
        reject_reason: None,
    })
}

fn saved_path(
    catalog: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    target: &str,
) -> Result<std::path::PathBuf, Failure> {
    let metadata: Value = serde_json::from_str(&image.metadata).map_err(|_| held())?;
    let filename = metadata["FileName"]
        .as_str()
        .and_then(|file| file.rsplit(['/', '\\']).next())
        .filter(|file| !file.is_empty())
        .ok_or_else(held)?;
    crate::server::handlers::find_fits_file(catalog, image, target, filename).map_err(|_| held())
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    pub import_id: Uuid,
    pub catalog: String,
    pub panel: u32,
    pub image_guids: Vec<Uuid>,
    #[serde(default)]
    pub source_digest: Option<String>,
    #[serde(default)]
    pub observing_night: Option<String>,
}
fn held() -> Failure {
    Failure(StatusCode::UNPROCESSABLE_ENTITY,"Report held: select accepted, saved images with stable GUIDs, matching exposures and fresh pixel solves; required calibration evidence must be present")
}
pub(super) async fn inputs(
    state: &AppState,
    service: Arc<Service>,
    b: &ConnectionBinding,
) -> Result<Value, Failure> {
    let catalogs = background::catalogs(state, service.clone(), b.rig_id).await?;
    if catalogs.len() != 1 {
        return Err(Failure(
            StatusCode::CONFLICT,
            "This rig must have exactly one available database before preparing reports",
        ));
    }
    let catalogs = catalogs
        .into_iter()
        .map(|(_, c)| json!({"id":c.id,"name":c.name}))
        .collect::<Vec<_>>();
    let id = b.id;
    service.query(move|s|{
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
    observing_night: Option<&str>,
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
    let (start, end) = night_bounds(observing_night.unwrap_or(import.night()))?;
    blocking(move||{
        let conn=crate::server::database_context::open_scheduler_connection_with_flags(&catalog.database_path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_|held())?;
        conn.busy_timeout(Duration::from_secs(2)).map_err(|_|held())?;
        let links=associations(&conn,import_id).map_err(|_|held())?;
        let tail=if links.is_empty() { "JOIN target t ON t.Id=ai.targetId WHERE ai.gradingStatus=1 AND ai.guid IS NOT NULL AND ai.acquireddate>=?1 AND ai.acquireddate<?2 ORDER BY ai.acquireddate,ai.Id LIMIT 4097" }
            else { "JOIN target t ON t.Id=ai.targetId WHERE ai.gradingStatus=1 AND ai.guid IS NOT NULL AND ai.acquireddate>=?1 AND ai.acquireddate<?2 AND EXISTS(SELECT 1 FROM psf_guard_collaboration_plan p WHERE p.import_id=?3 AND p.target_guid=t.guid COLLATE NOCASE) ORDER BY ai.acquireddate,ai.Id LIMIT 4097" };
        let mut statement=conn.prepare(&format!("{IMAGE_SELECT} {tail}")).map_err(|_|held())?;
        let mut result=Vec::new();
        let mut args=vec![rusqlite::types::Value::Integer(start),rusqlite::types::Value::Integer(end)];
        if !links.is_empty() { args.push(import_id.to_string().into()); }
        let rows=statement.query_map(rusqlite::params_from_iter(args),|r|Ok((image_row(r)?,r.get::<_,String>(9)?,r.get::<_,Option<String>>(10)?))).map_err(|_|held())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|held())?;
        if rows.len()>4096 {return Err(Failure(StatusCode::UNPROCESSABLE_ENTITY,"Too many candidate images; narrow this catalog before preparing a report"));}
        drop(statement);
        drop(conn);
        for (image,target,target_guid) in rows {
            let Some(time)=image.acquired_date else {continue;};
            let matches=matching(&links,target_guid.as_deref().unwrap_or(""),&image.filter_name,time);
            if !links.is_empty() && matches.len()!=1 {continue;}
            let Some(guid)=image.guid.as_deref().and_then(|g|Uuid::parse_str(g).ok()) else {continue;};
            let Ok(path)=saved_path(&catalog,&image,&target) else {continue;};
            let name=path.file_name().and_then(|s|s.to_str()).unwrap_or("Unknown file");
            result.push(json!({"guid":guid,"filter":image.filter_name,"captured_at":time,"target":target,"file":name,
                "panel":matches.first().map(|a|a.panel),"source_digest":matches.first().map(|a|a.source_digest.as_str())}));
        }
        Ok(json!({"images":result}))
    }).await?
}
fn night_bounds(night: &str) -> Result<(i64, i64), Failure> {
    psf_guard_director_interop::astrocollab::validate_night(night).map_err(|_| invalid())?;
    let date = chrono::NaiveDate::parse_from_str(night, "%Y-%m-%d")
        .map_err(|_| invalid())?
        .and_hms_opt(0, 0, 0)
        .ok_or_else(invalid)?
        .and_utc()
        .timestamp();
    // Rig-local observing nights can straddle UTC dates. This is a plausibility
    // bound, not assignment expiry; the operator reviews the actual night.
    Ok((date - 12 * 3600, date + 60 * 3600))
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
    let source_digest = selection.source_digest.clone();
    let import = service
        .clone()
        .query(move |s| {
            if s.catalog_rig(identity.id)?.is_none_or(|r| r.rig.id != rig) {
                return Err(StoreError::Conflict);
            }
            let import = match source_digest.as_deref() {
                Some(digest) => s.collaboration_import_revision(import_id, digest)?,
                None => s.collaboration_import(import_id)?,
            }
            .ok_or(StoreError::NotFound)?;
            if import.rig_id != rig || import.plan.source() != &source {
                return Err(StoreError::Conflict);
            }
            Ok(import.plan)
        })
        .await?;
    let (finalized, review_digest, director) = blocking(move || {
        let extracted = frames(&catalog, &import, &selection)?;
        let frames = extracted.frames;
        let night = selection
            .observing_night
            .as_deref()
            .unwrap_or(import.night());
        let finalized = collaboration::finalize_contribution_for_night(&import, &frames, night)
            .map_err(|_| held())?;
        let digest =
            collaboration::digest(&serde_json::to_vec(&(night, &frames)).map_err(|_| held())?);
        Ok::<_, Failure>((finalized, digest, extracted.director))
    })
    .await??;
    if !director.is_empty() {
        let matched = service
            .clone()
            .query(move |store| {
                let ids = director.iter().map(|c| c.id).collect::<Vec<_>>();
                let receipts = store.saved_receipts_for_captures(rig, &ids)?;
                let mut by_capture = BTreeMap::<Uuid, Vec<_>>::new();
                for receipt in &receipts {
                    if let Ok(id) = Uuid::parse_str(&receipt.capture_id) {
                        by_capture.entry(id).or_default().push(receipt);
                    }
                }
                Ok(director.iter().all(|capture| {
                    by_capture
                        .get(&capture.id)
                        .is_some_and(|found| found.len() == 1 && capture.matches(found[0]))
                }))
            })
            .await?;
        if !matched {
            return Err(Failure(StatusCode::UNPROCESSABLE_ENTITY, "Director capture check-in is missing or does not match this assignment; check in and review again"));
        }
    }
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
) -> Result<CatalogFrames, Failure> {
    let conn = crate::server::database_context::open_scheduler_connection_with_flags(
        &catalog.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| held())?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|_| held())?;
    let mut rows = Vec::new();
    let links = associations(&conn, import.import_id()).map_err(|_| held())?;
    let mut guids = selection.image_guids.clone();
    guids.sort();
    for guid in &guids {
        let mut statement=conn.prepare(&format!("{IMAGE_SELECT} LEFT JOIN target t ON t.Id=ai.targetId WHERE ai.guid=?1 COLLATE NOCASE LIMIT 2")).map_err(|_|held())?;
        let mut matches = statement
            .query_map([guid.to_string()], |r| {
                Ok((
                    image_row(r)?,
                    r.get::<_, Option<String>>(9)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(10)?,
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
    let mut director = Vec::new();
    let mut fingerprints = BTreeSet::new();
    for (guid, (image, target, target_guid)) in rows {
        if image.grading_status != 1 {
            return Err(held());
        }
        let metadata: Value = serde_json::from_str(&image.metadata).map_err(|_| held())?;
        let path =
            dunce::canonicalize(saved_path(catalog, &image, &target)?).map_err(|_| held())?;
        let id = image.id;
        let captured = image.acquired_date;
        let filter = image.filter_name;
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
        if !fingerprints.insert(fingerprint.clone()) {
            return Err(held());
        }
        let frame_header = crate::image_io::read_frame_header(&path, &path).map_err(|_| held())?;
        let headers =
            crate::astrometry_headers::FitsAstrometryHeaders::from_headers(&frame_header.cards);
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
        let capture_id = measurements::director_capture(&frame_header).map_err(|_| held())?;
        let mut assignment = None;
        if !links.is_empty() {
            let matches = matching(
                &links,
                target_guid.as_deref().unwrap_or(""),
                &filter,
                captured.ok_or_else(held)?,
            );
            if matches.len() != 1
                || matches[0].source_digest != import.digest()
                || matches[0].panel != selection.panel
                || !collaboration::exposure_matches(
                    matches[0].exposure_ms,
                    (exposure * 1000.0).round() as u64,
                )
            {
                return Err(held());
            }
            assignment = Some(matches[0]);
        }
        let captured = captured.ok_or_else(held)?;
        if let Some(id) = capture_id {
            let assignment = assignment.ok_or_else(held)?;
            director.push(DirectorCapture {
                id,
                goal: Uuid::parse_str(&assignment.exposureplan_guid).map_err(|_| held())?,
                assignment_start_ms: assignment.start_at_ms,
                assignment_end_ms: assignment.end_at_ms,
                captured_at_ms: captured
                    .checked_mul(1000)
                    .and_then(|t| u64::try_from(t).ok())
                    .ok_or_else(held)?,
                exposure_ms: (exposure * 1000.0).round() as u64,
            });
        }
        // The actual observing night is reviewed independently of when the
        // assignment was dealt. Do not relabel delayed captures as that night.
        let (start, end) = night_bounds(
            selection
                .observing_night
                .as_deref()
                .unwrap_or(import.night()),
        )?;
        if !(start..end).contains(&captured) {
            return Err(held());
        }
        let number = |key: &str| {
            metadata[key]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)
        };
        let (moon_illumination, moon_separation_degrees) = measurements::moon(
            &headers,
            captured,
            exposure,
            psf_guard_director_core::visibility::IcrsPosition {
                ra_degrees: solution.center_ra_deg,
                dec_degrees: solution.center_dec_deg,
            },
        );
        frames.push(FrameEvidence {
            capture_id: capture_id
                .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_OID, guid.as_bytes())),
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
                measurements::number(&frame_header, "PGHFR", 0.0, 1e6)
                    .or_else(|| number("HFR"))
                    .map(|hfr| hfr * solution.pixel_scale_arcsec_per_pixel)
            },
            guide_rms_arcsec: measurements::number(&frame_header, "PGGRMS", 0.0, 1e6),
            moon_illumination,
            moon_separation_degrees,
            calibrated,
            bandpass_nm: measurements::bandpass(&frame_header),
            colour: frame_colour,
        });
        solutions.push(solution);
    }
    let footprint = common_footprint(&solutions).ok_or_else(held)?;
    for frame in &mut frames {
        frame.solved_footprint = footprint.clone();
    }
    frames.sort_by_key(|f| f.image_guid);
    Ok(CatalogFrames { frames, director })
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
    #[test]
    fn director_receipt_requires_exact_identity_without_treating_trigger_time_as_capture_time() {
        use psf_guard_director_meta::inbox::Receipt;
        let capture = DirectorCapture {
            id: Uuid::new_v4(),
            goal: Uuid::new_v4(),
            assignment_start_ms: 9_000,
            assignment_end_ms: Some(11_000),
            captured_at_ms: 10_000,
            exposure_ms: 300_000,
        };
        let receipt = Receipt {
            rig_id: Uuid::new_v4(),
            ledger_id: "ledger".into(),
            sequence: 2,
            capture_id: capture.id.to_string(),
            goal_id: capture.goal.to_string(),
            state: "saved".into(),
            received_at_ms: 500_000,
            payload: json!({"attempt":{"capture_id":capture.id,"goal_id":capture.goal,
                "reserved_at_ms":10_000,"evidence":{"state":"saved","image_id":capture.id,"elapsed_ms":301_000}}}),
        };
        assert!(capture.matches(&receipt));
        for (key, bad) in [
            ("capture_id", json!(Uuid::new_v4())),
            ("goal_id", json!(Uuid::new_v4())),
            ("reserved_at_ms", json!(20_000)),
        ] {
            let mut invalid = receipt.clone();
            invalid.payload["attempt"][key] = bad;
            assert!(!capture.matches(&invalid));
        }
        for (key, bad) in [
            ("image_id", json!(Uuid::new_v4())),
            ("state", json!("uncertain")),
            ("elapsed_ms", json!(1)),
        ] {
            let mut invalid = receipt.clone();
            invalid.payload["attempt"]["evidence"][key] = bad;
            assert!(!capture.matches(&invalid));
        }
        let mut after_flip = capture;
        after_flip.captured_at_ms = 400_000;
        assert!(after_flip.matches(&receipt));
        after_flip.assignment_start_ms = 10_001;
        assert!(!after_flip.matches(&receipt));
        after_flip.assignment_start_ms = 9_000;
        after_flip.assignment_end_ms = Some(10_000);
        assert!(!after_flip.matches(&receipt));
    }

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
        conn.execute_batch("CREATE TABLE acquiredimage(Id INTEGER PRIMARY KEY,guid TEXT,gradingStatus INTEGER,filtername TEXT,metadata TEXT,acquireddate INTEGER,targetId INTEGER DEFAULT 1,projectId INTEGER DEFAULT 1,profileId TEXT); CREATE TABLE target(Id INTEGER PRIMARY KEY,guid TEXT,name TEXT DEFAULT 'Target'); INSERT INTO target(Id,guid) VALUES(1,'11111111-1111-4111-8111-111111111111')").unwrap();
        let guid = Uuid::new_v4();
        conn.execute(
            "INSERT INTO acquiredimage(Id,guid,gradingStatus,filtername,metadata,acquireddate) VALUES(1,?1,1,'Ha',?2,1791171000)",
            rusqlite::params![
                guid.to_string(),
                json!({"FileName":"saved.fits","HFR":2.0}).to_string()
            ],
        )
        .unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        let catalog = DatabaseContext::new(
            "catalog".into(),
            "Catalog".into(),
            db_path.to_string_lossy().into(),
            vec![
                empty.to_string_lossy().into(),
                dir.path().to_string_lossy().into(),
            ],
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
            source_digest: None,
            observing_night: None,
        };
        let extracted = frames(&catalog, &import, &selection).unwrap();
        assert_eq!(extracted.frames[0].exposure_ms, 300_000);
        assert_eq!(extracted.frames[0].hfr_arcsec, Some(7.2));
        assert!(!extracted.frames[0].calibrated);
        assert!(!extracted.frames[0].colour);
        conn.execute_batch(crate::server::director::collaboration_activation::DDL)
            .unwrap();
        conn.execute("INSERT INTO psf_guard_collaboration_plan VALUES('plan','11111111-1111-4111-8111-111111111111',?1,?2,0,'H',300000,1791170000000,NULL,11)",
            rusqlite::params![import.import_id().to_string(),import.digest()]).unwrap();
        assert!(frames(&catalog, &import, &selection).is_ok());
        conn.execute(
            "UPDATE acquiredimage SET acquireddate=acquireddate+7*86400",
            [],
        )
        .unwrap();
        assert!(frames(&catalog, &import, &selection).is_err());
        let late = Selection {
            observing_night: Some("2026-10-12".into()),
            ..selection.clone()
        };
        assert!(frames(&catalog, &import, &late).is_ok());
        conn.execute(
            "UPDATE acquiredimage SET acquireddate=acquireddate-7*86400",
            [],
        )
        .unwrap();
        let wrong_panel = Selection {
            panel: 1,
            ..selection.clone()
        };
        assert!(frames(&catalog, &import, &wrong_panel).is_err());
        conn.execute(
            "UPDATE psf_guard_collaboration_plan SET end_at_ms=1791171000000",
            [],
        )
        .unwrap();
        assert!(frames(&catalog, &import, &selection).is_err());
        conn.execute(
            "UPDATE psf_guard_collaboration_plan SET end_at_ms=1791180000000,exposure_ms=2000",
            [],
        )
        .unwrap();
        assert!(frames(&catalog, &import, &selection).is_err());
        conn.execute(
            "UPDATE psf_guard_collaboration_plan SET exposure_ms=300000",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO acquiredimage SELECT 2,guid,gradingStatus,filtername,metadata,acquireddate,targetId,projectId,profileId FROM acquiredimage WHERE Id=1", []).unwrap();
        assert!(frames(&catalog, &import, &selection).is_err());
        let duplicate_guid = Uuid::new_v4();
        conn.execute(
            "UPDATE acquiredimage SET guid=?1 WHERE Id=2",
            [duplicate_guid.to_string()],
        )
        .unwrap();
        let mut duplicate_analysis = analysis.clone();
        duplicate_analysis.image_id = 2;
        crate::astrometry::persist_pixel_analysis(&catalog.cache_dir_path, &duplicate_analysis)
            .unwrap();
        assert!(frames(
            &catalog,
            &import,
            &Selection {
                image_guids: vec![guid, duplicate_guid],
                ..selection.clone()
            }
        )
        .is_err());
        conn.execute("DELETE FROM acquiredimage WHERE Id=2", [])
            .unwrap();
        std::fs::write(&path, b"replaced image").unwrap();
        assert!(frames(&catalog, &import, &selection).is_err());
    }
}
