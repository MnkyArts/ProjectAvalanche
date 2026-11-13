use super::*;
use crate::game::fe::globe::audio::FeAudio;

pub type V3 = [f32; 3];
pub type Quat = [f32; 4];
pub type M4 = [[f32; 4]; 4];

pub(super) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(super) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(super) fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub(super) fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(super) fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub(super) fn normalize(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}
pub(super) fn lerp(a: V3, b: V3, t: f32) -> V3 {
    add(a, scale(sub(b, a), t))
}

pub fn qmul(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

pub fn qconj(q: Quat) -> Quat {
    [-q[0], -q[1], -q[2], q[3]]
}

pub fn qrot(q: Quat, v: V3) -> V3 {
    let p = qmul(qmul(q, [v[0], v[1], v[2], 0.0]), qconj(q));
    [p[0], p[1], p[2]]
}

pub(super) fn axis_angle(axis: V3, angle: f32) -> Quat {
    let (s, c) = (angle * 0.5).sin_cos();
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

pub fn qmat(q: Quat) -> M4 {
    let [x, y, z] = [qrot(q, [1.0, 0.0, 0.0]), qrot(q, [0.0, 1.0, 0.0]), qrot(q, [0.0, 0.0, 1.0])];
    [[x[0], x[1], x[2], 0.0], [y[0], y[1], y[2], 0.0], [z[0], z[1], z[2], 0.0], [0.0, 0.0, 0.0, 1.0]]
}

pub fn mmul(a: &M4, b: &M4) -> M4 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..4).map(|k| a[i][k] * b[k][j]).sum()))
}

pub fn transform(p: V3, m: &M4) -> [f32; 4] {
    std::array::from_fn(|j| p[0] * m[0][j] + p[1] * m[1][j] + p[2] * m[2][j] + m[3][j])
}

pub fn globe_rotation(pitch: f32, yaw: f32) -> Quat {
    let c = axis_angle([1.0, 0.0, 0.0], -std::f32::consts::FRAC_PI_2);
    qmul(c, qmul(axis_angle([1.0, 0.0, 0.0], pitch), axis_angle([0.0, 1.0, 0.0], yaw)))
}

