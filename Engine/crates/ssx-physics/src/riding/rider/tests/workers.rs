use super::*;
use crate::workers::{ScopedThreads, Workers};

fn ride_bits(workers: Option<&dyn Workers>) -> (Vec<u64>, usize) {
    let slope = 0.2f32;
    let y = |z: f32| -z * slope.tan();
    let mut c = plane(300.0, slope);
    add_quad(
        &mut c,
        [
            Vec3::new(2.0, y(0.0) + 1.0, 0.0),
            Vec3::new(2.0, y(80.0) + 1.0, 80.0),
            Vec3::new(2.5, y(80.0) + 1.0, 80.0),
            Vec3::new(2.5, y(0.0) + 1.0, 0.0),
        ],
        2,
    );
    add_floor(&mut c, [-3.0, 3.0], [20.0, 40.0], y(20.0) + 0.3, 11);
    c.finish();
    let w = world(c);
    let mut s = moving(&w, Vec3::new(0.0, y(-100.0) + 1.0, -100.0), Vec3::Z, kmh(60.0));
    let mut seen = 0;
    let mut out = Vec::new();
    let mut fnv = |bits: &[u32]| {
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        for b in bits {
            h = (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3);
        }
        out.push(h);
    };
    for i in 0..240 {
        let input = Input { steer: ((i as f32) * 0.05).sin(), jump: i % 50 < 10, ..Input::default() };
        s.grind.pad_l2_hold = (100..160).contains(&i);
        let x = StepContext { workers, ..StepContext::default() };
        step_full(&mut s, &w, &input, &TrickInput::default(), DT_TICKS, x);
        let sa = s.grind.sa.as_deref().expect("published");
        let la = s.grind.feature.look_ahead;
        let mut bits: Vec<u32> = [s.traj, s.traj_velocity, la.pos, la.vel, sa.anchor, sa.dir]
            .iter()
            .flat_map(|v| v.to_array().map(f32::to_bits))
            .collect();
        bits.extend(s.traj_orientation.to_array().map(f32::to_bits));
        bits.extend(sa.heights.iter().map(|&h| u32::from(h)));
        seen += usize::from(sa.heights.iter().any(|&h| h != sa.heights[0]));
        fnv(&bits);
    }
    (out, seen)
}

#[test]
fn the_side_work_on_a_thread_changes_no_bit() {
    let (inline, seen) = ride_bits(None);
    let threads = ride_bits(Some(&ScopedThreads));
    assert_eq!(inline.len(), 240);
    assert!(seen > 20, "the raster saw the strip and the floor in {seen} steps");
    assert_eq!((inline, seen), threads);
}
