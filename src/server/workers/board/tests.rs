use super::*;
use crate::remote_work::protocol::{
    FileAccess, PathMap, Progress, WrittenOutput, PROTOCOL_VERSION,
};
use crate::server::workers::Waiting;

fn schema(tool: &str, version: &str, licensed: bool) -> seiza_stacking::ExternalToolSchema {
    serde_json::from_value(serde_json::json!({
        "schema_version": 6,
        "cli_version": version,
        "key": tool,
        "name": format!("RC-Astro {tool}"),
        "ml_version": 4,
        "licensed": licensed,
        "license_message": null,
        "parameters": [],
    }))
    .unwrap()
}

fn offers(tools: &[(&str, &str)]) -> Offers {
    Offers {
        threads: 8,
        slots: 1,
        work: vec![TaskKind::RcAstroStep],
        stacking_version: None,
        stacking_release: None,
        steps: Vec::new(),
        rc_astro: tools
            .iter()
            .map(|(tool, version)| schema(tool, version, true))
            .collect(),
    }
}

fn registration(name: &str, offers: Offers, files: FileAccess) -> Registration {
    Registration {
        protocol: PROTOCOL_VERSION,
        name: name.into(),
        version: "test".into(),
        offers,
        files,
    }
}

fn needs(tool: &str, version: &str) -> TaskNeeds {
    TaskNeeds::Step {
        provider: tool.into(),
        name: format!("RC-Astro {tool}"),
        version: version.into(),
        ml_version: Some(4),
    }
}

fn task(label: &str, person: bool, needs: TaskNeeds) -> NewTask {
    NewTask {
        kind: TaskKind::RcAstroStep,
        label: label.into(),
        person,
        needs,
        spec: serde_json::json!({}),
        inputs: Vec::new(),
        outputs: vec![("out.bin".into(), true), ("extra.bin".into(), false)],
    }
}

/// A progress report with no events.
fn at(fraction: f32) -> Progress {
    Progress {
        fraction,
        events: Vec::new(),
        first_event: 0,
    }
}

fn board(dir: &Path, timings: Timings) -> Arc<Board> {
    Arc::new(Board::with_timings(dir.to_path_buf(), timings))
}

fn quick() -> Timings {
    Timings {
        lease: Duration::from_millis(200),
        no_worker_grace: Duration::from_millis(300),
    }
}

/// Write `bytes` as the output `name` of the task `claimed` holds, where
/// an upload would put it, and describe it as the worker would.
fn written(
    board: &Board,
    session: &str,
    claimed: &Assignment,
    name: &str,
    bytes: &[u8],
) -> WrittenOutput {
    let path = board
        .output_path(session, &claimed.task, &claimed.lease, name)
        .unwrap();
    std::fs::write(&path, bytes).unwrap();
    let (sha256, size) = crate::remote_work::file_digest(&path).unwrap();
    WrittenOutput {
        name: name.into(),
        size,
        sha256,
    }
}

#[test]
fn a_person_s_task_goes_first_and_a_worker_claims_only_what_it_can_run() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let automatic = board.ticket().unwrap();
    automatic
        .submit(task("automatic", false, needs("nxt", "2.6.6")))
        .unwrap();
    let other_version = board.ticket().unwrap();
    other_version
        .submit(task("other version", true, needs("nxt", "2.7.0")))
        .unwrap();
    let person = board.ticket().unwrap();
    person
        .submit(task("person", true, needs("nxt", "2.6.6")))
        .unwrap();
    let session = board.register(
        "token",
        registration("w", offers(&[("nxt", "2.6.6")]), FileAccess::default()),
        false,
    );
    let first = board.try_claim(&session).unwrap().unwrap();
    assert_eq!(first.label, "person");
    // Its one slot is busy.
    assert!(board.try_claim(&session).unwrap().is_none());
    board
        .fail(&session, &first.task, &first.lease, "no".into())
        .unwrap();
    let third = board.try_claim(&session).unwrap().unwrap();
    assert_eq!(third.label, "automatic");
    board
        .fail(&session, &third.task, &third.lease, "no".into())
        .unwrap();
    // 2.7.0 needs a worker with that version.
    assert!(board.try_claim(&session).unwrap().is_none());
    assert!(matches!(
        board.try_claim("unknown"),
        Err(Refusal::UnknownSession)
    ));
}

