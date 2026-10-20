use super::*;
use serde_json::json;

fn no_projects(_: &str, _: i32) -> Option<i32> {
    None
}

fn db(id: &str, name: &str) -> DatabaseState {
    DatabaseState {
        id: id.into(),
        name: name.into(),
        ..DatabaseState::default()
    }
}

fn scan(processed: usize, total: usize, stage: &str) -> SpatialScanProgress {
    SpatialScanProgress {
        running: true,
        stage: stage.into(),
        target_id: Some(42),
        total,
        processed,
        current_file: Some("sh2-86-r-004.fits".into()),
        started_at: Some(1),
        ..SpatialScanProgress::default()
    }
}

fn backfill(processed: usize, total: usize) -> QualityBackfillProgress {
    QualityBackfillProgress {
        running: true,
        total_targets: total,
        processed_targets: processed,
        current_target_id: Some(7),
        started_at: Some(1),
        ..QualityBackfillProgress::default()
    }
}

fn refresh(stage: RefreshStage) -> RefreshProgress {
    RefreshProgress {
        stage,
        ..RefreshProgress::default()
    }
}

fn stack(job_id: &str, state: &str, processed: usize, total: usize) -> StackActivityEntry {
    stack_with(job_id, state, processed, total, json!({}))
}

fn stack_with(
    job_id: &str,
    state: &str,
    processed: usize,
    total: usize,
    extra: Value,
) -> StackActivityEntry {
    let mut entry = json!({
        "kind": "mono", "job_id": job_id, "database_id": "db-a", "project_id": 1,
        "state": state, "label": format!("M44 · {job_id}"), "detail": "registering",
        "processed_units": processed, "total_units": total, "created_unix_seconds": 1,
    });
    for (key, value) in extra.as_object().unwrap() {
        entry[key] = value.clone();
    }
    serde_json::from_value(entry).unwrap()
}

fn with_stacks(entries: Vec<StackActivityEntry>) -> Snapshot {
    let mut snapshot = Snapshot::default();
    snapshot.stacks.active = entries;
    snapshot
}

fn with_db(database: DatabaseState) -> Snapshot {
    Snapshot {
        databases: vec![database],
        ..Snapshot::default()
    }
}

fn row_list(snapshot: &Snapshot) -> Vec<JobRow> {
    rows(snapshot, &no_projects)
}

fn wbpp_queued(id: &str, db_id: &str, project: Option<i32>, target: Option<i32>) -> Value {
    json!({
        "id": id, "db_id": db_id, "db_name": "Askar", "scope": format!("project {id}"),
        "project_id": project, "target_id": target, "position": 1, "queued_at": 1,
    })
}

fn wbpp(running: Vec<WbppActiveRun>, queued: Vec<Value>) -> WbppActivity {
    WbppActivity {
        running,
        queued: queued
            .into_iter()
            .map(|run| WbppQueuedActivity {
                db_id: run["db_id"].as_str().unwrap().into(),
                db_name: run["db_name"].as_str().unwrap().into(),
                run: crate::server::wbpp_queue::QueuedRunSummary {
                    id: run["id"].as_str().unwrap().into(),
                    scope: run["scope"].as_str().unwrap().into(),
                    project_id: run["project_id"].as_i64().map(|id| id as i32),
                    target_id: run["target_id"].as_i64().map(|id| id as i32),
                    position: 1,
                    queued_at: 1,
                },
            })
            .collect(),
    }
}

fn wbpp_running(stage: &str, wbpp_stage: &str, steps: usize) -> WbppActiveRun {
    WbppActiveRun {
        db_id: "a".into(),
        db_name: "Askar".into(),
        scope: "project Bubble".into(),
        project_id: Some(5),
        target_id: None,
        stage: stage.into(),
        wbpp_stage: Some(wbpp_stage.into()),
        wbpp_steps: steps,
        started_at: Some(1),
    }
}

fn transfer(id: &str, state: &str, automatic: bool) -> Value {
    json!({
        "id": id, "connection_id": "c", "revision": 1, "state": state, "catalog": "Askar",
        "label": "M44", "total_files": 10, "completed_files": 4, "total_bytes": 100,
        "completed_bytes": 40, "current_file": "", "sent_bytes": 0, "error": null,
        "next_attempt_ms": 0, "warnings": null, "manifest": null, "automatic": automatic,
    })
}

#[test]
fn a_build_being_prepared_is_running_and_stoppable() {
    let rows = row_list(&with_stacks(vec![stack_with(
        "asked",
        "preparing",
        0,
        0,
        json!({"label": "1263 frames", "detail": "Reading frames and matching calibration"}),
    )]));
    assert_eq!(rows[0].title, "Preparing stack build");
    assert_eq!(rows[0].scope, "1263 frames");
    assert_eq!(rows[0].detail, "Reading frames and matching calibration");
    assert_eq!(rows[0].status, RowStatus::Running);
    assert_eq!(rows[0].percent, None);
    assert!(rows[0].stoppable);
    assert_eq!(rows[0].position, None);

    let color = row_list(&with_stacks(vec![stack_with(
        "composition",
        "preparing",
        0,
        0,
        json!({"kind": "color", "label": "RGB", "detail": "Waiting for the channel builds to finish"}),
    )]));
    assert_eq!(color[0].title, "Preparing color preview");
    assert_eq!(color[0].detail, "Waiting for the channel builds to finish");
}

