use super::*;

pub(crate) const CLASS_GAMEOBJECT: u32 = fnv1("ge_scene_gameobject");
const CLASS_GEOMETRY: u32 = fnv1("ge_geometry");
const CLASS_MATERIAL: u32 = fnv1("ge_material");
pub(crate) const FIELD_MODELS: u32 = 0xB43A_3598;
pub(crate) const FIELD_LOD_THRESHOLDS: u32 = fnv1("LODThresholds");
const FIELD_MESHES: u32 = fnv1("Meshes");
const FIELD_MATERIAL: u32 = fnv1("Material");
const FIELD_GEOMETRY: u32 = fnv1("Geometry");
const FIELD_SKELETON: u32 = fnv1("SkeletonFileRef");
const FIELD_FILE: u32 = fnv1("FileRef");

#[derive(Clone, Debug)]
pub struct GhostPart {
    pub mesh_name: String,
    pub geom: PathBuf,
    pub mesh: ObjectMesh,
    pub skin: Skin,
    pub material: Material,
}

#[derive(Clone, Debug)]
pub struct GhostModel {
    pub lod_thresholds: Vec<f32>,
    pub lods: Vec<Vec<String>>,
    pub rig_file: PathBuf,
    pub rig: SceneGraph,
    pub bind: Vec<Srt>,
    pub parts: Vec<GhostPart>,
    pub problems: Vec<String>,
}

pub(crate) struct PartRef {
    pub name: String,
    pub geom: String,
    pub crsf: Option<String>,
    pub skeleton: Option<String>,
    pub material: Material,
}

pub(crate) fn part_refs(db: &Db, class: u32, key: u32) -> Result<Vec<PartRef>> {
    let mut out = Vec::new();
    let Some(field) = db.field(class, key, FIELD_MESHES)? else { return Ok(out) };
    for r in refs(&field.data).into_iter().filter(|r| r.collection != 0) {
        let fields = db.fields(r.class, r.collection)?;
        let get = |k: u32| fields.iter().find(|f| f.key == k);
        let text = |k: u32| get(k).and_then(|f| single_text(&f.data)).filter(|s| !s.is_empty());
        let reference = |k: u32, c: u32| match get(k).and_then(|f| single(&f.data)) {
            Some(Value::Ref(x)) if x.class == c && x.collection != 0 => Some(*x),
            _ => None,
        };
        let Some(g) = reference(FIELD_GEOMETRY, CLASS_GEOMETRY) else { continue };
        let geom = db.text_of(g.class, g.collection)?.context("geometry without a path")?;
        let material = match reference(FIELD_MATERIAL, CLASS_MATERIAL) {
            Some(m) => db.material(m.class, m.collection)?,
            None => Material::default(),
        };
        let name = db.collection_name(r.collection);
        out.push(PartRef { name, geom, crsf: text(FIELD_FILE), skeleton: text(FIELD_SKELETON), material });
    }
    Ok(out)
}

pub(crate) fn bind_pose(rig: &SceneGraph, face_offsets: &HashMap<String, [f32; 3]>) -> Vec<Srt> {
    rig.nodes
        .iter()
        .map(|n| {
            let mut translation = n.attribute("translate").unwrap_or([0.0; 3]);
            if let Some(o) = n.name.strip_prefix("Offset_").and_then(|j| face_offsets.get(j)) {
                translation = *o;
            }
            Srt {
                scale: n.attribute("scale").unwrap_or([1.0; 3]),
                rotation: ssx_formats::ant_anim::euler_xyz_to_quat(n.attribute("rotate").unwrap_or([0.0; 3])),
                translation,
            }
        })
        .collect()
}