#[test]
fn a_finished_task_hands_back_its_checked_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let session = board.register(
        "token",
        registration("studio", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let claimed = board.try_claim(&session).unwrap().unwrap();
    // A remote worker is told no paths for outputs: it uploads them.
    assert!(claimed.outputs.iter().all(|output| output.path.is_none()));
    assert!(
        !board
            .progress(&session, &claimed.task, &claimed.lease, at(0.5))
            .unwrap()
            .stop
    );
    let out = written(&board, &session, &claimed, "out.bin", b"pixels");
    board
        .complete(
            &session,
            &claimed.task,
            &claimed.lease,
            Completion {
                result: serde_json::json!({"device": "gpu"}),
                outputs: vec![out],
            },
        )
        .unwrap();
    let mut heard = Vec::new();
    let finished = ticket
        .wait(&|| false, &mut |fraction, worker| {
            heard.push((fraction, worker.map(str::to_string)))
        })
        .unwrap();
    assert_eq!(finished.worker, "studio");
    assert_eq!(finished.result["device"], "gpu");
    assert_eq!(
        std::fs::read(&finished.outputs["out.bin"]).unwrap(),
        b"pixels"
    );
    assert!(!finished.outputs.contains_key("extra.bin"));
    assert!(finished.outputs["out.bin"].starts_with(ticket.dir().join(&claimed.lease)));
    let task_dir = ticket.dir().to_path_buf();
    drop(ticket);
    assert!(!task_dir.exists(), "the ticket removes the task's files");
}

#[test]
fn an_output_that_does_not_match_its_digest_fails_the_task() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let session = board.register(
        "token",
        registration("w", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let claimed = board.try_claim(&session).unwrap().unwrap();
    let mut out = written(&board, &session, &claimed, "out.bin", b"pixels");
    out.sha256 = "0".repeat(64);
    let refused = board.complete(
        &session,
        &claimed.task,
        &claimed.lease,
        Completion {
            result: serde_json::json!({}),
            outputs: vec![out],
        },
    );
    assert!(matches!(refused, Err(Refusal::Invalid(_))), "{refused:?}");
    let failed = ticket.wait(&|| false, &mut |_, _| {}).unwrap_err();
    assert!(
        matches!(&failed, WaitError::Failed(message) if message.contains("digest")),
        "{failed:?}"
    );
}

#[test]
fn a_missing_required_output_fails_the_task() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let session = board.register(
        "token",
        registration("w", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let claimed = board.try_claim(&session).unwrap().unwrap();
    let extra = written(&board, &session, &claimed, "extra.bin", b"stars");
    assert!(board
        .complete(
            &session,
            &claimed.task,
            &claimed.lease,
            Completion {
                result: serde_json::json!({}),
                outputs: vec![extra],
            },
        )
        .is_err());
    let failed = ticket.wait(&|| false, &mut |_, _| {}).unwrap_err();
    assert!(
        matches!(&failed, WaitError::Failed(message) if message.contains("out.bin")),
        "{failed:?}"
    );
}

#[test]
fn a_lost_lease_puts_the_task_back_and_a_late_answer_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), quick());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let slow = board.register(
        "token",
        registration("slow", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let first = board.try_claim(&slow).unwrap().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let fast = board.register(
        "token",
        registration("fast", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let second = board.try_claim(&fast).unwrap().unwrap();
    assert_eq!(second.task, first.task);
    assert_ne!(second.lease, first.lease);
    // The slow worker's lease ran out: it is told to stop, and its result
    // is refused. Its lease folder is gone, so it cannot write over the
    // next worker's outputs.
    assert!(
        board
            .progress(&slow, &first.task, &first.lease, at(0.9))
            .unwrap()
            .stop
    );
    assert!(!ticket.dir().join(&first.lease).exists());
    let out = written(&board, &fast, &second, "out.bin", b"late");
    assert_eq!(
        board.complete(
            &slow,
            &first.task,
            &first.lease,
            Completion {
                result: serde_json::json!({}),
                outputs: vec![out.clone()],
            },
        ),
        Err(Refusal::NotCurrent)
    );
    board
        .complete(
            &fast,
            &second.task,
            &second.lease,
            Completion {
                result: serde_json::json!({}),
                outputs: vec![out],
            },
        )
        .unwrap();
    assert_eq!(
        ticket.wait(&|| false, &mut |_, _| {}).unwrap().worker,
        "fast"
    );
}

#[test]
fn a_task_that_loses_every_lease_fails() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), quick());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let session = board.register(
        "token",
        registration("flaky", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    for _ in 0..MAX_ATTEMPTS {
        // Each claim runs out without a report.
        let mut claimed = None;
        for _ in 0..50 {
            claimed = board.try_claim(&session).unwrap();
            if claimed.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(claimed.is_some());
        std::thread::sleep(Duration::from_millis(250));
        // Seen again, so the worker stays connected for the grace.
        board.update_offers(&session, offers(&[("bxt", "1")]));
    }
    let failed = ticket.wait(&|| false, &mut |_, _| {}).unwrap_err();
    assert!(
        matches!(&failed, WaitError::Failed(message) if message.contains("flaky stopped answering")),
        "{failed:?}"
    );
}

#[test]
fn a_task_no_connected_worker_can_run_fails_after_the_grace() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), quick());
    board.register(
        "token",
        registration("old", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "2"))).unwrap();
    let started = Instant::now();
    let failed = ticket.wait(&|| false, &mut |_, _| {}).unwrap_err();
    assert_eq!(
        failed,
        WaitError::Failed("No connected worker offers RC-Astro bxt 2".into())
    );
    assert!(started.elapsed() >= Duration::from_millis(300));
}

