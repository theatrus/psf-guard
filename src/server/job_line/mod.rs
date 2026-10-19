//! The job line: work waiting its turn, and the dispatch of it.
//!
//! Work that shares a worker waits in one ordered line. The line holds each
//! entry whole: its spec, serialized when it joined, and what the
//! coordinator does with its result (see [`Submission`]). A dispatcher per
//! lane takes the first entry that may run with [`JobLine::next`], and gets
//! back a [`Lease`] on the lane with it. The lane is free again, and the next
//! entry dispatched, when the lease is dropped. The choice and the lease are
//! taken under one lock, so a change to the line and a lane coming free can
//! never both let an entry through, nor both hold it back.
//!
//! Three kinds of waiting are one rule each:
//!
//! - A request still being prepared holds a place before it has any work
//!   ([`JobLine::hold_place`]). The place runs nothing and holds up nothing
//!   behind it; the work it becomes takes it.
//! - Work about to join can have the next turn kept for it
//!   ([`JobLine::reserve_turn`]). A reserved turn holds up the worker lane,
//!   but only for as long as it was given, counted from when it first holds
//!   anything up; the work that fills it takes its place.
//! - Places and running work can hold keys, and other work can wait until
//!   nothing holds the keys it names ([`JobLine::wait_clear`]): a color
//!   composition waits so for the builds of its target's channels.
//! - An entry can be blocked by keys ([`Entry::blocked_by`]). While running
//!   work, or a hold from outside the line ([`JobLine::hold_key`]), has one
//!   of those keys, the entry keeps its place and the entries behind it
//!   run first. A queued WBPP run is blocked so while its database copies
//!   masters.
//!
//! An entry stopped while it waits leaves at the next dispatch, with its
//! [`Task::stopped`] run, whatever else is running. The line knows nothing
//! of what its entries do. See `docs/design/job-line.md`.

use crate::concurrency::Stop;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

pub mod catalog;
mod dispatch;
mod work;
pub use dispatch::{DispatchRules, Turn};
pub use work::{Ending, LineWork, Needs, Submission, Task, Worker};

/// Where an entry runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    /// The shared worker, one entry at a time in the line's order.
    Worker,
    /// Beside the worker, one entry at a time in the order they joined:
    /// work short enough not to wait behind the line.
    Quick,
}

/// Work in the line.
pub struct Entry {
    id: String,
    lane: Lane,
    stop: Stop,
    task: Box<dyn Task>,
    holds: Vec<String>,
    blocked_by: Vec<String>,
}

impl Entry {
    /// `task` to run on `lane`, stopped by `stop`, followed by `id`.
    pub fn new(id: impl Into<String>, lane: Lane, stop: Stop, task: Box<dyn Task>) -> Self {
        Self {
            id: id.into(),
            lane,
            stop,
            task,
            holds: Vec::new(),
            blocked_by: Vec::new(),
        }
    }

    /// The keys it holds while it waits and while it runs.
    pub fn holding(mut self, keys: Vec<String>) -> Self {
        self.holds = keys;
        self
    }

    /// Block the entry while running work or a hold from outside the line
    /// has one of `keys`. A blocked entry keeps its place, and the entries
    /// behind it run first. Applies to the worker lane only.
    pub fn blocked_by(mut self, keys: Vec<String>) -> Self {
        self.blocked_by = keys;
        self
    }
}

/// A place in the worker lane's line.
enum Place {
    /// For work still being prepared, holding `holds` meanwhile.
    Held {
        id: String,
        holds: Vec<String>,
    },
    /// The next turn, kept for work about to join: for `hold_for` from when
    /// it first holds anything up.
    Reserved {
        id: String,
        hold_for: Duration,
        since: Option<Instant>,
    },
    Work(Entry),
}

impl Place {
    fn id(&self) -> &str {
        match self {
            Self::Held { id, .. } | Self::Reserved { id, .. } => id,
            Self::Work(entry) => &entry.id,
        }
    }

    fn holds(&self) -> &[String] {
        match self {
            Self::Held { holds, .. } => holds,
            Self::Reserved { .. } => &[],
            Self::Work(entry) => &entry.holds,
        }
    }

    fn is_work(&self) -> bool {
        matches!(self, Self::Work(_))
    }
}

