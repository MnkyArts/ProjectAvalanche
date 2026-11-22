use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use super::*;

struct Counted(u32, Arc<AtomicUsize>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.1.fetch_add(1, Relaxed);
    }
}

#[test]
fn messages_come_out_in_the_order_they_were_sent() {
    let (tx, mut rx) = queue();
    for k in 0..5 {
        tx.send(k).unwrap();
    }
    assert_eq!(rx.try_recv(), Ok(0));
    assert_eq!(rx.try_recv(), Ok(1));
    tx.send(5).unwrap();
    tx.send(6).unwrap();
    assert_eq!(rx.try_iter().collect::<Vec<_>>(), [2, 3, 4, 5, 6]);
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    tx.send(7).unwrap();
    assert_eq!(rx.try_recv(), Ok(7));
}

#[test]
fn the_receiver_gets_what_the_last_senders_sent_then_disconnected() {
    let (tx, mut rx) = queue();
    let tx2 = tx.clone();
    tx.send(1).unwrap();
    drop(tx);
    assert_eq!(rx.try_recv(), Ok(1));
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    tx2.send(2).unwrap();
    drop(tx2);
    assert_eq!(rx.try_recv(), Ok(2));
    assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    assert_eq!(rx.recv(), Err(RecvError));
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)), Err(RecvTimeoutError::Disconnected));
}

#[test]
fn every_message_is_dropped_once_and_a_send_without_receiver_fails() {
    let drops = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = queue();
    for k in 0..6 {
        assert!(tx.send(Counted(k, drops.clone())).is_ok());
    }
    drop(rx.try_recv().unwrap());
    tx.send(Counted(6, drops.clone())).unwrap();
    assert_eq!(drops.load(Relaxed), 1);
    drop(rx);
    assert_eq!(drops.load(Relaxed), 7);
    let back = tx.send(Counted(7, drops.clone())).unwrap_err();
    assert_eq!((back.0).0, 7);
    drop(back);
    assert_eq!(drops.load(Relaxed), 8);
}

#[test]
fn many_senders_one_receiver_lose_and_reorder_nothing() {
    const SENDERS: usize = 4;
    const EACH: u32 = 50_000;
    let (tx, mut rx) = queue::<(usize, u32)>();
    let threads: Vec<_> = (0..SENDERS)
        .map(|s| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for k in 0..EACH {
                    tx.send((s, k)).unwrap();
                }
            })
        })
        .collect();
    drop(tx);
    let mut next = [0u32; SENDERS];
    let mut wait = false;
    loop {
        let got = if wait { rx.recv().map_err(|_| ()) } else { rx.try_recv().map_err(|_| ()) };
        wait = !wait;
        match got {
            Ok((s, k)) => {
                assert_eq!(k, next[s], "sender {s}");
                next[s] += 1;
            }
            Err(()) if wait => {}
            Err(()) => break,
        }
    }
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(next, [EACH; SENDERS]);
}

#[test]
fn a_waiting_receiver_wakes_for_a_send_and_times_out_without_one() {
    let (tx, mut rx) = queue();
    let start = Instant::now();
    assert_eq!(rx.recv_timeout(Duration::from_millis(30)), Err(RecvTimeoutError::Timeout));
    assert!(start.elapsed() >= Duration::from_millis(30));
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        tx.send(42).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        tx.send(43).unwrap();
    });
    let start = Instant::now();
    assert_eq!(rx.recv_timeout(Duration::from_secs(60)), Ok(42));
    assert!(start.elapsed() < Duration::from_secs(30));
    assert_eq!(rx.recv(), Ok(43));
    assert_eq!(rx.recv(), Err(RecvError));
    sender.join().unwrap();
}

#[test]
fn the_ends_go_between_threads() {
    fn shared<T: Send + Sync>() {}
    fn moved<T: Send>() {}
    shared::<Sender<Box<dyn FnOnce() + Send>>>();
    moved::<Receiver<Box<dyn FnOnce() + Send>>>();
}
