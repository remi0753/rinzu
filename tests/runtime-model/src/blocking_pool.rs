//! Bounded pool isolation and shutdown-blocker registry.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

type Job = Box<dyn FnOnce() + Send + 'static>;

enum Message {
    Run(Job),
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitError {
    Full,
    Closed,
}

/// A fixed-size executor with a bounded admission queue.
pub struct BoundedPool {
    sender: mpsc::SyncSender<Message>,
    workers: Vec<JoinHandle<()>>,
}

impl BoundedPool {
    /// Create a pool and its bounded admission queue.
    ///
    /// # Panics
    ///
    /// Panics when `thread_count` is zero.
    #[must_use]
    pub fn new(thread_count: usize, queue_capacity: usize) -> Self {
        assert!(thread_count > 0);
        let (sender, receiver) = mpsc::sync_channel::<Message>(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = (0..thread_count)
            .map(|_| {
                let receiver = Arc::clone(&receiver);
                thread::spawn(move || {
                    loop {
                        let message = receiver
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .recv();
                        match message {
                            Ok(Message::Run(job)) => job(),
                            Ok(Message::Shutdown) | Err(_) => break,
                        }
                    }
                })
            })
            .collect();
        Self { sender, workers }
    }

    /// Submit without blocking the calling Domain.
    ///
    /// # Errors
    ///
    /// Returns `Full` when all queue slots are reserved and `Closed` after
    /// executor shutdown.
    pub fn try_submit<F>(&self, job: F) -> Result<(), SubmitError>
    where
        F: FnOnce() + Send + 'static,
    {
        self.sender
            .try_send(Message::Run(Box::new(job)))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => SubmitError::Full,
                mpsc::TrySendError::Disconnected(_) => SubmitError::Closed,
            })
    }
}

impl Drop for BoundedPool {
    fn drop(&mut self) {
        for _ in 0..self.workers.len() {
            if self.sender.send(Message::Shutdown).is_err() {
                break;
            }
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShutdownBlocker {
    pub id: usize,
    pub label: String,
    pub has_cancel_hook: bool,
}

#[derive(Clone, Debug, Default)]
pub struct BlockingRegistry {
    next_id: Arc<AtomicUsize>,
    active: Arc<Mutex<BTreeMap<usize, ShutdownBlocker>>>,
}

impl BlockingRegistry {
    #[must_use]
    pub fn enter(&self, label: impl Into<String>, has_cancel_hook: bool) -> BlockingCallGuard {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let blocker = ShutdownBlocker {
            id,
            label: label.into(),
            has_cancel_hook,
        };
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, blocker);
        BlockingCallGuard {
            id,
            registry: self.clone(),
        }
    }

    #[must_use]
    pub fn shutdown_blockers(&self) -> Vec<ShutdownBlocker> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|entry| !entry.has_cancel_hook)
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

#[derive(Debug)]
pub struct BlockingCallGuard {
    id: usize,
    registry: BlockingRegistry,
}

impl Drop for BlockingCallGuard {
    fn drop(&mut self) {
        self.registry
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}