pub fn node_direction(lat: f32, lon: f32) -> V3 {
    let q = qmul(axis_angle([0.0, 1.0, 0.0], lon), axis_angle([1.0, 0.0, 0.0], -lat));
    qrot(q, [0.0, 0.0, 1.0])
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spring {
    pub omega: f32,
    pub x: f32,
    pub v: f32,
    pub target: f32,
}

impl Spring {
    pub fn step(&mut self, dt: f32) {
        let e = (-self.omega * dt).exp();
        let d = self.x - self.target;
        let k = self.v + d * self.omega;
        let x = (k * dt + d) * e + self.target;
        let v = (self.v - k * self.omega * dt) * e;
        self.x = x;
        self.v = v;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: V3,
    pub forward: V3,
    pub up: V3,
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn from_shot(s: &Shot) -> Self {
        let forward = normalize(sub(s.look, s.offset));
        let k = (forward[1].abs() - 0.7).clamp(0.0, 1.0);
        let reference = [0.0, 1.0 - k, -k];
        let up = normalize(cross(forward, cross(reference, forward)));
        Camera { eye: add(s.offset, s.target), forward, up, fov: s.fov, near: 10.0, far: 100_000.0 }
    }

    pub fn view(&self) -> M4 {
        let z = scale(self.forward, -1.0);
        let x = normalize(cross(self.forward, self.up));
        let y = cross(z, x);
        let p = self.eye;
        [
            [x[0], y[0], z[0], 0.0],
            [x[1], y[1], z[1], 0.0],
            [x[2], y[2], z[2], 0.0],
            [-dot(x, p), -dot(y, p), -dot(z, p), 1.0],
        ]
    }

    pub fn projection(&self, aspect: f32) -> M4 {
        let ys = 1.0 / (self.fov * 0.5).tan();
        let xs = ys / aspect;
        let (n, f) = (self.near, self.far);
        [[xs, 0.0, 0.0, 0.0], [0.0, ys, 0.0, 0.0], [0.0, 0.0, f / (n - f), -1.0], [0.0, 0.0, n * f / (n - f), 0.0]]
    }
}

pub fn inverse_rigid(m: &M4) -> M4 {
    let r: [[f32; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]));
    let t = [m[3][0], m[3][1], m[3][2]];
    let it: [f32; 3] = std::array::from_fn(|j| -(0..3).map(|k| t[k] * r[k][j]).sum::<f32>());
    [
        [r[0][0], r[0][1], r[0][2], 0.0],
        [r[1][0], r[1][1], r[1][2], 0.0],
        [r[2][0], r[2][1], r[2][2], 0.0],
        [it[0], it[1], it[2], 1.0],
    ]
}

pub(super) fn ease(kind: u32, t: f32) -> f32 {
    let s = t * t * (3.0 - 2.0 * t);
    match kind {
        0 => 1.0,
        2 => s,
        3 => s * s * (3.0 - 2.0 * s),
        5 => t * t,
        6 => 1.0 - (1.0 - t) * (1.0 - t),
        _ => t,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Segment {
    pub shot: Shot,
    pub curves: [u32; 3],
    pub duration: f32,
}

impl Segment {
    pub fn new(shot: Shot, curve: u32, duration: f32) -> Self {
        Segment { shot, curves: [curve; 3], duration }
    }
}

#[derive(Clone, Debug)]
pub struct ShotBlend {
    pub current: Shot,
    pub from: Shot,
    pub queue: Vec<Segment>,
    pub index: usize,
    pub segment_time: f32,
    pub total: f32,
    pub time: f32,
}

impl Default for ShotBlend {
    fn default() -> Self {
        ShotBlend {
            current: Shot::default(),
            from: Shot::default(),
            queue: Vec::new(),
            index: 0,
            segment_time: 0.0,
            total: -1.0,
            time: 0.0,
        }
    }
}

impl ShotBlend {
    pub fn queue(&mut self, segments: &[Segment], audio: &mut FeAudio) {
        self.from = self.current;
        self.queue = segments.iter().take(4).copied().collect();
        self.index = 0;
        self.segment_time = 0.0;
        self.time = 0.0;
        let total: f32 = self.queue.iter().map(|s| s.duration).sum();
        self.total = audio.beat_sync(total, 1.0);
        audio.transition_end(self.total);
    }

    pub fn go(&mut self, to: Shot, duration: f32, cut: Option<Shot>, audio: &mut FeAudio) {
        if let Some(c) = cut {
            self.current = c;
        }
        self.queue(&[Segment::new(to, 2, duration)], audio);
    }

    pub fn progress(&self) -> f32 {
        if self.total <= 0.0 { -1.0 } else { self.time / self.total }
    }

    pub fn done(&self) -> bool {
        self.total < 0.0
    }

    pub fn step(&mut self, dt: f32, audio: &mut FeAudio) {
        if self.total < 0.0 {
            return;
        }
        let mut dt = dt;
        loop {
            let step_dt = dt;
            self.segment_time += dt;
            self.time += dt;
            dt = 0.0;
            let Some(seg) = self.queue.get(self.index).copied() else {
                self.total = -1.0;
                self.index = 0;
                return;
            };
            if self.segment_time > seg.duration || seg.duration <= 0.0 {
                self.queue[self.index].duration = -1.0;
                self.current = seg.shot;
                if self.index + 1 < self.queue.len() {
                    self.from = self.current;
                    self.segment_time -= seg.duration.max(0.0);
                    self.index += 1;
                    if self.total >= 0.0 {
                        continue;
                    }
                } else {
                    self.total = -1.0;
                    self.index = 0;
                }
                return;
            }
            let look = self.blend(&seg, self.segment_time / seg.duration);
            audio.shot_step(look, seg.duration, step_dt);
            return;
        }
    }

    pub(super) fn blend(&mut self, seg: &Segment, t: f32) -> f32 {
        let to = seg.shot;
        let [c_target, c_look, c_fov] = seg.curves;
        let (e_look, e_target, e_fov) = (ease(c_look, t), ease(c_target, t), ease(c_fov, t));
        let (f, c) = (self.from, &mut self.current);
        c.look = lerp(f.look, to.look, e_look);
        let (d_from, d_to) = (sub(f.look, f.offset), sub(to.look, to.offset));
        let (l_from, l_to) = (dot(d_from, d_from).sqrt(), dot(d_to, d_to).sqrt());
        let dir = lerp(normalize(d_from), normalize(d_to), e_look);
        c.offset = sub(c.look, scale(dir, l_from + (l_to - l_from) * e_look));
        c.target = lerp(f.target, to.target, e_target);
        c.fov = f.fov + (to.fov - f.fov) * e_fov;
        e_look
    }
}