#[test]
fn each_row_links_to_the_view_of_its_work() {
    let projects = |db: &str, target: i32| match (db, target) {
        ("db-a", 42) => Some(4),
        ("db-a", 9) => Some(2),
        _ => None,
    };
    let mut snapshot = with_db(DatabaseState {
        scan: SpatialScanProgress {
            filter_name: Some("Ha".into()),
            ..scan(3, 10, "astrometry")
        },
        ..db("db-a", "Askar")
    });
    snapshot.stacks.active = vec![stack_with("R", "running", 1, 3, json!({"target_id": 7}))];
    snapshot.wbpp = wbpp(Vec::new(), vec![wbpp_queued("q1", "db-a", None, Some(9))]);
    let links: Vec<RowLink> = rows(&snapshot, &projects)
        .into_iter()
        .map(|row| row.link.unwrap())
        .collect();
    assert_eq!(
        links[0],
        RowLink {
            view: LinkView::Sequence,
            db: "db-a".into(),
            project: Some(4),
            target: Some(42),
            params: vec![("filterName".into(), "Ha".into())],
        }
    );
    assert_eq!(
        (links[1].view, links[1].project, links[1].target),
        (LinkView::Stacks, Some(1), Some(7))
    );
    assert_eq!(
        (links[2].view, links[2].project, links[2].target),
        (LinkView::Stacks, Some(2), Some(9))
    );

    // A catalog refresh works on the whole database. A backfill between
    // targets whose project is unknown opens the database's images too.
    let idle = rows(
        &with_db(DatabaseState {
            refresh: Some(refresh(RefreshStage::ProcessingProjects)),
            backfill: backfill(1, 3),
            ..db("db-a", "Askar")
        }),
        &projects,
    );
    for row in idle {
        assert_eq!(row.link, Some(RowLink::view(LinkView::Grid, "db-a")));
    }
}

#[test]
fn a_quality_scan_shows_its_frames_and_folds_into_a_running_backfill() {
    let alone = row_list(&with_db(DatabaseState {
        scan: scan(4, 10, "astrometry"),
        ..db("a", "Askar")
    }));
    assert_eq!(alone.len(), 1);
    assert_eq!(
        (
            alone[0].title.as_str(),
            alone[0].scope.as_str(),
            alone[0].detail.as_str()
        ),
        ("Analyzing quality", "Askar", "Solving 4/10 frames")
    );
    assert_eq!(alone[0].percent, Some(40.0));

    // The analysis's own scan is automatic.
    let folded = row_list(&with_db(DatabaseState {
        scan: SpatialScanProgress {
            automatic: true,
            ..scan(4, 10, "spatial")
        },
        backfill: backfill(2, 8),
        ..db("a", "Askar")
    }));
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0].title, "Analyzing database quality");
    assert_eq!(folded[0].detail, "2/8 targets · Scanning 4/10 frames");
    assert_eq!(folded[0].percent, Some(25.0));
}

#[test]
fn a_persons_scan_is_listed_apart_from_the_analysis_waiting_for_it() {
    let rows = row_list(&with_db(DatabaseState {
        scan: scan(4, 10, "astrometry"),
        backfill: backfill(2, 8),
        ..db("db-a", "Askar")
    }));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].key, "scan:db-a");
    assert_eq!(rows[0].status, RowStatus::Running);
    assert_eq!(rows[1].key, "backfill:db-a");
    assert_eq!(rows[1].status, RowStatus::Waiting);
    assert_eq!(rows[1].waiting_for.as_deref(), Some("a quality scan"));
    assert_eq!(rows[1].detail, "Waiting for a quality scan · 2/8 targets");
}

#[test]
fn a_catalog_refresh_shows_its_stage() {
    let folders = row_list(&with_db(DatabaseState {
        refresh: Some(RefreshProgress {
            directories_processed: 12,
            files_scanned: 340,
            current_directory_name: Some("/mnt/frames/2026-09-30/LIGHT".into()),
            files_missing: 2,
            ..refresh(RefreshStage::InitializingDirectoryTree)
        }),
        ..db("a", "Askar")
    }));
    assert_eq!(folders[0].title, "Refreshing catalog");
    assert_eq!(
        folders[0].detail,
        "Scanning folders · 12 folders, 340 files · LIGHT · 2 missing"
    );
    assert_eq!(
        folders[0].hint.as_deref(),
        Some("/mnt/frames/2026-09-30/LIGHT")
    );

    let projects = row_list(&with_db(DatabaseState {
        refresh: Some(RefreshProgress {
            projects_total: 4,
            projects_processed: 1,
            ..refresh(RefreshStage::ProcessingProjects)
        }),
        ..db("a", "Askar")
    }));
    assert_eq!(projects[0].detail, "1/4 projects");
    assert!(projects[0].percent.is_some());
    assert!(row_list(&with_db(db("a", "Askar"))).is_empty());
}

#[test]
fn a_catalog_refresh_is_filed_by_who_requested_it() {
    let row = |requested: bool| {
        row_list(&with_db(DatabaseState {
            refresh: Some(RefreshProgress {
                requested,
                ..refresh(RefreshStage::LoadingProjects)
            }),
            ..db("a", "Askar")
        }))
        .remove(0)
    };
    assert!(!row(true).background);
    assert!(row(false).background);
}

#[test]
fn a_composition_in_the_quick_lane_waits_for_a_composition() {
    let color = |position: Value| {
        row_list(&with_stacks(vec![stack_with(
            "RGB",
            "queued",
            0,
            0,
            json!({"kind": "color", "queue_position": position}),
        )]))
        .remove(0)
    };
    assert_eq!(
        color(Value::Null).detail,
        "Waiting for the composition ahead"
    );
    assert_eq!(color(json!(0)).detail, "Waiting for the build ahead");
}