#[test]
fn a_stop_takes_the_task_off_the_board_and_the_worker_is_told() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let session = board.register(
        "token",
        registration("w", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let claimed = board.try_claim(&session).unwrap().unwrap();
    assert_eq!(
        ticket.wait(&|| true, &mut |_, _| {}).unwrap_err(),
        WaitError::Stopped
    );
    assert!(
        board
            .progress(&session, &claimed.task, &claimed.lease, at(0.5))
            .unwrap()
            .stop
    );
    assert_eq!(
        board.fail(&session, &claimed.task, &claimed.lease, "late".into()),
        Err(Refusal::NotCurrent)
    );
    // The worker is free for the next task.
    let next = board.ticket().unwrap();
    next.submit(task("next", true, needs("bxt", "1"))).unwrap();
    assert_eq!(board.try_claim(&session).unwrap().unwrap().label, "next");
}

#[test]
fn a_revoked_token_disconnects_its_worker_and_requeues_its_task() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let revoked = board.register(
        "revoked",
        registration("a", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let claimed = board.try_claim(&revoked).unwrap().unwrap();
    board.revoke_token("revoked");
    assert_eq!(board.token_of(&revoked), None);
    assert!(matches!(
        board.progress(&revoked, &claimed.task, &claimed.lease, at(0.1)),
        Err(Refusal::UnknownSession)
    ));
    let kept = board.register(
        "kept",
        registration("b", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    assert_eq!(board.try_claim(&kept).unwrap().unwrap().task, claimed.task);
}

#[test]
fn a_worker_that_registers_again_replaces_its_old_session() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let first = board.register(
        "token",
        registration("studio", offers(&[]), FileAccess::default()),
        false,
    );
    let second = board.register(
        "token",
        registration("studio", offers(&[]), FileAccess::default()),
        false,
    );
    assert_eq!(board.token_of(&first), None);
    assert_eq!(board.token_of(&second).as_deref(), Some("token"));
    assert_eq!(board.summary().0.len(), 1);
}

#[test]
fn a_shared_worker_gets_paths_once_it_answers_the_probe() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let shared = FileAccess {
        mode: FileMode::Shared,
        maps: vec![PathMap {
            server: "/srv".into(),
            local: "/mnt".into(),
        }],
    };
    let session = board.register(
        "token",
        registration("nas", offers(&[("bxt", "1")]), shared),
        false,
    );
    let probe = board.probe_of(&session).unwrap();
    // A wrong answer leaves the files going through the server.
    let result = board
        .answer_probe(
            &session,
            ProbeAnswer {
                digest: Some("nope".into()),
                problem: None,
            },
        )
        .unwrap();
    assert!(!result.shared);
    assert!(result.reason.is_some());

    let again = board.register(
        "token",
        registration(
            "nas",
            offers(&[("bxt", "1")]),
            FileAccess {
                mode: FileMode::Shared,
                maps: Vec::new(),
            },
        ),
        false,
    );
    assert_eq!(board.token_of(&session), None);
    let probe_again = board.probe_of(&again).unwrap();
    assert_ne!(probe.path, probe_again.path);
    let content = std::fs::read(&probe_again.path).unwrap();
    std::fs::write(&probe_again.reply_path, &content).unwrap();
    let digest = {
        use sha2::{Digest, Sha256};
        crate::remote_work::hex(&Sha256::digest(&content))
    };
    let result = board
        .answer_probe(
            &again,
            ProbeAnswer {
                digest: Some(digest),
                problem: None,
            },
        )
        .unwrap();
    assert!(result.shared, "{result:?}");
    assert!(!Path::new(&probe_again.path).exists());

    let input = dir.path().join("in.fits");
    std::fs::write(&input, b"frame").unwrap();
    let ticket = board.ticket().unwrap();
    let mut new = task("t", true, needs("bxt", "1"));
    new.inputs = vec![("input.fits".into(), input.clone())];
    ticket.submit(new).unwrap();
    let claimed = board.try_claim(&again).unwrap().unwrap();
    assert_eq!(
        claimed.inputs[0].path.as_deref(),
        Some(input.display().to_string().as_str())
    );
    assert_eq!(claimed.inputs[0].size, 5);
    assert_eq!(
        claimed.outputs[0].path.as_deref(),
        Some(
            ticket
                .dir()
                .join(&claimed.lease)
                .join("out.bin")
                .display()
                .to_string()
                .as_str()
        )
    );
}

#[test]
fn this_server_would_run_a_step_only_when_no_other_worker_could_claim_it() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let step = needs("sxt", "1");
    let here = || board.runs_here(TaskKind::RcAstroStep, &step);
    assert!(!here(), "this server has not offered the tool");
    board.register(
        "this-server",
        registration(
            "This server",
            offers(&[("sxt", "1")]),
            FileAccess::default(),
        ),
        true,
    );
    assert!(here());
    // Workers whose tool is unlicensed, or at another version, cannot
    // claim the step.
    let mut unlicensed = offers(&[("sxt", "1")]);
    unlicensed.rc_astro[0].licensed = false;
    board.register(
        "token-a",
        registration("unlicensed", unlicensed, FileAccess::default()),
        false,
    );
    board.register(
        "token-b",
        registration("older", offers(&[("sxt", "0.9")]), FileAccess::default()),
        false,
    );
    assert!(here());
    board.register(
        "token-c",
        registration("gpu-box", offers(&[("sxt", "1")]), FileAccess::default()),
        false,
    );
    assert!(!here(), "the worker that can claim it goes first");
    board.set_placement(TaskKind::RcAstroStep, WorkPlacement::ThisServer);
    assert!(here());
    board.set_placement(TaskKind::RcAstroStep, WorkPlacement::WorkersOnly);
    assert!(!here());
}

#[test]
fn this_server_runs_a_task_only_while_no_other_worker_can() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let local = board.register(
        "this-server",
        registration(
            "This server",
            offers(&[("bxt", "1")]),
            FileAccess::default(),
        ),
        true,
    );
    let remote = board.register(
        "token",
        registration("gpu-box", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    // The remote worker goes first in the schema order, this server last.
    let order = board
        .step_offers()
        .into_iter()
        .map(|(name, _, _)| name)
        .collect::<Vec<_>>();
    assert_eq!(order, ["gpu-box", "This server"]);

    let first = board.ticket().unwrap();
    first
        .submit(task("first", true, needs("bxt", "1")))
        .unwrap();
    assert!(
        board.try_claim(&local).unwrap().is_none(),
        "the remote worker takes it"
    );
    let claimed = board.try_claim(&remote).unwrap().unwrap();
    assert_eq!(claimed.label, "first");

    // A tool the remote worker does not take stays with this server.
    board.update_offers(&local, offers(&[("bxt", "1"), ("nxt", "1")]));
    let other = board.ticket().unwrap();
    other
        .submit(task("noise", true, needs("nxt", "1")))
        .unwrap();
    let noise = board.try_claim(&local).unwrap().unwrap();
    assert_eq!(noise.label, "noise");
    board
        .fail(&local, &noise.task, &noise.lease, "done".into())
        .unwrap();

    // Once the remote worker is gone, this server falls back to its work.
    board.leave(&remote);
    let fallback = board.try_claim(&local).unwrap().unwrap();
    assert_eq!(fallback.label, "first");
}

#[test]
fn a_worker_takes_only_the_work_it_advertises() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), quick());
    let mut declined = offers(&[("bxt", "1")]);
    declined.work.clear();
    let session = board.register(
        "token",
        registration("cpu-box", declined, FileAccess::default()),
        false,
    );
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    assert!(board.try_claim(&session).unwrap().is_none());
    assert!(matches!(
        ticket.wait(&|| false, &mut |_, _| {}),
        Err(WaitError::Failed(message)) if message.starts_with("No connected worker offers")
    ));
}

