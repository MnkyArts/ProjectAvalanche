use ssx_assets::career::globe::{GlobeData, Shot};

mod camera;
mod frame;
mod math;

pub use math::*;

pub fn level_of_screen(screen: u16) -> Option<u32> {
    match screen {
        2 => Some(2),
        0x42 | 0xC | 0xF => Some(3),
        0x43 | 0xD | 0x10 => Some(4),
        0x44 | 0xE | 0x11 => Some(5),
        _ => None,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlobeEntry {
    pub key: u32,
    pub kind: i32,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub opacity: f32,
    pub scale: f32,
    pub win_condition: i32,
    pub selectable: bool,
    pub selected: bool,
    pub recommended: bool,
    pub is_new: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MidgroundFrame {
    pub range: u32,
    pub world: M4,
    pub view: M4,
    pub proj: M4,
    pub camera_pos: V3,
    pub highlight: [f32; 4],
    pub highlight_xform: M4,
    pub lighting: u32,
    pub game_time: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlobeFrame {
    pub visible: bool,
    pub view: M4,
    pub proj: M4,
    pub view_inverse: M4,
    pub world_view: M4,
    pub w_inv: M4,
    pub camera_pos: V3,
    pub project_cam: [f32; 4],
    pub globe_r: [f32; 4],
    pub atmos_r: [f32; 4],
    pub highlight_view: V3,
    pub game_time: f32,
    pub midground: Option<MidgroundFrame>,
    pub midground_range: u32,
    pub lighting: u32,
    pub light: crate::game::fe::globe::look::LightState,
    pub postfx: Vec<(String, f32)>,
    pub fade: [f32; 2],
    pub cloud: [f32; 2],
    pub clouds: Vec<crate::game::fe::globe::clouds::CloudDraw>,
}

impl GlobeFrame {
    pub fn drawn(&self) -> bool {
        self.visible || self.midground.is_some()
    }
}

pub fn render_object(radius: f32, atmosphere: f32, rotation: Quat, camera: V3) -> (M4, [f32; 4], [f32; 4], [f32; 4]) {
    let l2 = dot(camera, camera);
    let l = l2.sqrt();
    let z = normalize(camera);
    let x = normalize(cross([0.0, 0.0, -1.0], z));
    let y = cross(z, x);
    let qi = qconj(rotation);
    let [lx, ly, lz] = [qrot(qi, x), qrot(qi, y), qrot(qi, z)];
    let w = [[lx[0], lx[1], lx[2], 0.0], [ly[0], ly[1], ly[2], 0.0], [lz[0], lz[1], lz[2], 0.0], [0.0, 0.0, 0.0, 1.0]];
    let sil = |r: f32| {
        let s = r * l / (l2 - r * r).sqrt();
        [r, s, r * r, s * s]
    };
    (w, [0.0, 0.0, l, l2], sil(radius), sil(atmosphere))
}

pub trait GlobeFlow {
    fn shown(&self, key: u32) -> bool;
    fn focusable(&self, key: u32) -> bool;
    fn range_valid(&self, _key: u32) -> bool {
        true
    }
    fn mode(&self) -> i64;
    fn recommended(&self) -> u32;
    fn is_new(&self, key: u32) -> bool;
    fn drop_point(&self, drop: u32) -> (f32, f32);
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneEvents {
    pub range: Option<u32>,
    pub peak: Option<u32>,
    pub drop: Option<u32>,
    pub transitioning: bool,
    pub list_type: Option<i32>,
    pub recommendation: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct GlobeScene {
    pub data: GlobeData,
    pub nodes: crate::game::fe::globe::nodes::NodeMap,
    pub level: Option<u32>,
    pub node: u32,
    pub queue: std::collections::VecDeque<(u32, u32)>,
    pub state: u32,
    pub state_node: u32,
    pub cloud: u32,
    pub cloud_in: f32,
    pub cloud_out: f32,
    pub swap_lighting: bool,
    pub dd: bool,
    pub globe_visible: bool,
    pub midground_visible: bool,
    pub midground_range: u32,
    pub lighting_slot: u32,
    pub transition: i32,
    pub shots: ShotBlend,
    pub peak_shot: Shot,
    pub drop_shot: Shot,
    pub scene_peak: u32,
    pub scene_drop: u32,
    pub dispatches: u32,
    pub drop_index: usize,
    pub drop_count: usize,
    pub drop_level: bool,
    pub pitch: Spring,
    pub yaw: Spring,
    pub focus: u32,
    pub steering: bool,
    pub stick: (f32, f32),
    pub dpad: f32,
    pub follow_focus: bool,
    pub repeat: f32,
    pub peak_focus: u32,
    pub peak_repeat: f32,
    pub drop_focus: u32,
    pub drop_repeat: f32,
    pub highlight: V3,
    pub highlight_off: f32,
    pub highlight_angle: f32,
    pub list: Vec<GlobeEntry>,
    pub time: f32,
    pub light: crate::game::fe::globe::look::LightFade,
    pub postfx: crate::game::fe::globe::look::PostFxLayer,
    pub fades: Vec<(i32, f32, f32)>,
    pub sounds: Vec<i32>,
    pub audio: crate::game::fe::globe::audio::FeAudio,
    pub fade_posted: bool,
    pub clouds: crate::game::fe::globe::clouds::CloudTransition,
}

pub const MENU_FOCUS: u32 = 1;

pub const PITCH_CLAMP: f32 = 0.863_938;

impl GlobeScene {
    pub fn new(data: GlobeData) -> Self {
        let nodes = crate::game::fe::globe::nodes::NodeMap::build(&data, true);
        let mut s = GlobeScene {
            data,
            nodes,
            level: None,
            node: 0,
            queue: std::collections::VecDeque::new(),
            state: 0,
            state_node: 0,
            cloud: 0,
            cloud_in: 0.0,
            cloud_out: 0.0,
            swap_lighting: false,
            dd: false,
            globe_visible: true,
            midground_visible: false,
            midground_range: 0,
            lighting_slot: 0,
            transition: 0,
            shots: ShotBlend::default(),
            peak_shot: Shot::default(),
            drop_shot: Shot::default(),
            scene_peak: 0,
            scene_drop: 0,
            dispatches: 0,
            drop_index: 0,
            drop_count: 0,
            drop_level: false,
            pitch: Spring::default(),
            yaw: Spring::default(),
            focus: 0,
            steering: false,
            stick: (0.0, 0.0),
            dpad: 0.0,
            follow_focus: true,
            repeat: -1.0,
            peak_focus: 0,
            peak_repeat: -1.0,
            drop_focus: 0,
            drop_repeat: -1.0,
            highlight: [0.0; 3],
            highlight_off: 1.0,
            highlight_angle: 0.0,
            list: Vec::new(),
            time: 0.0,
            light: Default::default(),
            postfx: Default::default(),
            fades: Vec::new(),
            sounds: Vec::new(),
            audio: Default::default(),
            fade_posted: false,
            clouds: Default::default(),
        };
        s.clouds = crate::game::fe::globe::clouds::CloudTransition::new(
            s.data
                .globe_particles
                .iter()
                .filter_map(|p| crate::game::fe::globe::clouds::CloudEffect::new(&p.effect))
                .collect(),
        );
        s.shots.current = s.data.globe.shots[3];
        s.yaw.x = -0.0;
        s.wrap_yaw();
        s.dd = true;
        s.set_lighting(0, 0.0);
        s
    }

    pub fn set_lighting(&mut self, slot: u32, time: f32) {
        self.lighting_slot = slot;
        let (key, postfx) = match (slot, self.data.range(self.midground_range)) {
            (1, Some(r)) => {
                if self.dd {
                    (r.lighting_dd, r.postfx_dd.clone())
                } else {
                    (r.lighting, r.postfx.clone())
                }
            }
            _ => {
                let g = &self.data.globe;
                (if self.dd { g.lighting_dd } else { g.lighting }, g.postfx.clone())
            }
        };
        if let Some(p) = self.data.lighting.get(&key) {
            self.light.set(key, p, time);
        }
        if !postfx.is_empty() {
            self.postfx.start(&postfx, time, 1.0);
        }
        self.swap_lighting = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focus_ends_at_the_top() {
        let (lat, lon) = (30f32.to_radians(), 83f32.to_radians());
        let q = globe_rotation(lat, -lon);
        let p = qrot(q, node_direction(lat, lon));
        assert!((p[1] - 1.0).abs() < 1e-5, "{p:?}");
    }

    #[test]
    fn the_spring_settles() {
        let mut s = Spring { omega: 42f32.sqrt(), x: 0.0, v: 0.0, target: 1.0 };
        for _ in 0..300 {
            s.step(1.0 / 30.0);
        }
        assert!((s.x - 1.0).abs() < 1e-4 && s.v.abs() < 1e-3);
    }

    #[test]
    fn the_shot_camera_looks_at_its_target() {
        let s = Shot { offset: [0.0, 5000.0, 100.0], look: [0.0; 3], target: [-1500.0, 500.0, 0.0], fov: 0.87 };
        let c = Camera::from_shot(&s);
        let v = c.view();
        let t = transform(s.target, &v);
        assert!(t[0].abs() < 1e-2 && t[1].abs() < 1e-2 && t[2] < 0.0, "{t:?}");
    }
}
