use super::*;

use crate::anim_db::{Asset, AssetDb};
use crate::nis::{HELI_RIG, NisClip, Rig};

pub const HELI_LAYERS: &str = "layer_heli_secondary";

pub type PoseMap<V> = HashMap<String, V, std::hash::BuildHasherDefault<NameHasher>>;

#[derive(Clone, Copy, Debug)]
pub struct NameHasher(u64);

impl Default for NameHasher {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl std::hash::Hasher for NameHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeliPose {
    pub q: PoseMap<[f32; 4]>,
    pub t: PoseMap<[f32; 3]>,
}

impl HeliPose {
    pub fn defaults(rig: &Rig) -> Self {
        let n = rig.skeleton.joints.len();
        let mut p = Self {
            q: PoseMap::with_capacity_and_hasher(n, Default::default()),
            t: PoseMap::with_capacity_and_hasher(n, Default::default()),
        };
        for j in &rig.skeleton.joints {
            let d = rig.defaults.get(&j.name).copied().unwrap_or(crate::rigamate::Sqt::IDENTITY);
            p.q.insert(j.name.clone(), d.q);
            p.t.insert(j.name.clone(), [d.t[0], d.t[1], d.t[2]]);
        }
        p
    }

    pub fn write_nis(&mut self, clip: &NisClip, tick: f32) {
        self.write_clip(clip, tick, |_| true);
    }

    pub fn write_trick(&mut self, clip: &NisClip, tick: f32, mirror: bool) {
        let names: Vec<String> = self.q.keys().cloned().collect();
        self.write_trick_on(&names, clip, tick, mirror);
    }

    pub fn trick_dofs(rig: &Rig, clip: &NisClip, tick: f32, mirror: bool) -> Self {
        let names: Vec<String> = rig.skeleton.joints.iter().map(|j| j.name.clone()).collect();
        let mut p = Self::default();
        p.write_trick_on(&names, clip, tick, mirror);
        p
    }

    fn write_trick_on(&mut self, joints: &[String], clip: &NisClip, tick: f32, mirror: bool) {
        if !mirror {
            self.write_nis(clip, tick);
            return;
        }
        let Ok(p) = clip.sample(tick) else { return };
        let lower: std::collections::HashMap<String, String> =
            joints.iter().map(|k| (k.to_ascii_lowercase(), k.clone())).collect();
        let partner = |j: &str| {
            crate::anim_pose::mirror_name(j).and_then(|m| lower.get(&m).cloned()).unwrap_or_else(|| j.to_string())
        };
        for dof in clip.channels.keys() {
            let Some((joint, comp)) = dof.rsplit_once('.') else { continue };
            let to = partner(joint);
            match comp {
                "q" => {
                    if let Some(q) = clip.quat(&p, dof) {
                        self.q.insert(to, [-q[0], -q[1], q[2], q[3]]);
                    }
                }
                "t" => {
                    if let Some(t) = clip.vec3(&p, dof) {
                        self.t.insert(to, [t[0], t[1], -t[2]]);
                    }
                }
                _ => {}
            }
        }
    }

