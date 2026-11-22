use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use ssx_assets::postfx_topogrid::TopogridValues;
use ssx_physics::SIM_HZ;

use super::Ride;
use super::hud::GearHud;
use crate::rider::anim::AnimInput;
use crate::rider::material::{CharacterMaterial, RiderShading};
use crate::rider::skin::{self as rider_skin, RiderRig};
use crate::shell::viewer_hud::HudText;

#[derive(Component)]
pub(super) struct OutroFade;

pub(super) fn spawn_outro_fade(mut commands: Commands) {
    commands.spawn((
        OutroFade,
        BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.0)),
        GlobalZIndex(-1),
        Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
    ));
}

pub(super) fn outro_fade(
    ride: Res<Ride>,
    shell: Option<Res<crate::shell::game_level::LevelGame>>,
    mut fade: Single<&mut BackgroundColor, With<OutroFade>>,
) {
    if shell.is_some() {
        fade.0 = Color::srgba(1.0, 1.0, 1.0, 0.0);
        return;
    }
    let a = ride.outro.as_ref().filter(|o| o.assets.is_some()).map_or(0.0, |_| ride.session.screen.level);
    let a = ride.session.survive.as_ref().map_or(a, |l| a.max(l.fade()));
    fade.0 = Color::srgba(1.0, 1.0, 1.0, a);
}

pub(super) fn world_render(ride: Option<Res<Ride>>, mut cameras: Query<&mut RenderLayers, With<Camera3d>>) {
    let hidden = ride.is_some_and(|r| r.world_hidden);
    let want = if hidden {
        RenderLayers::from_layers(&[crate::render::shadows::CHARACTER_LAYER, crate::ride::helicopter::HELI_LAYER])
    } else {
        RenderLayers::from_layers(&[0, crate::render::shadows::CHARACTER_LAYER])
    };
    for mut l in &mut cameras {
        if *l != want {
            *l = want.clone();
        }
    }
}

pub(super) fn hide_viewer_hud(mut huds: Query<&mut Visibility, With<HudText>>) {
    for mut visibility in &mut huds {
        *visibility = Visibility::Hidden;
    }
}

pub(super) fn toggle_viewer_hud(keys: Res<ButtonInput<KeyCode>>, mut huds: Query<&mut Visibility, With<HudText>>) {
    if keys.just_pressed(KeyCode::KeyH) {
        for mut visibility in &mut huds {
            *visibility = if *visibility == Visibility::Hidden { Visibility::Inherited } else { Visibility::Hidden };
        }
    }
}

pub(super) fn shade_rider(
    ride: Res<Ride>,
    shading: Option<ResMut<RiderShading>>,
    mut materials: ResMut<Assets<CharacterMaterial>>,
) {
    let Some(mut shading) = shading else { return };
    let s = &ride.state;
    use ssx_physics::surface::{AMBIENT_OCCLUSION, SNOW_COVERAGE};
    let active = s.contact.is_some() && !ride.rewind_backwards && s.weights[SNOW_COVERAGE] > 0.0;
    let contact_ao = active.then(|| s.weights[AMBIENT_OCCLUSION]);
    shading.ao_override =
        ride.session.controllers.as_ref().and_then(|c| c.fx.render_attribute("dynamicAOMultOverride")).map(|v| v[0]);
    shading.sample(ride.steps, contact_ao);
    shading.wingsuit_deployed = s.wingsuit.signal;
    static FORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*FORCED.get_or_init(|| std::env::var("SSX_RIDER_GLOW").is_ok_and(|v| v == "on" || v == "1"))
        && let Some(v) = ride.session.controllers.as_ref().and_then(|c| c.fx.render_attribute("dynamicGlowMult"))
    {
        shading.glow = v[1];
    }
    shading.apply(ride.render.traj, &mut materials);
}

pub(super) fn draw_rail_debug(ride: Res<Ride>, mut gizmos: Gizmos) {
    if !ride.rail_debug {
        return;
    }
    let g = &ride.state.grind;
    let mark = |gizmos: &mut Gizmos, p: Vec3, r: f32, c: Color| {
        gizmos.sphere(Isometry3d::from_translation(p), r, c);
    };
    if g.path.valid {
        let (a, b) = (g.path.pos_a, g.path.pos_b);
        mark(&mut gizmos, a, 0.15, Color::srgb(0.0, 1.0, 1.0));
        gizmos.arrow(a, a + g.path.dir_a.normalize_or_zero() * 2.0, Color::srgb(0.0, 1.0, 1.0));
        mark(&mut gizmos, b, 0.15, Color::srgb(0.2, 0.4, 1.0));
        gizmos.arrow(b, b + g.path.dir_b.normalize_or_zero() * 2.0, Color::srgb(0.2, 0.4, 1.0));
        gizmos.line(ride.state.traj, a, Color::srgb(0.0, 0.7, 0.7));
        gizmos.line(a, b, Color::srgb(0.2, 0.4, 1.0));
    }
    if let Some(d) = &g.debug {
        if d.rail {
            mark(&mut gizmos, d.rail_point, 0.2, Color::srgb(1.0, 1.0, 0.0));
        }
        mark(&mut gizmos, d.selected, 0.12, Color::srgb(0.0, 1.0, 0.0));
        mark(&mut gizmos, d.target, 0.1, Color::srgb(1.0, 0.0, 1.0));
    }
    gizmos.line(g.tail_pos, ride.state.traj, Color::WHITE);
}

