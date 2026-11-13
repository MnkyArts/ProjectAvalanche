use bevy::prelude::{Assets, Mat3, Mat4, Query, ResMut, Transform, Vec3, Visibility, debug, info};
use ssx_assets::career::random::GameRandom;
use ssx_assets::character::model_matrix;
use ssx_assets::ghosts::GhostRig;

use super::{BotVisual, record_transform};
use crate::rider::anim::JointRecord;
use crate::rider::material::CharacterMaterial;

pub(in crate::ride::bots) fn board_transform(rig: &GhostRig, board: usize, w: &JointRecord) -> Transform {
    let m = Mat4::from_scale_rotation_translation(w.s, w.q, w.p);
    let b0 = Mat4::from_cols_array_2d(&model_matrix(&rig.rig, &rig.bind, board));
    Transform::from_matrix(m * b0.inverse())
}

#[derive(bevy::ecs::system::SystemParam)]
pub(in crate::ride::bots) struct RacerDraw<'w, 's> {
    lights: ResMut<'w, crate::render::lighting::local::DynamicLocalLights>,
    transforms: Query<'w, 's, &'static mut Transform>,
    visibility: Query<'w, 's, &'static mut Visibility>,
    characters: ResMut<'w, Assets<CharacterMaterial>>,
}

#[derive(Clone, Copy)]
pub(in crate::ride::bots) struct FrameFacts {
    pub(in crate::ride::bots) view: Option<(Vec3, f32)>,
    pub(in crate::ride::bots) night: bool,
    pub(in crate::ride::bots) steps: u64,
}

impl RacerDraw<'_, '_> {
    pub(in crate::ride::bots) fn hide(&mut self, v: &BotVisual) {
        if let Ok(mut vis) = self.visibility.get_mut(v.root) {
            *vis = Visibility::Hidden;
        }
    }

    pub(in crate::ride::bots) fn holo(
        &mut self,
        v: &mut BotVisual,
        dt_s: f32,
        d2: f32,
        racers: usize,
        rng: &mut dyn GameRandom,
    ) {
        let holo = v.fade.update(dt_s, d2, racers, |max| rng.range_f(0.0, max));
        for m in &v.materials {
            if let Some(mut m) = self.characters.get_mut(m)
                && m.extension.params.holo != holo
            {
                m.extension.params.holo = holo;
            }
        }
    }

    pub(in crate::ride::bots) fn place(
        &mut self,
        who: &str,
        v: &mut BotVisual,
        records: &[JointRecord],
        facts: FrameFacts,
        lamp_on: bool,
    ) -> (Option<JointRecord>, Option<JointRecord>) {
        if let Ok(mut vis) = self.visibility.get_mut(v.root) {
            *vis = Visibility::Inherited;
        }
        // TODO(guess): the bounds are the LOD-0 bind-pose box placed by the drawn `AITrajectory`. The game's (the
        // render object's update 0x829CA1B8, `VisManager_SetBounds` 0x82D46048 type 1) are the min/max union of (a) the
        // palette joints' world record positions over all LODs' meshes (0x829F2630) and (b) the game object's model box
        // (its LOD set +0x40 centre, +0x50 half extents, +0x60 rotation) moved to (a)'s centre and placed by
        // M(state +0x90 = entity +0x960, the transform the entity's +0x940 object gives, 0x829C83B0) · M(state +0x40 =
        // the entity's own S/Q/P at +0x40/+0x30/+0x20, 0x829F06A0). Open: those two transforms for a racer's entity
        // (`Racers_CreateEntity` 0x82503600) — with world joints, a non-identity one moves (b) away from the racer.
        let traj = v.rig.rig.find("AITrajectory").and_then(|i| records.get(i)).copied();
        let metric = facts.view.zip(traj).map(|((eye, tan_min), t)| {
            let (lo, hi) = v.bounds;
            let (c, h) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
            let m = Mat3::from_quat(t.q);
            let h_world = Vec3::new(m.row(0).abs().dot(h), m.row(1).abs().dot(h), m.row(2).abs().dot(h));
            let centre = t.p + t.q * c;
            let d = centre.distance(eye);
            if d > 0.0 { h_world.length() / (d * tan_min) } else { f32::INFINITY }
        });
        let lod = match metric {
            Some(m) => (m > 0.002).then(|| v.thresholds.iter().position(|&th| th <= m.max(0.0))).flatten(),
            None => Some(0),
        };
        let lod = lod.filter(|&k| k < v.lods.len());
        if lod != v.lod {
            debug!("{who} ({}): LOD {lod:?} (size metric {metric:.3?}) at {:.1?}", v.name, traj.map(|t| t.p));
            v.lod = lod;
        }
        for (i, &e) in v.lods.iter().enumerate() {
            if let Ok(mut vis) = self.visibility.get_mut(e) {
                *vis = if Some(i) == lod { Visibility::Inherited } else { Visibility::Hidden };
            }
        }
        for (&entity, r) in v.nodes.iter().zip(records) {
            if let Ok(mut tr) = self.transforms.get_mut(entity) {
                *tr = record_transform(r);
            }
        }
        if let (Some(e), Some(board)) = (v.board, v.rig.board)
            && let Some(w) = records.get(board)
            && let Ok(mut tr) = self.transforms.get_mut(e)
        {
            *tr = board_transform(&v.rig, board, w);
        }
        let board = v.rig.board.and_then(|i| records.get(i)).copied();
        if lamp_on != v.lamp_on {
            info!("{who} ({}): head lamp {}", v.name, if lamp_on { "on (BotHeadlampOn)" } else { "off" });
            v.lamp_on = lamp_on;
        }
        if lamp_on && !crate::render::post::env_is("SSX_BOT_LAMP", "off") {
            for lamp in lod.and_then(|i| v.lamps.get(i)).into_iter().flatten() {
                let Some(joint) = records.get(lamp.joint).filter(|_| lamp.light.enable) else { continue };
                let r = crate::rider::skin::compose(joint, &lamp.local);
                if facts.steps.is_multiple_of(30) {
                    debug!("{who} ({}): lamp at {:.1} towards {:.2}", v.name, r.p, r.q * Vec3::NEG_Z);
                }
                self.lights.lights.push(crate::render::lighting::local::DynamicLight {
                    world: bevy::math::Affine3A::from_scale_rotation_translation(r.s, r.q, r.p),
                    light: lamp.light.clone(),
                    factor: 1.0,
                });
            }
        }
        (board, traj)
    }
}
