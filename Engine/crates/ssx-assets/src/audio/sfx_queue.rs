use std::marker::PhantomData;
use std::ptr::with_exposed_provenance_mut;
use std::sync::atomic::Ordering::{Acquire, Relaxed, SeqCst};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, PoisonError, RwLock};
use std::thread::{Thread, ThreadId};
use std::time::{Duration, Instant};

pub use std::sync::mpsc::{RecvError, RecvTimeoutError, SendError, TryRecvError};

struct Node<T> {
    next: usize,
    value: T,
}

const CLOSED: usize = 1;

struct Shared<T> {
    head: AtomicUsize,
    senders: AtomicUsize,
    sleeping: AtomicBool,
    sleeper: RwLock<Option<Thread>>,
    owns: PhantomData<T>,
}

unsafe impl<T: Send> Sync for Shared<T> {}

impl<T> Shared<T> {
    fn wake(&self) {
        if let Some(thread) = self.sleeper.read().unwrap_or_else(PoisonError::into_inner).as_ref() {
            thread.unpark();
        }
    }
}

// PORT(choice): the threads' messages (the game's commands to the real-time mixer thread, the stream file thread's
// blocks and requests, the reaper's, the replies) go through a list a sender links its complete message into with one
// compare-exchange and the receiver takes whole with one swap, so the mixer never waits for a thread it preempted
// (std::sync::mpsc, Rust 1.100, has its receiver spin with sched_yield on a slot a sender claimed and has not written:
// 55-126 ms stalls of the game thread under the SCHED_RR mixer, demoted by SIGXCPU at RLIMIT_RTTIME's 100 ms); the
// console's game-side call takes the audio system's lock (System::Lock 0x82A1AE90 / Unlock 0x82A1AEB0) and writes a
// 16-byte command at the system's buffer +0x28 + write offset +0xE8 (0x82A48C48), which the mixer's step 0x82A3EE48
// applies and resets.
pub fn queue<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared {
        head: AtomicUsize::new(0),
        senders: AtomicUsize::new(1),
        sleeping: AtomicBool::new(false),
        sleeper: RwLock::new(None),
        owns: PhantomData,
    });
    (Sender(shared.clone()), Receiver { shared, taken: 0, registered: None })
}

pub struct Sender<T>(Arc<Shared<T>>);

impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let node = Box::into_raw(Box::new(Node { next: 0, value }));
        let addr = node.expose_provenance();
        let mut head = self.0.head.load(Relaxed);
        loop {
            if head == CLOSED {
                let node = unsafe { Box::from_raw(node) };
                return Err(SendError(node.value));
            }
            unsafe { (*node).next = head };
            match self.0.head.compare_exchange_weak(head, addr, SeqCst, Relaxed) {
                Ok(_) => break,
                Err(now) => head = now,
            }
        }
        if self.0.sleeping.load(SeqCst) {
            self.0.wake();
        }
        Ok(())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.0.senders.fetch_add(1, Relaxed);
        Self(self.0.clone())
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.0.senders.fetch_sub(1, SeqCst) == 1 && self.0.sleeping.load(SeqCst) {
            self.0.wake();
        }
    }
}

pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
    taken: usize,
    registered: Option<ThreadId>,
}

impl<T> Receiver<T> {
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        if let Some(value) = self.pop() {
            return Ok(value);
        }
        if self.shared.senders.load(SeqCst) > 0 {
            return Err(TryRecvError::Empty);
        }
        self.pop().ok_or(TryRecvError::Disconnected)
    }

    pub fn try_iter(&mut self) -> impl Iterator<Item = T> + '_ {
        std::iter::from_fn(|| self.try_recv().ok())
    }

    pub fn recv(&mut self) -> Result<T, RecvError> {
        self.recv_until(None).map_err(|_| RecvError)
    }

    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        self.recv_until(Instant::now().checked_add(timeout))
    }

    fn recv_until(&mut self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        loop {
            match self.try_recv() {
                Ok(value) => return Ok(value),
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
                Err(TryRecvError::Empty) => {}
            }
            let left = match deadline {
                Some(d) => match d.checked_duration_since(Instant::now()).filter(|left| !left.is_zero()) {
                    Some(left) => Some(left),
                    None => return Err(RecvTimeoutError::Timeout),
                },
                None => None,
            };
            self.register();
            self.shared.sleeping.store(true, SeqCst);
            if self.shared.head.load(SeqCst) == 0 && self.shared.senders.load(SeqCst) > 0 {
                match left {
                    Some(left) => std::thread::park_timeout(left),
                    None => std::thread::park(),
                }
            }
            self.shared.sleeping.store(false, Relaxed);
        }
    }

    fn register(&mut self) {
        let me = std::thread::current();
        if self.registered != Some(me.id()) {
            self.registered = Some(me.id());
            *self.shared.sleeper.write().unwrap_or_else(PoisonError::into_inner) = Some(me);
        }
    }

    fn pop(&mut self) -> Option<T> {
        if self.taken == 0 {
            let list = self.shared.head.swap(0, Acquire);
            self.taken = unsafe { reversed::<T>(list) };
        }
        if self.taken == 0 {
            return None;
        }
        let node = unsafe { Box::from_raw(with_exposed_provenance_mut::<Node<T>>(self.taken)) };
        self.taken = node.next;
        Some(node.value)
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let rest = self.shared.head.swap(CLOSED, Acquire);
        unsafe {
            free::<T>(self.taken);
            free::<T>(reversed::<T>(rest));
        }
    }
}

unsafe fn reversed<T>(mut addr: usize) -> usize {
    let mut out = 0;
    while addr != 0 {
        let node = with_exposed_provenance_mut::<Node<T>>(addr);
        let next = std::mem::replace(unsafe { &mut (*node).next }, out);
        out = addr;
        addr = next;
    }
    out
}

unsafe fn free<T>(mut addr: usize) {
    while addr != 0 {
        let node = unsafe { Box::from_raw(with_exposed_provenance_mut::<Node<T>>(addr)) };
        addr = node.next;
    }
}

#[cfg(test)]
mod tests;
