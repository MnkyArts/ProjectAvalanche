use std::time::Instant;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use super::Ride;

const MAX_TAPS: usize = 1000;

#[derive(Clone, Debug, PartialEq)]
struct Script {
    first: f64,
    every: f64,
    count: usize,
    durations: Vec<f64>,
}

fn parse(v: &str) -> Result<Script, String> {
    let parts: Vec<&str> = v.split(',').map(str::trim).collect();
    let [first, every, count, durations] = parts.as_slice() else {
        return Err("FIRST_S,EVERY_S,COUNT,MS[/MS…] expected".into());
    };
    let seconds = |p: &str| p.parse::<f64>().ok().filter(|x| x.is_finite() && *x >= 0.0).ok_or(format!("{p:?}"));
    let (first, every) = (seconds(first)?, seconds(every)?);
    let count = count.parse::<usize>().ok().filter(|n| (1..=MAX_TAPS).contains(n)).ok_or(format!("{count:?}"))?;
    let durations = durations
        .split('/')
        .map(|d| d.trim().parse::<f64>().ok().filter(|x| x.is_finite() && (1.0..=10_000.0).contains(x)))
        .collect::<Option<Vec<f64>>>()
        .ok_or(format!("{durations:?}"))?;
    if durations.iter().any(|d| d / 1000.0 >= every) {
        return Err(format!("a tap of {durations:?} ms does not end within the period of {every} s"));
    }
    Ok(Script { first, every, count, durations })
}

fn script() -> Option<&'static Script> {
    static SCRIPT: std::sync::OnceLock<Option<Script>> = std::sync::OnceLock::new();
    SCRIPT
        .get_or_init(|| {
            let v = std::env::var("SSX_TEST_TAPS").ok()?;
            parse(&v).map_err(|e| warn!("SSX_TEST_TAPS: {e}; not used")).ok()
        })
        .as_ref()
}

// PORT(choice): a test switch (unset in play): Space taps written as the window system's key events at wall-clock
// times, for a check of which taps the ride's steps read at the frame rates a machine draws; the console has no such
// input (its steps poll the pads, 0x82BD6CF0).
#[derive(Resource)]
pub(super) struct TapScript {
    script: &'static Script,
    start: Instant,
    next: usize,
    frames: Vec<(u64, u64)>,
    read: Vec<u32>,
    a_was: bool,
    frame: u64,
    times: (u64, f64, f64, f64),
    reported: bool,
}

impl TapScript {
    fn duration(&self, k: usize) -> f64 {
        self.script.durations[k % self.script.durations.len()]
    }

    fn event_time(&self, i: usize) -> f64 {
        let k = i / 2;
        let press = self.script.first + k as f64 * self.script.every;
        if i.is_multiple_of(2) { press } else { press + self.duration(k) / 1000.0 }
    }
}

pub(super) fn inject(
    ride: Res<Ride>,
    taps: Option<ResMut<TapScript>>,
    window: Query<Entity, With<PrimaryWindow>>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut keys: MessageWriter<KeyboardInput>,
) {
    let Some(script) = script() else { return };
    let Some(mut taps) = taps else {
        if ride.steps > 0 && !ride.hold {
            info!("taps: SSX_TEST_TAPS {script:?} from ride step {}", ride.steps);
            commands.insert_resource(TapScript {
                script,
                start: Instant::now(),
                next: 0,
                frames: vec![(0, 0); script.count],
                read: vec![0; script.count],
                a_was: false,
                frame: 0,
                times: (0, 0.0, f64::INFINITY, 0.0),
                reported: false,
            });
        }
        return;
    };
    let Ok(window) = window.single() else { return };
    taps.frame += 1;
    let ms = time.delta_secs_f64() * 1000.0;
    let t = &mut taps.times;
    *t = (t.0 + 1, t.1 + ms, t.2.min(ms), t.3.max(ms));
    let elapsed = taps.start.elapsed().as_secs_f64();
    while taps.next < 2 * script.count && taps.event_time(taps.next) <= elapsed {
        let (k, down, frame) = (taps.next / 2, taps.next.is_multiple_of(2), taps.frame);
        let state = if down { ButtonState::Pressed } else { ButtonState::Released };
        keys.write(KeyboardInput {
            key_code: KeyCode::Space,
            logical_key: Key::Space,
            state,
            text: None,
            repeat: false,
            window,
        });
        if down {
            taps.frames[k].0 = frame;
        } else {
            taps.frames[k].1 = frame;
            let (p, d) = (taps.frames[k].0, taps.duration(k));
            let one = if p == frame { ", its press and release in one frame" } else { "" };
            info!(
                "taps: tap {k} of {d} ms: press at frame {p}, release at frame {frame} (ride step {}){one}",
                ride.steps
            );
        }
        taps.next += 1;
    }
}

pub(super) fn steps(ride: Res<Ride>, taps: Option<ResMut<TapScript>>) {
    let Some(mut taps) = taps else { return };
    let a = ride.input.jump;
    if a && !taps.a_was && taps.next > 0 {
        let k = (taps.next - 1) / 2;
        taps.read[k] += 1;
        info!("taps: ride step {} read A pressed (tap {k}, {} ms)", ride.steps, taps.duration(k));
    }
    taps.a_was = a;
    let count = taps.script.count;
    let last = taps.event_time(2 * count - 1);
    if taps.reported || taps.next < 2 * count || taps.start.elapsed().as_secs_f64() < last + 0.5 {
        return;
    }
    taps.reported = true;
    let read = taps.read.iter().filter(|&&n| n > 0).count();
    let mut per = String::new();
    for d in &taps.script.durations {
        let of = (0..count).filter(|&k| taps.duration(k) == *d);
        let n = of.clone().count();
        let r = of.filter(|&k| taps.read[k] > 0).count();
        per.push_str(&format!(" {d} ms {r}/{n};"));
    }
    let (n, sum, min, max) = taps.times;
    let lost: Vec<usize> = (0..count).filter(|&k| taps.read[k] == 0).collect();
    info!(
        "taps: {read} of {count} taps read by a step ({per} lost {lost:?}); {n} frames, {:.1} ms mean, {min:.1} … \
         {max:.1} ms",
        sum / n.max(1) as f64
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_parses_and_refuses_what_does_not_fit() {
        let s = parse("2, 0.6, 30, 20/40/100").expect("a script");
        assert_eq!(s, Script { first: 2.0, every: 0.6, count: 30, durations: vec![20.0, 40.0, 100.0] });
        for bad in ["", "1,1,1", "1,0.05,3,60", "1,1,0,20", "1,1,5000,20", "x,1,1,20", "1,1,1,0", "1,-1,1,20"] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