pub(super) fn animate_rider(ride: Res<Ride>, rig: Option<ResMut<RiderRig>>, mut transforms: Query<&mut Transform>) {
    let Some(mut rig) = rig else { return };
    let frac = (ride.render_time * SIM_HZ - (ride.steps as f32 - 1.0)).clamp(0.0, 1.0);
    let input = AnimInput { state: &ride.state, frac, roots: (ride.prev.traj, ride.curr.traj) };
    rider_skin::animate(&mut rig, &input, &mut transforms);
}

pub(super) fn light_gear(
    ride: Res<Ride>,
    rig: Option<Res<RiderRig>>,
    mut lights: ResMut<crate::render::lighting::local::DynamicLocalLights>,
    mut hud: ResMut<GearHud>,
) {
    let s = &ride.state;
    let fx = ride.session.controllers.as_ref().map(|c| &c.fx);
    let equipped = ride.world.survival.slots.get(6).copied().unwrap_or(false);
    let battery = s.survival.head_lamp_battery;
    let lamp = fx.and_then(ssx_physics::controllers::PlayerFx::head_lamp);
    hud.lamp = match (equipped, lamp) {
        (false, _) => String::new(),
        (true, Some(f)) => format!("\nhead lamp: on (HeadLampGearTrack), f = {f:.3} (battery {battery:.2})"),
        (true, None) => format!("\nhead lamp: off (battery {battery:.2})"),
    };
    lights.lights.clear();
    let (Some(rig), Some(fx)) = (rig, fx) else { return };
    for (g, r) in rig.gear_lights.iter().zip(&rig.drawn_gear_lights) {
        let factor = if g.slot == "headlamp" {
            lamp
        } else {
            fx.lights.values().find(|l| !l.head_lamp && l.on && l.group && l.name == g.group).map(|l| l.factor)
        };
        let Some(factor) = factor.filter(|_| g.light.enable) else { continue };
        let world = bevy::math::Affine3A::from_scale_rotation_translation(r.s, r.q, r.p);
        lights.lights.push(crate::render::lighting::local::DynamicLight { world, light: g.light.clone(), factor });
    }
}

#[derive(Resource, Default)]
pub struct TerrainRipple {
    pub(super) tuning: Option<ssx_physics::controllers::ripple::RippleTuning>,
    pub(super) ripple: ssx_physics::controllers::ripple::Ripple,
    pub(super) seen: u32,
    pub params: ssx_physics::controllers::ripple::RippleParams,
}

pub(super) fn terrain_ripple(time: Res<Time>, ride: Res<Ride>, mut ripple: ResMut<TerrainRipple>) {
    let r = &mut *ripple;
    let Some(t) = r.tuning else { return };
    if let Some(fx) = ride.session.controllers.as_ref().map(|c| &c.fx) {
        if fx.ripples < r.seen {
            r.seen = 0;
        }
        if fx.ripples > r.seen {
            r.seen = fx.ripples;
            r.ripple.start(fx.ripple.unwrap_or_default());
        }
    }
    let dt = if ride.hold { 0.0 } else { time.delta_secs() };
    r.params = r.ripple.update(&t, dt);
}

pub(super) fn screen_effects(
    ride: Res<Ride>,
    layers: Option<ResMut<crate::render::post::layers::PostFxLayers>>,
    (effects, overlay): (
        Option<Res<crate::render::post::overlay::ScreenEffects>>,
        Option<ResMut<crate::render::post::overlay::PostOverlay>>,
    ),
    (topogrid, post_topogrid): (
        Option<Res<crate::render::post::topogrid::TopogridLevel>>,
        Option<ResMut<crate::render::post::topogrid::PostTopogrid>>,
    ),
    mut cube: crate::render::post::cube::CubeTarget,
    mut hud: ResMut<GearHud>,
) {
    let Some(mut layers) = layers else { return };
    let forced = std::env::var("SSX_SCREEN_EFFECTS").ok().and_then(|s| {
        let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        <[f32; 3]>::try_from(v).ok()
    });
    let forced_service;
    let service = match (forced, ride.session.controllers.as_ref()) {
        (Some([h, t, o]), Some(c)) => {
            let mut copy = ride.session.postfx.clone();
            for (preset, (id, inverted)) in &c.fx.modulated {
                let v = match ssx_physics::objectives::game_state_name(*id) {
                    "FLT_VignetteHealth" => h,
                    "FLT_VignetteTemperature" => t,
                    "FLT_VignetteOxygen" => o,
                    _ => continue,
                }
                .clamp(0.0, 1.0);
                copy.set_weight(preset, if *inverted { 1.0 - v } else { v });
            }
            forced_service = copy;
            &forced_service
        }
        _ => &ride.session.postfx,
    };
    let pulse = ride.session.controllers.as_ref().map_or(0.0, |c| c.fx.goggles_pulse);
    let values = layers.scene.frame(service);
    let cubes = cube.frame(&values.colour_cubes);
    let mut screen = String::new();
    if let (Some(effects), Some(mut overlay)) = (effects, overlay) {
        let state = ssx_assets::postfx_overlay::OverlayState::of(values);
        overlay.uniform.set(state.shader_params());
        let [i1, i2] = state.intensity;
        screen = format!(
            " ({:?}): overlay_1_intensity {i1:.2}, overlay_2_intensity {i2:.2}{}",
            effects.overlay.drop,
            if i1 > 0.0 || i2 > 0.0 { " OVERLAY" } else { "" }
        );
    }
    if let (Some(_), Some(mut post)) = (topogrid, post_topogrid) {
        let params = TopogridValues::of(values).shader_params(pulse, ride.render.traj.to_array());
        if params.is_some() {
            screen.push_str(" TOPOGRID");
        }
        post.uniform.set(params);
    }
    hud.screen = format!("\nscreen fx{screen}{cubes}");
}
