//! Automatic reporting uses the same saved-pixel review and immutable outbox
//! as manual reporting. Only activation provenance grants frame attribution.
use super::*;

#[derive(Clone, Default, Serialize)]
pub(in crate::server::director::collaboration_auth) struct AutomaticResult {
    queued: usize,
    delivered: usize,
    held: usize,
}

pub(in crate::server::director::collaboration_auth) async fn automatic(
    state: &Arc<AppState>,
    service: Arc<Service>,
    b: &ConnectionBinding,
) -> Result<AutomaticResult, Failure> {
    let catalogs = background::catalogs(state, service.clone(), b.rig_id).await?;
    let [(identity, catalog)] = catalogs.as_slice() else {
        return Err(Failure(
            StatusCode::CONFLICT,
            "This rig must have exactly one available database for automatic reports",
        ));
    };
    if b.background
        .as_ref()
        .is_none_or(|policy| policy.catalog_id != identity.id)
    {
        return Err(Failure(
            StatusCode::CONFLICT,
            "The rig database changed; save the automation policy again",
        ));
    }
    let (id, rig) = (b.id, b.rig_id);
    let (imports, longitude) = service
        .clone()
        .query(move |s| {
            let profile = s.rig_profile(rig)?;
            let site = s
                .rig_site(rig, profile.as_ref())?
                .location
                .ok_or(StoreError::InvalidInput)?;
            Ok((
                s.collaboration_imports_for_connection(id)?,
                site.longitude_degrees,
            ))
        })
        .await?;
    let mut result = AutomaticResult::default();
    for import in imports {
        let catalog = catalog.clone();
        let selections =
            blocking(move || selections(&catalog, import.plan.import_id(), longitude)).await?;
        let selections = match selections {
            Ok(selections) => selections,
            Err(_) => {
                result.held += 1;
                continue;
            }
        };
        for (filter, selection) in selections {
            let (import, panel, night) = (
                selection.import_id,
                selection.panel,
                selection.observing_night.clone().ok_or_else(invalid)?,
            );
            let previous = service
                .clone()
                .query(move |s| s.latest_collaboration_report(import, panel, &filter, &night))
                .await?;
            if previous.as_ref().is_some_and(|old| {
                old.images.iter().copied().collect::<BTreeSet<_>>()
                    == selection.image_guids.iter().copied().collect()
            }) {
                continue;
            }
            // Re-read before queueing: a grade, file or solve may have changed
            // while review was running. Never accept caller-supplied evidence.
            let reviewed = review(state, service.clone(), b, selection.clone(), None).await;
            let queued = match reviewed {
                Ok(value) => {
                    review(
                        state,
                        service.clone(),
                        b,
                        selection,
                        Some(
                            value["review_digest"]
                                .as_str()
                                .ok_or_else(invalid)?
                                .to_owned(),
                        ),
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            match queued {
                Ok(_) => result.queued += 1,
                Err(_) => result.held += 1,
            }
        }
    }
    let delivery = workflows::checkin(state, service, b.id).await?;
    result.delivered = delivery["delivered"].as_u64().unwrap_or(0) as usize;
    tracing::info!(connection_id=%b.id, queued=result.queued, delivered=result.delivered, held=result.held,
        "Automatic collaboration report pass completed");
    Ok(result)
}

fn observing_date(timestamp: i64, longitude: f64) -> Result<String, Failure> {
    let offset = (longitude / 15.0 * 3600.0).round() as i64;
    chrono::DateTime::from_timestamp(
        timestamp
            .checked_add(offset)
            .and_then(|v| v.checked_sub(43200))
            .ok_or_else(held)?,
        0,
    )
    .map(|date| date.format("%Y-%m-%d").to_string())
    .ok_or_else(held)
}

fn selections(
    catalog: &DatabaseContext,
    import: Uuid,
    longitude: f64,
) -> Result<Vec<(String, Selection)>, Failure> {
    let conn = crate::server::database_context::open_scheduler_connection_with_flags(
        &catalog.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| held())?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|_| held())?;
    let links = associations(&conn, import).map_err(|_| held())?;
    if links.is_empty() {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare(&format!("{IMAGE_SELECT} JOIN target t ON t.Id=ai.targetId
        WHERE ai.gradingStatus=1 AND ai.guid IS NOT NULL AND EXISTS(
            SELECT 1 FROM psf_guard_collaboration_plan p WHERE p.import_id=?1 AND p.target_guid=t.guid COLLATE NOCASE
                AND ai.acquireddate>=p.start_at_ms/1000 AND (p.end_at_ms IS NULL OR ai.acquireddate<(p.end_at_ms+999)/1000))
        ORDER BY ai.acquireddate,ai.Id LIMIT 65537")).map_err(|_| held())?;
    let rows = stmt
        .query_map([import.to_string()], |r| {
            Ok((
                image_row(r)?,
                r.get::<_, String>(9)?,
                r.get::<_, Option<String>>(10)?,
            ))
        })
        .map_err(|_| held())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| held())?;
    if rows.len() > 65536 {
        return Err(held());
    }
    drop(stmt);
    drop(conn);
    let mut groups = BTreeMap::<(String, u32, String, String), Vec<Uuid>>::new();
    for (image, target, target_guid) in rows {
        let Some(time) = image.acquired_date else {
            continue;
        };
        let matched = matching(
            &links,
            target_guid.as_deref().unwrap_or(""),
            &image.filter_name,
            time,
        );
        let [link] = matched.as_slice() else {
            continue;
        };
        let Some(guid) = image
            .guid
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok())
        else {
            continue;
        };
        if saved_path(catalog, &image, &target).is_err() {
            continue;
        }
        let key = (
            link.source_digest.clone(),
            link.panel,
            astrocollab::fold_filter(&image.filter_name).map_err(|_| held())?,
            observing_date(time, longitude)?,
        );
        groups.entry(key).or_default().push(guid);
        if groups.len() > 256
            || groups
                .values()
                .any(|images| images.len() > collaboration::MAX_REPORT_FRAMES)
        {
            return Err(held());
        }
    }
    Ok(groups
        .into_iter()
        .map(|((source_digest, panel, filter, night), image_guids)| {
            (
                filter,
                Selection {
                    import_id: import,
                    catalog: catalog.id.clone(),
                    panel,
                    image_guids,
                    source_digest: Some(source_digest),
                    observing_night: Some(night),
                },
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_history_is_scoped_to_activation_and_split_into_capture_nights() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let mut conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE acquiredimage(Id INTEGER PRIMARY KEY,guid TEXT,gradingStatus INTEGER,filtername TEXT,metadata TEXT,acquireddate INTEGER,targetId INTEGER DEFAULT 1,projectId INTEGER DEFAULT 1,profileId TEXT);
            CREATE TABLE target(Id INTEGER PRIMARY KEY,guid TEXT,name TEXT DEFAULT 'Target');
            INSERT INTO target(Id,guid) VALUES(1,'11111111-1111-4111-8111-111111111111');").unwrap();
        conn.execute_batch(crate::server::director::collaboration_activation::DDL)
            .unwrap();
        let import = Uuid::new_v4();
        let start = 1791171000i64;
        conn.execute("INSERT INTO psf_guard_collaboration_plan VALUES('plan','11111111-1111-4111-8111-111111111111',?1,'revision',0,'H',300000,?2,NULL,11)",
            rusqlite::params![import.to_string(), start * 1000]).unwrap();
        let tx = conn.transaction().unwrap();
        {
            let mut insert = tx.prepare("INSERT INTO acquiredimage(Id,guid,gradingStatus,filtername,metadata,acquireddate) VALUES(?1,?2,1,'Ha',?3,?4)").unwrap();
            for id in 0..10_000 {
                let time = if id < 5000 {
                    start - 86400
                } else {
                    start + (id % 2) * 86400
                };
                insert
                    .execute(rusqlite::params![
                        id,
                        Uuid::new_v4().to_string(),
                        json!({"FileName":"saved.fits"}).to_string(),
                        time
                    ])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
        // Selection only verifies path presence; scientific review must still
        // refuse this placeholder because it has no saved pixels or solve.
        std::fs::write(dir.path().join("saved.fits"), b"selection-only fixture").unwrap();
        let catalog = DatabaseContext::new(
            "catalog".into(),
            "Catalog".into(),
            path.to_string_lossy().into(),
            vec![dir.path().to_string_lossy().into()],
            None,
            None,
            None,
            dir.path().join("cache"),
        )
        .unwrap();
        let selected = selections(&catalog, import, -105.0).unwrap();
        assert_eq!(selected.len(), 2);
        assert!(selected
            .iter()
            .all(|(_, group)| group.image_guids.len() == 2500));
        assert_ne!(selected[0].1.observing_night, selected[1].1.observing_night);
        conn.execute(
            "UPDATE psf_guard_collaboration_plan SET end_at_ms=?1",
            [(start + 86400) * 1000],
        )
        .unwrap();
        assert_eq!(selections(&catalog, import, -105.0).unwrap().len(), 1);
    }
    #[test]
    fn reports_follow_the_capture_night_at_the_rig_not_the_import_date() {
        let timestamp = chrono::DateTime::parse_from_rfc3339("2026-10-08T06:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(observing_date(timestamp, -105.0).unwrap(), "2026-10-07");
        assert_eq!(observing_date(timestamp, 150.0).unwrap(), "2026-10-08");
    }
}
