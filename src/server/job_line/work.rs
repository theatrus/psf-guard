//! What runs on a worker, apart from what the coordinator does around it.
//!
//! Each kind of work is split where it could leave this server. The
//! coordinator prepares a [`LineWork::Spec`]: everything the work needs,
//! self-contained and serializable. A worker runs [`LineWork::execute`] on
//! that spec and hands back an output. The coordinator then publishes the
//! output: it writes the indexes and the catalog, and starts what follows.
//!
//! Today every worker is this server, and some kinds still read this
//! server's catalog while they run; their [`LineWork::NEEDS`] is then
//! [`Needs::ThisServer`]. See
//! `docs/design/job-line.md`.

use crate::concurrency::Stop;
use std::sync::Arc;

use std::marker::PhantomData;

use serde::{de::DeserializeOwned, Serialize};

use crate::concurrency::Priority;
use crate::server::state::AppState;

/// The worker a kind of work can run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    /// Any worker: the spec lists every input.
    AnyWorker,
    /// This server: the work still reads its catalog or its caches while it
    /// runs.
    ThisServer,
}

/// What a worker offers the work it runs.
pub struct Worker<'a> {
    stop: &'a Stop,
    priority: Priority,
    server: Option<&'a Arc<AppState>>,
}

impl<'a> Worker<'a> {
    /// This server's own worker.
    pub fn this_server(server: &'a Arc<AppState>, stop: &'a Stop, priority: Priority) -> Self {
        Self {
            stop,
            priority,
            server: Some(server),
        }
    }

    pub fn stop_flag(&self) -> &'a Stop {
        self.stop
    }

    /// The tier the work runs at: a person's work, or automatic work.
    pub fn priority(&self) -> Priority {
        self.priority
    }

    /// The server the work runs on, for work whose [`LineWork::NEEDS`] is
    /// [`Needs::ThisServer`].
    ///
    /// # Panics
    ///
    /// On a worker that is not this server. Only work whose
    /// [`LineWork::NEEDS`] is [`Needs::ThisServer`] calls it.
    pub fn server(&self) -> &'a Arc<AppState> {
        self.server
            .expect("work that reads this server's state runs on this server")
    }
}

/// One kind of work on the line.
pub trait LineWork {
    /// Everything the work needs, prepared by the coordinator.
    type Spec: Serialize + DeserializeOwned + Send + 'static;
    /// What the work made, for the coordinator to publish.
    type Output: Send + 'static;
    /// The worker it can run on.
    const NEEDS: Needs;

    /// Run the work from its spec, on the worker, with its lane's lease
    /// held. Blocking.
    fn execute(spec: Self::Spec, worker: &Worker<'_>) -> Self::Output;
}

/// Work in the line, as its dispatcher sees it: something to run, or to
/// settle unrun once it was stopped while it waited.
pub trait Task: Send + 'static {
    /// What a person is waiting on while it runs, as "a stack build".
    fn what(&self) -> &'static str;
    /// Run the work on `worker`, then publish what it made on this server.
    fn run(self: Box<Self>, state: &Arc<AppState>, worker: &Worker<'_>);
    /// It was stopped before it ran.
    fn stopped(self: Box<Self>, state: &Arc<AppState>);
    /// The work as a future, for work that spends its time waiting on
    /// async I/O, such as a WBPP run that watches PixInsight for hours.
    /// When this returns a future, the dispatcher awaits it on the runtime
    /// with the lane's lease held, and does not call [`Task::run`].
    fn take_future(&mut self, _state: &Arc<AppState>) -> Option<TaskFuture> {
        None
    }
}

/// Work a task runs on the async runtime; see [`Task::take_future`].
pub type TaskFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

type Publish<W> = Box<dyn FnOnce(&Arc<AppState>, Result<<W as LineWork>::Output, String>) + Send>;
type Settle = Box<dyn FnOnce(&Arc<AppState>) + Send>;
type Starts = Box<dyn FnOnce(&Arc<AppState>) -> bool + Send>;
type Finally = Box<dyn FnOnce(&Arc<AppState>, Ending) + Send>;

/// How an entry's time in the line ended, for its last step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// It ran, and what it made was published.
    Published,
    /// It was stopped before it ran, and settled.
    Stopped,
    /// Publishing or settling it panicked: whatever that step had left to
    /// record was not recorded.
    Panicked(String),
}