#[derive(Default)]
struct LineState {
    /// The worker lane's line, in the order it runs. A person can change it.
    order: Vec<Place>,
    /// The quick lane's line, in the order entries joined.
    quick: VecDeque<Entry>,
    worker_busy: bool,
    quick_busy: bool,
    /// The keys the work running on each lane holds.
    worker_holds: Vec<String>,
    quick_holds: Vec<String>,
    /// Work waiting to reserve the worker outside the line. It goes before
    /// the line's next entry, as a reservation always has.
    reservations: usize,
    /// Keys held from outside the line, with the number of holds on each.
    outside: HashMap<String, usize>,
}

impl LineState {
    fn busy(&mut self, lane: Lane) -> &mut bool {
        match lane {
            Lane::Worker => &mut self.worker_busy,
            Lane::Quick => &mut self.quick_busy,
        }
    }

    fn running_holds(&mut self, lane: Lane) -> &mut Vec<String> {
        match lane {
            Lane::Worker => &mut self.worker_holds,
            Lane::Quick => &mut self.quick_holds,
        }
    }

    /// Whether any place, queued entry or running work holds one of `keys`.
    fn holds_any(&self, keys: &[String]) -> bool {
        let held = |holds: &[String]| holds.iter().any(|key| keys.contains(key));
        self.order.iter().any(|place| held(place.holds()))
            || self.quick.iter().any(|entry| held(&entry.holds))
            || self.is_blocked(keys)
    }

    /// Whether running work or a hold from outside the line has one of
    /// `keys`, which blocks an entry [`Entry::blocked_by`] them.
    fn is_blocked(&self, keys: &[String]) -> bool {
        let held = |holds: &[String]| holds.iter().any(|key| keys.contains(key));
        held(&self.worker_holds)
            || held(&self.quick_holds)
            || keys.iter().any(|key| self.outside.contains_key(key))
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.order.iter().position(|place| place.id() == id)
    }

    /// Take out the first entry of `lane` that was asked to stop.
    fn take_stopped(&mut self, lane: Lane) -> Option<Entry> {
        match lane {
            Lane::Worker => {
                let index = self.order.iter().position(
                    |place| matches!(place, Place::Work(entry) if entry.stop.requested()),
                )?;
                match self.order.remove(index) {
                    Place::Work(entry) => Some(entry),
                    _ => unreachable!("only work is taken"),
                }
            }
            Lane::Quick => {
                let index = self.quick.iter().position(|entry| entry.stop.requested())?;
                self.quick.remove(index)
            }
        }
    }

    /// Take out the entry that runs next on `lane`, or say until when a
    /// reserved turn holds the lane. Held places are passed over; a reserved
    /// turn that ran out leaves.
    fn take_next(&mut self, lane: Lane) -> Result<Entry, Option<Instant>> {
        if lane == Lane::Quick {
            return self.quick.pop_front().ok_or(None);
        }
        let now = Instant::now();
        let mut index = 0;
        while index < self.order.len() {
            if let Place::Work(entry) = &self.order[index]
                && self.is_blocked(&entry.blocked_by)
            {
                index += 1;
                continue;
            }
            match &mut self.order[index] {
                Place::Held { .. } => index += 1,
                Place::Reserved {
                    id,
                    hold_for,
                    since,
                } => {
                    let until = *since.get_or_insert(now) + *hold_for;
                    if now < until {
                        return Err(Some(until));
                    }
                    tracing::warn!(
                        reserved = id.as_str(),
                        "The next turn on the worker was kept for work that did not join \
                         in time; the line moves on"
                    );
                    self.order.remove(index);
                }
                Place::Work(_) => match self.order.remove(index) {
                    Place::Work(entry) => return Ok(entry),
                    _ => unreachable!("only work is taken"),
                },
            }
        }
        Err(None)
    }
}

/// One line of work and its lanes.
#[derive(Default)]
pub struct JobLine {
    state: Arc<Mutex<LineState>>,
    /// Wakes the dispatchers when the line or a lane changes. Kept inside:
    /// callers say what changed, not how to wake anyone.
    wakeup: Arc<Notify>,
    /// The dispatcher task of each lane, started when work first joins.
    dispatchers: Mutex<HashMap<Lane, tokio::task::JoinHandle<()>>>,
}

/// A key held from outside the line, until it is dropped.
#[must_use = "the key is let go as soon as the hold is dropped"]
pub struct KeyHold {
    state: Arc<Mutex<LineState>>,
    wakeup: Arc<Notify>,
    key: String,
}

