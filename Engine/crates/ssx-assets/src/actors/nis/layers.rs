use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use ssx_formats::ant::Value;
use ssx_formats::ant_anim::{Pose, nlerp, rig_hierarchies};

use super::{ElementClock, NisClip, Walk};
use crate::anim_db::Asset;
use crate::anim_graph::LoopFields;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayerClock {
    Node,
    Element(ElementClock),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayerSource {
    Clip(NisClip),
    BlendArray {
        signal: String,
        signal_default: f32,
        clamp: [f32; 2],
        damping: [f32; 2],
        range: [f32; 2],
        items: Vec<(f32, NisClip)>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct NisLayer {
    pub name: String,
    pub clock: LayerClock,
    pub length: f32,
    pub blend_mode: i64,
    pub mask: Option<HashMap<String, f32>>,
    pub source: LayerSource,
}

impl NisLayer {
    pub fn time(&self, t: f32) -> f32 {
        if self.length > 0.0 { t.rem_euclid(self.length) } else { 0.0 }
    }

    fn items(&self, v: f32) -> Vec<(&NisClip, f32)> {
        match &self.source {
            LayerSource::Clip(c) => vec![(c, 1.0)],
            LayerSource::BlendArray { clamp, damping, range, items, .. } => {
                let cur = crate::anim_leaf::blend_array_damp(v, v, *clamp, *damping, 0.0);
                let pos = cur.clamp(range[0].min(range[1]), range[0].max(range[1]));
                let xs: Vec<f32> = items.iter().map(|i| i.0).collect();
                let picked = crate::anim_leaf::blend_array_select(&xs, pos);
                picked.into_iter().filter_map(|(k, w)| Some((&items.get(k)?.1, w))).collect()
            }
        }
    }

    fn own<T>(
        &self,
        t: f32,
        v: f32,
        get: impl Fn(&NisClip, &Pose) -> Option<T>,
        mix: impl Fn(T, T, f32) -> T,
    ) -> Option<T> {
        let mut acc: Option<T> = None;
        for (clip, w) in self.items(v) {
            let b = clip.sample(t).ok().and_then(|p| get(clip, &p));
            acc = match (acc, b) {
                (Some(a), Some(b)) => Some(mix(a, b, w)),
                (a, b) => b.or(a),
            };
        }
        acc
    }

    fn weight(&self, dof: &str) -> f32 {
        self.mask.as_ref().map_or(1.0, |m| m.get(dof).copied().unwrap_or(0.0))
    }

    pub fn over_quat(&self, t: f32, v: f32, dof: &str, base: Option<[f32; 4]>) -> Option<[f32; 4]> {
        let layer = self.own(t, v, |c, p| c.quat(p, dof), nlerp);
        let w = self.weight(dof);
        match (self.blend_mode, base, layer) {
            (0, Some(a), Some(b)) => Some(nlerp(a, b, w)),
            (0, None, Some(b)) => Some(b),
            (1 | 2, Some(a), Some(b)) => {
                let b = if self.blend_mode == 2 { [-b[0], -b[1], -b[2], b[3]] } else { b };
                Some(crate::rigamate::quat_mul(a, nlerp([0.0, 0.0, 0.0, 1.0], b, w)))
            }
            (_, a, _) => a,
        }
    }

    pub fn over_vec3(&self, t: f32, v: f32, dof: &str, base: Option<[f32; 3]>) -> Option<[f32; 3]> {
        let lerp = |a: [f32; 3], b: [f32; 3], w: f32| -> [f32; 3] { std::array::from_fn(|k| a[k] + (b[k] - a[k]) * w) };
        let layer = self.own(t, v, |c, p| c.vec3(p, dof), lerp);
        let w = self.weight(dof);
        match (self.blend_mode, base, layer) {
            (0, Some(a), Some(b)) => Some(lerp(a, b, w)),
            (0, None, Some(b)) => Some(b),
            (1 | 2, Some(a), Some(b)) => {
                let w = if self.blend_mode == 2 { -w } else { w };
                Some(std::array::from_fn(|k| a[k] + w * b[k]))
            }
            (_, a, _) => a,
        }
    }

    pub fn over_float(&self, t: f32, v: f32, dof: &str, base: Option<f32>) -> Option<f32> {
        let layer = self.own(t, v, |c, p| c.float(p, dof), |a, b, w| a + (b - a) * w);
        let w = self.weight(dof);
        match (self.blend_mode, base, layer) {
            (0, Some(a), Some(b)) => Some(a + (b - a) * w),
            (0, None, Some(b)) => Some(b),
            (1 | 2, Some(a), Some(b)) => Some(a + if self.blend_mode == 2 { -w } else { w } * b),
            (_, a, _) => a,
        }
    }
}

impl<'a> Walk<'a> {
    pub(super) fn layers_for(
        &self,
        role: i64,
        found: &[(Asset<'a>, usize)],
        clock: LayerClock,
    ) -> Result<Vec<NisLayer>> {
        let mut out = Vec::new();
        for (layers, i) in found.iter().rev() {
            if let Some(l) = self.layer(layers, *i, role, clock)? {
                out.push(l);
            }
        }
        Ok(out)
    }

    pub(super) fn upper_layers(a: &Asset<'a>) -> Vec<(Asset<'a>, usize)> {
        (1..a.array("ControllerAssets").len()).map(|i| (*a, i)).collect()
    }

    fn layer(&self, layers: &Asset<'a>, i: usize, role: i64, clock: LayerClock) -> Result<Option<NisLayer>> {
        let name = layers.name();
        let at = |f: &str| layers.array(f).get(i);
        let int = |f: &str| at(f).and_then(Value::as_i64).unwrap_or(-1);
        let guid = |f: &str| at(f).and_then(Value::as_guid).filter(|g| !g.is_null());
        anyhow::ensure!(guid("SignalAssets").is_none(), "{name}: layer {i} weighted by a signal (not ported)");
        let (update, rate, blend, branch_in) =
            (int("UpdateModes"), int("UpdateRates"), int("BlendModes"), int("BranchInModes"));
        anyhow::ensure!(
            update == 1 && rate == 0 && branch_in == 0 && (0..=2).contains(&blend),
            "{name}: layer {i} update {update} rate {rate} blend {blend} branch-in {branch_in} (not ported)"
        );
        let ctrl =
            guid("ControllerAssets").and_then(|g| self.db.get(g)).with_context(|| format!("{name}: layer {i}"))?;
        anyhow::ensure!(ctrl.is("LoopingControllerAsset"), "{name}: layer {i} is a {} (not ported)", ctrl.class());
        let fields = LoopFields::of_asset(&ctrl);
        anyhow::ensure!(
            !fields.loop_control && fields.start == 0.0 && fields.loop_start == 0.0,
            "{}: loop control or a start (not ported)",
            ctrl.name()
        );
        let subject =
            ctrl.guid("SubjectControllerAsset").and_then(|g| self.db.get(g)).context("layer loop without subject")?;
        let rig = self.rig(role);
        let tree = rig_hierarchies(self.all, &rig.name)?;
        let bound = |c: &Asset| c.guid("Target").is_some_and(|t| tree.contains(&t));
        let mut clips: Vec<(Asset<'a>, NisClip)> = Vec::new();
        let source = match subject.class() {
            "ClipControllerAsset" => {
                if !bound(&subject) {
                    return Ok(None);
                }
                let clip = NisClip::load(self.db, self.all, &subject, &rig.name)?;
                clips.push((subject, clip.clone()));
                LayerSource::Clip(clip)
            }
            "SignalBlendArrayControllerAsset" => {
                let sba = &subject;
                anyhow::ensure!(
                    !sba.bool("Mirrored").unwrap_or(false) && sba.guid("MasterController").is_none(),
                    "{}: mirrored or with a master (not ported)",
                    sba.name()
                );
                let arr = sba.guid("BlendArray").and_then(|g| self.db.get(g)).context("blend array")?;
                anyhow::ensure!(arr.int("SnapToAnim").unwrap_or(0) == 0, "{}: SnapToAnim (not ported)", sba.name());
                let mut items = Vec::new();
                for it in arr.array("Items") {
                    let x = crate::anim_db::field(arr.schema, it, "X").and_then(Value::as_f32).unwrap_or(0.0);
                    let c = crate::anim_db::field(arr.schema, it, "Clip")
                        .and_then(Value::as_guid)
                        .and_then(|g| self.db.get(g))
                        .with_context(|| format!("{}: item clip", sba.name()))?;
                    anyhow::ensure!(c.is("ClipControllerAsset"), "{}: item {} (not ported)", sba.name(), c.class());
                    items.push((x, c));
                }
                let n = items.iter().filter(|(_, c)| bound(c)).count();
                if n == 0 {
                    return Ok(None);
                }
                anyhow::ensure!(n == items.len(), "{}: items on different rigs (not ported)", sba.name());
                let mut loaded = Vec::new();
                for (x, c) in items {
                    let clip = NisClip::load(self.db, self.all, &c, &rig.name)?;
                    clips.push((c, clip.clone()));
                    loaded.push((x, clip));
                }
                let sig = sba.guid("BlendSignal").and_then(|g| self.db.get(g));
                let sig = sig.with_context(|| format!("{}: no signal", sba.name()))?;
                let f = |a: &Asset, k: &str| a.f32(k).or_else(|| a.int(k).map(|v| v as f32)).unwrap_or(0.0);
                LayerSource::BlendArray {
                    signal: sig.name().to_string(),
                    signal_default: f(&sig, "Default"),
                    clamp: [f(sba, "ClampMin"), f(sba, "ClampMax")],
                    damping: [f(sba, "PositiveDamping"), f(sba, "NegativeDamping")],
                    range: [f(&arr, "Min"), f(&arr, "Max")],
                    items: loaded,
                }
            }
            c => bail!("{}: a loop of a {c} (not ported)", ctrl.name()),
        };
        let length = clips.first().map_or(0.0, |(_, c)| c.num_ticks);
        anyhow::ensure!(
            length > 0.0 && clips.iter().all(|(_, c)| c.num_ticks == length && c.fps_scale == 1.0),
            "{}: clips of different lengths or with an FPSScale (not ported)",
            ctrl.name()
        );
        let mask = match guid("BlendMaskControllers").and_then(|g| self.db.get(g)) {
            None => None,
            Some(m) if m.is("BlendMaskListAsset") => Some(self.mask_weights(&m, &rig.name, &clips)?),
            Some(m) => bail!("{name}: layer {i} masked by a {} (not ported)", m.class()),
        };
        Ok(Some(NisLayer { name: ctrl.name().to_string(), clock, length, blend_mode: blend, mask, source }))
    }

    fn mask_weights(&self, m: &Asset, rig: &str, clips: &[(Asset, NisClip)]) -> Result<HashMap<String, f32>> {
        let layout = |g| self.db.get(g).map(|l| l.name().to_string()).unwrap_or_default();
        let values: Vec<f32> = m.array("DefaultValues").iter().filter_map(Value::as_f32).collect();
        let defaults: HashMap<String, f32> = m.guids("DefaultDofSetKeys").into_iter().map(layout).zip(values).collect();
        let slots: HashMap<String, Vec<f32>> = m
            .guids("MaskKeys")
            .into_iter()
            .filter_map(|k| self.db.get(k))
            .map(|k| {
                let l = k.guid("DofSetKey").map(layout).unwrap_or_default();
                (l, k.array("Values").iter().filter_map(Value::as_f32).collect())
            })
            .collect();
        let mut out = HashMap::new();
        for (c, _) in clips {
            let target = c.guid("Target").and_then(|g| self.db.get(g)).map_or("AnimRig", |t| t.name());
            let mut k_in_layout: HashMap<String, usize> = HashMap::new();
            for d in ssx_formats::ant_anim::dof_list(self.all, rig, target)? {
                let k = k_in_layout.entry(d.layout.clone()).or_default();
                let w = slots.get(&d.layout).and_then(|v| v.get(*k)).or_else(|| defaults.get(&d.layout));
                out.insert(d.name.clone(), w.copied().unwrap_or(0.0));
                *k += 1;
            }
        }
        Ok(out)
    }
}
