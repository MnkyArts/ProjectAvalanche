use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use ssx_content::model::{Model, ModelCost};
use ssx_content::object::{ObjectCollision, ObjectDef, ObjectLods};
use ssx_content::{ContentId, Layer};

use super::models::model_id;
use super::resolve::Found;
use super::store::GameContent;
use crate::track_objects::{TrackObjects, asset_file, load_track_objects};

pub const MAX_VERTICES: u64 = 4 << 20;
pub const MAX_TRIANGLES: u64 = 4 << 20;
pub const MAX_HELD_BYTES: u64 = 512 << 20;
pub const MAX_IMAGE_BYTES: u64 = 1 << 30;
pub const MAX_JOINTS: u64 = 32_768;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub collision_triangles: u64,
    pub vertices: u64,
    pub triangles: u64,
    pub held_bytes: u64,
    pub image_bytes: u64,
    pub joints: u64,
}

impl Usage {
    pub(super) fn plus(&self, o: &Usage) -> Usage {
        Usage {
            collision_triangles: self.collision_triangles.saturating_add(o.collision_triangles),
            vertices: self.vertices.saturating_add(o.vertices),
            triangles: self.triangles.saturating_add(o.triangles),
            held_bytes: self.held_bytes.saturating_add(o.held_bytes),
            image_bytes: self.image_bytes.saturating_add(o.image_bytes),
            joints: self.joints.saturating_add(o.joints),
        }
    }

    pub(super) fn over(&self, budget: &Usage) -> Option<(&'static str, u64, u64)> {
        [
            ("collision triangles placed", self.collision_triangles, budget.collision_triangles),
            ("vertices drawn", self.vertices, budget.vertices),
            ("triangles drawn", self.triangles, budget.triangles),
            ("bytes of models held", self.held_bytes, budget.held_bytes),
            ("bytes of images decoded", self.image_bytes, budget.image_bytes),
            ("skinned parts' joints", self.joints, budget.joints),
        ]
        .into_iter()
        .find(|(_, v, cap)| v > cap)
    }