impl Drop for KeyHold {
    fn drop(&mut self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = state.outside.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                state.outside.remove(&self.key);
            }
        }
        drop(state);
        self.wakeup.notify_waiters();
    }
}

/// A turn on a lane. The lane is free again, and the next entry dispatched,
/// when it is dropped.
#[must_use = "the lane is free again as soon as the lease is dropped"]
pub struct Lease {
    state: Arc<Mutex<LineState>>,
    wakeup: Arc<Notify>,
    lane: Lane,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state.busy(self.lane) = false;
        state.running_holds(self.lane).clear();
        drop(state);
        self.wakeup.notify_waiters();
    }
}

/// The answer to whoever waits for a piece of work: given once, by
/// whichever way the work ends first. It can be staged first and delivered
/// once the work's last steps are done.
pub struct Reply<T>(Arc<Mutex<ReplyState<T>>>);

struct ReplyState<T> {
    sender: Option<tokio::sync::oneshot::Sender<T>>,
    staged: Option<T>,
}

impl<T> Clone for Reply<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Reply<T> {
    /// A reply, and where it arrives. A reply never given arrives as the
    /// receiver's error once every copy is gone.
    pub fn channel() -> (Self, tokio::sync::oneshot::Receiver<T>) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let state = ReplyState {
            sender: Some(sender),
            staged: None,
        };
        (Self(Arc::new(Mutex::new(state))), receiver)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ReplyState<T>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Answer now, unless an answer was already given or nobody waits.
    pub fn send(&self, value: T) {
        if let Some(sender) = self.lock().sender.take() {
            let _ = sender.send(value);
        }
    }

    /// Keep the answer until [`Self::deliver`].
    pub fn stage(&self, value: T) {
        self.lock().staged = Some(value);
    }

    /// Give the staged answer, if there is one.
    pub fn deliver(&self) {
        let staged = self.lock().staged.take();
        if let Some(value) = staged {
            self.send(value);
        }
    }
}

/// A reservation waiting for the worker lane, counted until it is granted
/// or its caller gives up.
struct Reserving<'a>(&'a JobLine);

impl<'a> Reserving<'a> {
    fn new(line: &'a JobLine) -> Self {
        line.lock().reservations += 1;
        Self(line)
    }
}

impl Drop for Reserving<'_> {
    fn drop(&mut self) {
        self.0.lock().reservations -= 1;
        self.0.wake();
    }
}

/// What a dispatcher does next.
pub enum Dispatch {
    /// Run this entry's work, holding the lease while it runs.
    Run {
        id: String,
        stop: Stop,
        task: Box<dyn Task>,
        lease: Lease,
    },
    /// The entry was stopped while it waited: settle it without running it.
    Stopped { id: String, task: Box<dyn Task> },
}

impl JobLine {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LineState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lease(&self, lane: Lane) -> Lease {
        Lease {
            state: Arc::clone(&self.state),
            wakeup: Arc::clone(&self.wakeup),
            lane,
        }
    }

    fn wake(&self) {
        self.wakeup.notify_waiters();
    }

    /// Something the line's choice depends on changed outside it, such as an
    /// entry asked to stop: look again.
    pub fn changed(&self) {
        self.wake();
    }

    /// Hold a place in the worker lane's line for work still being
    /// prepared, at the back, holding `holds` meanwhile. The place runs
    /// nothing and holds up nothing behind it; the work it becomes takes it.
    pub fn hold_place(&self, id: &str, holds: Vec<String>) {
        {
            let mut state = self.lock();
            if state.index_of(id).is_none() {
                state.order.push(Place::Held {
                    id: id.to_string(),
                    holds,
                });
            }
        }
        self.wake();
    }

    /// Keep the next turn on the worker lane for work about to join under
    /// another id: at the front, or just after `after` when it is in line.
    /// It holds the lane for `hold_for` from when it first holds anything
    /// up, and then the line moves on without it.
    pub fn reserve_turn(&self, id: &str, hold_for: Duration, after: Option<&str>) {
        {
            let mut state = self.lock();
            if state.index_of(id).is_none() {
                let index = after
                    .and_then(|after| state.index_of(after))
                    .map_or(0, |index| index + 1);
                state.order.insert(
                    index,
                    Place::Reserved {
                        id: id.to_string(),
                        hold_for,
                        since: None,
                    },
                );
            }
        }
        self.wake();
    }