#[test]
fn a_task_whose_worker_keeps_reconnecting_fails() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    // Each time the worker claims the task, it crashes and its restart
    // registers again.
    for _ in 0..MAX_ATTEMPTS {
        let session = board.register(
            "token",
            registration("crashy", offers(&[("bxt", "1")]), FileAccess::default()),
            false,
        );
        if board.try_claim(&session).unwrap().is_none() {
            break;
        }
    }
    board.register(
        "token",
        registration("crashy", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let failed = ticket.wait(&|| false, &mut |_, _| {}).unwrap_err();
    assert!(
        matches!(&failed, WaitError::Failed(message)
            if message.contains("went back in the queue 3 times") && message.contains("crashy connected again")),
        "{failed:?}"
    );
}

#[test]
fn a_worker_with_two_slots_runs_two_tasks_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let mut two = offers(&[("bxt", "1")]);
    two.slots = 2;
    let session = board.register(
        "token",
        registration("gpu-box", two, FileAccess::default()),
        false,
    );
    let tickets = (0..3)
        .map(|index| {
            let ticket = board.ticket().unwrap();
            ticket
                .submit(task(&format!("t{index}"), true, needs("bxt", "1")))
                .unwrap();
            ticket
        })
        .collect::<Vec<_>>();
    let first = board.try_claim(&session).unwrap().unwrap();
    let second = board.try_claim(&session).unwrap().unwrap();
    assert_ne!(first.task, second.task);
    assert!(
        board.try_claim(&session).unwrap().is_none(),
        "both slots are busy"
    );
    let summary = board.summary().0;
    assert_eq!(summary[0].slots, 2);
    assert_eq!(summary[0].running.len(), 2);
    board
        .fail(&session, &first.task, &first.lease, "no".into())
        .unwrap();
    assert!(board.try_claim(&session).unwrap().is_some());
    drop(tickets);
}

