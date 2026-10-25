use super::*;
use crate::remote_work::protocol::{FileAccess, PathMap, WrittenOutput, PROTOCOL_VERSION};

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
        work: vec![TaskKind::RcAstroStep],
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
    TaskNeeds::RcAstro {
        tool: tool.into(),
        name: format!("RC-Astro {tool}"),
        cli_version: version.into(),
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

fn board(dir: &Path, timings: Timings) -> Arc<Board> {
    Arc::new(Board::with_timings(dir.to_path_buf(), timings))
}

fn quick() -> Timings {
    Timings {
        lease: Duration::from_millis(200),
        no_worker_grace: Duration::from_millis(300),
    }
}

/// Write `bytes` as the output `name` of the claimed task and say so.
fn written(ticket: &Ticket, name: &str, bytes: &[u8]) -> WrittenOutput {
    std::fs::write(ticket.dir().join(name), bytes).unwrap();
    let (sha256, size) = crate::remote_work::file_digest(&ticket.dir().join(name)).unwrap();
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
    // A claim while holding a task gives that task up.
    let second = board.try_claim(&session).unwrap().unwrap();
    assert_eq!(second.label, "person");
    board
        .fail(&session, &second.task, &second.lease, "no".into())
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
    assert!(!board
        .progress(&session, &claimed.task, &claimed.lease, 0.5)
        .unwrap());
    let out = written(&ticket, "out.bin", b"pixels");
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
    let mut out = written(&ticket, "out.bin", b"pixels");
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
    let extra = written(&ticket, "extra.bin", b"stars");
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
    // The slow worker's lease ran out: it hears the server no longer wants
    // the task, and its result is refused.
    assert!(board
        .progress(&slow, &first.task, &first.lease, 0.9)
        .unwrap());
    let out = written(&ticket, "out.bin", b"late");
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
fn a_stop_takes_the_task_off_the_board_and_the_worker_hears_it() {
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
    assert!(board
        .progress(&session, &claimed.task, &claimed.lease, 0.5)
        .unwrap());
    assert!(!board.still_wanted(&session, &claimed.task, &claimed.lease));
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
    assert_eq!(
        board.progress(&revoked, &claimed.task, &claimed.lease, 0.1),
        Err(Refusal::UnknownSession)
    );
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
        Some(ticket.dir().join("out.bin").display().to_string().as_str())
    );
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
        .rc_astro_offers()
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
    assert_eq!(board.try_claim(&local).unwrap().unwrap().label, "noise");

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