    /// Put the work in line under `id` in the turn `reserved` kept for it, or
    /// at the front when that reservation is gone, and give up the
    /// reservation.
    pub fn fill_reservation(&self, reserved: &str, id: &str) {
        {
            let mut state = self.lock();
            if let Some(index) = state.index_of(id) {
                let place = state.order.remove(index);
                let target = state.index_of(reserved).unwrap_or(0);
                state.order.insert(target, place);
                state
                    .order
                    .retain(|place| !matches!(place, Place::Reserved { id, .. } if id == reserved));
            }
        }
        self.wake();
    }

    /// Give up a held place or a reserved turn. Work that took it is left
    /// alone.
    pub fn release_place(&self, id: &str) {
        {
            let mut state = self.lock();
            state
                .order
                .retain(|place| place.is_work() || place.id() != id);
        }
        self.wake();
    }

    /// Whether any place, waiting work or running work holds one of `keys`.
    pub fn holds_any(&self, keys: &[String]) -> bool {
        self.lock().holds_any(keys)
    }

    /// Hold `key` from outside the line until the returned hold is dropped.
    /// Entries [`Entry::blocked_by`] the key stay in line meanwhile.
    pub fn hold_key(&self, key: impl Into<String>) -> KeyHold {
        let key = key.into();
        *self.lock().outside.entry(key.clone()).or_default() += 1;
        KeyHold {
            state: Arc::clone(&self.state),
            wakeup: Arc::clone(&self.wakeup),
            key,
        }
    }

    /// Reserve the worker lane when it is free and no work is queued on it,
    /// so the caller can start work without going ahead of queued work.
    pub fn try_reserve_idle_worker(&self) -> Option<Lease> {
        let mut state = self.lock();
        if state.worker_busy || state.reservations > 0 || state.order.iter().any(Place::is_work) {
            return None;
        }
        state.worker_busy = true;
        Some(self.lease(Lane::Worker))
    }

    /// Wait until nothing in the line, waiting or running, holds any of
    /// `keys`.
    pub async fn wait_clear(&self, keys: &[String]) {
        loop {
            let notified = self.wakeup.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.holds_any(keys) {
                return;
            }
            notified.await;
        }
    }

    /// Put work in its lane's line. On the worker lane it takes the place
    /// held under its own id, or goes just before `before`, a place held
    /// for the request it came from, or at the back. Work already in line
    /// under the same id stays as it is, and this entry is handed back.
    pub fn submit(&self, entry: Entry, before: Option<&str>) -> Result<(), Entry> {
        {
            let mut state = self.lock();
            match entry.lane {
                Lane::Worker => match state.index_of(&entry.id) {
                    Some(index) => match &state.order[index] {
                        Place::Work(_) => return Err(entry),
                        _ => state.order[index] = Place::Work(entry),
                    },
                    None => {
                        let index = before
                            .and_then(|before| state.index_of(before))
                            .unwrap_or(state.order.len());
                        state.order.insert(index, Place::Work(entry));
                    }
                },
                Lane::Quick => {
                    if state.quick.iter().any(|queued| queued.id == entry.id) {
                        return Err(entry);
                    }
                    state.quick.push_back(entry);
                }
            }
        }
        self.wake();
        Ok(())
    }

    /// Move a place in the worker lane's line to `position` (0 is next),
    /// clamped to the line. False when it is not in that line.
    pub fn move_to(&self, id: &str, position: usize) -> bool {
        {
            let mut state = self.lock();
            let Some(index) = state.index_of(id) else {
                return false;
            };
            let place = state.order.remove(index);
            let position = position.min(state.order.len());
            state.order.insert(position, place);
        }
        self.wake();
        true
    }