#[test]
fn running_builds_come_before_the_line_in_its_order() {
    let rows = row_list(&with_stacks(vec![
        stack_with("late", "queued", 0, 0, json!({"queue_position": 1})),
        stack("run", "running", 3, 12),
        stack_with("next", "queued", 0, 0, json!({"queue_position": 0})),
    ]));
    let scopes: Vec<&str> = rows.iter().map(|row| row.scope.as_str()).collect();
    assert_eq!(scopes, ["M44 · run", "M44 · next", "M44 · late"]);
    assert_eq!(
        rows.iter().map(|row| row.position).collect::<Vec<_>>(),
        [None, Some(0), Some(1)]
    );
    assert_eq!(rows[0].detail, "3/12 frames");
    assert_eq!(rows[0].percent, Some(25.0));
    assert_eq!(rows[1].status, RowStatus::Queued);
    assert_eq!(rows[1].percent, None);
    assert_eq!(rows[1].detail, "Waiting for the build ahead");
    assert_eq!(
        rows[1].control,
        Some(RowControl::Stack {
            job_id: "next".into()
        })
    );
}

#[test]
fn a_persons_work_comes_before_automatic_work() {
    let rows = row_list(&with_stacks(vec![
        stack_with("B", "queued", 0, 0, json!({"queue_position": 0})),
        stack_with("R", "running", 3, 12, json!({"automatic": true})),
        stack_with("G", "queued", 0, 0, json!({"queue_position": 1})),
    ]));
    let scopes: Vec<&str> = rows.iter().map(|row| row.scope.as_str()).collect();
    assert_eq!(scopes, ["M44 · B", "M44 · G", "M44 · R"]);
    assert!(rows[2].automatic && rows[2].background);
    assert_eq!(rows[2].status, RowStatus::Running);
}

#[test]
fn the_final_pass_and_a_color_step_are_shown_in_words() {
    let rows = row_list(&with_stacks(vec![
        stack_with(
            "R",
            "running",
            21,
            34,
            json!({"progress_label": "Rejecting transients · pass 2/3 · frame 3/8"}),
        ),
        stack_with(
            "C",
            "running",
            1250,
            4000,
            json!({"kind": "color", "progress_label": "H-alpha: BlurXTerminator (42%)"}),
        ),
    ]));
    assert_eq!(
        rows[0].detail,
        "Rejecting transients · pass 2/3 · frame 3/8"
    );
    assert!(rows[0].stoppable);
    assert_eq!(rows[1].title, "Composing color");
    assert_eq!(rows[1].detail, "H-alpha: BlurXTerminator (42%)");
    assert_eq!(rows[1].percent, Some(31.25));
    assert!(rows[1].stoppable);
}

#[test]
fn processing_runs_are_listed_in_the_stack_line() {
    let blur = stack_with(
        &"b".repeat(64),
        "running",
        420,
        1000,
        json!({
            "kind": "processing", "target_id": 42, "label": "M44 · G", "detail": "BlurXTerminator",
            "progress_label": "BlurXTerminator · 42%",
            "processing": {
                "operation": "BlurXTerminator", "channel": "G", "stage": "BlurXTerminator",
                "fraction": 0.42, "started_unix_seconds": 1_760_000_000, "stoppable": true,
            },
        }),
    );
    let stretch = stack_with(
        &"s".repeat(64),
        "queued",
        0,
        0,
        json!({
            "kind": "processing", "target_id": 42, "label": "M44 · R",
            "detail": "Waiting for BlurXTerminator on M44 · G to finish", "queue_position": 0,
            "processing": {
                "operation": "Deconvolution", "channel": "R",
                "waiting_for": "BlurXTerminator on M44 · G", "stoppable": false,
            },
        }),
    );
    let build = stack_with("Ha", "queued", 0, 0, json!({"queue_position": 1}));
    let rows = row_list(&with_stacks(vec![stretch.clone(), blur.clone(), build]));
    let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
    assert_eq!(titles, ["BlurXTerminator", "Deconvolution", "Stacking"]);
    // The title is the tool; the client adds the start time to the detail.
    assert_eq!(rows[0].kind, RowKind::Processing);
    assert_eq!(rows[0].detail, "");
    assert_eq!(rows[0].started_at, Some(1_760_000_000));
    assert!((rows[0].percent.unwrap() - 42.0).abs() < 1e-3);
    assert!(rows[0].stoppable);
    assert_eq!(rows[0].queue, Some(RowQueue::Stack));
    assert_eq!(rows[0].link, Some(stacks_link("db-a", 1, Some(42))));
    // A queued run shows what it waits for, and leaves the line like a
    // build.
    assert_eq!(rows[1].detail, "Waiting for BlurXTerminator on M44 · G");
    assert_eq!(rows[1].status, RowStatus::Queued);
    assert_eq!(rows[1].position, Some(0));
    assert_eq!(rows[1].started_at, None);
    assert!(rows[1].stoppable);

    // A running stretch or deconvolution cannot be stopped.
    let mut deconvolving: Value = serde_json::to_value(&stretch).unwrap();
    deconvolving["state"] = json!("running");
    deconvolving["queue_position"] = Value::Null;
    deconvolving["processing"]["waiting_for"] = Value::Null;
    deconvolving["processing"]["stage"] = json!("Reading the stack");
    let deconvolving = row_list(&with_stacks(vec![
        serde_json::from_value(deconvolving).unwrap()
    ]));
    assert_eq!(deconvolving[0].detail, "Reading the stack");
    assert_eq!(deconvolving[0].percent, None);
    assert!(!deconvolving[0].stoppable);
}

