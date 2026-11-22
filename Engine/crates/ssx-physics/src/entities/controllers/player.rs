use std::collections::BTreeMap;

use ssx_assets::postfx_state::GOGGLES_PRIORITY;
use ssx_lua::game::controllers::{ControllerWorld, Phase, Track, TrackCall};
use ssx_lua::game::objectives::{GsValue, Name};

use super::{Ctx, InstState, ParticleTarget, PlayerFx, Switch, apply_light, as_float, player_name, push_op};
use crate::objectives::ObjectiveWorld;

pub(super) struct PlayerWorld<'a, 'b, 'c> {
    pub ctx: &'a mut Ctx<'b, 'c>,
    pub fx: &'a mut PlayerFx,
    pub inst: &'a mut BTreeMap<u64, InstState>,
    pub log: Option<&'a mut Vec<String>>,
}

const AO_WINDOW: usize = 45;

const GOGGLES_BATTERY: u16 = 0xC4;

fn ao_blend(start: f32, target: f32, elapsed: f32, blend: f32) -> f32 {
    let r = elapsed / blend;
    let t = if r.is_nan() { 1.0 } else { r.clamp(0.0, 1.0) };
    t * (target - start) + start
}

impl ControllerWorld for PlayerWorld<'_, '_, '_> {
    fn game_state(&mut self, id: u16) -> Option<GsValue> {
        self.ctx.sw.game_state(&player_name(), id)
    }
    fn gear_item(&mut self, item: i32) -> Option<bool> {
        Some(usize::try_from(item).ok().and_then(|i| self.ctx.sw.w.survival.slots.get(i)).copied().unwrap_or(false))
    }
    fn gear_stat(&mut self, stat: i32) -> Option<i32> {
        let v = usize::try_from(stat).ok().and_then(|i| self.ctx.sw.w.survival.stats.get(i)).copied();
        Some(v.unwrap_or(0.0) as i32)
    }
    fn named_flag(&mut self, n: &Name) -> bool {
        self.ctx.flags.check(n)
    }
    fn posted_event(&mut self, e: &Name) -> bool {
        self.ctx.events.posted(e, Some(&player_name()))
    }
    fn global_posted_event(&mut self, i: i32) -> bool {
        usize::try_from(i).is_ok_and(|i| self.ctx.events.global(i))
    }
    fn player_distance(&mut self) -> Option<f32> {
        Some(0.0)
    }
    fn track(&mut self, c: TrackCall<'_>, phase: Phase) {
        if let Some(log) = self.log.as_deref_mut()
            && !matches!(phase, Phase::Update(_))
        {
            log.push(format!("{} {} {phase:?}", super::PLAYER, c.def.kind));
        }
        let s = &mut *self.ctx.sw.s;
        let fx = &mut *self.fx;
        match (c.kind, phase) {
            (Track::Oxygen, Phase::Begin | Phase::End) => {
                let on = phase == Phase::Begin;
                (fx.oxygen, s.survival.low_oxygen) = (on, on);
            }
            (Track::Temperature, Phase::Begin | Phase::End) => {
                let on = phase == Phase::Begin;
                (fx.temperature, s.survival.temperature_enabled) = (on, on);
            }
            (Track::GameState { id, value }, Phase::Begin) => self.ctx.sw.write(&player_name(), *id, *value),
            (Track::Particle { target, on_start, .. }, Phase::Begin) if *on_start != Switch::None => {
                push_op(&mut fx.particles, &mut fx.next_seq, ParticleTarget::Track(target.clone()), *on_start)
            }
            (Track::Particle { target, on_end, .. }, Phase::End) if *on_end != Switch::None => {
                push_op(&mut fx.particles, &mut fx.next_seq, ParticleTarget::Track(target.clone()), *on_end)
            }
            (Track::ParticleFromGear { item, .. }, Phase::Begin) => {
                push_op(&mut fx.particles, &mut fx.next_seq, ParticleTarget::GearItem(*item), Switch::Activate)
            }
            (Track::ParticleFromGear { item, instant }, Phase::End) => {
                let sw = if *instant { Switch::DeactivateImmediately } else { Switch::Deactivate };
                push_op(&mut fx.particles, &mut fx.next_seq, ParticleTarget::GearItem(*item), sw)
            }
            (Track::Light { .. }, _) => {
                let battery = s.survival.head_lamp_battery;
                apply_light(&mut fx.lights, c, phase, battery, fx.head_lamp_scale)
            }
            (Track::PostFxModulated { preset, id, inverted }, Phase::Update(_)) => {
                let v = as_float(self.ctx.sw.game_state(&player_name(), *id)).clamp(0.0, 1.0);
                let weight = if *inverted { 1.0 - v } else { v };
                if self.inst.contains_key(&c.inst) {
                    self.ctx.sw.postfx.set_weight(&preset.text, weight);
                } else {
                    self.ctx.sw.postfx.start(&preset.text, 0.0, weight);
                    self.inst.insert(c.inst, InstState::PostFxStarted);
                    self.fx.modulated.insert(preset.text.to_string(), (*id, *inverted));
                }
            }
            (Track::PostFxModulated { preset, .. }, Phase::End) => {
                if self.inst.remove(&c.inst).is_some() {
                    self.ctx.sw.postfx.stop(&preset.text, 0.0);
                    self.fx.modulated.remove(&*preset.text);
                }
            }
            (Track::PostFx { preset, blend_in, weight, .. }, Phase::Begin) => {
                if let Some(handle) = self.ctx.sw.postfx.request(&preset.text, *blend_in, *weight) {
                    self.inst.insert(c.inst, InstState::PostFx { handle });
                }
            }
            (Track::PostFx { blend_out, .. }, Phase::End) => {
                if let Some(InstState::PostFx { handle }) = self.inst.remove(&c.inst) {
                    self.ctx.sw.postfx.release(handle, *blend_out);
                }
            }
            (Track::RenderAttribute { .. }, _) => self.render_attribute(c, phase),
            (Track::Ao { .. } | Track::AoGroundContact { .. }, _) => self.ao(c, phase),
            (Track::Ripple, Phase::Begin) => (fx.ripples, fx.ripple) = (fx.ripples + 1, Some(s.traj)),
            (Track::PulseGogglesGear { .. }, _) => self.pulse_goggles(c, phase),
            _ => {}
        }
    }
}

