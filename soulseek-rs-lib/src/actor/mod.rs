use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::utils::semaphore::{BoundedSender, bounded_channel};
use crate::{error, trace, warn};

pub mod peer_actor;
pub mod peer_registry;
pub mod server_actor;

#[derive(Debug, Clone)]
pub enum ConnectionState {
    Disconnected,
    Connecting { since: Instant },
    Connected,
}
/// Core actor trait - each actor processes messages
pub trait Actor: Send + 'static {
    type Message: Send + Clone + 'static;

    /// Handle a single message
    fn handle(&mut self, msg: Self::Message);

    /// Called when actor starts (optional hook)
    fn on_start(&mut self) {}

    /// Called when actor stops (optional hook)
    fn on_stop(&mut self) {}

    /// Optional periodic tick for background work
    fn tick(&mut self) {}
}

/// Mailbox depth per actor. A bounded mailbox is a decision to refuse work
/// rather than grow without limit until the process is killed; the full-queue
/// report goes back to the sender instead of being lost.
pub const ACTOR_MAILBOX_DEPTH: usize = 256;

#[derive(Clone)]
pub struct ActorHandle<M: Send> {
    pub(crate) sender: BoundedSender<ActorMessage<M>>,
    stop: Arc<AtomicBool>,
}

impl<M: Send> ActorHandle<M> {
    /// Build a handle over an externally owned bounded channel. Used by actor
    /// tests that drive `handle` directly without an actor thread.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_bounded(sender: BoundedSender<ActorMessage<M>>) -> Self {
        Self {
            sender,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn send(&self, msg: M) -> Result<(), String> {
        match self.sender.try_send(ActorMessage::UserMessage(msg)) {
            Ok(()) => Ok(()),
            Err(std::sync::mpsc::TrySendError::Full(_)) => Err(format!(
                "actor mailbox full at {}; message rejected",
                self.sender.capacity()
            )),
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                Err("actor is no longer running; message rejected".to_string())
            }
        }
    }

    /// Request actor to stop gracefully. The flag is authoritative: when the
    /// mailbox is full there is no room for the stop message, so the actor
    /// observes the flag on its next loop turn instead of waiting for a slot.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "Result is the published stop API"
    )]
    pub fn stop(&self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        let _ = self.sender.try_send(ActorMessage::Stop);
        Ok(())
    }
}

/// Internal actor message wrapper
#[derive(Debug)]
pub enum ActorMessage<M> {
    UserMessage(M),
    Stop,
}

/// Stack size for an actor thread. An actor loop parses messages into
/// heap-allocated buffers and never recurses, so it needs a fraction of the
/// 2 MiB default — and at a thousand concurrent peers that difference is the
/// difference between 2 GiB of reserved address space and 256 MiB.
const ACTOR_STACK_SIZE: usize = 256 * 1024;

struct ActorThread {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

/// Actor system that manages actor lifecycle.
///
/// Each actor owns a thread for its whole lifetime. That is deliberate: an
/// actor loop blocks in `recv_timeout` until it is stopped, so running actors
/// on a fixed-size pool would make the pool a semaphore capping the client at
/// one peer per core — and every peer past the cap would silently never start.
///
/// The system owns every actor thread it starts. Stopping or dropping it sets
/// each actor's stop flag and joins the handles, so an actor cannot outlive
/// the client context that registered it.
#[derive(Default)]
pub struct ActorSystem {
    stopped: AtomicBool,
    threads: Mutex<Vec<ActorThread>>,
}

impl ActorSystem {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a new actor and return its handle
    pub fn spawn<A: Actor>(&self, actor: A) -> ActorHandle<A::Message> {
        self.spawn_with_handle(actor, |_, _| {})
    }

    /// Spawn a new actor with initialization callback and return its handle
    /// The callback receives the actor handle before `on_start` is called
    pub fn spawn_with_handle<A: Actor, F>(
        &self,
        actor: A,
        init: F,
    ) -> ActorHandle<A::Message>
    where
        F: FnOnce(&mut A, ActorHandle<A::Message>) + Send + 'static,
    {
        match self.try_spawn_with_handle(actor, init) {
            Ok(handle) => handle,
            Err(e) => {
                error!("[actor_system] failed to spawn actor thread: {}", e);
                // A handle nobody serves: sends fail, which callers of the
                // infallible spawn already tolerate.
                let (sender, _) =
                    bounded_channel(1, "unserved actor");
                ActorHandle {
                    sender,
                    stop: Arc::new(AtomicBool::new(true)),
                }
            }
        }
    }

    /// Like [`Self::spawn_with_handle`], but hands a thread-spawn failure back
    /// to the caller instead of returning a handle no actor will ever serve.
    /// Storing such a dead handle is how a registry entry becomes a zombie
    /// that claims its username for the rest of the process.
    pub(crate) fn try_spawn_with_handle<A: Actor, F>(
        &self,
        mut actor: A,
        init: F,
    ) -> std::io::Result<ActorHandle<A::Message>>
    where
        F: FnOnce(&mut A, ActorHandle<A::Message>) + Send + 'static,
    {
        if self.stopped.load(Ordering::Acquire) {
            return Err(std::io::Error::other("actor system is stopping"));
        }
        let (sender, receiver) =
            bounded_channel::<ActorMessage<A::Message>>(
                ACTOR_MAILBOX_DEPTH,
                "actor",
            );
        let handle = ActorHandle {
            sender,
            stop: Arc::new(AtomicBool::new(false)),
        };
        let handle_for_init = handle.clone();
        let stop = Arc::clone(&handle.stop);
        let for_loop = Arc::clone(&stop);

        let thread_handle = thread::Builder::new()
            .name("soulseek-actor".to_string())
            .stack_size(ACTOR_STACK_SIZE)
            .spawn(move || {
                init(&mut actor, handle_for_init);
                actor.on_start();
                Self::run_actor_loop(&mut actor, receiver, for_loop);
                actor.on_stop();
            })?;

        self.threads.lock().unwrap_or_else(PoisonError::into_inner).push(
            ActorThread {
                stop,
                handle: thread_handle,
            },
        );
        Ok(handle)
    }