#[test]
fn wbpp_runs_are_listed_running_and_queued() {
    let mut snapshot = Snapshot {
        wbpp: wbpp(
            vec![wbpp_running("running", "Image Integration", 7)],
            vec![
                wbpp_queued("q3", "b", Some(2), Some(9)),
                wbpp_queued("q4", "a", Some(3), None),
            ],
        ),
        ..Snapshot::default()
    };
    let rows = row_list(&snapshot);
    assert_eq!(rows[0].title, "WBPP");
    assert_eq!(rows[0].scope, "Askar · project Bubble");
    assert_eq!(
        rows[0].detail,
        "Running in PixInsight · Image Integration · 7 steps done"
    );
    assert_eq!(
        rows[0].control,
        Some(RowControl::WbppRunning { db_id: "a".into() })
    );
    assert_eq!(rows[2].status, RowStatus::Queued);
    assert_eq!(rows[2].position, Some(1));
    assert_eq!(
        rows[2].control,
        Some(RowControl::WbppQueued {
            db_id: "a".into(),
            queue_id: "q4".into()
        })
    );

    // A masters copy is listed without a Stop.
    snapshot.wbpp = wbpp(
        vec![wbpp_running(
            "publishing",
            "1 of 3 files processed · masterLight.xisf",
            0,
        )],
        Vec::new(),
    );
    let copying = row_list(&snapshot);
    assert_eq!(
        copying[0].detail,
        "Copying masters · 1 of 3 files processed · masterLight.xisf"
    );
    assert!(!copying[0].stoppable);
    assert_eq!(copying[0].control, None);
}

#[test]
fn automatic_work_that_paused_is_listed_as_waiting_after_running_work() {
    let mut snapshot = with_db(DatabaseState {
        backfill: backfill(2, 8),
        scan: SpatialScanProgress {
            automatic: true,
            waiting_for: Some("a stack build".into()),
            ..scan(4, 10, "astrometry")
        },
        ..db("db-a", "Askar")
    });
    snapshot.stacks.active = vec![
        stack_with("A", "running", 5, 10, json!({"automatic": true})),
        stack("P", "running", 3, 10),
    ];
    snapshot.stacks.scheduled = vec![
        serde_json::from_value(json!({
            "database_id": "db-a", "database_name": "Askar", "project_id": 3,
            "project_name": "Bravo", "reason": "arrival", "due_in_seconds": 0,
            "channels": ["M44 · Ha"], "waiting": true,
            "waiting_for": "the quality analysis of Askar",
        }))
        .unwrap(),
        // Still settling: not work yet.
        serde_json::from_value(json!({
            "database_id": "db-a", "database_name": "Askar", "project_id": 4,
            "project_name": "Charlie", "reason": "grade", "due_in_seconds": 600,
            "channels": ["M44 · R"], "waiting": false,
        }))
        .unwrap(),
    ];
    snapshot.stacks.waiting_reapplies = vec![serde_json::from_value(json!({
        "database_id": "db-a", "project_id": 3, "target_id": 9, "label": "M44 · G",
    }))
    .unwrap()];
    let rows = row_list(&snapshot);
    let listed: Vec<(&str, RowStatus)> = rows
        .iter()
        .map(|row| (row.title.as_str(), row.status))
        .collect();
    assert_eq!(
        listed,
        [
            ("Stacking", RowStatus::Running),
            ("Stacking", RowStatus::Running),
            ("Analyzing database quality", RowStatus::Waiting),
            ("Refreshing stacks", RowStatus::Waiting),
            ("Re-applying processing", RowStatus::Waiting),
        ]
    );
    assert_eq!(rows[0].scope, "M44 · P");
    assert!(rows[1].automatic && rows[1].background);
    assert_eq!(
        rows[2].detail,
        "Waiting for a stack build · 2/8 targets · Solving 4/10 frames"
    );
    assert_eq!(rows[2].percent, Some(25.0), "the progress it had reached");
    assert_eq!(rows[3].key, "refresh-waiting:db-a:3");
    assert_eq!(rows[3].scope, "Askar · Bravo");
    assert_eq!(rows[3].detail, "Waiting for the quality analysis of Askar");
    assert_eq!(rows[3].percent, None);
    assert_eq!(rows[3].link, Some(stacks_link("db-a", 3, None)));
    assert_eq!(rows[3].hint.as_deref(), Some("Restacks M44 · Ha"));
    assert_eq!(rows[3].remaining.as_deref(), Some("1 channel to stack"));
    assert_eq!(rows[3].control, None, "waiting work resumes on its own");
    assert_eq!(rows[4].detail, "Waiting to start");
    assert_eq!(rows[4].link, Some(stacks_link("db-a", 3, Some(9))));
}

#[test]
fn a_reapply_after_a_persons_build_is_their_work() {
    let mut snapshot = Snapshot::default();
    snapshot.stacks.waiting_reapplies = vec![
        serde_json::from_value(json!({
            "database_id": "db-a", "project_id": 3, "target_id": 9, "label": "M44 · G",
            "personal": true, "waiting_for": "the processing of M44 · R",
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "database_id": "db-a", "project_id": 3, "target_id": 9, "label": "M44 · B",
            "waiting_for": "a stack build",
        }))
        .unwrap(),
    ];
    let rows = row_list(&snapshot);
    assert_eq!(rows[0].scope, "M44 · G");
    assert_eq!(rows[0].detail, "Waiting for the processing of M44 · R");
    assert!(!rows[0].background);
    assert_eq!(rows[1].scope, "M44 · B");
    assert!(rows[1].automatic && rows[1].background);
}