pub(crate) fn skin_of(file: &CrsfFile, rig: &SceneGraph, vertices: usize) -> Result<Skin> {
    let w = file.weights.as_ref().context("no WEIT chunk")?;
    let p = file.palette.as_ref().context("no SKTN chunk")?;
    if w.vertex_count != vertices || w.influences > 4 {
        bail!("WEIT: {} vertices × {} influences, the .geom has {vertices}", w.vertex_count, w.influences);
    }
    let palette = p
        .bones
        .iter()
        .map(|b| {
            let node = rig.find(&b.name).with_context(|| format!("palette joint {} is not in the rig", b.name))?;
            Ok(PaletteJoint { name: b.name.clone(), node, inverse_bind: b.inverse_bind })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut skin = Skin { palette, ..Skin::default() };
    for v in 0..w.vertex_count {
        let (ws, bs) = w.vertex(v);
        let (mut joints, mut weights) = ([0u16; 4], [0.0f32; 4]);
        for (k, (&x, &b)) in ws.iter().zip(bs).enumerate().filter(|(_, (x, _))| **x != 0.0) {
            if usize::from(b) >= skin.palette.len() {
                bail!("vertex {v}: palette index {b} of {}", skin.palette.len());
            }
            (joints[k], weights[k]) = (u16::from(b), x);
        }
        skin.joints.push(joints);
        skin.weights.push(weights);
    }
    Ok(skin)
}

pub fn load_model(big_root: &Path, track: &str) -> Result<GhostModel> {
    let object = GhostSchemas::load(big_root)?.ghost_object;
    let key = fnv1(&object);
    let (lod_thresholds, models) = with_track_db(big_root, track, |db| {
        let mut thresholds = Vec::new();
        let mut models = Vec::new();
        for f in db.fields(CLASS_GAMEOBJECT, key)? {
            match f.key {
                FIELD_LOD_THRESHOLDS => {
                    let vs: Vec<&Value> = match &f.data {
                        ssx_formats::attrib::FieldData::Array(vs) => vs.iter().collect(),
                        ssx_formats::attrib::FieldData::Single(v) => vec![v],
                    };
                    thresholds =
                        vs.into_iter().filter_map(|v| if let Value::Float(x) = v { Some(*x) } else { None }).collect();
                }
                FIELD_MODELS => {
                    for m in refs(&f.data).into_iter().filter(|r| r.collection != 0) {
                        models.push(part_refs(db, m.class, m.collection)?);
                    }
                }
                _ => {}
            }
        }
        Ok((thresholds, models))
    })?;
    let lods = models.iter().map(|m| m.iter().map(|p| p.name.clone()).collect()).collect();
    let lod0 = models.into_iter().next().context("ghost_gameobject has no model")?;
    let skeleton = lod0.iter().find_map(|p| p.skeleton.clone()).context("no part names a skeleton")?;
    let rig_file = asset_file(big_root, &skeleton).with_context(|| format!("rig {skeleton} not found"))?;
    let rig = CrsfFile::parse(&std::fs::read(&rig_file).with_context(|| format!("reading {}", rig_file.display()))?)
        .with_context(|| format!("parsing {}", rig_file.display()))?
        .scene
        .with_context(|| format!("{} has no SGRF scene graph", rig_file.display()))?;
    let mut problems = Vec::new();
    let mut loaded = Vec::new();
    for p in lod0 {
        if p.skeleton.as_deref() != Some(skeleton.as_str()) {
            problems.push(format!("{}: rig {:?}, drawn on {skeleton}", p.name, p.skeleton));
        }
        let read = || -> Result<(PathBuf, ObjectMesh, CrsfFile)> {
            let crsf_rel = p.crsf.as_deref().context("no FileRef (.crsf)")?;
            let geom = asset_file(big_root, &p.geom).with_context(|| format!("{} not found", p.geom))?;
            let crsf = asset_file(big_root, crsf_rel).with_context(|| format!("{crsf_rel} not found"))?;
            let file = CrsfFile::parse(&std::fs::read(&crsf).with_context(|| format!("reading {}", crsf.display()))?)
                .with_context(|| format!("parsing {}", crsf.display()))?;
            Ok((geom.clone(), crate::content::models::rider_mesh(big_root, &p.geom)?, file))
        };
        match read() {
            Ok(x) => loaded.push((p, x)),
            Err(e) => problems.push(format!("{}: {e:#}", p.name)),
        }
    }
    let mut face_offsets = HashMap::new();
    for (_, (_, _, file)) in &loaded {
        for o in file.face_offsets.iter().flatten() {
            face_offsets.insert(o.joint.clone(), o.translate);
        }
    }
    let bind = bind_pose(&rig, &face_offsets);
    let mut parts = Vec::new();
    for (p, (geom, mesh, file)) in loaded {
        match skin_of(&file, &rig, mesh.positions.len()) {
            Ok(skin) => parts.push(GhostPart { mesh_name: p.name, geom, mesh, skin, material: p.material }),
            Err(e) => problems.push(format!("{}: {e:#}", p.name)),
        }
    }
    if parts.is_empty() {
        bail!("ghost_gameobject: no part loaded ({})", problems.join("; "));
    }
    Ok(GhostModel { lod_thresholds, lods, rig_file, rig, bind, parts, problems })
}

const CLASS_CHARACTER: u32 = 0x1A08_2B62;
const FIELD_CHARACTER_GAMEOBJECT: u32 = 0x030B_B788;
const FIELD_LIGHT_NODES: u32 = 0x562C_4452;

#[derive(Clone, Debug)]
pub struct BotModel {
    pub game_object: u32,
    pub lod_thresholds: Vec<f32>,
    pub lods: Vec<Vec<GhostPart>>,
    pub rig_file: PathBuf,
    pub rig: SceneGraph,
    pub bind: Vec<Srt>,
    pub light_nodes: Vec<Vec<crate::track_lights::LightNode>>,
    pub problems: Vec<String>,
}

pub fn load_bot_model(big_root: &Path, track: Option<&str>, character: &str) -> Result<BotModel> {
    let vaults = crate::helicopter::Vaults::load(big_root)?;
    let key = vaults
        .reference(CLASS_CHARACTER, fnv1(character), FIELD_CHARACTER_GAMEOBJECT)
        .with_context(|| format!("character {character}"))?;
    if key == 0 {
        bail!("character {character} has no game object (field 0x030BB788)");
    }
    let (lod_thresholds, models, light_nodes) = crate::vault_db::with_level_db(big_root, track, |db| {
        let mut thresholds = Vec::new();
        let (mut models, mut lights) = (Vec::new(), Vec::new());
        for f in db.fields(CLASS_GAMEOBJECT, key)? {
            match f.key {
                FIELD_LOD_THRESHOLDS => {
                    let vs: Vec<&Value> = match &f.data {
                        ssx_formats::attrib::FieldData::Array(vs) => vs.iter().collect(),
                        ssx_formats::attrib::FieldData::Single(v) => vec![v],
                    };
                    thresholds =
                        vs.into_iter().filter_map(|v| if let Value::Float(x) = v { Some(*x) } else { None }).collect();
                }
                FIELD_MODELS => {
                    for m in refs(&f.data).into_iter().filter(|r| r.collection != 0) {
                        models.push(part_refs(db, m.class, m.collection)?);
                        let nodes = match db.field(m.class, m.collection, FIELD_LIGHT_NODES)? {
                            Some(n) => refs(&n.data)
                                .into_iter()
                                .filter(|r| r.collection != 0)
                                .map(|r| crate::track_lights::light_node(db, r.collection))
                                .collect::<Result<Vec<_>>>()?,
                            None => Vec::new(),
                        };
                        lights.push(nodes);
                    }
                }
                _ => {}
            }
        }
        Ok((thresholds, models, lights))
    })?;
    let skeleton = models
        .first()
        .and_then(|m| m.iter().find_map(|p| p.skeleton.clone()))
        .with_context(|| format!("game object {key:08x}: no LOD-0 part names a skeleton"))?;
    let rig_file = asset_file(big_root, &skeleton).with_context(|| format!("rig {skeleton} not found"))?;
    let rig = CrsfFile::parse(&std::fs::read(&rig_file).with_context(|| format!("reading {}", rig_file.display()))?)
        .with_context(|| format!("parsing {}", rig_file.display()))?
        .scene
        .with_context(|| format!("{} has no SGRF scene graph", rig_file.display()))?;
    let mut problems = Vec::new();
    let mut read_lods = Vec::new();
    for parts in models {
        let mut loaded = Vec::new();
        for p in parts {
            if p.skeleton.as_deref() != Some(skeleton.as_str()) {
                problems.push(format!("{}: rig {:?}, drawn on {skeleton}", p.name, p.skeleton));
            }
            let read = || -> Result<(PathBuf, ObjectMesh, CrsfFile)> {
                let crsf_rel = p.crsf.as_deref().context("no FileRef (.crsf)")?;
                let geom = asset_file(big_root, &p.geom).with_context(|| format!("{} not found", p.geom))?;
                let crsf = asset_file(big_root, crsf_rel).with_context(|| format!("{crsf_rel} not found"))?;
                let file =
                    CrsfFile::parse(&std::fs::read(&crsf).with_context(|| format!("reading {}", crsf.display()))?)
                        .with_context(|| format!("parsing {}", crsf.display()))?;
                Ok((geom.clone(), crate::content::models::rider_mesh(big_root, &p.geom)?, file))
            };
            match read() {
                Ok(x) => loaded.push((p, x)),
                Err(e) => problems.push(format!("{}: {e:#}", p.name)),
            }
        }
        read_lods.push(loaded);
    }
    let mut face_offsets = HashMap::new();
    for (_, (_, _, file)) in read_lods.iter().flatten() {
        for o in file.face_offsets.iter().flatten() {
            face_offsets.insert(o.joint.clone(), o.translate);
        }
    }
    let bind = bind_pose(&rig, &face_offsets);
    let mut lods = Vec::new();
    for loaded in read_lods {
        let mut parts = Vec::new();
        for (p, (geom, mesh, file)) in loaded {
            match skin_of(&file, &rig, mesh.positions.len()) {
                Ok(skin) => parts.push(GhostPart { mesh_name: p.name, geom, mesh, skin, material: p.material }),
                Err(e) => problems.push(format!("{}: {e:#}", p.name)),
            }
        }
        lods.push(parts);
    }
    if lods.first().is_none_or(|l| l.is_empty()) {
        bail!("game object {key:08x}: no LOD-0 part loaded ({})", problems.join("; "));
    }
    Ok(BotModel { game_object: key, lod_thresholds, lods, rig_file, rig, bind, light_nodes, problems })
}
