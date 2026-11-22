use bevy::light::cluster::GlobalClusterSettings;
use bevy::pbr::wireframe::WireframeConfig;
use bevy::prelude::*;

use crate::render::lighting::GameLighting;

const SKY: Color = Color::srgb(0.72, 0.82, 0.93);
const WIREFRAME: Color = Color::srgb(0.08, 0.12, 0.22);
const SUN_DIRECTION: Vec3 = Vec3::new(-0.35, -0.5, -0.78);
const SUN_ILLUMINANCE: f32 = 4_500.0;
const AMBIENT_BRIGHTNESS: f32 = 350.0;

pub struct ScenePlugin {
    pub wireframe: bool,
}

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(SKY))
            .insert_resource(GlobalAmbientLight { color: SKY, brightness: AMBIENT_BRIGHTNESS, ..default() })
            .insert_resource(WireframeConfig { global: self.wireframe, default_color: WIREFRAME, ..default() })
            .add_plugins(crate::render::shadows::GameShadowPlugin)
            .add_systems(
                crate::shell::level::LevelStartup,
                |commands: Commands, lighting: Option<Res<GameLighting>>| spawn_sun(commands, lighting.as_deref()),
            )
            .add_systems(Startup, cpu_clustering)
            .add_systems(Update, toggle_wireframe);
        if let Some(perf) = crate::shell::perf::PerfPlugin::from_env() {
            app.add_plugins(perf);
        }
    }
}

fn spawn_sun(mut commands: Commands, lighting: Option<&GameLighting>) {
    let travel = lighting.map_or(SUN_DIRECTION, |l| -l.sun_offset);
    commands.spawn(crate::render::shadows::sun_components(travel, SUN_ILLUMINANCE));
    commands.spawn(crate::render::shadows::character_light_components(travel));
}

fn cpu_clustering(settings: Option<ResMut<GlobalClusterSettings>>) {
    if let Some(mut settings) = settings {
        settings.gpu_clustering = None;
    }
}

pub(crate) fn toggle_wireframe(keys: Res<ButtonInput<KeyCode>>, mut config: ResMut<WireframeConfig>) {
    if keys.just_pressed(KeyCode::KeyF) {
        config.global = !config.global;
    }
}
