use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

// PORT(choice): 2 s (the browser's connect time; a working name service answers in milliseconds, and glibc's default
// retry cycle is 5 s per attempt and server); tuning: phase 5's browser over real links, measured; the original has no
// server browser.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(2);

// PORT(choice): 16 (twice the browser's 8 workers, so a list of dead names cannot pile up threads), the port's bound;
// the original has no server browser.
pub const MAX_LOOKUPS: usize = 16;

const CANCEL_POLL: Duration = Duration::from_millis(50);

static UNDER_WAY: AtomicUsize = AtomicUsize::new(0);

pub(crate) type Lookup = fn(&str, u16) -> std::io::Result<Option<SocketAddr>>;

pub(crate) fn system(name: &str, port: u16) -> std::io::Result<Option<SocketAddr>> {
    Ok((name, port).to_socket_addrs()?.next())
}

pub(crate) fn resolve(
    name: &str,
    port: u16,
    limit: Duration,
    cancel: Option<&AtomicBool>,
    lookup: Lookup,
) -> Result<SocketAddr, String> {
    if UNDER_WAY.fetch_add(1, Ordering::AcqRel) >= MAX_LOOKUPS {
        UNDER_WAY.fetch_sub(1, Ordering::AcqRel);
        return Err(format!("{name} cannot be looked up now: {MAX_LOOKUPS} name lookups are under way"));
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let owned = name.to_string();
    let spawned = std::thread::Builder::new().name("name lookup".into()).spawn(move || {
        let answer = lookup(&owned, port);
        UNDER_WAY.fetch_sub(1, Ordering::AcqRel);
        let _ = tx.send(answer);
    });
    if let Err(e) = spawned {
        UNDER_WAY.fetch_sub(1, Ordering::AcqRel);
        return Err(format!("{name} cannot be looked up: no thread for it ({e})"));
    }
    let deadline = Instant::now() + limit;
    loop {
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            return Err(format!("the lookup of {name} was cancelled"));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!(
                "{name} cannot be found: no answer from the name service within {} s",
                limit.as_secs()
            ));
        }
        match rx.recv_timeout(left.min(CANCEL_POLL)) {
            Ok(Ok(Some(a))) => return Ok(a),
            Ok(Ok(None)) => return Err(format!("{name} has no address")),
            Ok(Err(e)) => return Err(format!("{name} cannot be found: {e}")),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(format!("the lookup of {name} ended without an answer")),
        }
    }
}

#[cfg(test)]
pub(super) fn under_way() -> usize {
    UNDER_WAY.load(Ordering::Acquire)
}

#[cfg(test)]
pub(super) static COUNTER_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn slow(_: &str, port: u16) -> std::io::Result<Option<SocketAddr>> {
        std::thread::sleep(Duration::from_secs(3));
        Ok(Some(SocketAddr::from(([192, 0, 2, 1], port))))
    }

    fn fast(_: &str, port: u16) -> std::io::Result<Option<SocketAddr>> {
        Ok(Some(SocketAddr::from(([192, 0, 2, 7], port))))
    }

    fn none(_: &str, _: u16) -> std::io::Result<Option<SocketAddr>> {
        Ok(None)
    }

    #[test]
    fn a_lookup_is_bounded_in_time_cancellable_and_capped() {
        let _counter = COUNTER_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let a = resolve("example.invalid", 27450, Duration::from_secs(2), None, fast).unwrap();
        assert_eq!(a, SocketAddr::from(([192, 0, 2, 7], 27450)));
        assert!(
            resolve("example.invalid", 1, Duration::from_secs(2), None, none).is_err_and(|e| e.contains("no address"))
        );
        let t = Instant::now();
        let e = resolve("slow.invalid", 1, Duration::from_millis(400), None, slow).unwrap_err();
        assert!(e.contains("within"), "{e}");
        assert!(t.elapsed() < Duration::from_millis(700), "{:?}", t.elapsed());
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&cancel);
        let t = Instant::now();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::Release);
        });
        let e = resolve("slow.invalid", 1, Duration::from_secs(10), Some(&cancel), slow).unwrap_err();
        stopper.join().unwrap();
        assert!(e.contains("cancelled"), "{e}");
        assert!(t.elapsed() < Duration::from_millis(400), "{:?}", t.elapsed());
        let mut refused = 0;
        for _ in 0..MAX_LOOKUPS + 4 {
            if resolve("slow.invalid", 1, Duration::from_millis(1), None, slow).is_err_and(|e| e.contains("under way"))
            {
                refused += 1;
            }
        }
        assert!(refused >= 4, "{refused} refused, {} under way", under_way());
        assert!(under_way() <= MAX_LOOKUPS);
        let end = Instant::now() + Duration::from_secs(5);
        while under_way() > 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(under_way(), 0);
        assert!(resolve("example.invalid", 2, Duration::from_secs(2), None, fast).is_ok());
    }
}