#[test]
fn placement_keeps_work_on_workers_or_on_this_server() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), quick());
    let local = board.register(
        "this-server",
        registration(
            "This server",
            offers(&[("bxt", "1")]),
            FileAccess::default(),
        ),
        true,
    );
    let remote = board.register(
        "token",
        registration("gpu-box", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );

    // Workers only: this server never takes it, even with no worker free.
    board.set_placement(TaskKind::RcAstroStep, WorkPlacement::WorkersOnly);
    let names = |board: &Board| {
        board
            .step_offers()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&board), ["gpu-box"]);
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    board.leave(&remote);
    assert!(board.try_claim(&local).unwrap().is_none());
    let failed = ticket.wait(&|| false, &mut |_, _| {}).unwrap_err();
    assert!(
        matches!(&failed, WaitError::Failed(message) if message.contains("keeps this work off this server")),
        "{failed:?}"
    );

    // This server only: a connected worker never takes it.
    board.set_placement(TaskKind::RcAstroStep, WorkPlacement::ThisServer);
    let remote = board.register(
        "token",
        registration("gpu-box", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    assert_eq!(names(&board), ["This server"]);
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    assert!(board.try_claim(&remote).unwrap().is_none());
    assert!(board.try_claim(&local).unwrap().is_some());
}

#[test]
fn a_server_start_removes_only_stale_task_files_it_made() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let files = dir.path().join(EXCHANGE_SUBFOLDER);
    let old = files.join("tasks").join(uuid::Uuid::new_v4().to_string());
    let young = files.join("tasks").join(uuid::Uuid::new_v4().to_string());
    let other = files.join("tasks").join("notes");
    let old_probe = files
        .join("probes")
        .join(format!("{}.probe", uuid::Uuid::new_v4()));
    for folder in [&old, &young, &other] {
        std::fs::create_dir_all(folder).unwrap();
    }
    std::fs::create_dir_all(old_probe.parent().unwrap()).unwrap();
    std::fs::write(&old_probe, b"x").unwrap();
    // Someone's own folder beside PSF Guard's is never touched.
    let theirs = dir.path().join("tasks");
    std::fs::create_dir_all(&theirs).unwrap();
    let a_day_ago = filetime::FileTime::from_system_time(
        std::time::SystemTime::now() - Duration::from_secs(24 * 3600),
    );
    for path in [&old, &other, &old_probe, &theirs] {
        filetime::set_file_mtime(path, a_day_ago).unwrap();
    }
    board.remove_stale_files();
    assert!(!old.exists());
    assert!(!old_probe.exists());
    assert!(young.exists(), "another server may still use it");
    assert!(other.exists(), "not a task folder");
    assert!(theirs.exists());
}

#[test]
fn a_task_a_worker_fails_falls_back_to_this_server_when_it_can_run_it() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let local = board.register(
        "this-server",
        registration(
            "This server",
            offers(&[("bxt", "1")]),
            FileAccess::default(),
        ),
        true,
    );
    let remote = board.register(
        "token",
        registration(
            "gpu-box",
            offers(&[("bxt", "1"), ("nxt", "1")]),
            FileAccess::default(),
        ),
        false,
    );
    let ticket = board.ticket().unwrap();
    ticket
        .submit(task("blur", true, needs("bxt", "1")))
        .unwrap();
    let claimed = board.try_claim(&remote).unwrap().unwrap();
    board
        .fail(
            &remote,
            &claimed.task,
            &claimed.lease,
            "the GPU driver crashed".into(),
        )
        .unwrap();
    // The failing worker does not get it back; this server runs it.
    assert!(board.try_claim(&remote).unwrap().is_none());
    let fallback = board.try_claim(&local).unwrap().unwrap();
    assert_eq!(fallback.task, claimed.task);

    // A tool this server lacks fails with the worker's error.
    let noise = board.ticket().unwrap();
    noise
        .submit(task("noise", true, needs("nxt", "1")))
        .unwrap();
    let claimed = board.try_claim(&remote).unwrap().unwrap();
    board
        .fail(&remote, &claimed.task, &claimed.lease, "no licence".into())
        .unwrap();
    assert_eq!(
        noise.wait(&|| false, &mut |_, _| {}).unwrap_err(),
        WaitError::Failed("no licence (on gpu-box)".into())
    );
}

