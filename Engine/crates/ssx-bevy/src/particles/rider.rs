use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use bevy::prelude::*;
use ssx_assets::character::Loadout;
use ssx_assets::particles::ParticleNode;
use ssx_assets::particles::rider::rider_particle_nodes;
use ssx_formats::crsf::SceneGraph;

use crate::particles::sim::{EffectInstance, RecordView, SetArena};
use crate::particles::spray::SprayInputs;
use crate::particles::track::{DefCache, textures_of};
use crate::rider::anim::JointRecord;
use ssx_physics::controllers::{ParticleOp, ParticleTarget, Switch, Target};

pub struct RiderSlot {
    pub slot: String,
    pub node: ParticleNode,
    pub joint: Option<usize>,
    pub instance: EffectInstance,
    pub behaviour: Option<std::sync::Arc<crate::particles::inputs::Script>>,
    pub start: f32,
    pub stop: f32,
    pub instant: bool,
}

impl RiderSlot {
    fn on(&mut self, clock: f32) {
        (self.start, self.stop, self.instant) = (clock, -1.0, false);
    }
    fn off(&mut self, clock: f32, instant: bool) {
        (self.start, self.stop, self.instant) = (-1.0, clock, instant);
    }
}

pub struct RiderEffects {
    pub slots: Vec<RiderSlot>,
    seq: u64,
}

impl RiderEffects {
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        root: &Path,
        track: Option<&str>,
        name: &str,
        loadout: &Loadout,
        rig: &SceneGraph,
        arena: &mut SetArena,
        defs: &mut DefCache,
        textures: &mut Vec<String>,
    ) -> Result<RiderEffects> {
        let nodes = rider_particle_nodes(root, track, name, loadout)?.into_iter().map(|n| (n.slot, n.node));
        Self::from_nodes(root, nodes, rig, arena, defs, textures)
    }

    pub fn from_nodes(
        root: &Path,
        nodes: impl IntoIterator<Item = (String, ParticleNode)>,
        rig: &SceneGraph,
        arena: &mut SetArena,
        defs: &mut DefCache,
        textures: &mut Vec<String>,
    ) -> Result<RiderEffects> {
        let mut slots = Vec::new();
        for (slot, node) in nodes {
            let (def, behaviour) = defs.node(root, &node)?;
            textures_of(&def, textures);
            slots.push(RiderSlot {
                slot,
                joint: rig.find(&node.joint_name),
                node,
                instance: EffectInstance::new(def, arena),
                behaviour,
                start: -1.0,
                stop: -1.0,
                instant: false,
            });
        }
        Ok(RiderEffects { slots, seq: 0 })
    }

    pub fn switch(&mut self, clock: f32, ops: &[ParticleOp], inputs: &SprayInputs) -> Vec<String> {
        let before: Vec<(f32, f32)> = self.slots.iter().map(|s| (s.start, s.stop)).collect();
        let seen = self.seq;
        for op in ops.iter().filter(|o| o.seq > seen) {
            for i in self.targets(&op.target) {
                let s = &mut self.slots[i];
                match op.switch {
                    Switch::Activate => s.on(clock),
                    Switch::Deactivate => s.off(clock, false),
                    Switch::DeactivateImmediately => s.off(clock, true),
                    Switch::None => {}
                }
            }
            self.seq = op.seq;
        }
        for s in &mut self.slots {
            if let Some(b) = &s.behaviour {
                crate::particles::inputs::apply_triggers(b, inputs, clock, (&mut s.start, &mut s.stop, &mut s.instant));
            }
        }
        self.slots
            .iter()
            .zip(before)
            .filter(|(s, b)| (s.start, s.stop) != *b)
            .map(|(s, _)| {
                let what = match (s.start != -1.0 && s.stop == -1.0, s.instant) {
                    (true, _) => "on",
                    (false, true) => "off instantly",
                    (false, false) => "off",
                };
                format!("{}@{} {what}", s.instance.def.name, s.node.joint_name)
            })
            .collect()
    }

    fn targets(&self, t: &ParticleTarget) -> Vec<usize> {
        let field = |item: i32| {
            ssx_assets::character::GEAR_SLOTS.iter().find(|g| g.item as i32 == item).map(|g| g.field.to_lowercase())
        };
        (0..self.slots.len())
            .filter(|&i| {
                let s = &self.slots[i];
                match t {
                    ParticleTarget::Track(Target::Node(n)) => {
                        n.is_empty() || ssx_assets::vault_db::fnv1(n) == s.node.key
                    }
                    ParticleTarget::Track(Target::Effect(e)) => {
                        s.node.lynx.as_ref().is_some_and(|l| &l.definition == e)
                    }
                    ParticleTarget::Track(Target::Group(g)) => &s.node.group_name == g,
                    ParticleTarget::GearItem(item) => field(*item).is_some_and(|f| s.slot == f),
                }
            })
            .collect()
    }

    pub fn records(&self, joints: &[JointRecord], inputs: &SprayInputs, precipitation: f32) -> Vec<Option<RecordView>> {
        self.slots
            .iter()
            .map(|s| {
                let j = joints.get(s.joint?)?;
                let (scale, rotation, position) = compose(j, &s.node);
                let blend = crate::particles::inputs::default_blend(s.behaviour.as_deref(), inputs, precipitation);
                Some(RecordView {
                    scale,
                    rotation,
                    position,
                    start: s.start,
                    stop: s.stop,
                    instant_off: s.instant,
                    ao: inputs.ao,
                    blend,
                })
            })
            .collect()
    }

    pub fn summary(&self, arena: &SetArena) -> String {
        let mut per: HashMap<&str, (usize, usize, usize)> = HashMap::new();
        for s in &self.slots {
            let e = per.entry(s.instance.def.name.as_str()).or_default();
            e.0 += (s.instance.state == 0) as usize;
            e.1 += 1;
            e.2 += s.instance.sets.iter().map(|&i| arena.sets[i].particles.len()).sum::<usize>();
        }
        let mut parts: Vec<String> = per.iter().map(|(n, (r, t, p))| format!("{n} {r}/{t} {p}")).collect();
        parts.sort();
        parts.join(", ")
    }
}