    fn of_model(c: &ModelCost) -> Usage {
        Usage {
            collision_triangles: 0,
            vertices: c.vertices,
            triangles: c.triangles,
            held_bytes: c.held_bytes,
            image_bytes: c.drawn_image_bytes,
            joints: c.joints,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub id: ContentId,
    pub source: String,
    pub placements: usize,
    pub cost: Usage,
    pub refused: Option<String>,
}

type FileKey = (String, String);

type Provider = (String, Layer, i32);

#[derive(Default)]
pub struct MeshPass {
    models: BTreeMap<ContentId, Found<Model>>,
    pub outcomes: Vec<Outcome>,
    pub used: Usage,
    counted: BTreeSet<FileKey>,
}

impl MeshPass {
    pub fn model(&self, id: &ContentId) -> Option<&Found<Model>> {
        self.models.get(id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextureChoice {
    Package { source: String, layer: Layer, canonical: ContentId },
    Disc,
    Unavailable(String),
}

#[derive(Default)]
pub struct TexturePass {
    pub choices: BTreeMap<ContentId, TextureChoice>,
    pub outcomes: Vec<Outcome>,
    pub used: Usage,
}

pub struct TrackPackages {
    pub track: String,
    pub generation: u64,
    pub world_triangles: Option<u64>,
    objects: BTreeMap<u32, Found<ObjectDef>>,
    models: BTreeMap<ContentId, Found<Model>>,
    pub outcomes: Vec<Outcome>,
    pub used: Usage,
    counted: BTreeSet<FileKey>,
    meshes: OnceLock<MeshPass>,
}

impl TrackPackages {
    fn budget(&self) -> Usage {
        Usage {
            collision_triangles: self.world_triangles.unwrap_or(0),
            vertices: MAX_VERTICES,
            triangles: MAX_TRIANGLES,
            held_bytes: MAX_HELD_BYTES,
            image_bytes: MAX_IMAGE_BYTES,
            joints: 0,
        }
    }

    pub fn object(&self, key: u32) -> Option<&Found<ObjectDef>> {
        self.objects.get(&key)
    }

    pub fn object_model(&self, id: &ContentId) -> Option<&Found<Model>> {
        self.models.get(id)
    }

    pub fn digest(&self) -> String {
        let mut text = format!("{}\t{:?}\n", self.track, self.world_triangles);
        for (key, f) in &self.objects {
            text.push_str(&format!("{key:08x}\t{}\t{}\n", f.id, f.source));
        }
        ssx_content::ContentHash::of(text.as_bytes()).to_string()[..16].to_string()
    }

    pub fn sim_input(&self) -> Option<(ssx_content::ContentHash, String)> {
        if self.objects.is_empty() {
            return None;
        }
        let mut text = format!("{}\n", self.track);
        let mut from = BTreeSet::new();
        for (key, f) in &self.objects {
            let model = match &f.value.collision {
                ObjectCollision::Model(id) => self.models.get(id).and_then(|m| m.value.source).map(|h| h.to_string()),
                ObjectCollision::Decoded { path, .. } => Some(path.clone()),
                ObjectCollision::None => None,
            };
            text.push_str(&format!("{key:08x}\t{}\t{}\t{}\n", f.id, f.source, model.unwrap_or_default()));
            from.insert(f.layer.source_text(&f.source));
        }
        let from: Vec<String> = from.into_iter().collect();
        Some((ssx_content::ContentHash::of(text.as_bytes()), from.join(", ")))
    }

    pub fn summary(&self) -> String {
        let refused = self.outcomes.iter().filter(|o| o.refused.is_some()).count();
        let world = self.world_triangles.map_or_else(|| "not measured".to_string(), |w| w.to_string());
        format!(
            "{}: package objects: {} taken, {refused} on the disc's (decision {}); collision placed {} of {world}, \
             drawn {} vertices, {} triangles, {} bytes held, {} bytes of images",
            self.track,
            self.objects.len(),
            self.digest(),
            self.used.collision_triangles,
            self.used.vertices,
            self.used.triangles,
            self.used.held_bytes,
            self.used.image_bytes
        )
    }

    pub fn meshes(&self, content: &GameContent, objects: &TrackObjects) -> &MeshPass {
        self.meshes.get_or_init(|| {
            let mut pass = MeshPass { used: self.used, counted: self.counted.clone(), ..MeshPass::default() };
            let mut ids = BTreeSet::new();
            for (idx, _) in &objects.instances {
                let Some(object) = objects.objects.get(*idx) else { continue };
                if self.objects.contains_key(&object.key) {
                    continue;
                }
                ids.extend(object.lods.iter().flatten().filter_map(|m| model_id(&m.geometry)));
            }
            let mut candidates: Vec<(ContentId, Vec<Provider>)> = ids
                .into_iter()
                .filter_map(|id| {
                    let p = content.package_providers(&id);
                    (!p.is_empty()).then_some((id, p))
                })
                .collect();
            candidates.sort_by_key(|c| order(&c.1[0], &c.0));
            let budget = self.budget();
            for (id, providers) in candidates {
                for (source, layer, _) in &providers {
                    let (cost, file) = match content.model_cost_from(&id, source, *layer) {
                        Ok(c) => c,
                        Err(e) => {
                            content.notice(format!("content: {id} from {source} not used on {}: {e}", self.track));
                            continue;
                        }
                    };
                    let key = (source.clone(), file);
                    let add = if pass.counted.contains(&key) { Usage::default() } else { Usage::of_model(&cost) };
                    let mut outcome =
                        Outcome { id: id.clone(), source: source.clone(), placements: 0, cost: add, refused: None };
                    if let Some(over) = pass.used.plus(&add).over(&budget) {
                        outcome.refused = Some(over_text(over));
                    } else {
                        match content.model_from(&id, source, *layer) {
                            Ok(found) => {
                                pass.used = pass.used.plus(&add);
                                pass.counted.insert(key);
                                pass.models.insert(id.clone(), found);
                            }
                            Err(e) => outcome.refused = Some(e.to_string()),
                        }
                    }
                    report(content, &self.track, "the disc's mesh", &outcome);
                    pass.outcomes.push(outcome);
                    break;
                }
            }
            pass
        })
    }

    pub fn textures(
        &self,
        content: &GameContent,
        objects: &TrackObjects,
        ids: &BTreeSet<ContentId>,
        beside: Usage,
    ) -> TexturePass {
        let meshes = self.meshes(content, objects);
        let mut pass = TexturePass { used: meshes.used.plus(&beside), ..TexturePass::default() };
        let mut candidates: Vec<(ContentId, Vec<Provider>)> = ids
            .iter()
            .filter_map(|id| {
                let p = content.package_providers(id);
                (!p.is_empty()).then(|| (id.clone(), p))
            })
            .collect();
        candidates.sort_by_key(|c| order(&c.1[0], &c.0));
        let budget = self.budget();
        let mut files: BTreeMap<FileKey, ContentId> = BTreeMap::new();
        for (id, providers) in candidates {
            let on_disc = content.has_base(&id);
            let fallback = |why: String| if on_disc { TextureChoice::Disc } else { TextureChoice::Unavailable(why) };
            let mut choice = None;
            for (source, layer, _) in &providers {
                let (probe, file) = match content.texture_probe_from(&id, source, *layer) {
                    Ok(p) => p,
                    Err(e) => {
                        content.notice(format!("content: {id} from {source} not used on {}: {e}", self.track));
                        continue;
                    }
                };
                let key = (source.clone(), file);
                let shared = files.get(&key).cloned();
                let add =
                    Usage { image_bytes: if shared.is_some() { 0 } else { probe.decoded_bytes() }, ..Usage::default() };
                let mut outcome =
                    Outcome { id: id.clone(), source: source.clone(), placements: 0, cost: add, refused: None };
                if let Some(over) = pass.used.plus(&add).over(&budget) {
                    let why = over_text(over);
                    outcome.refused = Some(why.clone());
                    choice = Some(fallback(why));
                } else {
                    pass.used = pass.used.plus(&add);
                    let canonical = shared.unwrap_or_else(|| {
                        files.insert(key, id.clone());
                        id.clone()
                    });
                    choice = Some(TextureChoice::Package { source: source.clone(), layer: *layer, canonical });
                }
                report(content, &self.track, if on_disc { "the disc's texture" } else { "no texture" }, &outcome);
                pass.outcomes.push(outcome);
                break;
            }
            let choice = choice.unwrap_or_else(|| fallback("no package's file of it can be read".into()));
            pass.choices.insert(id, choice);
        }
        pass
    }
}

fn order(provider: &Provider, id: &ContentId) -> (std::cmp::Reverse<Layer>, std::cmp::Reverse<i32>, String) {
    (std::cmp::Reverse(provider.1), std::cmp::Reverse(provider.2), id.to_string())
}

fn over_text((what, amount, cap): (&'static str, u64, u64)) -> String {
    let whose = if what == "collision triangles placed" { " (the disc world's own size)" } else { "" };
    format!("the packages' {what} on this track would be {amount}; at most {cap}{whose}")
}

fn report(content: &GameContent, track: &str, instead: &str, o: &Outcome) {
    if let Some(why) = &o.refused {
        let at = if o.placements > 0 { format!(" at {} placements", o.placements) } else { String::new() };
        content
            .notice(format!("content: on {track}, {} from {}{at} is not used ({instead} is): {why}", o.id, o.source));
    }
}

pub(super) fn decide(content: &GameContent, big_root: &Path, track: &str, generation: u64) -> TrackPackages {
    let mut d = TrackPackages {
        track: track.to_string(),
        generation,
        world_triangles: None,
        objects: BTreeMap::new(),
        models: BTreeMap::new(),
        outcomes: Vec::new(),
        used: Usage::default(),
        counted: BTreeSet::new(),
        meshes: OnceLock::new(),
    };
    let objects = match load_track_objects(big_root, track, None) {
        Ok(o) => o,
        Err(e) => {
            content.notice(format!("content: {track}: its objects for the package decision: {e:#}"));
            return d;
        }
    };
    let mut placements: BTreeMap<usize, usize> = BTreeMap::new();
    for (idx, p) in &objects.instances {
        *placements.entry(*idx).or_default() += p.len();
    }
    let sim_off = content.sim_policy() != ssx_content::policy::ClientMods::Any;
    let mut candidates: Vec<(u32, usize, ContentId, Vec<Found<ObjectDef>>)> = placements
        .iter()
        .filter_map(|(&idx, &n)| {
            let object = objects.objects.get(idx)?;
            let (id, mut found) = content.object_candidates(object.key)?;
            if sim_off {
                found.retain(|f| f.layer != Layer::ClientMods);
            }
            (!found.is_empty()).then_some((object.key, n, id, found))
        })
        .collect();
    if candidates.is_empty() {
        return d;
    }
    let world = world_triangles(big_root, track, &objects, &placements);
    d.world_triangles = Some(world);
    let budget = d.budget();
    candidates.sort_by_cached_key(|c| {
        let p = &c.3[0];
        order(&(p.source.clone(), p.layer, content.priority_of(&p.source, p.layer)), &c.2)
    });
    for (key, n, id, found) in candidates {
        for f in found {
            let measured = match object_cost(content, &f, n, &d.counted) {
                Ok(m) => m,
                Err(e) => {
                    content.notice(format!("content: {id} from {} not used on {track}: {e}", f.source));
                    continue;
                }
            };
            let mut outcome =
                Outcome { id: id.clone(), source: f.source.clone(), placements: n, cost: measured.add, refused: None };
            if let Some(over) = d.used.plus(&measured.add).over(&budget) {
                outcome.refused = Some(over_text(over));
            } else {
                let decoded: Result<Vec<Found<Model>>, String> = measured
                    .models
                    .iter()
                    .map(|(m, source, layer)| match layer {
                        Some(layer) => content.model_from(m, source, *layer).map_err(|e| e.to_string()),
                        None => content.base_model(m).map_err(|e| e.to_string()),
                    })
                    .collect();
                match decoded {
                    Ok(models) => {
                        d.used = d.used.plus(&measured.add);
                        d.counted.extend(measured.files);
                        for m in models {
                            d.models.insert(m.id.clone(), m);
                        }
                        d.objects.insert(key, f.clone());
                    }
                    Err(e) => outcome.refused = Some(format!("its model: {e}")),
                }
            }
            report(content, track, "the disc's object", &outcome);
            d.outcomes.push(outcome);
            break;
        }
    }
    content.info(format!("content: {}", d.summary()));
    d
}

struct ObjectMeasure {
    add: Usage,
    files: Vec<FileKey>,
    models: Vec<(ContentId, String, Option<Layer>)>,
}

fn object_cost(
    content: &GameContent,
    f: &Found<ObjectDef>,
    placements: usize,
    counted: &BTreeSet<FileKey>,
) -> Result<ObjectMeasure, String> {
    let ObjectLods::Model { model, .. } = &f.value.lods else {
        return Err("a package's object names its model (`model` in its object.toml)".into());
    };
    let mut m = ObjectMeasure { add: Usage::default(), files: Vec::new(), models: Vec::new() };
    let mut collision_model = None;
    let mut ids = vec![model.clone()];
    if let ObjectCollision::Model(c) = &f.value.collision {
        collision_model = Some(c.clone());
        if c != model {
            ids.push(c.clone());
        }
    }
    for id in ids {
        let providers = content.package_providers(&id);
        let Some((source, layer, _)) = providers.first() else {
            m.models.push((id, String::new(), None));
            continue;
        };
        let (cost, file) = content.model_cost_from(&id, source, *layer).map_err(|e| format!("its model {id}: {e}"))?;
        if collision_model.as_ref() == Some(&id) {
            let placed = cost.collision_triangles.saturating_mul(placements as u64);
            m.add.collision_triangles = m.add.collision_triangles.saturating_add(placed);
        }
        let key = (source.clone(), file);
        if !counted.contains(&key) && !m.files.contains(&key) {
            m.add = m.add.plus(&Usage::of_model(&cost));
            m.files.push(key);
        }
        m.models.push((id, source.clone(), Some(*layer)));
    }
    Ok(m)
}

fn world_triangles(big_root: &Path, track: &str, objects: &TrackObjects, placements: &BTreeMap<usize, usize>) -> u64 {
    let Some(dir) = ssx_rules::data_roots::track_dir(big_root, track) else { return 0 };
    let terrain: u64 = crate::terrain::load_track(&dir)
        .map(|pieces| {
            pieces
                .iter()
                .flat_map(|p| &p.subd.meshes)
                .flat_map(|m| &m.faces)
                .filter(|f| !crate::terrain::is_hidden_face(f.unk_a))
                .map(|f| 8 * f.edges.len() as u64)
                .sum()
        })
        .unwrap_or(0);
    let mut pobs: BTreeMap<String, u64> = BTreeMap::new();
    let mut props = 0u64;
    for (&idx, &n) in placements {
        let Some(pob) = objects.objects.get(idx).and_then(|o| o.pob.as_ref()) else { continue };
        let triangles = *pobs.entry(pob.clone()).or_insert_with(|| {
            asset_file(big_root, pob)
                .and_then(|path| crate::track::object_collision::load_pob(&path).ok())
                .map_or(0, |(c, _)| c.triangle_count() as u64)
        });
        props = props.saturating_add(triangles.saturating_mul(n as u64));
    }
    terrain.saturating_add(props)
}

impl GameContent {
    pub fn track_packages(self: &Arc<Self>, track: &str) -> Arc<TrackPackages> {
        let generation = self.generation();
        let cell = self.track_decision(track, generation, self.sim_policy() as u8);
        cell.get_or_init(|| Arc::new(decide(self, self.big_root(), track, generation))).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_go_in_the_stated_order() {
        let id = |s: &str| s.parse::<ContentId>().unwrap();
        let mut c = [
            (("b".to_string(), Layer::ClientMods, 0), id("ssx:object/rock_a")),
            (("a".to_string(), Layer::ClientMods, 5), id("ssx:object/tree_b")),
            (("s".to_string(), Layer::Server, 0), id("ssx:object/z")),
            (("a".to_string(), Layer::ClientMods, 5), id("ssx:object/tree_a")),
        ];
        c.sort_by_key(|x| order(&x.0, &x.1));
        let got: Vec<String> = c.iter().map(|(_, i)| i.to_string()).collect();
        assert_eq!(got, ["ssx:object/z", "ssx:object/tree_a", "ssx:object/tree_b", "ssx:object/rock_a"]);
    }

    #[test]
    fn the_first_amount_over_the_budget_is_named() {
        let budget = Usage {
            collision_triangles: 1_000,
            vertices: 10,
            triangles: 10,
            held_bytes: 10,
            image_bytes: 10,
            joints: 0,
        };
        let used = Usage { collision_triangles: 600, ..Usage::default() };
        let add = Usage { collision_triangles: 50 * 678, ..Usage::default() };
        assert_eq!(used.plus(&add).over(&budget), Some(("collision triangles placed", 34_500, 1_000)));
        assert_eq!(
            Usage { image_bytes: 11, ..Usage::default() }.over(&budget),
            Some(("bytes of images decoded", 11, 10))
        );
        assert_eq!(used.over(&budget), None);
    }
}