#[test]
fn flat_measuring_is_automatic_with_its_rig_progress_stop_and_link() {
    let measuring = FlatMeasureStatus {
        state: "running".into(),
        active: true,
        rig_uuid: Some("c925".into()),
        rig_name: Some("C925 · ASI2600".into()),
        priority: Some("background".into()),
        stage: Some("measuring flat sessions".into()),
        done: 12,
        total: 40,
        ..FlatMeasureStatus::default()
    };
    let rows = row_list(&with_db(DatabaseState {
        flats: measuring.clone(),
        ..db("astro", "Observatory")
    }));
    let row = &rows[0];
    assert_eq!(row.key, "flats:astro");
    assert_eq!(row.scope, "Observatory · C925 · ASI2600");
    assert_eq!(row.detail, "Measuring flats · 12 of 40 sessions");
    assert_eq!(row.percent, Some(30.0));
    assert!(row.automatic && row.background && row.stoppable);
    assert_eq!(
        row.link,
        Some(
            RowLink::view(LinkView::Calibration, "astro")
                .param("calKind", "flat")
                .param("calRig", "c925")
        )
    );

    // A pass that is settling, stopped or has nothing to read is not listed.
    for status in [
        FlatMeasureStatus::default(),
        FlatMeasureStatus {
            state: "queued".into(),
            rigs: vec!["c925".into()],
            ..FlatMeasureStatus::default()
        },
        FlatMeasureStatus {
            active: false,
            ..measuring.clone()
        },
    ] {
        assert!(row_list(&with_db(DatabaseState {
            flats: status,
            ..db("astro", "Observatory")
        }))
        .is_empty());
    }

    let waiting = row_list(&with_db(DatabaseState {
        flats: FlatMeasureStatus {
            stage: Some("comparing flat sessions".into()),
            done: 1,
            total: 4,
            waiting_for: Some("a stack build".into()),
            ..measuring
        },
        ..db("astro", "Observatory")
    }));
    assert_eq!(waiting[0].status, RowStatus::Waiting);
    assert_eq!(
        waiting[0].detail,
        "Waiting for a stack build · Comparing flats · 1 of 4 pairs"
    );
    assert_eq!(waiting[0].percent, Some(25.0));
}

#[test]
fn frame_sites_show_progress_and_the_next_pass() {
    let reading = FrameSitesStatus {
        state: "running".into(),
        active: true,
        done: 812,
        total: 1867,
        ..FrameSitesStatus::default()
    };
    let rows = row_list(&with_db(DatabaseState {
        sites: reading.clone(),
        ..db("astro", "Observatory")
    }));
    assert_eq!(rows[0].key, "sites:astro");
    assert_eq!(rows[0].detail, "Reading frame sites · 812 of 1,867 frames");
    assert!(rows[0].automatic && rows[0].stoppable);
    assert_eq!(
        rows[0].control,
        Some(RowControl::Sites {
            db_id: "astro".into()
        })
    );
    assert_eq!(rows[0].remaining, None);

    let waiting = row_list(&with_db(DatabaseState {
        sites: FrameSitesStatus {
            waiting_for: Some("a stack build".into()),
            next_pass: true,
            ..reading.clone()
        },
        ..db("astro", "Observatory")
    }));
    assert_eq!(
        waiting[0].detail,
        "Waiting for a stack build · Reading frame sites · 812 of 1,867 frames"
    );
    assert_eq!(waiting[0].remaining.as_deref(), Some("another pass queued"));
    assert!(row_list(&with_db(DatabaseState {
        sites: FrameSitesStatus {
            active: false,
            ..reading
        },
        ..db("astro", "Observatory")
    }))
    .is_empty());
    assert_eq!(
        crate::server::frame_sites::label(0, 1),
        "Reading frame sites · 0 of 1 frame"
    );
}

#[test]
fn reject_removal_shows_its_stage_and_a_stop() {
    let removing = RemovalJobStatus {
        state: "running".into(),
        action: Some("remove".into()),
        stage: Some("moving".into()),
        done: 37,
        total: 203,
        ..RemovalJobStatus::default()
    };
    let rows = row_list(&with_db(DatabaseState {
        rejects: removing.clone(),
        ..db("c925", "C925")
    }));
    assert_eq!(rows[0].title, "Removing rejects");
    assert_eq!(rows[0].detail, "Moving to the trash 37 of 203");
    assert!(!rows[0].background && rows[0].stoppable);
    assert_eq!(rows[0].link, None);
    let preview = row_list(&with_db(DatabaseState {
        rejects: RemovalJobStatus {
            action: Some("preview".into()),
            stage: Some("indexing".into()),
            done: 2,
            total: 5,
            ..removing.clone()
        },
        ..db("c925", "C925")
    }));
    assert_eq!(preview[0].title, "Previewing reject removal");
    assert_eq!(preview[0].detail, "Indexing folders 2 of 5");
    assert!(row_list(&with_db(DatabaseState {
        rejects: RemovalJobStatus {
            state: "stopped".into(),
            ..removing
        },
        ..db("c925", "C925")
    }))
    .is_empty());
}

/// A duplicate status, which has a private field, set by `edit`.
fn duplicates(edit: impl FnOnce(&mut DuplicateJobStatus)) -> DuplicateJobStatus {
    edited(&DuplicateJobStatus::default(), edit)
}

fn edited(
    status: &DuplicateJobStatus,
    edit: impl FnOnce(&mut DuplicateJobStatus),
) -> DuplicateJobStatus {
    let mut status = status.clone();
    edit(&mut status);
    status
}

