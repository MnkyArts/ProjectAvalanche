use std::ffi::OsString;
use std::time::{Duration, SystemTime};

use super::driver::{forwarded, utc};
use super::report::{FrameStats, Machine, Report, Summary};
use super::{SCENES, scene};

#[test]
fn scenes_are_unique_and_found() {
    for (i, s) in SCENES.iter().enumerate() {
        assert_eq!(scene(s.id), Some(s));
        assert!(SCENES[..i].iter().all(|o| o.id != s.id), "{} twice", s.id);
    }
    assert_eq!(scene("nope"), None);
}

#[test]
fn percentiles_by_nearest_rank() {
    let v: Vec<f32> = (1..=100).map(|i| i as f32).collect();
    let s = Summary::of(&v);
    assert_eq!((s.p50, s.p95, s.p99, s.max), (50.0, 95.0, 99.0, 100.0));
    assert!((s.avg - 50.5).abs() < 1e-6);
    assert_eq!(Summary::of(&[]), Summary::default());
    assert_eq!(Summary::of(&[7.0]).p99, 7.0);
}

#[test]
fn lows_variance_and_pacing() {
    let mut v = vec![10.0f32; 990];
    v.extend([50.0; 10]);
    let f = FrameStats::of(&v);
    assert_eq!(f.frames, 1000);
    assert!((f.seconds - 10.4).abs() < 1e-9);
    assert!((f.low_1_fps - 20.0).abs() < 1e-4 && (f.low_01_fps - 20.0).abs() < 1e-4);
    let mean = 10.4f64;
    let var = (990.0 * (10.0 - mean).powi(2) + 10.0 * (50.0 - mean).powi(2)) / 1000.0;
    assert!((f64::from(f.variance_ms2) - var).abs() < 1e-3);
    assert!((f64::from(f.mean_delta_ms) - 40.0 / 999.0).abs() < 1e-6);
    assert_eq!(FrameStats::of(&[]), FrameStats::default());
}

#[test]
fn utc_dates() {
    let at = |s: u64| utc(SystemTime::UNIX_EPOCH + Duration::from_secs(s));
    let d = at(0);
    assert_eq!((d.year, d.month, d.day, d.hour, d.minute, d.second), (1970, 1, 1, 0, 0, 0));
    let d = at(951_827_696);
    assert_eq!((d.year, d.month, d.day, d.hour, d.minute, d.second), (2000, 2, 29, 12, 34, 56));
    let d = at(1_794_787_200);
    assert_eq!((d.year, d.month, d.day), (2026, 11, 16));
}

#[test]
fn the_scenes_get_the_game_options_without_the_benchmarks_own() {
    let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
    let got = forwarded(args(&[
        "--big-root",
        "/d/big",
        "--benchmark",
        "fe",
        "alps",
        "--out",
        "r.json",
        "--save",
        "/p/s",
        "--no-vsync",
        "--benchmark=heli",
        "--out=x",
        "--save=y",
        "--window-mode=borderless",
    ]));
    assert_eq!(got, args(&["--big-root", "/d/big", "--no-vsync", "--window-mode=borderless"]));
    assert_eq!(forwarded(args(&["--benchmark", "--subdivide", "2"])), args(&["--subdivide", "2"]));
}

#[test]
fn the_mixers_share_of_a_window() {
    use super::report::AudioCounters;
    let from = AudioCounters { rendered: 48_000, rendered_ns: 100_000_000, late: 2, starved: 1, underruns: 0 };
    let to = AudioCounters { rendered: 144_000, rendered_ns: 600_000_000, late: 5, starved: 1, underruns: 4 };
    let a = from.until(to, 48_000).expect("audio rendered");
    assert_eq!((a.rendered_s, a.load_pct, a.late_pulls, a.starved, a.underruns), (2.0, 25.0, 3, 0, 4));
    assert_eq!(from.until(from, 48_000), None);
}

#[test]
fn the_gpu_times_say_why_they_are_missing() {
    let report = |switches: &[&str]| Report {
        format: 1,
        program: String::new(),
        version: String::new(),
        started: String::new(),
        machine: None,
        options: Vec::new(),
        switches: switches.iter().map(|s| s.to_string()).collect(),
        scenes: Vec::new(),
    };
    let none = Machine::default();
    let some = Machine { gpu_timestamps: true, ..Machine::default() };
    assert_eq!(report(&[]).gpu_times(&some), "measured");
    assert_eq!(report(&[]).gpu_times(&none), "not available (no timestamp queries on this adapter)");
    assert_eq!(report(&["SSX_BENCH_GPU_MARKS=OFF"]).gpu_times(&none), "not measured (SSX_BENCH_GPU_MARKS=off)");
    assert_eq!(
        report(&["SSX_BENCH_GPU_MARKS=on"]).gpu_times(&none),
        "not available (no timestamp queries on this adapter)"
    );
}

#[test]
fn the_spacing_of_presents_and_the_lateness_on_a_frame_limits_grid() {
    use super::report::{Pacing, WaitOvershoot};
    assert_eq!(Pacing::of(&[1.0]), None);
    let p = Pacing::of(&[0.0, 0.010, 0.030, 0.040]).unwrap();
    assert_eq!(
        (p.intervals, p.min_ms.round(), p.max_ms.round(), p.p1_ms.round(), p.p99_ms.round()),
        (3, 10.0, 20.0, 10.0, 20.0)
    );
    assert!((p.mean_ms - 13.333).abs() < 1e-3 && (p.std_dev_ms - 4.714).abs() < 1e-3);
    let t = [0.000_05, 0.010_12, 0.020_00, 0.035_00, 0.045_10];
    let busy = [0.005, 0.005, 0.016, 0.005, 0.005];
    let o = WaitOvershoot::of(&t, &busy, 0.010).unwrap();
    assert_eq!((o.frames, o.regrids), (5, 1));
    assert!((o.max_us - 120.0).abs() < 0.5 && (o.p99_us - 120.0).abs() < 0.5, "{o:?}");
    let o = WaitOvershoot::of(&t, &[0.005, f64::NAN, 0.005, 0.001, 0.005], 0.010).unwrap();
    assert_eq!(o.regrids, 1);
    assert_eq!(WaitOvershoot::of(&[], &[], 0.010), None);
}

#[test]
fn a_changed_update_mode_starts_the_grid_anew() {
    use crate::shell::perf::timing::{MainTimes, Samples, Tagged};
    use bevy::winit::UpdateMode;
    let mode = |ms: u64| Some(UpdateMode::reactive(Duration::from_millis(ms)));
    let starts = [0.000_05, 0.010_05, 0.020_05, 0.030_05, 0.045_10, 0.055_10, 0.065_15];
    let modes = [mode(10), mode(10), mode(10), mode(15), mode(10), mode(10), mode(10)];
    let mut samples = Samples::default();
    for (k, (&t, &update_mode)) in starts.iter().zip(&modes).enumerate() {
        let frame = k as u32;
        samples.main.push(MainTimes { frame, frame_ms: 10.0, update_ms: 4.0, own_ms: 0.0, update_mode });
        samples.extract.push(Tagged { frame, value: 1.0 });
        samples.starts.push(Tagged { frame, value: t });
    }
    let mut m = super::report::Measured::default();
    m.pace(&samples, 0, 6, Some(0.010));
    let o = m.overshoot.unwrap();
    assert_eq!(o.regrids, 2, "{o:?}");
    assert!((o.max_us - 50.0).abs() < 0.5, "{o:?}");
}