/// One piece of [`LineWork`] in the line: its spec, serialized as it joins,
/// and what the coordinator does once it has run or was stopped first.
/// Running it reads the spec back, as a worker on another machine would.
pub struct Submission<W: LineWork> {
    what: &'static str,
    spec: serde_json::Value,
    starts: Option<Starts>,
    publish: Publish<W>,
    stopped: Settle,
    finally: Option<Finally>,
    kind: PhantomData<fn() -> W>,
}

impl<W: LineWork> Submission<W> {
    /// `spec`, serialized now. `publish` takes what the work made, or why
    /// it could not run; `stopped` settles it when it is stopped before it
    /// runs.
    pub fn new(
        what: &'static str,
        spec: &W::Spec,
        publish: impl FnOnce(&Arc<AppState>, Result<W::Output, String>) + Send + 'static,
        stopped: impl FnOnce(&Arc<AppState>) + Send + 'static,
    ) -> Result<Self, String> {
        let spec = serde_json::to_value(spec)
            .map_err(|error| format!("The work could not be written down to queue: {error}"))?;
        Ok(Self {
            what,
            spec,
            starts: None,
            publish: Box::new(publish),
            stopped: Box::new(stopped),
            finally: None,
            kind: PhantomData,
        })
    }

    /// Call `starts` on this server when the work's turn comes. When it
    /// returns false, the work is settled as stopped instead of run.
    pub fn starting_with(
        mut self,
        starts: impl FnOnce(&Arc<AppState>) -> bool + Send + 'static,
    ) -> Self {
        self.starts = Some(Box::new(starts));
        self
    }

    /// The last step, however the entry ends, even when publishing or
    /// settling it panicked: what must not be left behind, such as its stop
    /// flag and the server stop's wait.
    pub fn finally(
        mut self,
        finally: impl FnOnce(&Arc<AppState>, Ending) + Send + 'static,
    ) -> Self {
        self.finally = Some(Box::new(finally));
        self
    }
}

/// Run a step that may panic, and return how it ended.
fn guarded(step: impl FnOnce(), on_success: Ending) -> Ending {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(step)) {
        Ok(()) => on_success,
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|message| message.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "no message".into());
            tracing::error!("A queued job's last steps panicked: {message}");
            Ending::Panicked(message)
        }
    }
}

impl<W: LineWork + 'static> Task for Submission<W> {
    fn what(&self) -> &'static str {
        self.what
    }

    fn run(self: Box<Self>, state: &Arc<AppState>, worker: &Worker<'_>) {
        let Self {
            spec,
            starts,
            publish,
            stopped,
            finally,
            ..
        } = *self;
        let starts = starts.is_none_or(|starts| starts(state));
        let ending = match starts {
            false => guarded(|| stopped(state), Ending::Stopped),
            true => {
                let output = serde_json::from_value::<W::Spec>(spec)
                    .map_err(|error| format!("The queued work could not be read back: {error}"))
                    .and_then(|spec| {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            W::execute(spec, worker)
                        }))
                        .map_err(|_| "The worker stopped unexpectedly".to_string())
                    });
                guarded(|| publish(state, output), Ending::Published)
            }
        };
        if let Some(finally) = finally {
            finally(state, ending);
        }
    }

    fn stopped(self: Box<Self>, state: &Arc<AppState>) {
        let Self {
            stopped, finally, ..
        } = *self;
        let ending = guarded(|| stopped(state), Ending::Stopped);
        if let Some(finally) = finally {
            finally(state, ending);
        }
    }
}