#[test]
fn duplicate_checks_and_repairs_show_their_stage() {
    let checking = duplicates(|status| {
        status.state = "running".into();
        status.action = Some("check".into());
        status.stage = Some("checking_files".into());
        status.done = 812;
        status.total = 2140;
    });
    let rows = row_list(&with_db(DatabaseState {
        duplicates: checking.clone(),
        ..db("ultracat", "Ultracat")
    }));
    assert_eq!(rows[0].title, "Checking for duplicates");
    assert_eq!(rows[0].detail, "Checking files 812 of 2,140");
    assert!(!rows[0].automatic);
    assert_eq!(
        rows[0].link,
        Some(RowLink::view(LinkView::Calibration, "ultracat").param("calDupes", "true"))
    );
    let repairing = row_list(&with_db(DatabaseState {
        duplicates: edited(&checking, |status| {
            status.action = Some("repair".into());
            status.stage = Some("merging".into());
            status.done = 37;
            status.total = 203;
        }),
        ..db("ultracat", "Ultracat")
    }));
    assert_eq!(repairing[0].title, "Repairing duplicates");
    assert_eq!(repairing[0].detail, "Merging 37 of 203");
    // The check a page starts on its own is background work, listed as
    // waiting while it pauses for a person's work.
    let waiting = row_list(&with_db(DatabaseState {
        duplicates: edited(&checking, |status| {
            status.done = 0;
            status.total = 0;
            status.priority = Some("background".into());
            status.waiting_for = Some("a stack build".into());
        }),
        ..db("ultracat", "Ultracat")
    }));
    assert_eq!(waiting[0].status, RowStatus::Waiting);
    assert!(waiting[0].automatic && waiting[0].background);
    assert_eq!(
        waiting[0].detail,
        "Waiting for a stack build · Checking files…"
    );
}

fn backup_job(trigger: crate::backup::Trigger) -> BackupJob {
    BackupJob {
        trigger,
        scope: crate::backup::BackupScope::Full,
        include_secrets: false,
        destination: "/backups".into(),
        location: Location::Elsewhere,
        stage: "copying".into(),
        current: Some("catalog imaging-rig".into()),
        files_done: 2,
        files_total: 8,
        bytes_done: 30,
        bytes_total: 120,
        waiting_for: None,
        started_at: 1700,
        stopping: false,
    }
}

#[test]
fn backups_show_their_progress_and_trigger() {
    let snapshot = Snapshot {
        backup: Some(backup_job(crate::backup::Trigger::Manual)),
        ..Snapshot::default()
    };
    let row = row_list(&snapshot).remove(0);
    assert_eq!(row.key, "backup:1700");
    assert_eq!(row.title, "Backing up");
    assert_eq!(row.scope, "Settings and catalogs");
    assert_eq!(row.detail, "Copying catalog imaging-rig · 2/8 files");
    assert_eq!(row.percent, Some(25.0));
    assert!(!row.automatic && row.stoppable);
    assert_eq!(row.control, Some(RowControl::Backup));
    assert_eq!(row.hint.as_deref(), Some("/backups"));
    assert_eq!(row.link, None);
    assert_eq!(row.remaining, None);

    let scheduled = Snapshot {
        backup: Some(BackupJob {
            scope: crate::backup::BackupScope::Meta,
            waiting_for: Some("a stack build".into()),
            ..backup_job(crate::backup::Trigger::Schedule)
        }),
        ..Snapshot::default()
    };
    let row = row_list(&scheduled).remove(0);
    assert_eq!(row.title, "Scheduled backup");
    assert_eq!(row.scope, "Settings");
    assert_eq!(row.status, RowStatus::Waiting);
    assert_eq!(row.detail, "Waiting for a stack build");
    assert!(row.automatic && row.background);
}

#[test]
fn catalog_installs_show_bytes_or_files() {
    let progress = CatalogInstallProgress {
        running: true,
        message: "Downloading stars-gaia-photometry.bin…".into(),
        preset: Some(CatalogInstallPreset::GaiaPhotometry),
        output_dir: Some("/catalogs".into()),
        files_completed: 0,
        files_total: 1,
        bytes_completed: Some(119),
        bytes_total: Some(476),
        started_at: Some(1800),
        ..CatalogInstallProgress::default()
    };
    let row = row_list(&Snapshot {
        catalog_install: Some(progress.clone()),
        ..Snapshot::default()
    })
    .remove(0);
    assert_eq!(row.key, "catalog:1800");
    assert_eq!(row.title, "Installing Gaia colors");
    assert_eq!(row.scope, "Seiza catalogs");
    assert_eq!(row.percent, Some(25.0));
    assert!(!row.background);

    let files = row_list(&Snapshot {
        catalog_install: Some(CatalogInstallProgress {
            preset: Some(CatalogInstallPreset::BlindDeep),
            bytes_completed: None,
            bytes_total: None,
            files_completed: 3,
            files_total: 6,
            ..progress.clone()
        }),
        ..Snapshot::default()
    });
    assert_eq!(files[0].title, "Installing catalogs");
    assert_eq!(files[0].percent, Some(50.0));
    assert!(row_list(&Snapshot {
        catalog_install: Some(CatalogInstallProgress {
            running: false,
            ..progress
        }),
        ..Snapshot::default()
    })
    .is_empty());
}

#[test]
fn transfers_are_filed_by_who_queued_them_and_review_needs_attention() {
    let row = |job: Value| transfer_row(&job).unwrap();
    assert!(row(transfer("t", "queued", true)).background);
    assert!(!row(transfer("t", "queued", false)).background);
    assert_eq!(
        row(transfer("t", "review", true)).status,
        RowStatus::Attention
    );
    assert_eq!(
        row(transfer("t", "needs_review", false)).status,
        RowStatus::Attention
    );
    assert_eq!(
        row(transfer("t", "retry", false)).status,
        RowStatus::Waiting
    );
    assert_eq!(
        row(transfer("t", "paused", false)).status,
        RowStatus::Waiting
    );
    let uploading = row(transfer("t", "uploading", false));
    assert_eq!(uploading.status, RowStatus::Running);
    assert_eq!(uploading.state_label.as_deref(), Some("Uploading"));
    assert_eq!(uploading.detail, "4/10 files · 0.0 MiB");
    assert_eq!(uploading.percent, Some(40.0));
    assert!(uploading.transfer.is_some());
    assert!(transfer_row(&transfer("t", "complete", false)).is_none());
}