pub fn compose(j: &JointRecord, node: &ParticleNode) -> (Vec3, Quat, Vec3) {
    let (ns, nq, np) = (
        Vec4::from_array(node.scale).truncate(),
        Quat::from_array(node.orientation),
        Vec4::from_array(node.position).truncate(),
    );
    (j.s * ns, j.q * nq, j.p + j.q * (j.s * np))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_formats::lynx::ParticleSystem;

    fn slot(slot: &str, group: &str, arena: &mut SetArena) -> RiderSlot {
        let xml = r#"<particleSystem><particleEffect name="e"><particleGroup name="g">
            <particleAction className="ParticleActionKillAge" /></particleGroup></particleEffect></particleSystem>"#;
        let sys = ParticleSystem::parse(xml.as_bytes()).unwrap();
        let def = crate::particles::sim::EffectDef::compile(&sys.effects[0]);
        RiderSlot {
            slot: slot.to_owned(),
            node: ParticleNode {
                key: 0,
                scale: [1.0; 4],
                position: [0.0; 4],
                orientation: [0.0, 0.0, 0.0, 1.0],
                joint_name: "Board".to_owned(),
                group_name: group.to_owned(),
                lynx: None,
                behaviour_path: None,
                unk_77e1adcd: 0,
                locator_type: 1,
                unk_a20a4677: false,
                unk_d8f68fb4: false,
            },
            joint: Some(0),
            instance: EffectInstance::new(def, arena),
            behaviour: None,
            start: -1.0,
            stop: -1.0,
            instant: false,
        }
    }

    fn effects() -> RiderEffects {
        let mut arena = SetArena::default();
        let slots = vec![
            slot("tricky", "character_tricky_flare_group", &mut arena),
            slot("headlamp", "character_headlamp_group", &mut arena),
            slot("common", "character_nis_boardshake_group", &mut arena),
        ];
        RiderEffects { slots, seq: 0 }
    }

    fn op(seq: u64, target: ParticleTarget, switch: Switch) -> ParticleOp {
        ParticleOp { seq, target, switch }
    }

    #[test]
    fn controller_switches_apply_once_in_order() {
        let (mut fx, spray) = (effects(), SprayInputs::default());
        let group = ParticleTarget::Track(Target::Group("character_tricky_flare_group".into()));
        let mut ops =
            vec![op(1, group.clone(), Switch::Activate), op(2, ParticleTarget::GearItem(6), Switch::Activate)];
        fx.switch(1.0, &ops, &spray);
        assert_eq!((fx.slots[0].start, fx.slots[1].start, fx.slots[2].start), (1.0, 1.0, -1.0));
        fx.switch(2.0, &ops, &spray);
        assert_eq!(fx.slots[0].start, 1.0);
        ops.push(op(3, ParticleTarget::GearItem(6), Switch::DeactivateImmediately));
        ops.push(op(4, group, Switch::Deactivate));
        fx.switch(3.0, &ops, &spray);
        assert_eq!((fx.slots[1].start, fx.slots[1].stop, fx.slots[1].instant), (-1.0, 3.0, true));
        assert_eq!((fx.slots[0].start, fx.slots[0].stop, fx.slots[0].instant), (-1.0, 3.0, false));
        ops.push(op(5, ParticleTarget::Track(Target::Node(String::new())), Switch::Activate));
        fx.switch(4.0, &ops, &spray);
        assert!(fx.slots.iter().all(|s| s.start == 4.0));
    }
}