#[test]
fn events_arrive_once_in_order_even_when_a_report_is_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let session = board.register(
        "token",
        registration("w", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let claimed = board.try_claim(&session).unwrap().unwrap();
    let report = |first_event: u64, events: &[i32]| {
        board
            .progress(
                &session,
                &claimed.task,
                &claimed.lease,
                Progress {
                    fraction: 0.5,
                    events: events
                        .iter()
                        .map(|event| serde_json::json!(event))
                        .collect(),
                    first_event,
                },
            )
            .unwrap()
    };
    assert_eq!(report(0, &[1, 2]).events_received, 2);
    // The answer was lost, so the worker sends 1 and 2 again with 3.
    assert_eq!(report(0, &[1, 2, 3]).events_received, 3);
    // A gap is refused until the missing events come.
    assert_eq!(report(5, &[6]).events_received, 3);
    assert_eq!(report(3, &[4]).events_received, 4);
    let out = written(&board, &session, &claimed, "out.bin", b"done");
    board
        .complete(
            &session,
            &claimed.task,
            &claimed.lease,
            Completion {
                result: serde_json::json!({}),
                outputs: vec![out],
            },
        )
        .unwrap();
    let mut heard = Vec::new();
    ticket
        .wait_with(Waiting {
            stopped: &|| false,
            progress: &mut |_, _| {},
            events: &mut |event| heard.push(event.as_i64().unwrap()),
            preview: &mut |_| {},
            finish_on_stop: None,
        })
        .unwrap();
    assert_eq!(heard, [1, 2, 3, 4]);
}

#[test]
fn only_the_newest_running_image_reaches_the_waiting_work_after_the_events() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let session = board.register(
        "token",
        registration("w", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let claimed = board.try_claim(&session).unwrap().unwrap();
    let image = |lease: &str, frames: u32| {
        board.preview(
            &session,
            &claimed.task,
            lease,
            TaskPreview {
                png: vec![frames as u8].into(),
                frames,
            },
        )
    };
    image(&claimed.lease, 2).unwrap();
    board
        .progress(
            &session,
            &claimed.task,
            &claimed.lease,
            Progress {
                fraction: 0.5,
                events: vec![serde_json::json!(1), serde_json::json!(2)],
                first_event: 0,
            },
        )
        .unwrap();
    // A newer image replaces the one not yet read.
    image(&claimed.lease, 3).unwrap();
    assert!(image("another lease", 4).is_err());
    let out = written(&board, &session, &claimed, "out.bin", b"done");
    board
        .complete(
            &session,
            &claimed.task,
            &claimed.lease,
            Completion {
                result: serde_json::json!({}),
                outputs: vec![out],
            },
        )
        .unwrap();
    let heard = std::cell::RefCell::new(Vec::new());
    ticket
        .wait_with(Waiting {
            stopped: &|| false,
            progress: &mut |_, _| {},
            events: &mut |event| heard.borrow_mut().push(format!("event {event}")),
            preview: &mut |image| {
                assert_eq!(image.png.as_ref(), [image.frames as u8]);
                heard
                    .borrow_mut()
                    .push(format!("image of {}", image.frames));
            },
            finish_on_stop: None,
        })
        .unwrap();
    assert_eq!(heard.into_inner(), ["event 1", "event 2", "image of 3"]);
}

#[test]
fn a_stop_that_lets_the_worker_finish_hands_back_what_it_made() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let session = board.register(
        "token",
        registration("w", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let claimed = board.try_claim(&session).unwrap().unwrap();
    let worker = {
        let (board, session, claimed) = (Arc::clone(&board), session.clone(), claimed.clone());
        std::thread::spawn(move || {
            // The worker is asked to finish, and completes with what it has.
            loop {
                let reply = board
                    .progress(&session, &claimed.task, &claimed.lease, at(0.3))
                    .unwrap();
                assert!(!reply.stop);
                if reply.finish {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let path = board
                .output_path(&session, &claimed.task, &claimed.lease, "out.bin")
                .unwrap();
            std::fs::write(&path, b"partial").unwrap();
            let (sha256, size) = crate::remote_work::file_digest(&path).unwrap();
            board
                .complete(
                    &session,
                    &claimed.task,
                    &claimed.lease,
                    Completion {
                        result: serde_json::json!({"built": false}),
                        outputs: vec![WrittenOutput {
                            name: "out.bin".into(),
                            size,
                            sha256,
                        }],
                    },
                )
                .unwrap();
        })
    };
    let finished = ticket
        .wait_with(Waiting {
            stopped: &|| true,
            progress: &mut |_, _| {},
            events: &mut |_| {},
            preview: &mut |_| {},
            finish_on_stop: Some(Duration::from_secs(10)),
        })
        .unwrap();
    worker.join().unwrap();
    assert_eq!(finished.result["built"], false);
    assert_eq!(
        std::fs::read(&finished.outputs["out.bin"]).unwrap(),
        b"partial"
    );
}

#[test]
fn only_a_worker_that_stacks_at_the_same_version_takes_a_channel() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    board.set_placement(TaskKind::StackChannel, WorkPlacement::WorkersFirst);
    let stacker = |version: &str| Offers {
        threads: 8,
        slots: 1,
        work: vec![TaskKind::StackChannel],
        rc_astro: Vec::new(),
        stacking_version: Some(version.into()),
        stacking_release: Some("0.12".into()),
        steps: Vec::new(),
    };
    let older = board.register(
        "token",
        registration("older", stacker("37"), FileAccess::default()),
        false,
    );
    let needs = TaskNeeds::Stack {
        stacking_version: "38".into(),
        release: "0.12".into(),
    };
    assert!(!board.remote_can_run(&needs));
    let current = board.register(
        "token",
        registration("current", stacker("38"), FileAccess::default()),
        false,
    );
    assert!(board.remote_can_run(&needs));
    let ticket = board.ticket().unwrap();
    let mut channel = task("channel", true, needs);
    channel.kind = TaskKind::StackChannel;
    ticket.submit(channel).unwrap();
    assert!(board.try_claim(&older).unwrap().is_none());
    assert_eq!(board.try_claim(&current).unwrap().unwrap().label, "channel");
}

#[test]
fn a_requeued_task_takes_the_next_worker_s_events_from_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let first = board.register(
        "token",
        registration("first", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let claimed = board.try_claim(&first).unwrap().unwrap();
    let event = |session: &str, claimed: &Assignment, number: u64| {
        board
            .progress(
                session,
                &claimed.task,
                &claimed.lease,
                Progress {
                    fraction: 0.1,
                    events: vec![serde_json::json!(number)],
                    first_event: number,
                },
            )
            .unwrap()
    };
    assert_eq!(event(&first, &claimed, 0).events_received, 1);
    assert_eq!(event(&first, &claimed, 1).events_received, 2);
    board
        .preview(
            &first,
            &claimed.task,
            &claimed.lease,
            TaskPreview {
                png: vec![1].into(),
                frames: 1,
            },
        )
        .unwrap();
    board.leave(&first);
    // The first worker's running image went with its lease.
    assert!(board
        .lock()
        .tasks
        .values()
        .all(|task| task.preview.is_none()));
    let second = board.register(
        "token",
        registration("second", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    let again = board.try_claim(&second).unwrap().unwrap();
    // The second worker's first event is taken, not skipped as a repeat.
    assert_eq!(event(&second, &again, 0).events_received, 1);
}

#[test]
fn a_stopped_task_goes_to_no_new_worker() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let ticket = board.ticket().unwrap();
    ticket.submit(task("t", true, needs("bxt", "1"))).unwrap();
    let first = board.register(
        "token",
        registration("first", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    board.try_claim(&first).unwrap().unwrap();
    let waiting = {
        std::thread::spawn(move || {
            ticket.wait_with(Waiting {
                stopped: &|| true,
                progress: &mut |_, _| {},
                events: &mut |_| {},
                preview: &mut |_| {},
                finish_on_stop: Some(Duration::from_millis(400)),
            })
        })
    };
    // The stop is in place; the worker then leaves without finishing.
    std::thread::sleep(Duration::from_millis(100));
    board.leave(&first);
    let second = board.register(
        "token",
        registration("second", offers(&[("bxt", "1")]), FileAccess::default()),
        false,
    );
    assert!(board.try_claim(&second).unwrap().is_none());
    assert_eq!(waiting.join().unwrap().unwrap_err(), WaitError::Stopped);
}

/// What a worker of this release registers: its providers as `steps`, and
/// its RC-Astro tools again for a server older than providers.
fn step_offers(schemas: Vec<seiza_stacking::ExternalToolSchema>) -> Offers {
    Offers {
        steps: schemas
            .into_iter()
            .filter_map(crate::remote_work::steps::StepOffer::of)
            .collect(),
        ..offers(&[])
    }
}

#[test]
fn a_noise_model_step_goes_only_to_a_worker_that_offers_the_same_model() {
    use crate::remote_work::noise_model::testing::identity;

    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let wanted = identity("noise-v1");
    // The step's schema is what a chain plans with; its needs come from it.
    let needs = TaskNeeds::step(&wanted.schema());
    let ticket = board.ticket().unwrap();
    ticket
        .submit(NewTask {
            kind: TaskKind::ProcessingStep,
            ..task("denoise", true, needs.clone())
        })
        .unwrap();

    // RC-Astro's own noise reduction, another model, and the same model on
    // another backend all leave the task where it is.
    let other_backend = crate::remote_work::noise_model::Identity {
        backend: "coreml-gpu".into(),
        ..wanted.clone()
    };
    for (name, offered) in [
        ("nxt-only", schema("nxt", "2.6.6", true)),
        ("other-model", identity("noise-v2").schema()),
        ("other-backend", other_backend.schema()),
    ] {
        let session = board.register(
            name,
            registration(name, step_offers(vec![offered]), FileAccess::default()),
            false,
        );
        assert!(board.try_claim(&session).unwrap().is_none(), "{name}");
        assert!(!board.remote_can_run(&needs), "{name}");
    }
    let session = board.register(
        "same",
        registration(
            "same",
            step_offers(vec![schema("nxt", "2.6.6", true), wanted.schema()]),
            FileAccess::default(),
        ),
        false,
    );
    assert!(board.remote_can_run(&needs));
    let claimed = board.try_claim(&session).unwrap().unwrap();
    assert_eq!(claimed.kind, TaskKind::ProcessingStep);
    assert_eq!(claimed.label, "denoise");
    // The worker's summary shows the model without its card's hash, beside
    // its RC-Astro tools.
    let (sessions, _) = board.summary();
    let summary = sessions.iter().find(|found| found.name == "same").unwrap();
    assert_eq!(
        summary.noise_model.as_deref(),
        Some("noise-v1 cpu f32, seiza-model 0.1.0, band scale 5, ONNX Runtime 1.31.1")
    );
    assert_eq!(summary.rc_astro, ["RC-Astro nxt 2.6.6"]);
}

#[test]
fn a_worker_older_than_providers_is_sent_an_rc_astro_step_under_its_old_kind() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    // A step of this release: one kind for every provider.
    let submit = |label: &str| {
        let ticket = board.ticket().unwrap();
        ticket
            .submit(NewTask {
                kind: TaskKind::ProcessingStep,
                spec: serde_json::json!({"category": "denoise", "provider": "nxt", "schema": {}}),
                ..task(label, true, TaskNeeds::step(&schema("nxt", "2.6.6", true)))
            })
            .unwrap();
        ticket
    };
    // The older worker registers RC-Astro tools and no providers. It never
    // heard of the new kind, so the step reaches it as `rc_astro_step`, with
    // the spec an RC-Astro task has always had beside the new fields.
    let _first = submit("first");
    let old = board.register(
        "old",
        registration("old", offers(&[("nxt", "2.6.6")]), FileAccess::default()),
        false,
    );
    let sent = board.try_claim(&old).unwrap().unwrap();
    assert_eq!(sent.kind, TaskKind::RcAstroStep);
    assert_eq!(
        serde_json::to_value(&sent).unwrap()["kind"],
        "rc_astro_step"
    );
    assert_eq!(sent.spec["provider"], "nxt");
    // It is never sent a provider it does not have.
    let model = crate::remote_work::noise_model::testing::identity("noise-v1");
    let denoise = board.ticket().unwrap();
    denoise
        .submit(NewTask {
            kind: TaskKind::ProcessingStep,
            ..task("model", true, TaskNeeds::step(&model.schema()))
        })
        .unwrap();
    board
        .fail(&old, &sent.task, &sent.lease, "done".into())
        .unwrap();
    assert!(board.try_claim(&old).unwrap().is_none());

    // A worker of this release gets the same step under the one kind.
    let _second = submit("second");
    let new = board.register(
        "new",
        registration(
            "new",
            step_offers(vec![schema("nxt", "2.6.6", true)]),
            FileAccess::default(),
        ),
        false,
    );
    let sent = board.try_claim(&new).unwrap().unwrap();
    assert_eq!(sent.kind, TaskKind::ProcessingStep);
}

#[test]
fn every_provider_runs_where_the_stored_rc_astro_choice_says() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    for placement in [
        WorkPlacement::WorkersOnly,
        WorkPlacement::ThisServer,
        WorkPlacement::WorkersFirst,
    ] {
        // The setting is stored and set under the RC-Astro kind, as before
        // providers.
        board.set_placement(TaskKind::RcAstroStep, placement);
        assert_eq!(board.placement(TaskKind::ProcessingStep), placement);
    }
}

#[test]
fn a_person_s_step_takes_this_server_s_worker_from_automatic_work() {
    let dir = tempfile::tempdir().unwrap();
    let board = board(dir.path(), Timings::default());
    let model = crate::remote_work::noise_model::testing::identity("noise-v1");
    let local = board.register(
        crate::server::workers::THIS_SERVER,
        registration(
            "This server",
            step_offers(vec![schema("nxt", "2.6.6", true), model.schema()]),
            FileAccess::default(),
        ),
        true,
    );
    let step = |label: &str, person: bool, schema: &seiza_stacking::ExternalToolSchema| {
        let ticket = board.ticket().unwrap();
        ticket
            .submit(NewTask {
                kind: TaskKind::ProcessingStep,
                ..task(label, person, TaskNeeds::step(schema))
            })
            .unwrap();
        ticket
    };
    // An automatic model run, which can take most of an hour, has the
    // worker.
    let _automatic = step("automatic model", false, &model.schema());
    let running = board.try_claim(&local).unwrap().unwrap();
    let stop = crate::concurrency::Stop::new();
    board
        .attach_stop(&local, &running.task, &running.lease, stop.clone())
        .unwrap();
    assert!(!stop.requested());
    // A person applies a step this server would run: the automatic run is
    // told to stop.
    let _person = step("person nxt", true, &schema("nxt", "2.6.6", true));
    assert!(stop.requested());
    // The worker answers that it stopped. The run goes back in the queue,
    // behind the person's step, and the stop does not count as a failure.
    board
        .fail(
            &local,
            &running.task,
            &running.lease,
            "The noise model was stopped".into(),
        )
        .unwrap();
    let next = board.try_claim(&local).unwrap().unwrap();
    assert_eq!(next.label, "person nxt");
    board
        .fail(&local, &next.task, &next.lease, "done".into())
        .unwrap();
    for _ in 0..MAX_ATTEMPTS + 1 {
        let again = board.try_claim(&local).unwrap().unwrap();
        assert_eq!(again.label, "automatic model");
        let stop = crate::concurrency::Stop::new();
        board
            .attach_stop(&local, &again.task, &again.lease, stop.clone())
            .unwrap();
        let _another = step("person again", true, &schema("nxt", "2.6.6", true));
        assert!(stop.requested());
        board
            .fail(&local, &again.task, &again.lease, "stopped".into())
            .unwrap();
        let person = board.try_claim(&local).unwrap().unwrap();
        assert_eq!(person.label, "person again");
        board
            .fail(&local, &person.task, &person.lease, "done".into())
            .unwrap();
    }
    // Another person's work on a step this server does not run leaves the
    // automatic run alone.
    let again = board.try_claim(&local).unwrap().unwrap();
    let stop = crate::concurrency::Stop::new();
    board
        .attach_stop(&local, &again.task, &again.lease, stop.clone())
        .unwrap();
    let _elsewhere = step("person bxt", true, &schema("bxt", "2.6.6", true));
    assert!(!stop.requested());
}