#[test]
fn rows_go_by_tier_then_attention_running_queued_in_line_order_and_waiting() {
    let mut snapshot = with_db(DatabaseState {
        refresh: Some(RefreshProgress {
            targets_total: 15,
            targets_processed: 3,
            ..refresh(RefreshStage::ProcessingTargets)
        }),
        backfill: backfill(2, 8),
        scan: SpatialScanProgress {
            automatic: true,
            waiting_for: Some("a stack build".into()),
            ..scan(4, 10, "astrometry")
        },
        ..db("a", "Askar")
    });
    snapshot.stacks.active = vec![
        stack_with("second", "queued", 0, 0, json!({"queue_position": 1})),
        stack_with(
            "auto",
            "queued",
            0,
            0,
            json!({"queue_position": 2, "automatic": true}),
        ),
        stack("run", "running", 3, 12),
        stack_with("first", "queued", 0, 0, json!({"queue_position": 0})),
    ];
    snapshot.wbpp = wbpp(
        Vec::new(),
        vec![
            wbpp_queued("next", "a", Some(1), None),
            wbpp_queued("late", "a", Some(1), None),
        ],
    );
    snapshot.transfers = vec![
        {
            let mut paused = transfer("paused", "paused", false);
            paused["label"] = json!("M13");
            paused
        },
        transfer("held", "needs_review", false),
        transfer("policy", "queued", true),
    ];
    let rows = row_list(&snapshot);
    let listed: Vec<(&str, &str, RowStatus, bool)> = rows
        .iter()
        .map(|row| {
            (
                row.title.as_str(),
                row.scope.as_str(),
                row.status,
                row.background,
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("Transfers", "Askar · M44", RowStatus::Attention, false),
            ("Stacking", "M44 · run", RowStatus::Running, false),
            ("Stacking", "M44 · first", RowStatus::Queued, false),
            ("Stacking", "M44 · second", RowStatus::Queued, false),
            ("WBPP", "Askar · project next", RowStatus::Queued, false),
            ("WBPP", "Askar · project late", RowStatus::Queued, false),
            ("Transfers", "Askar · M13", RowStatus::Waiting, false),
            ("Refreshing catalog", "Askar", RowStatus::Running, true),
            ("Stacking", "M44 · auto", RowStatus::Queued, true),
            ("Transfers", "Askar · M44", RowStatus::Queued, true),
            (
                "Analyzing database quality",
                "Askar",
                RowStatus::Waiting,
                true
            ),
        ]
    );
}

#[test]
fn no_row_repeats_what_its_detail_says() {
    let mut snapshot = with_db(DatabaseState {
        refresh: Some(RefreshProgress {
            targets_total: 15,
            targets_processed: 3,
            ..refresh(RefreshStage::ProcessingTargets)
        }),
        backfill: backfill(2, 8),
        scan: scan(4, 10, "astrometry"),
        ..db("a", "Askar")
    });
    snapshot.stacks.active = vec![
        stack("R", "running", 12, 40),
        stack_with("RGB", "running", 1, 4, json!({"kind": "color"})),
    ];
    snapshot.backup = Some(backup_job(crate::backup::Trigger::Manual));
    let rows = row_list(&snapshot);
    assert!(rows.len() > 4);
    for row in rows {
        assert_eq!(row.remaining, None, "{}", row.title);
    }
}

#[test]
fn flat_rigs_left_counts_this_pass_and_the_next() {
    assert_eq!(
        crate::server::flat_measure::rigs_left(2, &[]).as_deref(),
        Some("2 more rigs to check")
    );
    assert_eq!(
        crate::server::flat_measure::rigs_left(1, &["x".into(), "y".into(), "z".into()]).as_deref(),
        Some("1 more rig to check · 3 rigs queued")
    );
    assert_eq!(
        crate::server::flat_measure::rigs_left(0, &["*".into()]).as_deref(),
        Some("every rig queued")
    );
    assert_eq!(crate::server::flat_measure::rigs_left(0, &[]), None);
}

#[test]
fn target_scheduler_jobs_run_wait_or_need_attention() {
    let job = TsSyncJob {
        id: "import:askar".into(),
        name: "Askar".into(),
        running: true,
        source_connected: true,
        stage: "copying".into(),
        pages_done: 1,
        pages_total: 4,
        ..TsSyncJob::default()
    };
    let row = ts_sync_row(&job).unwrap();
    assert_eq!(row.title, "Importing Target Scheduler");
    assert_eq!(row.detail, "Copying a consistent database snapshot");
    assert_eq!(row.percent, Some(25.0));
    assert!(row.stoppable);
    assert_eq!(
        row.control,
        Some(RowControl::TsSync {
            job_id: "import:askar".into()
        })
    );
    let failed = ts_sync_row(&TsSyncJob {
        running: false,
        error: Some("locked".into()),
        ..job.clone()
    })
    .unwrap();
    assert_eq!(failed.status, RowStatus::Attention);
    assert_eq!(failed.detail, "locked");
    assert_eq!(failed.control, None);
    assert!(ts_sync_row(&TsSyncJob {
        running: false,
        ..job
    })
    .is_none());
}

#[test]
fn a_database_is_busy_while_work_changes_its_data() {
    let busy = |state: DatabaseState| databases(&with_db(state), &no_projects)[0].busy;
    assert!(!busy(db("a", "Askar")));
    assert!(busy(DatabaseState {
        flats: FlatMeasureStatus {
            active: true,
            ..FlatMeasureStatus::default()
        },
        ..db("a", "Askar")
    }));
    // A duplicate check and a removal preview change nothing.
    let check = duplicates(|status| {
        status.state = "running".into();
        status.action = Some("check".into());
    });
    assert!(!busy(DatabaseState {
        duplicates: check.clone(),
        ..db("a", "Askar")
    }));
    assert!(busy(DatabaseState {
        duplicates: edited(&check, |status| status.action = Some("repair".into())),
        ..db("a", "Askar")
    }));
    let preview = RemovalJobStatus {
        state: "running".into(),
        action: Some("preview".into()),
        ..RemovalJobStatus::default()
    };
    assert!(!busy(DatabaseState {
        rejects: preview.clone(),
        ..db("a", "Askar")
    }));
    assert!(busy(DatabaseState {
        rejects: RemovalJobStatus {
            action: Some("remove".into()),
            ..preview
        },
        ..db("a", "Askar")
    }));

    let scanned = databases(
        &with_db(DatabaseState {
            scan: SpatialScanProgress {
                running: false,
                errors: 2,
                last_error: Some("unreadable".into()),
                ..scan(10, 10, "astrometry")
            },
            ..db("a", "Askar")
        }),
        &|_, target| (target == 42).then_some(4),
    );
    assert!(!scanned[0].scanning);
    assert_eq!(
        scanned[0].scan,
        ScanResult {
            errors: 2,
            last_error: Some("unreadable".into()),
            target_id: Some(42),
            project_id: Some(4),
        }
    );
}

#[test]
fn counts_have_thousands_separators() {
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1867), "1,867");
    assert_eq!(grouped(1_234_567), "1,234,567");
}