impl PlayerWorld<'_, '_, '_> {
    fn pulse_goggles(&mut self, c: TrackCall<'_>, phase: Phase) {
        use ssx_assets::postfx_topogrid::{GogglesDef, GogglesTrack, Modification};
        let Track::PulseGogglesGear { blend_in, blend_out, outer_range, static_threshold, modifications, .. } = c.kind
        else {
            return;
        };
        match phase {
            Phase::Begin => {
                let Some(preset) = self.ctx.sw.w.survival.goggles_preset.clone() else {
                    self.fx.goggles = Some(GogglesTrack::default());
                    return;
                };
                let mods: Vec<Modification> = modifications
                    .iter()
                    .map(|m| Modification {
                        field: m.field.clone(),
                        stat: self.gear_stat(m.stat).unwrap_or(0),
                        x: m.x,
                        y: m.y,
                    })
                    .collect();
                let copy = ssx_formats::attrib::fnv1(format!("modified_{:08x}", self.fx.goggles_copies).as_bytes());
                self.fx.goggles_copies = self.fx.goggles_copies.wrapping_add(1);
                let whiteout = self.ctx.sw.w.survival.whiteout_lighting;
                let g = GogglesTrack::begin(preset, &mods, copy, whiteout);
                let handle =
                    self.ctx.sw.postfx.request_state(g.key, g.holder.clone(), GOGGLES_PRIORITY, *blend_in, 1.0);
                self.inst.insert(c.inst, InstState::PostFx { handle });
                self.fx.goggles = Some(g);
            }
            Phase::Update(dt) => {
                let battery = as_float(self.ctx.sw.game_state(&player_name(), GOGGLES_BATTERY));
                let Some(g) = self.fx.goggles.as_mut() else { return };
                let def = GogglesDef { outer: *outer_range, threshold: *static_threshold };
                self.fx.goggles_pulse = g.update(dt, battery, def);
                if g.key != 0 {
                    self.ctx.sw.postfx.reload(g.key, GOGGLES_PRIORITY, g.holder.clone());
                }
            }
            Phase::End => {
                self.fx.goggles = None;
                if let Some(InstState::PostFx { handle }) = self.inst.remove(&c.inst) {
                    self.ctx.sw.postfx.release(handle, *blend_out);
                }
            }
        }
    }

    fn render_attribute(&mut self, c: TrackCall<'_>, phase: Phase) {
        let Track::RenderAttribute { name, blend, value, reset_on_end } = c.kind else { return };
        let key = name.text.to_string();
        match phase {
            Phase::Begin => {
                let kept = self.fx.render_attributes.get(&key).copied();
                self.inst.insert(c.inst, InstState::RenderAttribute { kept, elapsed: 0.0 });
            }
            Phase::Update(dt) => {
                let v = match self.inst.get_mut(&c.inst) {
                    Some(InstState::RenderAttribute { kept: Some(k), elapsed }) if *blend > *elapsed => {
                        *elapsed += dt;
                        let t = (*elapsed / *blend).min(1.0);
                        std::array::from_fn(|i| k[i] + (value[i] - k[i]) * t)
                    }
                    _ => *value,
                };
                self.fx.render_attributes.insert(key, v);
            }
            Phase::End => {
                if let Some(InstState::RenderAttribute { kept, .. }) = self.inst.remove(&c.inst)
                    && *reset_on_end
                {
                    match kept {
                        Some(k) => self.fx.render_attributes.insert(key, k),
                        None => self.fx.render_attributes.remove(&key),
                    };
                }
            }
        }
    }

    fn ao(&mut self, c: TrackCall<'_>, phase: Phase) {
        let fx_ao = self.fx.world_ao;
        match (c.kind, phase) {
            (Track::Ao { .. }, Phase::Begin) => {
                self.inst.insert(c.inst, InstState::Ao { elapsed: 0.0, start: fx_ao });
            }
            (Track::Ao { ao, blend }, Phase::Update(dt)) => {
                if let Some(InstState::Ao { elapsed, start }) = self.inst.get_mut(&c.inst) {
                    *elapsed += dt;
                    self.fx.world_ao = ao_blend(*start, *ao, *elapsed, *blend);
                }
            }
            (Track::AoGroundContact { .. }, Phase::Begin) => {
                let samples = Default::default();
                self.inst.insert(c.inst, InstState::AoGroundContact { samples, elapsed: 0.0, start: fx_ao });
            }
            (Track::AoGroundContact { blend }, Phase::Update(dt)) => {
                use crate::surface::{AMBIENT_OCCLUSION, SNOW_COVERAGE};
                let s = &*self.ctx.sw.s;
                let active = s.contact.is_some() && !self.ctx.sw.backwards && s.weights[SNOW_COVERAGE] > 0.0;
                let sample = if active { (s.weights[AMBIENT_OCCLUSION], 2.0) } else { (0.5, 1.0) };
                if let Some(InstState::AoGroundContact { samples, elapsed, start }) = self.inst.get_mut(&c.inst) {
                    *elapsed += dt;
                    samples.push_back(sample);
                    while samples.len() > AO_WINDOW {
                        samples.pop_front();
                    }
                    let (sw, swv) = samples.iter().fold((0.0, 0.0), |(w, wv), &(v, x)| (w + x, wv + v * x));
                    let mean = if sw > 0.0 { (swv / sw).clamp(0.0, 1.0) } else { 0.0 };
                    self.fx.world_ao = ao_blend(*start, mean, *elapsed, *blend);
                }
            }
            (Track::Ao { .. } | Track::AoGroundContact { .. }, Phase::End) => {
                self.inst.remove(&c.inst);
            }
            _ => {}
        }
    }
}