    fn write_clip(&mut self, clip: &NisClip, tick: f32, keep: impl Fn(&str) -> bool) {
        let Ok(p) = clip.sample(tick) else { return };
        for (dof, _) in clip.channels.iter().filter(|(d, _)| keep(d)) {
            let Some((joint, comp)) = dof.rsplit_once('.') else { continue };
            match comp {
                "q" => {
                    if let Some(q) = clip.quat(&p, dof) {
                        self.q.insert(joint.to_string(), q);
                    }
                }
                "t" => {
                    if let Some(t) = clip.vec3(&p, dof) {
                        self.t.insert(joint.to_string(), t);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn blend_valid(&self, new: &HeliPose, w: f32) -> HeliPose {
        let mut out = self.clone();
        out.blend_from(new, |j, c| match c {
            'q' if self.q.contains_key(j) => w,
            't' if self.t.contains_key(j) => w,
            _ => 1.0,
        });
        out
    }

    pub fn blend_from(&mut self, other: &HeliPose, weight: impl Fn(&str, char) -> f32) {
        for (j, b) in &other.q {
            let w = weight(j, 'q');
            if w <= 0.0 {
                continue;
            }
            let a = self.q.get(j).copied().unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let s = if (0..4).map(|k| a[k] * b[k]).sum::<f32>() < 0.0 { -1.0 } else { 1.0 };
            let q: [f32; 4] = std::array::from_fn(|k| a[k] + (s * b[k] - a[k]) * w);
            let n = q.iter().map(|x| x * x).sum::<f32>().sqrt();
            self.q.insert(j.clone(), if n > 0.0 { q.map(|x| x / n) } else { a });
        }
        for (j, b) in &other.t {
            let w = weight(j, 't');
            if w <= 0.0 {
                continue;
            }
            let a = self.t.get(j).copied().unwrap_or([0.0; 3]);
            self.t.insert(j.clone(), std::array::from_fn(|k| a[k] + (b[k] - a[k]) * w));
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DofMask {
    dofs: HashMap<String, f32>,
    q: PoseMap<f32>,
    t: PoseMap<f32>,
}

impl DofMask {
    pub(super) fn load(db: &AssetDb, name: &str) -> Result<Self> {
        let m = db.by_name(name).with_context(|| format!("no {name}"))?;
        let values: Vec<f32> = m.array("DefaultValues").iter().filter_map(|v| v.as_f32()).collect();
        let mut out = HashMap::new();
        for (layout, w) in m.guids("DefaultDofSetKeys").into_iter().zip(values) {
            let Some(l) = db.get(layout) else { continue };
            for slot in l.array("Slots") {
                if let Some(n) = crate::anim_db::field(l.schema, slot, "Name").and_then(|v| v.as_str()) {
                    out.insert(n.to_string(), w);
                }
            }
        }
        let by_joint = |c: &str| -> PoseMap<f32> {
            out.iter().filter_map(|(d, &w)| Some((d.strip_suffix(c)?.to_string(), w))).collect()
        };
        Ok(Self { q: by_joint(".q"), t: by_joint(".t"), dofs: out })
    }

    pub fn weight(&self, dof: &str) -> f32 {
        self.dofs.get(dof).copied().unwrap_or(0.0)
    }

    pub fn weight_of(&self, joint: &str, c: char) -> f32 {
        let m = if c == 'q' { &self.q } else { &self.t };
        m.get(joint).copied().unwrap_or(0.0)
    }
}

#[derive(Clone, Debug)]
struct SeqElem {
    clip: NisClip,
    begin: f32,
    start_in: f32,
    scale: f32,
    blend: Option<(f32, i64)>,
}

#[derive(Clone, Debug)]
struct LoopedSeq {
    elems: Vec<SeqElem>,
    length: f32,
}

impl LoopedSeq {
    pub(super) fn load(db: &AssetDb, all: &ssx_formats::ant::Stream, seq: &Asset) -> Result<Self> {
        let actor = seq.guids("ActorAssets").first().and_then(|g| db.get(*g)).context("sequence without actor")?;
        let track = actor.array("Tracks").first().context("actor without track")?;
        let anims = crate::anim_db::field(actor.schema, track, "Anims").and_then(|v| v.as_array()).unwrap_or(&[]);
        let mut elems = Vec::new();
        for a in anims {
            let get = |f: &str| crate::anim_db::field(actor.schema, a, f);
            let num = |f: &str| get(f).and_then(|v| v.as_f32().or_else(|| v.as_i64().map(|x| x as f32))).unwrap_or(0.0);
            let ctrl =
                get("Asset").and_then(|v| v.as_guid()).and_then(|g| db.get(g)).context("element without clip")?;
            let blend = get("BlendAsset").and_then(|v| v.as_guid()).and_then(|g| db.get(g)).and_then(|b| {
                b.is("BlendCurveAsset").then(|| (b.f32("BlendTime").unwrap_or(0.0), b.int("CurveType").unwrap_or(-1)))
            });
            let scale = num("Scale");
            elems.push(SeqElem {
                clip: NisClip::load(db, all, &ctrl, HELI_RIG)?,
                begin: num("StartTime") + num("StartInTime"),
                start_in: num("StartInTime"),
                scale: if scale != 0.0 { scale } else { 1.0 },
                blend,
            });
        }
        anyhow::ensure!(!elems.is_empty(), "{}: no elements", seq.name());
        Ok(Self { elems, length: actor.f32("Length").unwrap_or(0.0).max(1.0) })
    }

    fn pose(&self, defaults: &HeliPose, t: f32) -> HeliPose {
        let t = t.rem_euclid(self.length);
        let k = self.elems.iter().rposition(|e| e.begin <= t + 1e-4).unwrap_or(0);
        let at = |e: &SeqElem| (e.start_in + (t - e.begin)) * e.scale;
        let mut out = defaults.clone();
        let e = &self.elems[k];
        match (k.checked_sub(1).map(|i| &self.elems[i]), e.blend) {
            (Some(prev), Some((time, curve))) if time > 0.0 && t - e.begin < time => {
                out.write_clip(&prev.clip, at(prev), |_| true);
                let mut new = defaults.clone();
                new.write_clip(&e.clip, at(e), |_| true);
                let w = 1.0 - crate::anim_eval::curve_weight((t - e.begin) / time, curve);
                out.blend_from(&new, |_, _| w);
            }
            _ => out.write_clip(&e.clip, at(e), |_| true),
        }
        out
    }
}

#[derive(Clone, Debug)]
struct Rpm {
    joint: String,
    rpm: f32,
    axis: [f32; 3],
    angle: f32,
}

#[derive(Clone, Debug)]
pub struct HeliAnimator {
    pub rig: Rig,
    pilot: Option<LoopedSeq>,
    objects: Option<NisClip>,
    static_pose: Option<NisClip>,
    pilot_mask: DofMask,
    objects_mask: DofMask,
    blades_mask: DofMask,
    rotors: Vec<Rpm>,
    default_pose: HeliPose,
    pilot_time: f32,
    pub hover: Option<HoverNoise>,
    pub problems: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct HoverNoise {
    pub items: Vec<(f32, NisClip)>,
    pub loop_end: f32,
}

impl HoverNoise {
    pub fn load_disc(big_root: &Path) -> Result<Self> {
        let dir = big_root.join(crate::rig_anim::ANIM_DIR);
        let db = AssetDb::load(&dir, &[crate::rig_anim::RIG_BUNDLE])?;
        let all = &db.streams.iter().find(|(n, _)| n == crate::rig_anim::RIG_BUNDLE).context("no anim_all.cba")?.1;
        Self::load(&db, all)
    }

    pub(super) fn load(db: &AssetDb, all: &ssx_formats::ant::Stream) -> Result<Self> {
        let looping = db.by_name("sba_helicopter_noise (Looping)").context("no sba_helicopter_noise (Looping)")?;
        let sba = looping.guid("SubjectControllerAsset").and_then(|g| db.get(g)).context("no noise array")?;
        let array = sba.guid("BlendArray").and_then(|g| db.get(g)).context("no BlendArray")?;
        let mut items = Vec::new();
        for it in array.array("Items") {
            let get = |f: &str| crate::anim_db::field(array.schema, it, f);
            let x = get("X").and_then(|v| v.as_f32()).unwrap_or(0.0);
            let clip = get("Clip").and_then(|v| v.as_guid()).and_then(|g| db.get(g)).context("item without clip")?;
            items.push((x, NisClip::load(db, all, &clip, HELI_RIG)?));
        }
        anyhow::ensure!(!items.is_empty(), "sba_helicopter_noise: no items");
        Ok(Self { items, loop_end: looping.f32("LoopEndTime").unwrap_or(0.0) })
    }

    pub fn write(&self, pose: &mut HeliPose, speed_ratio: f32, ticks: f32) {
        let t = if self.loop_end > 0.0 { ticks.rem_euclid(self.loop_end) } else { ticks };
        let xs: Vec<f32> = self.items.iter().map(|i| i.0).collect();
        let (lo, hi) = (xs[0].min(xs[xs.len() - 1]), xs[0].max(xs[xs.len() - 1]));
        let pos = speed_ratio.clamp(0.0, 1.0).clamp(lo, hi);
        let mut sel = vec![(xs.len() - 1, 1.0)];
        if pos <= xs[0] {
            sel = vec![(0, 1.0)];
        } else {
            for i in 0..xs.len() - 1 {
                let (a, b) = (xs[i], xs[i + 1]);
                if a - 0.001 <= pos && pos <= b + 0.001 {
                    let f = (pos - a) / (b - a);
                    sel = if (pos - a).abs() < 0.001 || f < 0.03 {
                        vec![(i, 1.0)]
                    } else if f > 0.97 {
                        vec![(i + 1, 1.0)]
                    } else {
                        vec![(i, 1.0 - f), (i + 1, f)]
                    };
                    break;
                }
            }
        }
        let mut acc: Option<HeliPose> = None;
        for (i, w) in sel {
            let mut p = pose.clone();
            p.write_clip(&self.items[i].1, t, |_| true);
            match &mut acc {
                None => acc = Some(p),
                Some(a) => a.blend_from(&p, |_, _| w),
            }
        }
        if let Some(a) = acc {
            *pose = a;
        }
    }
}

impl HeliAnimator {
    pub fn load(big_root: &Path, name: &str) -> Result<Self> {
        let dir = big_root.join(crate::rig_anim::ANIM_DIR);
        let own = format!("{name}.cba");
        let bundles: Vec<&str> = if dir.join(&own).exists() {
            vec![&own, crate::rig_anim::RIG_BUNDLE]
        } else {
            vec![crate::rig_anim::RIG_BUNDLE]
        };
        let db = AssetDb::load(&dir, &bundles)?;
        let all = &db.streams.iter().find(|(n, _)| n == crate::rig_anim::RIG_BUNDLE).context("no anim_all.cba")?.1;
        let rig = Rig::load(all, HELI_RIG)?;
        let mut problems = Vec::new();
        let pilot = (|| -> Result<LoopedSeq> {
            let config = db.by_name(&format!("{name}_config")).with_context(|| format!("no {name}_config"))?;
            let bank_ptr = config
                .guids("OverrideGameStateTarget")
                .into_iter()
                .zip(config.guids("OverrideGameStateData"))
                .find(|(t, _)| db.get(*t).is_some_and(|a| a.name() == "PTR_proxybank_character"))
                .and_then(|(_, d)| db.get(d))
                .context("no character bank")?;
            let bank = bank_ptr.guid("SubjectBankAsset").and_then(|g| db.get(g)).context("bank not found")?;
            let proxy = db.by_name("proxy_pilot_idle_loop").context("no proxy_pilot_idle_loop")?;
            let seq = bank
                .guids("AssetListItems")
                .into_iter()
                .filter_map(|p| db.get(p))
                .find(|p| p.guid("ProxyController") == Some(proxy.guid))
                .and_then(|p| p.guid("SubjectController"))
                .and_then(|g| db.get(g))
                .context("the bank has no pilot idle")?;
            LoopedSeq::load(&db, all, &seq)
        })();
        let pilot = pilot.map_err(|e| problems.push(format!("pilot idle: {e:#}"))).ok();
        let clip = |n: &str| -> Option<NisClip> {
            let c = db.by_name(n)?;
            NisClip::load(&db, all, &c, HELI_RIG).ok()
        };
        let objects = clip("pilot_idle_loop_helicopter");
        let static_pose = clip("heli_pilot_static_pose");
        let hover = HoverNoise::load(&db, all).map_err(|e| problems.push(format!("hover noise: {e:#}"))).ok();
        let mut rotors = Vec::new();
        for n in ["rpm_heli_main_rotor", "rpm_heli_tail_rotor"] {
            let Some(a) = db.by_name(n) else {
                problems.push(format!("no {n}"));
                continue;
            };
            let dof = a.value("RotationRigDof").and_then(|v| v.as_str()).unwrap_or("");
            let joint = dof.split('.').nth(1).unwrap_or("").to_string();
            let axis = match a.value("RotationAxis") {
                Some(ssx_formats::ant::Value::Vector3(v)) => *v,
                _ => [0.0, 1.0, 0.0],
            };
            let d = rig.defaults.get(&joint).copied().unwrap_or(crate::rigamate::Sqt::IDENTITY);
            rotors.push(Rpm { angle: twist_angle(d.q, axis), joint, rpm: a.f32("RPM").unwrap_or(0.0), axis });
        }
        Ok(Self {
            pilot_mask: DofMask::load(&db, "BlendMask_HeliPilot")?,
            objects_mask: DofMask::load(&db, "BlendMask_heli_internal_objects")?,
            blades_mask: DofMask::load(&db, "BlendMask_HeliBlades")?,
            default_pose: HeliPose::defaults(&rig),
            rig,
            pilot,
            objects,
            static_pose,
            rotors,
            pilot_time: 0.0,
            hover,
            problems,
        })
    }

    pub fn default_pose(&self) -> &HeliPose {
        &self.default_pose
    }

    pub fn update(&mut self, dt: f32) {
        self.pilot_time += dt;
        if dt.abs() > 1.525_878_9e-5 {
            for r in &mut self.rotors {
                r.angle += r.rpm * dt;
            }
        }
    }

    pub fn pose(&self, base: &HeliPose, nis_active: bool) -> HeliPose {
        let time = self.pilot_time;
        let mut out = base.clone();
        let mut pilot = self.default_pose.clone();
        if nis_active {
            if let Some(p) = &self.pilot {
                pilot = p.pose(&self.default_pose, time);
            }
            if let Some(o) = &self.objects {
                let t = time.rem_euclid(o.num_ticks.max(1.0));
                let mut layer = pilot.clone();
                layer.write_clip(o, t, |_| true);
                pilot.blend_from(&layer, |j, c| self.objects_mask.weight_of(j, c));
            }
        } else if let Some(s) = &self.static_pose {
            pilot.write_clip(s, 0.0, |_| true);
        }
        out.blend_from(&pilot, |j, c| self.pilot_mask.weight_of(j, c));
        for r in self.rotors.iter().filter(|r| self.blades_mask.weight_of(&r.joint, 'q') > 0.0) {
            let (s, c) = (0.5 * r.angle).sin_cos();
            out.q.insert(r.joint.clone(), [r.axis[0] * s, r.axis[1] * s, r.axis[2] * s, c]);
        }
        out
    }
}

fn twist_angle(q: [f32; 4], axis: [f32; 3]) -> f32 {
    let d = axis[0] * q[0] + axis[1] * q[1] + axis[2] * q[2];
    2.0 * d.atan2(q[3])
}

impl HeliModel {
    pub fn node_pose(&self, pose: &HeliPose, frame: ([f32; 4], [f32; 3])) -> Vec<Srt> {
        use crate::rigamate::{quat_mul, rotate};
        let mut out = self.bind.clone();
        for (i, n) in self.rig.nodes.iter().enumerate() {
            if let Some(q) = pose.q.get(&n.name) {
                out[i].rotation = *q;
            }
            if let Some(t) = pose.t.get(&n.name) {
                out[i].translation = *t;
            }
        }
        if let Some(tj) = self.rig.find("AITrajectory") {
            let mut p = self.rig.nodes[tj].parent;
            while let Some(r) = p {
                out[r] = Srt { scale: [1.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], translation: [0.0; 3] };
                p = self.rig.nodes[r].parent;
            }
            let (fq, fp) = frame;
            let t = out[tj].translation;
            let r = rotate(fq, [t[0], t[1], t[2], 0.0]);
            out[tj].rotation = quat_mul(fq, out[tj].rotation);
            out[tj].translation = [fp[0] + r[0], fp[1] + r[1], fp[2] + r[2]];
        }
        out
    }
}

pub fn end_level_clip(big_root: &Path) -> Result<NisClip> {
    let dir = big_root.join(crate::rig_anim::ANIM_DIR);
    let db = AssetDb::load(&dir, &[crate::rig_anim::RIG_BUNDLE])?;
    let all = &db.streams.iter().find(|(n, _)| n == crate::rig_anim::RIG_BUNDLE).context("no anim_all.cba")?.1;
    let ctrl = db.by_name("nis_default_helicopter_end_level").context("no nis_default_helicopter_end_level")?;
    NisClip::load(&db, all, &ctrl, HELI_RIG)
}

#[derive(Clone, Debug)]
pub struct HeliTrickSeq {
    pub name: String,
    pub clip: NisClip,
    pub length: f32,
    pub end_window: Option<(f32, f32)>,
    pub end_blend: (f32, i64),
    pub true_signals: Vec<String>,
    pub false_signals: Vec<String>,
}

pub fn heli_trick_sequences(big_root: &Path) -> Result<Vec<HeliTrickSeq>> {
    trick_sequences(big_root, "chooser_heli_interaction_helicopter", HELI_RIG)
}

pub fn trick_sequences(big_root: &Path, chooser: &str, rig: &str) -> Result<Vec<HeliTrickSeq>> {
    use ssx_formats::ant::Value as AntValue;
    let dir = big_root.join(crate::rig_anim::ANIM_DIR);
    let db = AssetDb::load(&dir, &[crate::rig_anim::RIG_BUNDLE])?;
    let all = &db.streams.iter().find(|(n, _)| n == crate::rig_anim::RIG_BUNDLE).context("no anim_all.cba")?.1;
    let chooser = db.by_name(chooser).with_context(|| format!("no {chooser}"))?;
    let names = |e: &Asset, f: &str| -> Vec<String> {
        e.guids(f).into_iter().filter_map(|g| db.get(g)).map(|a| a.name().to_string()).collect()
    };
    let mut out = Vec::new();
    for g in chooser.guids("EntryAssetList") {
        let e = db.get(g).context("a chooser entry is not loaded")?;
        let seq = e.guid("ControllerAsset").and_then(|g| db.get(g)).context("an entry without a sequence")?;
        let actor = seq.guids("ActorAssets").first().and_then(|&g| db.get(g)).context("a sequence without actor")?;
        let sc = actor.schema;
        let track = actor.array("Tracks").first().context("a sequence without track")?;
        let anim = crate::anim_db::field(sc, track, "Anims")
            .and_then(AntValue::as_array)
            .and_then(|a| a.first())
            .context("a track without animation")?;
        let clip = crate::anim_db::field(sc, anim, "Asset")
            .and_then(AntValue::as_guid)
            .and_then(|g| db.get(g))
            .context("an animation without clip")?;
        let mut end_window = None;
        let mut end_blend = (0.0, 0);
        for w in actor.guids("BranchTable").into_iter().filter_map(|g| db.get(g)) {
            let kind = w.guid("BranchTypeAsset").and_then(|g| db.get(g)).map(|b| b.name().to_string());
            if kind.as_deref() == Some("End") {
                end_window = Some((w.f32("Time").unwrap_or(0.0), w.f32("Length").unwrap_or(0.0)));
                if let Some(b) = w.guid("BlendAsset").and_then(|g| db.get(g)) {
                    end_blend = (b.f32("BlendTime").unwrap_or(0.0), b.int("CurveType").unwrap_or(0));
                }
            }
        }
        out.push(HeliTrickSeq {
            name: seq.name().to_string(),
            clip: NisClip::load(&db, all, &clip, rig)?,
            length: actor.f32("Length").unwrap_or(0.0),
            end_window,
            end_blend,
            true_signals: names(&e, "TrueSignals"),
            false_signals: names(&e, "FalseSignals"),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(q: &[(&str, [f32; 4])], t: &[(&str, [f32; 3])]) -> HeliPose {
        let mut p = HeliPose::default();
        p.q.extend(q.iter().map(|(j, v)| (j.to_string(), *v)));
        p.t.extend(t.iter().map(|(j, v)| (j.to_string(), *v)));
        p
    }

    #[test]
    fn blend_valid_takes_the_holding_side() {
        let id = [0.0, 0.0, 0.0, 1.0];
        let old = side(&[("Root", id)], &[("Roll", [0.0, 2.0, 0.0])]);
        let new =
            side(&[("Floor", [0.0, 1.0, 0.0, 0.0])], &[("Roll", [0.0, 4.0, 0.0]), ("AITrajectory", [9.0, 0.0, 0.0])]);
        let b = old.blend_valid(&new, 0.25);
        assert_eq!(b.t["Roll"], [0.0, 2.5, 0.0]);
        assert_eq!(b.t["AITrajectory"], [9.0, 0.0, 0.0]);
        assert_eq!(b.q["Root"], id);
        let f = b.q["Floor"];
        assert!((f[1].abs() - 1.0).abs() < 1e-6 && f[3].abs() < 1e-6);
    }
}
