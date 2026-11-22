use bevy::prelude::*;

use crate::particles::sim::{RecordView, Weather};

#[derive(Debug)]
pub(super) struct Records<R = RecordView> {
    pub trails: Vec<R>,
    pub spray: Vec<R>,
    pub bots: Vec<Vec<R>>,
    pub live: Vec<Vec<R>>,
    pub rider: Vec<Option<R>>,
    pub heli: Vec<Option<R>>,
    pub track: Vec<R>,
    pub sun: Option<R>,
    pub geotags: Vec<R>,
    pub tombstones: Vec<R>,
}

fn list<R, S, T, F: FnMut(&R, Option<&S>) -> T>(v: &[R], other: Option<&[S]>, f: &mut F) -> Vec<T> {
    v.iter().enumerate().map(|(i, r)| f(r, other.and_then(|o| o.get(i)))).collect()
}

fn lists<R, S, T, F: FnMut(&R, Option<&S>) -> T>(v: &[Vec<R>], other: Option<&[Vec<S>]>, f: &mut F) -> Vec<Vec<T>> {
    v.iter().enumerate().map(|(i, r)| list(r, other.and_then(|o| o.get(i)).map(Vec::as_slice), f)).collect()
}

fn options<R, S, T, F: FnMut(&R, Option<&S>) -> T>(
    v: &[Option<R>],
    other: Option<&[Option<S>]>,
    f: &mut F,
) -> Vec<Option<T>> {
    v.iter()
        .enumerate()
        .map(|(i, r)| r.as_ref().map(|r| f(r, other.and_then(|o| o.get(i)).and_then(Option::as_ref))))
        .collect()
}

