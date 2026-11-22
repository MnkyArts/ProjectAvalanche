use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

pub fn workers() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

pub fn ordered<T: Sync, R: Send>(name: &str, items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = workers().min(items.len());
    if threads <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let (f, next) = (&f, &next);
    let mut out: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..threads)
            .map(|k| {
                std::thread::Builder::new().name(format!("{name} {k}")).spawn_scoped(scope, move || {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        done.push((i, f(item)));
                    }
                    done
                })
            })
            .collect();
        let mut panic = None;
        for job in jobs {
            match job.map(std::thread::ScopedJoinHandle::join) {
                Ok(Ok(done)) => {
                    for (i, r) in done {
                        out[i] = Some(r);
                    }
                }
                Ok(Err(p)) => panic = panic.or(Some(p)),
                Err(_) => {}
            }
        }
        if let Some(p) = panic {
            std::panic::resume_unwind(p);
        }
    });
    out.into_iter().zip(items).map(|(r, item)| r.unwrap_or_else(|| f(item))).collect()
}

pub fn ordered_owned<T: Send, R: Send>(name: &str, items: Vec<T>, f: impl Fn(T) -> R + Sync) -> Vec<R> {
    let slots: Vec<Mutex<Option<T>>> = items.into_iter().map(|t| Mutex::new(Some(t))).collect();
    ordered(name, &slots, |slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take().map(&f))
        .into_iter()
        .flatten()
        .collect()
}

pub fn join<A, B: Send>(name: &str, a: impl FnOnce() -> A, b: impl FnOnce() -> B + Send) -> (A, B) {
    let b = Mutex::new(Some(b));
    let take = || b.lock().unwrap_or_else(PoisonError::into_inner).take();
    std::thread::scope(|scope| {
        let side = std::thread::Builder::new().name(name.to_string()).spawn_scoped(scope, || take().map(|f| f()));
        let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(a));
        let b = match side {
            Ok(h) => h.join(),
            Err(_) => Ok(None),
        };
        match (a, b) {
            (Ok(a), Ok(Some(b))) => (a, b),
            (Ok(a), Ok(None)) => match take() {
                Some(f) => (a, f()),
                None => unreachable!("the side work neither ran nor is left"),
            },
            (Err(p), _) | (_, Err(p)) => std::panic::resume_unwind(p),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_keeps_the_items_order() {
        let items: Vec<u64> = (0..1000).collect();
        let out = ordered("test", &items, |&x| x * x);
        assert_eq!(out, items.iter().map(|x| x * x).collect::<Vec<_>>());
        assert!(ordered("test", &[] as &[u8], |&x| x).is_empty());
    }

    #[test]
    fn ordered_owned_moves_each_item_once_in_order() {
        let items: Vec<String> = (0..200).map(|i| format!("item {i}")).collect();
        let out = ordered_owned("test", items.clone(), |s| s + "!");
        assert_eq!(out, items.iter().map(|s| format!("{s}!")).collect::<Vec<_>>());
    }

    #[test]
    fn join_gives_both_results() {
        let data = [1u32, 2, 3];
        let (a, b) = join("test", || data.iter().sum::<u32>(), || data.len());
        assert_eq!((a, b), (6, 3));
    }

    #[test]
    #[should_panic(expected = "item 7")]
    fn a_panic_goes_on_through_the_caller() {
        let items: Vec<u32> = (0..64).collect();
        ordered("test", &items, |&x| assert!(x != 7, "item {x}"));
    }
}