    /// The stop of the work in line under `id`.
    pub fn stop_of(&self, id: &str) -> Option<Stop> {
        let state = self.lock();
        state
            .order
            .iter()
            .find_map(|place| match place {
                Place::Work(entry) if entry.id == id => Some(entry.stop.clone()),
                _ => None,
            })
            .or_else(|| {
                state
                    .quick
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| entry.stop.clone())
            })
    }

    /// A place's position in the worker lane's line, 0 next.
    pub fn position(&self, id: &str) -> Option<usize> {
        self.lock().index_of(id)
    }

    /// The worker lane's line, in order: work, held places and reserved
    /// turns.
    pub fn order(&self) -> Vec<String> {
        self.lock()
            .order
            .iter()
            .map(|place| place.id().to_string())
            .collect()
    }

    /// Whether something holds the worker lane now.
    pub fn worker_busy(&self) -> bool {
        self.lock().worker_busy
    }

    /// The worker lane, when nothing holds it: work that must not run beside
    /// any entry, such as removing files a build may read. It does not wait
    /// its turn in the line.
    pub fn try_reserve_worker(&self) -> Option<Lease> {
        let mut state = self.lock();
        if state.worker_busy {
            return None;
        }
        state.worker_busy = true;
        Some(self.lease(Lane::Worker))
    }

    /// The quick lane, when nothing holds it, for tests that need it busy.
    #[cfg(test)]
    pub(crate) fn try_reserve_quick(&self) -> Option<Lease> {
        let mut state = self.lock();
        if state.quick_busy {
            return None;
        }
        state.quick_busy = true;
        Some(self.lease(Lane::Quick))
    }

    /// [`Self::try_reserve_worker`], waiting for the worker lane to come
    /// free.
    pub async fn reserve_worker(&self) -> Lease {
        // Counted while it waits, so the dispatcher leaves the lane to it.
        let _waiting = Reserving::new(self);
        loop {
            let notified = self.wakeup.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(lease) = self.try_reserve_worker() {
                return lease;
            }
            notified.await;
        }
    }

    /// What `lane`'s dispatcher does next, now: settle an entry stopped
    /// while it waited, or run the next entry while the lane is free.
    fn try_next(&self, lane: Lane) -> Result<Dispatch, Option<Instant>> {
        let mut state = self.lock();
        if let Some(entry) = state.take_stopped(lane) {
            return Ok(Dispatch::Stopped {
                id: entry.id,
                task: entry.task,
            });
        }
        if *state.busy(lane) || (lane == Lane::Worker && state.reservations > 0) {
            return Err(None);
        }
        let entry = state.take_next(lane)?;
        *state.busy(lane) = true;
        *state.running_holds(lane) = entry.holds.clone();
        drop(state);
        Ok(Dispatch::Run {
            id: entry.id,
            stop: entry.stop,
            task: entry.task,
            lease: self.lease(lane),
        })
    }

    /// Wait for what `lane`'s dispatcher does next. A reserved turn that
    /// holds the lane is looked at again when it runs out.
    pub async fn next(&self, lane: Lane) -> Dispatch {
        loop {
            let notified = self.wakeup.notified();
            tokio::pin!(notified);
            // Registered before the line is read, so a change between the
            // read and the wait still wakes the dispatcher.
            notified.as_mut().enable();
            match self.try_next(lane) {
                Ok(dispatch) => return dispatch,
                Err(Some(until)) => {
                    tokio::select! {
                        _ = &mut notified => {}
                        _ = tokio::time::sleep_until(until.into()) => {}
                    }
                }
                Err(None) => notified.await,
            }
        }
    }
}

impl JobLine {
    /// Take a place out of the line, whatever is in it, for tests that stand
    /// in for the work's own end.
    #[cfg(test)]
    pub(crate) fn remove(&self, id: &str) {
        {
            let mut state = self.lock();
            state.order.retain(|place| place.id() != id);
            state.quick.retain(|entry| entry.id != id);
        }
        self.wake();
    }
}

