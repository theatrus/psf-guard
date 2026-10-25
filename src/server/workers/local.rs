//! This server's own worker. It registers on the board like any other and
//! claims tasks through it, but reads and writes task files in place. It
//! runs its tasks one at a time, on a thread of its own, while the work
//! that submitted each task waits on its ticket.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::board::{Board, Refusal, POLL};
use crate::remote_work::protocol::{
    Assignment, Completion, FileAccess, FileMode, Offers, Registration, WrittenOutput,
    PROTOCOL_VERSION,
};

pub(super) struct LocalWorker {
    session: String,
}

impl LocalWorker {
    /// Register this server's worker and start the thread that claims its
    /// tasks. The thread ends when the board closes.
    pub(super) fn start(board: Arc<Board>, offers: Offers) -> Self {
        let session = board.register(
            super::THIS_SERVER,
            Registration {
                protocol: PROTOCOL_VERSION,
                name: "This server".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                offers,
                files: FileAccess {
                    mode: FileMode::Shared,
                    maps: Vec::new(),
                },
            },
            true,
        );
        let thread_session = session.clone();
        let started = std::thread::Builder::new()
            .name("worker-this-server".into())
            .spawn(move || claim_until_closed(&board, &thread_session));
        if let Err(error) = started {
            tracing::error!("This server's worker could not start: {error}");
        }
        Self { session }
    }

    pub(super) fn session(&self) -> &str {
        &self.session
    }
}

fn claim_until_closed(board: &Arc<Board>, session: &str) {
    while !board.is_closed() {
        match board.claim_blocking(session, POLL) {
            Ok(Some(assignment)) => {
                let task = assignment.task.clone();
                run(board, session, assignment);
                board.local_done(&task);
            }
            Ok(None) => {}
            Err(refusal) => {
                tracing::error!("This server's worker stopped: {refusal}");
                return;
            }
        }
    }
}

/// Run one task where its files are, and answer for it on the board.
fn run(board: &Arc<Board>, session: &str, assignment: Assignment) {
    let Assignment {
        task,
        lease,
        kind,
        spec,
        inputs,
        outputs,
        ..
    } = assignment;
    let input_paths = inputs
        .iter()
        .filter_map(|input| Some((input.name.clone(), PathBuf::from(input.path.as_ref()?))))
        .collect::<BTreeMap<_, _>>();
    let Some(output_dir) = outputs.iter().find_map(|output| {
        PathBuf::from(output.path.as_ref()?)
            .parent()
            .map(PathBuf::from)
    }) else {
        let _ = board.fail(
            session,
            &task,
            &lease,
            "The task has no output folder".into(),
        );
        return;
    };
    let cancel = {
        let (board, session, task, lease) = (
            Arc::clone(board),
            session.to_string(),
            task.clone(),
            lease.clone(),
        );
        seiza_stacking::CancelSignal::new(move || !board.still_wanted(&session, &task, &lease))
    };
    let tools = super::local_tools();
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::remote_work::execute(
            kind,
            spec,
            &input_paths,
            &output_dir,
            &tools,
            &cancel,
            &mut |fraction| {
                let _ = board.progress(session, &task, &lease, fraction);
            },
        )
    }))
    .unwrap_or_else(|_| Err("The task panicked on this server".into()));
    let answered = match ran {
        Ok(result) => {
            let mut written = Vec::new();
            for output in &outputs {
                let path = output_dir.join(&output.name);
                if !path.exists() {
                    continue;
                }
                match crate::remote_work::file_digest(&path) {
                    Ok((sha256, size)) => written.push(WrittenOutput {
                        name: output.name.clone(),
                        size,
                        sha256,
                    }),
                    Err(error) => {
                        let _ = board.fail(
                            session,
                            &task,
                            &lease,
                            format!("Reading {}: {error}", path.display()),
                        );
                        return;
                    }
                }
            }
            board.complete(
                session,
                &task,
                &lease,
                Completion {
                    result,
                    outputs: written,
                },
            )
        }
        Err(error) => board.fail(session, &task, &lease, error),
    };
    match answered {
        // The work that waited for it stopped.
        Ok(()) | Err(Refusal::NotCurrent) => {}
        Err(refusal) => tracing::warn!(task, "This server's worker: {refusal}"),
    }
}