#[tokio::test]
async fn the_route_lists_a_queued_wbpp_run_and_each_database() {
    let state = Arc::new(AppState::new_for_test(
        rusqlite::Connection::open_in_memory().unwrap(),
    ));
    let db_id = state.all_databases()[0].id.clone();
    // PixInsight is busy, so the run stays queued.
    let _pixinsight = state.wbpp_queue.try_reserve_pixinsight().unwrap();
    state.wbpp_queue.push(
        &state,
        &db_id,
        "project Bubble".into(),
        crate::server::wbpp_run::StartWbppRunRequest {
            project_id: Some(1),
            ..Default::default()
        },
        1,
    );
    let Json(response) = get_jobs(State(Arc::clone(&state)), None).await;
    let response = response.data.unwrap();
    let wbpp: Vec<&JobRow> = response
        .jobs
        .iter()
        .filter(|row| row.kind == RowKind::Wbpp)
        .collect();
    assert_eq!(wbpp.len(), 1);
    assert_eq!(wbpp[0].status, RowStatus::Queued);
    assert_eq!(wbpp[0].position, Some(0));
    assert_eq!(response.databases.len(), state.all_databases().len());
    assert!(!response.databases[0].busy);
    // The JSON has the shape the header reads.
    let json = serde_json::to_value(wbpp[0]).unwrap();
    assert_eq!(json["status"], "queued");
    assert_eq!(json["queue"], "wbpp");
    assert_eq!(json["control"]["kind"], "wbpp-queued");
    assert_eq!(json["link"]["view"], "stacks");
}

#[test]
fn a_queued_scan_says_it_finds_its_frames_first() {
    let rows = row_list(&with_db(DatabaseState {
        scan: scan(0, 0, crate::server::spatial_scan::STAGE_PREPARING),
        ..db("a", "Askar")
    }));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail, "Finding frames to analyze");
    assert_eq!(rows[0].percent, None);
}

#[test]
fn calibration_grading_is_listed_once_it_measures_frames() {
    use crate::server::calibration_quality::CalibrationQualityProgress;
    let grading = |stage: &str, done: usize, to_measure: usize| CalibrationQualityProgress {
        running: true,
        stage: Some(stage.into()),
        to_measure,
        done,
        ..CalibrationQualityProgress::default()
    };
    // A pass that only regrades the library is quick: no row.
    assert!(row_list(&with_db(DatabaseState {
        grading: grading("grading", 0, 0),
        ..db("a", "Askar")
    }))
    .is_empty());
    let rows = row_list(&with_db(DatabaseState {
        grading: grading("measuring", 3, 1_200),
        ..db("a", "Askar")
    }));
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].title.as_str(),
            rows[0].scope.as_str(),
            rows[0].detail.as_str()
        ),
        (
            "Grading calibration frames",
            "Askar",
            "Measuring 3 of 1,200 frames"
        )
    );
    assert_eq!(rows[0].percent, Some(0.25));
    assert!(rows[0].background && rows[0].automatic);
    assert_eq!(
        rows[0].link,
        Some(RowLink::view(LinkView::Calibration, "a"))
    );
    // While a person's work runs, the row waits for it.
    let mut snapshot = with_db(DatabaseState {
        grading: grading("waiting", 3, 1_200),
        ..db("a", "Askar")
    });
    snapshot.interactive = Some("a stack build".into());
    let waiting = row_list(&snapshot);
    assert_eq!(waiting[0].status, RowStatus::Waiting);
    assert_eq!(waiting[0].waiting_for.as_deref(), Some("a stack build"));
    assert_eq!(
        waiting[0].detail,
        "Waiting for a stack build · Measuring 3 of 1,200 frames"
    );
}

#[test]
fn calibration_health_is_listed_once_its_pass_has_projects_to_judge() {
    use crate::server::calibration_health::HealthProgress;
    let health = |judged: usize, total: usize| HealthProgress {
        running: true,
        judged,
        total,
    };
    assert!(row_list(&with_db(DatabaseState {
        health: health(0, 0),
        ..db("a", "Askar")
    }))
    .is_empty());
    let rows = row_list(&with_db(DatabaseState {
        health: health(1, 4),
        ..db("a", "Askar")
    }));
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].title.as_str(), rows[0].detail.as_str()),
        ("Checking calibration health", "1 of 4 projects")
    );
    assert_eq!(rows[0].percent, Some(25.0));
    assert!(rows[0].background);
    assert_eq!(
        rows[0].link,
        Some(RowLink::view(LinkView::Calibration, "a"))
    );
}