/// Entries for tests of the line's order, which run a closure or nothing.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    struct Run(Option<Box<dyn FnOnce() + Send>>);

    impl Task for Run {
        fn what(&self) -> &'static str {
            "a test entry"
        }
        fn run(
            self: Box<Self>,
            _state: &Arc<crate::server::state::AppState>,
            _worker: &Worker<'_>,
        ) {
            if let Some(run) = self.0 {
                run();
            }
        }
        fn stopped(self: Box<Self>, _state: &Arc<crate::server::state::AppState>) {}
    }

    /// An entry that does nothing when it runs.
    pub(crate) fn entry(id: &str, lane: Lane, stop: Stop) -> Entry {
        Entry::new(id, lane, stop, Box::new(Run(None)))
    }

    /// An entry that runs `run` on the worker.
    pub(crate) fn entry_running(
        id: &str,
        lane: Lane,
        stop: Stop,
        run: impl FnOnce() + Send + 'static,
    ) -> Entry {
        Entry::new(id, lane, stop, Box::new(Run(Some(Box::new(run)))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A task that says which way it ended.
    struct Probe(std::sync::mpsc::Sender<(&'static str, String)>, String);

    impl Task for Probe {
        fn what(&self) -> &'static str {
            "a probe"
        }
        fn run(
            self: Box<Self>,
            _state: &Arc<crate::server::state::AppState>,
            _worker: &Worker<'_>,
        ) {
            self.0.send(("ran", self.1)).unwrap();
        }
        fn stopped(self: Box<Self>, _state: &Arc<crate::server::state::AppState>) {
            self.0.send(("stopped", self.1)).unwrap();
        }
    }

    fn entry(
        id: &str,
        lane: Lane,
        sender: &std::sync::mpsc::Sender<(&'static str, String)>,
    ) -> (Entry, Stop) {
        let stop = Stop::new();
        let task = Box::new(Probe(sender.clone(), id.to_string()));
        (Entry::new(id, lane, stop.clone(), task), stop)
    }

    fn id_of(dispatch: &Dispatch) -> (&'static str, &str) {
        match dispatch {
            Dispatch::Run { id, .. } => ("run", id),
            Dispatch::Stopped { id, .. } => ("stopped", id),
        }
    }

    #[tokio::test]
    async fn work_takes_its_held_place_and_runs_in_line_order() {
        let (sender, _) = std::sync::mpsc::channel();
        let line = JobLine::new();
        line.hold_place("slot", Vec::new());
        line.submit(entry("after", Lane::Worker, &sender).0, None)
            .ok()
            .unwrap();
        // A held place holds up nothing behind it.
        let first = line.next(Lane::Worker).await;
        assert_eq!(id_of(&first), ("run", "after"));
        // The work a request became goes in its place, ahead of what came
        // after it; work prepared under the place's own id takes it.
        line.submit(entry("b", Lane::Worker, &sender).0, Some("slot"))
            .ok()
            .unwrap();
        line.submit(entry("slot", Lane::Worker, &sender).0, None)
            .ok()
            .unwrap();
        line.submit(entry("c", Lane::Worker, &sender).0, None)
            .ok()
            .unwrap();
        assert_eq!(line.order(), ["b", "slot", "c"]);
        assert!(
            line.submit(entry("c", Lane::Worker, &sender).0, None)
                .is_err(),
            "already in line"
        );
        assert!(line.move_to("c", 0));
        assert_eq!(line.position("c"), Some(0));
        assert!(line.worker_busy(), "the first entry holds the worker");
        assert!(line.try_reserve_worker().is_none());
        drop(first);
        assert_eq!(id_of(&line.next(Lane::Worker).await), ("run", "c"));
    }

    #[tokio::test]
    async fn an_entry_stopped_while_it_waits_leaves_though_the_worker_is_busy() {
        let (sender, _) = std::sync::mpsc::channel();
        let line = JobLine::new();
        let _busy = line.try_reserve_worker().unwrap();
        let (waiting, stop) = entry("waits", Lane::Worker, &sender);
        line.submit(waiting, None).ok().unwrap();
        stop.request();
        line.changed();
        let dispatch = line.next(Lane::Worker).await;
        assert_eq!(id_of(&dispatch), ("stopped", "waits"));
        assert!(line.order().is_empty());
    }

    #[tokio::test]
    async fn a_reserved_turn_holds_the_worker_until_its_work_fills_it_or_it_runs_out() {
        let (sender, _) = std::sync::mpsc::channel();
        let line = Arc::new(JobLine::new());
        line.submit(entry("waiting", Lane::Worker, &sender).0, None)
            .ok()
            .unwrap();
        // A turn kept for work about to join goes first, and holds the lane.
        line.reserve_turn("kept", Duration::from_secs(60), None);
        assert_eq!(line.order(), ["kept", "waiting"]);
        let next = {
            let line = Arc::clone(&line);
            tokio::spawn(async move { line.next(Lane::Worker).await })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!next.is_finished(), "the kept turn holds the worker");
        // The work it was kept for joins at the back and takes the turn.
        line.submit(entry("joined", Lane::Worker, &sender).0, None)
            .ok()
            .unwrap();
        line.fill_reservation("kept", "joined");
        assert_eq!(id_of(&next.await.unwrap()), ("run", "joined"));
        assert_eq!(line.order(), ["waiting"]);

        // A kept turn whose work never joins runs out, counted from when it
        // first held anything up, and the line moves on.
        line.reserve_turn("lapses", Duration::from_millis(60), None);
        let started = Instant::now();
        assert_eq!(id_of(&line.next(Lane::Worker).await), ("run", "waiting"));
        assert!(started.elapsed() >= Duration::from_millis(50));
        assert!(line.order().is_empty());
    }

    #[test]
    fn a_reserved_turn_goes_after_the_place_it_follows() {
        let (sender, _) = std::sync::mpsc::channel();
        let line = JobLine::new();
        for id in ["a", "theirs", "b"] {
            line.submit(entry(id, Lane::Worker, &sender).0, None)
                .ok()
                .unwrap();
        }
        line.reserve_turn("next of theirs", Duration::from_secs(60), Some("theirs"));
        line.reserve_turn("first", Duration::from_secs(60), Some("gone"));
        assert_eq!(
            line.order(),
            ["first", "a", "theirs", "next of theirs", "b"]
        );
        line.release_place("first");
        assert_eq!(line.order(), ["a", "theirs", "next of theirs", "b"]);
    }

    #[tokio::test]
    async fn work_waits_until_nothing_holds_its_keys() {
        let (sender, _) = std::sync::mpsc::channel();
        let line = Arc::new(JobLine::new());
        let keys = vec!["target 7".to_string(), "project".to_string()];
        line.hold_place("preparing", vec!["project".into()]);
        let building = Entry::new(
            "build",
            Lane::Worker,
            Stop::new(),
            Box::new(Probe(sender.clone(), "build".into())),
        )
        .holding(vec!["target 7".into()]);
        line.submit(building, None).ok().unwrap();
        assert!(line.holds_any(&keys));
        let waiting = {
            let (line, keys) = (Arc::clone(&line), keys.clone());
            tokio::spawn(async move { line.wait_clear(&keys).await })
        };
        line.release_place("preparing");
        // The build still holds its key while it waits, and while it runs.
        let running = line.next(Lane::Worker).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        drop(running);
        waiting.await.unwrap();
        assert!(!line.holds_any(&keys));
    }

    #[tokio::test]
    async fn the_quick_lane_runs_beside_the_worker_in_the_order_joined() {
        let (sender, _) = std::sync::mpsc::channel();
        let line = Arc::new(JobLine::new());
        let _worker = line.try_reserve_worker().unwrap();
        for id in ["one", "two"] {
            line.submit(entry(id, Lane::Quick, &sender).0, None)
                .ok()
                .unwrap();
        }
        let first = line.next(Lane::Quick).await;
        assert_eq!(id_of(&first), ("run", "one"));
        let second = {
            let line = Arc::clone(&line);
            tokio::spawn(async move {
                matches!(line.next(Lane::Quick).await, Dispatch::Run { id, .. } if id == "two")
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!second.is_finished(), "one at a time");
        drop(first);
        assert!(second.await.unwrap());
    }

    #[tokio::test]
    async fn a_reservation_waiting_for_the_worker_goes_before_the_line() {
        let (sender, _) = std::sync::mpsc::channel();
        let line = Arc::new(JobLine::new());
        let busy = line.try_reserve_worker().unwrap();
        line.submit(entry("queued", Lane::Worker, &sender).0, None)
            .ok()
            .unwrap();
        let reserving = {
            let line = Arc::clone(&line);
            tokio::spawn(async move { line.reserve_worker().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(busy);
        let reserved = reserving.await.unwrap();
        assert_eq!(
            line.order(),
            ["queued"],
            "the line waits for the reservation"
        );
        drop(reserved);
        assert_eq!(id_of(&line.next(Lane::Worker).await), ("run", "queued"));
    }

    #[test]
    fn a_released_place_leaves_and_work_under_its_id_stays() {
        let (sender, _) = std::sync::mpsc::channel();
        let line = JobLine::new();
        line.hold_place("gone", Vec::new());
        line.hold_place("taken", Vec::new());
        line.submit(entry("taken", Lane::Worker, &sender).0, None)
            .ok()
            .unwrap();
        line.release_place("gone");
        line.release_place("taken");
        assert_eq!(line.order(), ["taken"]);
    }
}