impl<R> Records<R> {
    pub fn zip_map<S, T>(&self, other: Option<&Records<S>>, mut f: impl FnMut(&R, Option<&S>) -> T) -> Records<T> {
        let f = &mut f;
        Records {
            trails: list(&self.trails, other.map(|o| &o.trails[..]), f),
            spray: list(&self.spray, other.map(|o| &o.spray[..]), f),
            bots: lists(&self.bots, other.map(|o| &o.bots[..]), f),
            live: lists(&self.live, other.map(|o| &o.live[..]), f),
            rider: options(&self.rider, other.map(|o| &o.rider[..]), f),
            heli: options(&self.heli, other.map(|o| &o.heli[..]), f),
            track: list(&self.track, other.map(|o| &o.track[..]), f),
            sun: self.sun.as_ref().map(|r| f(r, other.and_then(|o| o.sun.as_ref()))),
            geotags: list(&self.geotags, other.map(|o| &o.geotags[..]), f),
            tombstones: list(&self.tombstones, other.map(|o| &o.tombstones[..]), f),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct StateRecord {
    pub view: RecordView,
    pub pairs: [(u8, f32); 4],
}

impl StateRecord {
    pub fn new(view: &RecordView, before: Option<&StateRecord>) -> Self {
        let mut pairs = before.map_or([(0, 0.0); 4], |b| b.pairs);
        for (p, b) in pairs.iter_mut().zip(&view.blend) {
            *p = *b;
        }
        let mut view = view.clone();
        view.blend.truncate(4);
        StateRecord { view, pairs }
    }
}

pub(super) fn blend_transform(a: (Vec3, Quat, Vec3), b: (Vec3, Quat, Vec3), t: f32) -> (Vec3, Quat, Vec3) {
    let (qa, qb) = (Vec4::from(a.1), Vec4::from(b.1));
    let d = qa.dot(qb);
    let (qa, d) = if d < 0.0 { (-qa, -d) } else { (qa, d) };
    let q = if d > 0.996_194_7 {
        (qa + (qb - qa) * t).normalize()
    } else {
        let th = ssx_formats::vmx::acos(d);
        let s = ssx_formats::vmx::sin(th);
        qa * (ssx_formats::vmx::sin((1.0 - t) * th) / s) + qb * (ssx_formats::vmx::sin(t * th) / s)
    };
    (a.0 + (b.0 - a.0) * t, Quat::from_vec4(q), a.2 + (b.2 - a.2) * t)
}

pub(super) fn interpolate(a: Option<&StateRecord>, b: &StateRecord, t: f32) -> RecordView {
    let Some(a) = a else { return b.view.clone() };
    let (av, bv) = (&a.view, &b.view);
    let same = av.start == bv.start && av.instant_off == bv.instant_off;
    let ((scale, rotation, position), t) = if same {
        (blend_transform((av.scale, av.rotation, av.position), (bv.scale, bv.rotation, bv.position), t), t)
    } else {
        ((bv.scale, bv.rotation, bv.position), 1.0)
    };
    let from = if t > 0.5 { b } else { a };
    RecordView {
        scale,
        rotation,
        position,
        start: from.view.start,
        stop: from.view.stop,
        instant_off: from.view.instant_off,
        ao: av.ao + (bv.ao - av.ao) * t,
        blend: (0..from.view.blend.len())
            .map(|i| (from.pairs[i].0, a.pairs[i].1 + (b.pairs[i].1 - a.pairs[i].1) * t))
            .collect(),
    }
}

#[derive(Debug)]
pub(super) struct Snapshot {
    pub records: Records<StateRecord>,
    pub clock: f32,
    pub point: Vec3,
    pub weather: Weather,
    pub steps: u64,
}

pub(super) struct Frame {
    pub records: Records,
    pub clock: f32,
    pub point: Vec3,
    pub weather: Weather,
}

#[derive(Debug, Default)]
pub(super) struct Snapshots {
    a: Option<Snapshot>,
    b: Option<Snapshot>,
}

impl Snapshots {
    pub fn push(&mut self, records: &Records, clock: f32, point: Vec3, weather: Weather, steps: u64) {
        let before = self.b.as_ref().map(|b| &b.records);
        let records = records.zip_map(before, StateRecord::new);
        let a = self.b.take().filter(|b| b.steps < steps);
        (self.a, self.b) = (a, Some(Snapshot { records, clock, point, weather, steps }));
    }

    pub fn frame(&self, render_steps: Option<f32>) -> Option<Frame> {
        let b = self.b.as_ref()?;
        let t = match (&self.a, render_steps) {
            (Some(a), Some(r)) => ((r - a.steps as f32) / (b.steps - a.steps) as f32).clamp(0.0, 1.0),
            _ => 1.0,
        };
        Some(match self.a.as_ref().filter(|_| t < 1.0) {
            None => Frame {
                records: b.records.zip_map(None::<&Records<StateRecord>>, |r, _| r.view.clone()),
                clock: b.clock,
                point: b.point,
                weather: b.weather,
            },
            Some(a) => Frame {
                records: b.records.zip_map(Some(&a.records), |rb, ra| interpolate(ra, rb, t)),
                clock: a.clock + (b.clock - a.clock) * t,
                point: a.point + (b.point - a.point) * t,
                weather: b.weather,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(p: Vec3, q: Quat, start: f32, blend: &[(u8, f32)]) -> RecordView {
        RecordView {
            scale: Vec3::ONE,
            rotation: q,
            position: p,
            start,
            stop: -1.0,
            instant_off: false,
            ao: 1.0,
            blend: blend.to_vec(),
        }
    }

    fn records(rider: Option<RecordView>, track: Vec<RecordView>) -> Records {
        Records {
            trails: Vec::new(),
            spray: Vec::new(),
            bots: Vec::new(),
            live: Vec::new(),
            rider: vec![rider],
            heli: Vec::new(),
            track,
            sun: None,
            geotags: Vec::new(),
            tombstones: Vec::new(),
        }
    }

    #[test]
    fn transforms_lerp_and_turn_within_5_degrees_and_slerp_beyond() {
        let a = (Vec3::ONE, Quat::IDENTITY, Vec3::ZERO);
        let b = (Vec3::splat(3.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), Vec3::new(2.0, 4.0, 6.0));
        let half = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let (s, q, p) = blend_transform(a, b, 0.5);
        assert_eq!((s, p), (Vec3::splat(2.0), Vec3::new(1.0, 2.0, 3.0)));
        assert!(q.angle_between(half) < 1e-3, "{q:?}");
        assert!((q.length() - 1.0).abs() < 1e-5);
        let (_, q, _) = blend_transform(a, (b.0, -b.1, b.2), 0.5);
        assert!(q.angle_between(half) < 1e-3, "{q:?}");
        let small = Quat::from_rotation_x(4f32.to_radians());
        let (_, q, _) = blend_transform(a, (b.0, small, b.2), 0.25);
        let lerped = (Vec4::from(Quat::IDENTITY) * 0.75 + Vec4::from(small) * 0.25).normalize();
        assert!(Vec4::from(q).abs_diff_eq(lerped, 1e-6), "{q:?} {lerped:?}");
    }

    #[test]
    fn a_record_switched_between_the_updates_takes_b_whole() {
        let a = StateRecord::new(&rec(Vec3::ZERO, Quat::IDENTITY, 1.0, &[(1, 0.0), (2, 1.0)]), None);
        let mut bv = rec(Vec3::X * 4.0, Quat::IDENTITY, 1.0, &[(1, 1.0), (2, 0.0)]);
        bv.ao = 0.0;
        bv.stop = 3.0;
        let b = StateRecord::new(&bv, Some(&a));
        let r = interpolate(Some(&a), &b, 0.25);
        assert_eq!((r.position, r.ao, r.stop), (Vec3::X, 0.75, -1.0));
        assert_eq!(r.blend, vec![(1, 0.25), (2, 0.75)]);
        assert_eq!(interpolate(Some(&a), &b, 0.75).stop, 3.0);
        let mut bv2 = bv.clone();
        bv2.start = 2.0;
        let r = interpolate(Some(&a), &StateRecord::new(&bv2, Some(&a)), 0.25);
        assert_eq!((r.position, r.ao, r.start), (Vec3::X * 4.0, 0.0, 2.0));
        assert_eq!(r.blend, vec![(1, 1.0), (2, 0.0)]);
        let mut bv3 = bv.clone();
        bv3.instant_off = true;
        let r = interpolate(Some(&a), &StateRecord::new(&bv3, Some(&a)), 0.25);
        assert_eq!((r.position, r.instant_off), (Vec3::X * 4.0, true));
        let r = interpolate(None, &b, 0.25);
        assert_eq!((r.position, r.ao), (Vec3::X * 4.0, 0.0));
    }

    #[test]
    fn a_shorter_blend_list_keeps_the_pairs_an_earlier_update_wrote() {
        let four = [(1, 0.1), (2, 0.2), (3, 0.3), (4, 0.4)];
        let a = StateRecord::new(&rec(Vec3::ZERO, Quat::IDENTITY, 1.0, &four), None);
        let b = StateRecord::new(&rec(Vec3::ZERO, Quat::IDENTITY, 1.0, &[(1, 0.5), (2, 0.5)]), Some(&a));
        assert_eq!(b.pairs, [(1, 0.5), (2, 0.5), (3, 0.3), (4, 0.4)]);
        let r = interpolate(Some(&a), &b, 0.5);
        assert_eq!(r.blend.len(), 4);
        assert!((r.blend[0].1 - 0.3).abs() < 1e-6 && (r.blend[1].1 - 0.35).abs() < 1e-6);
        assert_eq!((r.blend[2], r.blend[3]), ((3, 0.3), (4, 0.4)));
        let r = interpolate(Some(&a), &b, 0.75);
        assert_eq!(r.blend.len(), 2);
        assert!((r.blend[0].1 - 0.4).abs() < 1e-6);
    }

    #[test]
    fn frames_draw_between_the_last_two_updates_at_the_render_clock() {
        let mut s = Snapshots::default();
        assert!(s.frame(Some(0.0)).is_none());
        let w = Weather::default();
        let at = |x: f32| records(Some(rec(Vec3::X * x, Quat::IDENTITY, 0.5, &[(1, 1.0), (2, 0.0)])), Vec::new());
        let rider = |f: &Frame| f.records.rider[0].as_ref().map(|r| r.position);
        s.push(&at(0.0), 0.3, Vec3::ZERO, w, 9);
        let f = s.frame(Some(8.5)).unwrap();
        assert_eq!((f.clock, rider(&f)), (0.3, Some(Vec3::ZERO)));
        let clock = 0.3 + 1.0 / 30.0;
        s.push(&at(3.0), clock, Vec3::Y * 6.0, w, 10);
        let f = s.frame(Some(9.5)).unwrap();
        assert!((rider(&f).unwrap() - Vec3::X * 1.5).length() < 1e-4);
        assert!((f.clock - (0.3 + 0.5 / 30.0)).abs() < 1e-6 && (f.point - Vec3::Y * 3.0).length() < 1e-4);
        let f = s.frame(Some(10.0)).unwrap();
        assert_eq!((f.clock, rider(&f)), (clock, Some(Vec3::X * 3.0)));
        assert_eq!(s.frame(None).unwrap().clock, clock);
        s.push(&records(None, vec![rec(Vec3::Z, Quat::IDENTITY, 0.0, &[])]), 0.4, Vec3::ZERO, w, 11);
        let f = s.frame(Some(10.5)).unwrap();
        assert_eq!((rider(&f), f.records.track[0].position), (None, Vec3::Z));
        s.push(&at(7.0), 0.5, Vec3::ZERO, w, 1);
        assert_eq!(rider(&s.frame(Some(0.5)).unwrap()), Some(Vec3::X * 7.0));
    }
}
