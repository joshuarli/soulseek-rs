use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::error::Error;
use std::sync::mpsc::{
    Receiver, SendError, SyncSender, TrySendError, sync_channel,
};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

/// Default ceiling on owned background workers owned by one client context.
///
/// It is a bound, not a measured budget: the existing stress and RSS workloads
/// are the evidence for whether a session stays within it.
pub const DEFAULT_WORKER_CAPACITY: usize = 512;

pub struct Semaphore {
    permits: Mutex<usize>,
    available: Condvar,
}

pub struct Permit(Arc<Semaphore>);

impl Semaphore {
    #[must_use]
    pub const fn new(permits: usize) -> Self {
        Self {
            permits: Mutex::new(permits),
            available: Condvar::new(),
        }
    }

    /// Take a permit if one is free, or return `None` immediately. Callers
    /// accepting untrusted connections use this so a full queue refuses the
    /// newcomer instead of blocking the accept loop behind stalled handshakes.
    #[must_use]
    pub fn try_acquire(self: &Arc<Self>) -> Option<Permit> {
        let mut permits =
            self.permits.lock().unwrap_or_else(PoisonError::into_inner);
        if *permits == 0 {
            return None;
        }
        *permits -= 1;
        Some(Permit(Arc::clone(self)))
    }

    #[cfg(test)]
    #[must_use]
    pub fn acquire(self: &Arc<Self>) -> Permit {
        let mut permits =
            self.permits.lock().unwrap_or_else(PoisonError::into_inner);
        while *permits == 0 {
            permits = self
                .available
                .wait(permits)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *permits -= 1;
        Permit(Arc::clone(self))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut permits = self
            .0
            .permits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *permits += 1;
        self.0.available.notify_one();
    }
}

/// The sending half of a bounded queue.
///
/// Unlike `mpsc::Sender`, a full queue is never silently waited on: `send`
/// reports it, and the caller decides whether to reject the work or retry.
/// `label` appears in the report so an operator can tell which queue refused.
#[derive(Debug)]
pub struct BoundedSender<T> {
    inner: SyncSender<T>,
    capacity: usize,
    label: &'static str,
}

impl<T> Clone for BoundedSender<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            capacity: self.capacity,
            label: self.label,
        }
    }
}

impl<T> BoundedSender<T> {
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Reject when full instead of blocking the producer. A blocked producer
    /// on an untrusted path is a thread nobody owns waiting on work nobody can
    /// drain; a reported refusal lets the caller fail loudly.
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        match self.inner.try_send(value) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(value)) => {
                error!(
                    "[admission] {} queue full at {}; rejecting",
                    self.label, self.capacity
                );
                Err(TrySendError::Full(value))
            }
            Err(TrySendError::Disconnected(value)) => {
                Err(TrySendError::Disconnected(value))
            }
        }
    }

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        match self.try_send(value) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(value) | TrySendError::Disconnected(value)) => {
                Err(SendError(value))
            }
        }
    }
}

/// Create a bounded queue with an explicit capacity. The receiver is an
/// ordinary `mpsc::Receiver`, so consumers keep their existing loop.
#[must_use]
pub fn bounded_channel<T>(
    capacity: usize,
    label: &'static str,
) -> (BoundedSender<T>, Receiver<T>) {
    assert!(capacity > 0, "a bounded queue needs at least one slot");
    let (inner, receiver) = sync_channel(capacity);
    (
        BoundedSender {
            inner,
            capacity,
            label,
        },
        receiver,
    )
}

/// Why a request for a worker thread was refused.
#[derive(Debug)]
pub enum SpawnError {
    Full { label: &'static str, capacity: usize },
    Stopped,
    Io(io::Error),
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full { label, capacity } => {
                write!(f, "{label} worker cap {capacity} reached")
            }
            Self::Stopped => write!(f, "the worker owner is stopping"),
            Self::Io(e) => write!(f, "cannot start worker thread: {e}"),
        }
    }
}

impl Error for SpawnError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Full { .. } | Self::Stopped => None,
        }
    }
}

struct WorkTask {
    label: &'static str,
    cancel: Arc<AtomicBool>,
    unblock: Option<Arc<dyn Fn() + Send + Sync>>,
    handle: Option<JoinHandle<()>>,
}

/// Owns a bounded set of worker threads.
///
/// The owner holds the stop flag and the handles: stopping signals every
/// worker, unblocks the I/O it is parked on, and joins before returning.
/// Dropping it runs the same path, so an early return cannot leave a thread.
pub struct WorkRegistry {
    capacity: usize,
    stopped: AtomicBool,
    tasks: Mutex<Vec<WorkTask>>,
}

