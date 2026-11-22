pub trait Workers: Send + Sync + std::fmt::Debug {
    fn join(&self, side: &mut (dyn FnMut() + Send), main: &mut dyn FnMut());
}

// PORT(choice): the step's independent work (the surface analysis' height raster) runs on another core while the
// step goes on; the console's simulation thread ran the scene ops one after another (`LoopSimulation` 0x83047120).
// The work reads only the collision world and its own inputs, and nothing reads its result before the step's last op
// (`SurfaceAnalysisGatherSceneOp`), so the results are the same bits on one thread or two.
pub fn join<A: Send, B>(
    workers: Option<&dyn Workers>,
    side: impl FnOnce() -> A + Send,
    main: impl FnOnce() -> B,
) -> (A, B) {
    let (mut side, mut main) = (Some(side), Some(main));
    let (mut a, mut b) = (None, None);
    if let Some(w) = workers {
        let mut run_side = || a = side.take().map(|f| f());
        let mut run_main = || b = main.take().map(|f| f());
        w.join(&mut run_side, &mut run_main);
    }
    let a = match (a, side.take()) {
        (Some(a), _) => a,
        (None, Some(f)) => f(),
        (None, None) => unreachable!("the side work neither ran nor is left"),
    };
    let b = match (b, main.take()) {
        (Some(b), _) => b,
        (None, Some(f)) => f(),
        (None, None) => unreachable!("the main work neither ran nor is left"),
    };
    (a, b)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScopedThreads;

impl Workers for ScopedThreads {
    fn join(&self, side: &mut (dyn FnMut() + Send), main: &mut dyn FnMut()) {
        std::thread::scope(|s| {
            let h = s.spawn(side);
            main();
            if let Err(panic) = h.join() {
                std::panic::resume_unwind(panic);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_in_line_and_on_threads_gives_both_results() {
        let data = [1u32, 2, 3];
        let (a, b) = join(None, || data.iter().sum::<u32>(), || data.len());
        assert_eq!((a, b), (6, 3));
        let mut written = 0;
        let (a, ()) = join(Some(&ScopedThreads), || data.iter().product::<u32>(), || written = 7);
        assert_eq!((a, written), (6, 7));
    }

    #[derive(Debug)]
    struct Idle;
    impl Workers for Idle {
        fn join(&self, _: &mut (dyn FnMut() + Send), _: &mut dyn FnMut()) {}
    }

    #[test]
    fn join_runs_what_an_implementation_left() {
        assert_eq!(join(Some(&Idle), || 2, || 3), (2, 3));
    }
}
