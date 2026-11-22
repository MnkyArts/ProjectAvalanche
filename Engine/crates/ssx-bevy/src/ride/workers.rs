use bevy::tasks::{ComputeTaskPool, TaskPool};

// PORT(choice): the side work (the surface analysis' height raster, 0.52 ms of a 3.25 ms step on Mount Eddie, traced)
// runs on a free core of Bevy's compute pool beside the rest of the step; the console ran its scene ops one after
// another on the simulation thread (`LoopSimulation` 0x83047120).
#[derive(Clone, Copy, Debug, Default)]
pub struct PoolWorkers;

impl ssx_physics::workers::Workers for PoolWorkers {
    fn join(&self, side: &mut (dyn FnMut() + Send), main: &mut dyn FnMut()) {
        ComputeTaskPool::get_or_init(TaskPool::default).scope(|s| {
            s.spawn(async move { side() });
            main();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_physics::workers::join;

    #[test]
    fn the_pool_runs_both_sides() {
        let data: Vec<u64> = (1..=1000).collect();
        let mut seen = 0;
        let (sum, ()) = join(Some(&PoolWorkers), || data.iter().sum::<u64>(), || seen = data.len());
        assert_eq!((sum, seen), (500_500, 1000));
    }
}