    fn run_actor_loop<A: Actor>(
        actor: &mut A,
        receiver: Receiver<ActorMessage<A::Message>>,
        stop: Arc<AtomicBool>,
    ) {
        let tick_interval = Duration::from_millis(100);
        let mut last_tick = Instant::now();
        let mut message_count = 0;
        let mut tick_count = 0;

        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            match receiver.recv_timeout(tick_interval) {
                Ok(ActorMessage::UserMessage(msg)) => {
                    message_count += 1;
                    actor.handle(msg);
                }
                Ok(ActorMessage::Stop) => {
                    trace!("[actor_system] Received Stop message, breaking loop");
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if last_tick.elapsed() >= tick_interval {
                        tick_count += 1;
                        actor.tick();
                        last_tick = Instant::now();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    trace!("[actor_system] Channel disconnected, breaking loop");
                    break;
                }
            }
        }
        trace!(
            "[actor_system] run_actor_loop ENDED - processed {} messages, {} ticks",
            message_count, tick_count
        );
    }

    /// Signal every actor and join. Idempotent; safe to call before dropping
    /// the context that holds this system.
    pub fn stop_all(&self) {
        self.stopped.store(true, Ordering::Release);
        let threads = std::mem::take(
            &mut *self.threads.lock().unwrap_or_else(PoisonError::into_inner),
        );
        for thread in threads {
            thread.stop.store(true, Ordering::Release);
            if let Err(panic) = thread.handle.join() {
                warn!("[actor_system] actor panicked: {panic:?}");
            }
        }
    }
}

impl Drop for ActorSystem {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::atomic::AtomicUsize;

    struct CounterActor {
        count: Arc<AtomicUsize>,
    }

    impl Actor for CounterActor {
        type Message = usize;

        fn handle(&mut self, msg: Self::Message) {
            self.count.fetch_add(msg, Ordering::SeqCst);
        }

        fn on_start(&mut self) {
            println!("Counter actor started");
        }

        fn on_stop(&mut self) {
            println!("Counter actor stopped");
        }
    }

    #[test]
    fn test_actor_system() {
        let system = ActorSystem::new();

        let count = Arc::new(AtomicUsize::new(0));
        let actor = CounterActor {
            count: count.clone(),
        };

        let handle = system.spawn(actor);

        // Send some messages
        handle.send(1).unwrap();
        handle.send(2).unwrap();
        handle.send(3).unwrap();

        // Give actor time to process
        std::thread::sleep(Duration::from_millis(100));

        assert_eq!(count.load(Ordering::SeqCst), 6);

        handle.stop().unwrap();

        // Give actor time to process the stop message
        std::thread::sleep(Duration::from_millis(100));
        system.stop_all();
    }

    // Actors must not be capped by a worker count: every peer runs one, and a
    // cap silently strands every peer past it. Far more actors than cores must
    // all make progress concurrently.
    #[test]
    fn far_more_actors_than_cores_all_run() {
        let system = ActorSystem::new();
        let count = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..256)
            .map(|_| {
                system.spawn(CounterActor {
                    count: count.clone(),
                })
            })
            .collect();
        for handle in &handles {
            handle.send(1).unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        while count.load(Ordering::SeqCst) < 256 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            count.load(Ordering::SeqCst),
            256,
            "every actor should have processed its message"
        );

        for handle in &handles {
            handle.stop().unwrap();
        }
        system.stop_all();
    }

    struct BlockingActor {
        entered: mpsc::SyncSender<()>,
        gate: mpsc::Receiver<()>,
    }

    impl Actor for BlockingActor {
        type Message = ();

        fn handle(&mut self, _msg: ()) {
            let _ = self.entered.send(());
            let _ = self.gate.recv();
        }
    }

    #[test]
    fn a_full_mailbox_rejects_and_stop_still_arrives() {
        let system = ActorSystem::new();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (gate_tx, gate_rx) = mpsc::channel();
        let handle = system
            .try_spawn_with_handle(
                BlockingActor {
                    entered: entered_tx,
                    gate: gate_rx,
                },
                |_, _| {},
            )
            .expect("actor starts");
        assert_eq!(handle.sender.capacity(), ACTOR_MAILBOX_DEPTH);

        // The actor is busy in `handle`, so the mailbox stops draining.
        handle.send(()).expect("first message");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("actor is handling");
        for _ in 0..ACTOR_MAILBOX_DEPTH {
            handle.send(()).expect("fill every mailbox slot");
        }
        let refused = handle.send(()).expect_err("a full mailbox refuses");
        assert!(
            refused.contains("full"),
            "the full queue must be reported loudly: {refused}"
        );

        // A full mailbox must not swallow stop: the flag is authoritative.
        assert!(handle.stop().is_ok(), "stop succeeds even when full");
        drop(gate_tx);
        system.stop_all();
    }
}
