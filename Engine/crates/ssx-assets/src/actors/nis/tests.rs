use ssx_formats::ant_anim::{Animation, AnimationInfo, RawAnimation};

use super::*;

fn trimmed_clip(trim_offset: f32) -> NisClip {
    let frames = [0i16, 10, 20, 30, 40];
    let raw = RawAnimation {
        info: AnimationInfo {
            name: "trimmed".into(),
            codec_type: 0,
            anim_id: 0,
            trim_offset,
            end_frame: 40,
            additive: false,
            channel_to_dof: Default::default(),
        },
        key_times: frames.to_vec(),
        quat_count: 0,
        vec3_count: 1,
        float_count: 0,
        cycle: false,
        data: frames.iter().flat_map(|&f| [f32::from(f), 0.0, 0.0, 0.0]).collect(),
    };
    NisClip {
        name: "trimmed".into(),
        num_ticks: 200.0,
        fps_scale: 1.0,
        delta_trajectory: true,
        anim: crate::anim_clip::ClipData::Disc(Box::new(Animation::Raw(raw))),
        channels: HashMap::from([("AITrajectory.t".to_string(), Channel::Vec3(0))]),
    }
}

#[test]
fn a_clip_is_sampled_at_its_time_plus_the_trim_offset() {
    let clip = trimmed_clip(-161.0);
    let x = |tick: f32| clip.vec3(&clip.sample(tick).expect("sample"), "AITrajectory.t").expect("channel")[0];
    assert!((x(161.0) - 0.0).abs() < 1e-5, "tick 161 is frame 0: {}", x(161.0));
    assert!((x(176.0) - 15.0).abs() < 1e-5, "tick 176 is frame 15: {}", x(176.0));
    assert!((x(190.0) - 29.0).abs() < 1e-5, "tick 190 is frame 29: {}", x(190.0));
    let plain = trimmed_clip(0.0);
    assert!(
        (plain.vec3(&plain.sample(15.0).expect("sample"), "AITrajectory.t").expect("channel")[0] - 15.0).abs() < 1e-5
    );
}

#[test]
fn an_element_plays_its_clip_from_start_in_on_its_scale_to_its_end() {
    let eddie = ElementClock { begin: 0.0, start_in: 80.0, scale: 1.0, end: 520.0 };
    assert_eq!([eddie.clip_time(0.0), eddie.clip_time(440.0), eddie.clip_time(560.0)], [80.0, 520.0, 600.0]);
    let antar = ElementClock { begin: 0.0, start_in: 0.0, scale: 420.0 / 900.0, end: 900.0 };
    assert!((antar.clip_time(900.0) - 420.0).abs() < 1e-3 && antar.clip_time(1000.0) == antar.clip_time(900.0));
    let (outer, inner) = (
        ElementClock { begin: 0.0, start_in: 0.0, scale: 2.0, end: 100.0 },
        ElementClock { begin: 0.0, start_in: 10.0, scale: 0.5, end: 50.0 },
    );
    let both = outer.then(&inner);
    for t in [0.0, 10.0, 24.0, 40.0, 120.0] {
        assert!((both.clip_time(t) - inner.clip_time(outer.clip_time(t))).abs() < 1e-5, "{t}");
    }
    assert_eq!(ElementClock::SEQUENCE.then(&eddie), eddie);
}

#[test]
fn an_actor_plays_its_track_element_by_element() {
    let mut later = trimmed_clip(-300.0);
    later.num_ticks = 520.0;
    let actor = NisActor {
        name: "snowboarder".into(),
        role: ROLE_CHARACTER1,
        length: 520.0,
        clip: trimmed_clip(-80.0),
        offset: None,
        choices: Vec::new(),
        seed: None,
        chooser: None,
        sequences: Vec::new(),
        root: None,
        tags: Tags::default(),
        clock: ElementClock { begin: 0.0, start_in: 80.0, scale: 1.0, end: 300.0 },
        pieces: vec![NisPiece {
            clock: ElementClock { begin: 300.0, start_in: 300.0, scale: 1.0, end: 520.0 },
            clip: later,
            offset: None,
        }],
        layers: Vec::new(),
    };
    let frame = |t: f32| {
        let (clip, at) = actor.clip_at(t);
        clip.vec3(&clip.sample(at).expect("sample"), "AITrajectory.t").expect("channel")[0]
    };
    assert!((frame(15.0) - 15.0).abs() < 1e-5, "the first element: frame = sequence time, {}", frame(15.0));
    assert!(actor.piece_at(299.0).is_none() && actor.piece_at(300.0).is_some());
    assert!((frame(312.0) - 12.0).abs() < 1e-5, "the later one from its begin: {}", frame(312.0));
}

