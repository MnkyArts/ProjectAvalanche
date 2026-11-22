// PORT(choice): the benchmark is the port's own tool (v0.4.2, the performance update): fixed scenes and their
// frame times, the adapter, the system and the settings in one file a player can send; the console has no benchmark
// (one machine, one renderer).

pub mod driver;
pub mod record;
pub mod report;

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Camera {
    Chase,
    Flight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    FrontEnd { script: &'static str, from: f64, seconds: f64 },
    Ride { drop: &'static str, after_go: f32, seconds: f32, camera: Camera },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    pub id: &'static str,
    pub title: &'static str,
    pub kind: Kind,
}

pub const FLIGHT_FRAMES_PER_SECOND: f32 = 60.0;

const AFTER_GO: f32 = 5.0;
const RIDE_SECONDS: f32 = 30.0;

pub const SCENES: [Scene; 8] = [
    Scene {
        id: "fe",
        title: "the front end: the Project Avalanche menu over the turning 3-D globe",
        kind: Kind::FrontEnd { script: "7:start,9.5:a", from: 15.0, seconds: 30.0 },
    },
    Scene {
        id: "alps",
        title: "alps0101, Vorassay drop 1: an Explore race at night (exp_alps_vorassay_drop_1)",
        kind: Kind::Ride {
            drop: "exp_alps_vorassay_drop_1",
            after_go: AFTER_GO,
            seconds: RIDE_SECONDS,
            camera: Camera::Chase,
        },
    },
    Scene {
        id: "whitehorn",
        title: "rockies0701, Whitehorn drop 1: the Explore race (exp_rockies_whitehorn_drop_1_race)",
        kind: Kind::Ride {
            drop: "exp_rockies_whitehorn_drop_1_race",
            after_go: AFTER_GO,
            seconds: RIDE_SECONDS,
            camera: Camera::Chase,
        },
    },
    Scene {
        id: "heli",
        title: "siberia0401, Boris drop 4: the helicopter trick drop (exp_siberia_boris_drop_4_trick)",
        kind: Kind::Ride {
            drop: "exp_siberia_boris_drop_4_trick",
            after_go: 25.0,
            seconds: RIDE_SECONDS,
            camera: Camera::Chase,
        },
    },
    Scene {
        id: "eddie",
        title: "rockies0706, Mount Eddie drop 2 (DLC): a trick drop (exp_dlc_mnt_eddie_drop_2_trick)",
        kind: Kind::Ride {
            drop: "exp_dlc_mnt_eddie_drop_2_trick",
            after_go: AFTER_GO,
            seconds: RIDE_SECONDS,
            camera: Camera::Chase,
        },
    },
    Scene {
        id: "deathzone",
        title: "himeast1001, Lhotse drop 1: Death Zone, a survive drop (exp_himeast_lhotse_drop_1_survive)",
        kind: Kind::Ride {
            drop: "exp_himeast_lhotse_drop_1_survive",
            after_go: AFTER_GO,
            seconds: RIDE_SECONDS,
            camera: Camera::Chase,
        },
    },
    Scene {
        id: "night",
        title: "himeast1001 at night: the career's chapter 11 head-to-head, head lamps and snowfall (chapter_11_drop_h2h)",
        kind: Kind::Ride {
            drop: "chapter_11_drop_h2h",
            after_go: AFTER_GO,
            seconds: RIDE_SECONDS,
            camera: Camera::Chase,
        },
    },
    Scene {
        id: "flight",
        title: "rockies0701, Whitehorn drop 1: a camera flight along the first rival's recorded ride, 20 m above it",
        kind: Kind::Ride {
            drop: "exp_rockies_whitehorn_drop_1_race",
            after_go: AFTER_GO,
            seconds: RIDE_SECONDS,
            camera: Camera::Flight,
        },
    },
];

pub fn scene(id: &str) -> Option<&'static Scene> {
    SCENES.iter().find(|s| s.id == id)
}

pub fn scene_names() -> String {
    SCENES.iter().map(|s| s.id).collect::<Vec<_>>().join(", ")
}

#[derive(Clone, Debug)]
pub struct SceneRun {
    pub scene: &'static Scene,
    pub part: PathBuf,
    pub gpu_marks: bool,
}

impl SceneRun {
    pub fn new(id: &str, part: PathBuf) -> anyhow::Result<SceneRun> {
        let scene =
            scene(id).ok_or_else(|| anyhow::anyhow!("no benchmark scene {id:?} (scenes: {})", scene_names()))?;
        let gpu_marks = !crate::render::post::env_is("SSX_BENCH_GPU_MARKS", "off");
        Ok(SceneRun { scene, part, gpu_marks })
    }
}

#[cfg(test)]
mod tests;