impl WorkRegistry {
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "a worker owner needs at least one slot");
        Self {
            capacity,
            stopped: AtomicBool::new(false),
            tasks: Mutex::new(Vec::new()),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    fn reap(tasks: &mut Vec<WorkTask>) {
        tasks.retain(|task| {
            task.handle
                .as_ref()
                .is_some_and(|handle| !handle.is_finished())
        });
    }

    /// How many workers are still live. Reaping makes this the occupied slot
    /// count, not a history of everything ever started.
    #[cfg(test)]
    #[must_use]
    pub fn active(&self) -> usize {
        let mut tasks =
            self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        Self::reap(&mut tasks);
        tasks.len()
    }

    /// Start a worker, or refuse it. The job receives the cancellation token;
    /// `unblock` is called on shutdown to wake I/O the worker may be parked
    /// on. Both the cap and the release path belong to this owner.
    pub fn try_spawn<F>(
        &self,
        label: &'static str,
        unblock: Option<Arc<dyn Fn() + Send + Sync>>,
        job: F,
    ) -> Result<Arc<AtomicBool>, SpawnError>
    where
        F: FnOnce(Arc<AtomicBool>) + Send + 'static,
    {
        // Hold the owner lock across the stopped check, the spawn and the
        // registration: stop_all must not drain the collection while a worker
        // is starting behind it.
        let mut tasks =
            self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        if self.stopped.load(Ordering::Acquire) {
            return Err(SpawnError::Stopped);
        }
        Self::reap(&mut tasks);
        if tasks.len() >= self.capacity {
            error!(
                "[admission] {label} worker cap {} reached; refusing",
                self.capacity
            );
            return Err(SpawnError::Full {
                label,
                capacity: self.capacity,
            });
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let for_job = Arc::clone(&cancel);
        let token = Arc::clone(&cancel);
        let handle = thread::Builder::new()
            .name(label.to_string())
            .spawn(move || job(for_job))
            .map_err(SpawnError::Io)?;
        tasks.push(WorkTask {
            label,
            cancel,
            unblock,
            handle: Some(handle),
        });
        drop(tasks);
        Ok(token)
    }

    /// Signal every worker, unblock parked I/O, then join. Idempotent: a
    /// second call sees an empty set and does nothing.
    pub fn stop_all(&self) {
        self.stopped.store(true, Ordering::Release);
        let mut tasks = std::mem::take(
            &mut *self.tasks.lock().unwrap_or_else(PoisonError::into_inner),
        );
        for task in &mut tasks {
            task.cancel.store(true, Ordering::Release);
            if let Some(unblock) = &task.unblock {
                unblock();
            }
        }
        for task in tasks {
            if let Some(handle) = task.handle
                && let Err(panic) = handle.join()
            {
                warn!("[admission] {} worker panicked: {panic:?}", task.label);
            }
        }
    }
}

impl fmt::Debug for WorkRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkRegistry")
            .field("capacity", &self.capacity)
            .field("stopped", &self.stopped)
            .field(
                "active",
                &self
                    .tasks
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .len(),
            )
            .finish()
    }
}

impl Drop for WorkRegistry {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn try_acquire_refuses_instead_of_waiting() {
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = semaphore.acquire();
        assert!(semaphore.try_acquire().is_none());
        drop(permit);
        assert!(semaphore.try_acquire().is_some());
    }

    #[test]
    fn permits_bound_concurrent_holders() {
        let semaphore = Arc::new(Semaphore::new(2));
        let first = semaphore.acquire();
        let _second = semaphore.acquire();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _third = semaphore.acquire();
            tx.send(()).unwrap();
        });

        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "a third acquire must block while both permits are held"
        );
        drop(first);
        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "releasing a permit must wake the waiter"
        );
    }

    #[test]
    fn a_full_bounded_queue_rejects_instead_of_buffering() {
        let (tx, rx) = bounded_channel::<u32>(2, "test");
        assert_eq!(tx.capacity(), 2);
        tx.send(1).expect("first slot");
        tx.send(2).expect("second slot");
        let rejected = tx.send(3).expect_err("the third message must be refused");
        assert_eq!(rejected.0, 3, "the rejected value comes back to the caller");
        assert_eq!(rx.try_recv(), Ok(1));
        assert_eq!(rx.try_recv(), Ok(2));
        assert!(
            rx.try_recv().is_err(),
            "a rejected message must never enter the queue"
        );
    }

    #[test]
    fn a_registry_refuses_work_past_its_cap_and_joins_on_stop() {
        let workers = WorkRegistry::new(1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let token = workers
            .try_spawn("test-worker", None, move |cancel| {
                started_tx.send(()).expect("signalled");
                while !cancel.load(Ordering::Acquire) {
                    match release_rx.recv_timeout(Duration::from_millis(5)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break;
                        }
                    }
                }
            })
            .expect("the first slot is free");
        started_rx.recv_timeout(Duration::from_secs(2)).expect("started");
        assert_eq!(workers.active(), 1);
        assert!(
            matches!(
                workers.try_spawn("test-worker", None, |_| {}),
                Err(SpawnError::Full { capacity: 1, .. })
            ),
            "a second worker must be refused while the first holds the slot"
        );

        drop(release_tx);
        workers.stop_all();
        assert!(
            token.load(Ordering::Acquire),
            "the owner's stop token reaches the worker"
        );
        assert_eq!(workers.active(), 0, "stop joins every worker it started");
        workers.stop_all();
    }

    #[test]
    fn stopping_a_registry_unblocks_a_parked_worker() {
        let workers = WorkRegistry::new(1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let wake = Arc::new(move || {
            let _ = release_tx.send(());
        });
        let _token = workers
            .try_spawn("test-worker", Some(wake), move |_cancel| {
                started_tx.send(()).expect("signalled");
                let _ = release_rx.recv();
            })
            .expect("the slot is free");
        started_rx.recv_timeout(Duration::from_secs(2)).expect("started");
        workers.stop_all();
        assert_eq!(
            workers.active(),
            0,
            "shutdown unblocks I/O the worker was parked on before joining"
        );
    }
}