#[test]
fn a_nis_loop_wraps_where_its_sequence_ends() {
    let actor = |length: f32| NisActor {
        name: "a".into(),
        role: ROLE_CHARACTER1,
        length,
        clip: trimmed_clip(0.0),
        offset: None,
        choices: Vec::new(),
        seed: None,
        chooser: None,
        sequences: Vec::new(),
        root: None,
        tags: Tags::default(),
        clock: ElementClock::SEQUENCE,
        pieces: Vec::new(),
        layers: Vec::new(),
    };
    let fields = LoopFields { loop_control: false, start: 0.0, loop_start: 0.0, loop_end: 1000.0 };
    let l = NisLoop::of_scene(fields, false, &[actor(100.0), actor(100.0), actor(100.0)]).expect("the ready loop");
    assert_eq!(l.length, 100.0);
    for (ticks, t) in [(0.0, 0.0), (98.0, 98.0), (100.0, 0.0), (102.0, 2.0), (1800.0, 0.0), (1802.0, 2.0)] {
        assert_eq!(l.time(ticks), t, "{ticks}");
    }
    assert!(NisLoop::of_scene(fields, false, &[actor(100.0), actor(240.0)]).is_err(), "actors of two lengths");
    assert!(NisLoop::of_scene(LoopFields { loop_control: true, ..fields }, false, &[actor(100.0)]).is_err());
    assert!(NisLoop::of_scene(LoopFields { start: 10.0, ..fields }, false, &[actor(100.0)]).is_err());
    assert!(NisLoop::of_scene(fields, false, &[]).is_err(), "no actor: no length");
}

#[test]
fn a_layer_plays_over_the_pose_below_it() {
    let clip = trimmed_clip(0.0);
    let mut layer = NisLayer {
        name: "noise (Looping)".into(),
        clock: LayerClock::Node,
        length: 40.0,
        blend_mode: 0,
        mask: Some(HashMap::from([("AITrajectory.t".to_string(), 0.25)])),
        source: LayerSource::Clip(clip.clone()),
    };
    assert_eq!([layer.time(0.0), layer.time(39.0), layer.time(40.0), layer.time(101.0)], [0.0, 39.0, 0.0, 21.0]);
    let x = |v: Option<[f32; 3]>| v.expect("value")[0];
    assert!((x(layer.over_vec3(20.0, 0.0, "AITrajectory.t", Some([4.0, 0.0, 0.0]))) - 8.0).abs() < 1e-5);
    assert!((x(layer.over_vec3(20.0, 0.0, "AITrajectory.t", None)) - 20.0).abs() < 1e-5, "only the layer holds it");
    assert_eq!(layer.over_vec3(20.0, 0.0, "Other.t", Some([1.0, 2.0, 3.0])), Some([1.0, 2.0, 3.0]));
    layer.blend_mode = 1;
    assert!((x(layer.over_vec3(20.0, 0.0, "AITrajectory.t", Some([4.0, 0.0, 0.0]))) - 9.0).abs() < 1e-5);
    assert_eq!(layer.over_vec3(20.0, 0.0, "AITrajectory.t", None), None, "additive: both must hold it");
    let mut later = trimmed_clip(10.0);
    later.name = "later".into();
    layer.blend_mode = 0;
    layer.mask = None;
    layer.source = LayerSource::BlendArray {
        signal: "FLT_NisWindValue".into(),
        signal_default: 0.0,
        clamp: [0.0, 1.0],
        damping: [1.0, 1.0],
        range: [0.0, 1.0],
        items: vec![(0.0, clip), (1.0, later)],
    };
    let at = |v: f32| x(layer.over_vec3(20.0, v, "AITrajectory.t", Some([0.0; 3])));
    assert!((at(0.0) - 20.0).abs() < 1e-5 && (at(1.0) - 30.0).abs() < 1e-5 && (at(5.0) - 30.0).abs() < 1e-5);
    assert!((at(0.5) - 25.0).abs() < 1e-5 && (at(-1.0) - 20.0).abs() < 1e-5);
}
