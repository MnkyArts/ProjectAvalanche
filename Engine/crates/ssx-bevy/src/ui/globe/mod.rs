mod backdrop;
mod dump;
mod midground;
mod render;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use anyhow::{Context, Result};
use bevy::prelude::{App, Last, Plugin, Res, ResMut, Resource, SystemSet, Time, error, info};
use bevy::render::RenderApp;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use ssx_assets::career::globe::GlobeData;
use ssx_assets::career::globe::look::PostFxState;
use ssx_assets::postfx::ColourCubeBlend;
use ssx_assets::vault_db::Material;
use ssx_ui::game::fe::globe_look::LightState;
use ssx_ui::game::fe::globe_scene::{GlobeFrame, M4};

pub use midground::{Blocks, MidgroundAssets, MipTexture, uniform as midground_uniform};

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct GlobeSystems;

pub struct GlobePlugin;

impl Plugin for GlobePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractResourcePlugin::<GlobeView>::default())
            .init_resource::<GlobeView>()
            .init_resource::<MidgroundLoad>()
            .init_resource::<PostCache>()
            .add_systems(Last, (load_midground, update_post));
        render::build(app);
    }
}

#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(RenderApp)]
pub struct GlobeView {
    pub assets: Option<Arc<GlobeAssets>>,
    pub frame: Option<GlobeFrame>,
    pub raw_scale: Option<f32>,
    pub midground: Option<Arc<MidgroundAssets>>,
    pub post: Option<Arc<FePost>>,
    pub dt: f32,
    pub dump: Option<Arc<PathBuf>>,
    pub backdrop_movie: Option<bevy::prelude::Handle<bevy::prelude::Image>>,
}

#[derive(Clone, Debug, Default)]
pub struct FePost {
    pub bloom: bool,
    pub blur: [f32; 2],
    pub vignette: Option<([f32; 3], [f32; 2])>,
    pub final_modifier: [f32; 2],
    pub lut: Arc<Vec<u8>>,
    pub lut_version: u64,
}

#[derive(Resource, Default)]
struct PostCache {
    cubes: Option<Vec<ColourCubeBlend>>,
    lut: Arc<Vec<u8>>,
    version: u64,
}

fn update_post(mut view: ResMut<GlobeView>, mut cache: ResMut<PostCache>, time: Res<Time>) {
    view.dt = time.delta_secs();
    let (Some(assets), Some(frame)) = (view.assets.clone(), view.frame.as_ref()) else { return };
    let state: PostFxState = assets.data.postfx.state(&frame.postfx);
    if cache.cubes.as_ref() != Some(&state.cubes) {
        match assets.data.postfx.cube(&state) {
            Ok(b) => {
                cache.lut = Arc::new(b.cube.rgba8());
                cache.version += 1;
            }
            Err(e) => error!("globe: the colour cube of {:?}: {e:#}", frame.postfx),
        }
        cache.cubes = Some(state.cubes.clone());
    }
    view.post = Some(Arc::new(FePost {
        bloom: state.bloom(),
        blur: state.blur(),
        vignette: state.vignette(),
        final_modifier: state.final_modifier(),
        lut: cache.lut.clone(),
        lut_version: cache.version,
    }));
}

#[derive(Resource, Default)]
struct MidgroundLoad {
    pending: Option<(u32, std::sync::Mutex<Receiver<anyhow::Result<MidgroundAssets>>>)>,
}

fn load_midground(mut view: ResMut<GlobeView>, mut load: ResMut<MidgroundLoad>) {
    let want = view.frame.as_ref().map_or(0, |f| f.midground_range);
    if let Some((range, rx)) = load.pending.as_ref() {
        let got = rx.lock().ok().and_then(|r| r.try_recv().ok());
        if let Some(r) = got {
            let range = *range;
            load.pending = None;
            match r {
                Ok(m) => {
                    let textures: Vec<&MipTexture> = m
                        .parts
                        .iter()
                        .flat_map(|p| &p.textures)
                        .chain(m.waves.iter().flat_map(|w| &w.textures))
                        .collect();
                    let bytes = midground::texture_bytes(textures.iter().map(|t| (&t.levels[..], t.blocks.as_ref())));
                    info!(
                        "globe: the mid-ground of range {range:#x} loaded ({} meshes, {} wave meshes; {})",
                        m.parts.len(),
                        m.waves.len(),
                        midground::bytes_line(textures.len(), bytes)
                    );
                    view.midground = Some(Arc::new(m));
                }
                Err(e) => error!("globe: the mid-ground of range {range:#x}: {e:#}"),
            }
        }
        return;
    }
    if want == 0 || view.midground.as_ref().is_some_and(|m| m.range == want) {
        return;
    }
    let Some(assets) = view.assets.clone() else { return };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(MidgroundAssets::load(&assets.big_root, &assets.data, want));
    });
    load.pending = Some((want, std::sync::Mutex::new(rx)));
}

#[derive(Clone, Debug, Default)]
pub struct GlobeMesh {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct GlobeTexture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub blocks: Option<midground::Blocks>,
}

#[derive(Clone, Debug)]
pub struct GlobeAssets {
    pub earth: GlobeMesh,
    pub halo: GlobeMesh,
    pub diffuse: GlobeTexture,
    pub relief: GlobeTexture,
    pub frost: GlobeTexture,
    pub earth_material: Material,
    pub halo_material: Material,
    pub background: GlobeTexture,
    pub sky_textures: BTreeMap<String, MipTexture>,
    pub particle_textures: BTreeMap<String, MipTexture>,
    pub data: Arc<GlobeData>,
    pub big_root: PathBuf,
}

impl GlobeAssets {
    pub fn texture_ids(&self) -> Vec<ssx_content::ContentId> {
        let materials = [&self.earth_material, &self.halo_material];
        let paths = materials.iter().flat_map(|m| m.textures.values()).chain(self.sky_textures.keys());
        let mut ids: Vec<ssx_content::ContentId> = paths
            .filter_map(|rel| ssx_assets::content::textures::texture_id(&rel.replace('\\', "/")))
            .chain(ssx_assets::content::textures::texture_id(BACKGROUND))
            .chain(self.particle_textures.keys().filter_map(|n| ssx_assets::particles::texture_id(n)))
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    pub fn texture_line(&self) -> String {
        let singles = [&self.diffuse, &self.relief, &self.frost, &self.background];
        let one: Vec<Vec<Vec<u8>>> = singles.iter().map(|t| vec![t.rgba.clone()]).collect();
        let mips = self.sky_textures.values().chain(self.particle_textures.values());
        let all: Vec<(&[Vec<u8>], Option<&Blocks>)> = one
            .iter()
            .zip(singles)
            .map(|(l, t)| (&l[..], t.blocks.as_ref()))
            .chain(mips.map(|t| (&t.levels[..], t.blocks.as_ref())))
            .collect();
        midground::bytes_line(all.len(), midground::texture_bytes(all))
    }
}

const BACKGROUND: &str = "frontend/textures/fe_background";

fn texture(big_root: &Path, rel: &str) -> Result<GlobeTexture> {
    let id = ssx_assets::content::textures::texture_id(rel).with_context(|| format!("{rel:?}: not a texture id"))?;
    let t = midground::texture_by_id(big_root, &id, true)?;
    let rgba = t.levels.into_iter().next().context("no texture level")?;
    Ok(GlobeTexture { width: t.width, height: t.height, rgba, blocks: t.blocks })
}

fn mesh(big_root: &Path, rel: &str) -> Result<GlobeMesh> {
    let m = crate::render::sky::midground::static_mesh(big_root, rel)?;
    let uvs = m.uv0.clone().unwrap_or_else(|| vec![[0.0, 0.0]; m.positions.len()]);
    Ok(GlobeMesh { positions: m.positions, uvs, indices: m.indices })
}

impl GlobeAssets {
    pub fn load(big_root: &Path, data: &GlobeData) -> Result<Self> {
        let earth_mesh = data.globe_mesh("forward").context("no earth mesh (technique forward)")?;
        let halo_mesh = data.globe_mesh("forwardHalo").context("no halo mesh (technique forwardHalo)")?;
        let tex = |m: &Material, p: &str| -> Result<GlobeTexture> {
            texture(big_root, &m.textures.get(p).with_context(|| format!("material without {p}"))?.replace('\\', "/"))
        };
        let mut sky_textures = BTreeMap::new();
        for p in data.lighting.values() {
            for t in ["cloudDensityTexture", "cloudDensityNormalTexture", "dirLightTexture"] {
                if let Some(rel) = p.textures.get(t).map(|r| r.replace('\\', "/"))
                    && !sky_textures.contains_key(&rel)
                {
                    sky_textures.insert(rel.clone(), midground::mip_texture(big_root, &rel)?);
                }
            }
        }
        let mut particle_textures = BTreeMap::new();
        for fx in &data.globe_particles {
            for a in fx.effect.groups.iter().flat_map(|g| &g.actions).filter(|a| a.class_name == "ParticleActionRender")
            {
                let Some(name) = a.params.iter().find(|p| p.name == "sTexture").map(|p| p.raw.clone()) else {
                    continue;
                };
                if let std::collections::btree_map::Entry::Vacant(e) = particle_textures.entry(name) {
                    let id = ssx_assets::particles::texture_id(e.key())
                        .with_context(|| format!("{:?}: not a texture id", e.key()))?;
                    e.insert(midground::texture_by_id(big_root, &id, false).with_context(|| id.to_string())?);
                }
            }
        }
        Ok(GlobeAssets {
            background: texture(big_root, BACKGROUND)?,
            sky_textures,
            particle_textures,
            earth: mesh(big_root, &earth_mesh.geometry.replace('\\', "/"))?,
            halo: mesh(big_root, &halo_mesh.geometry.replace('\\', "/"))?,
            diffuse: tex(&earth_mesh.material, "diffuseTexture")?,
            relief: tex(&earth_mesh.material, "reliefTexture")?,
            frost: tex(&halo_mesh.material, "diffuseTexture")?,
            earth_material: earth_mesh.material.clone(),
            halo_material: halo_mesh.material.clone(),
            data: Arc::new(data.clone()),
            big_root: big_root.to_path_buf(),
        })
    }
}

pub const UNIFORM_FLOATS: usize = 172;

fn scatter(radius: f32, atmosphere: f32) -> [[f32; 4]; 3] {
    let (kr, km, esun) = (0.0025f32, 0.0015f32, 15.0f32);
    let inv = |l: f32| l.powi(-4);
    let scale = 1.0 / (atmosphere - radius);
    [
        [inv(0.65), inv(0.57), inv(0.475), 0.0],
        [kr * esun, km * esun, kr * 4.0 * std::f32::consts::PI, km * 4.0 * std::f32::consts::PI],
        [scale, 0.25, scale / 0.25, 0.0],
    ]
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 { [v[0] / l, v[1] / l, v[2] / l] } else { v }
}

fn to_view(v: [f32; 3], view: &M4) -> [f32; 3] {
    norm(std::array::from_fn(|j| v[0] * view[0][j] + v[1] * view[1][j] + v[2] * view[2][j]))
}

pub fn uniform(frame: &GlobeFrame, m: &Material, light: &LightState, radius: f32, atmosphere: f32) -> Vec<f32> {
    let f = |n: &str| m.float(n).unwrap_or(0.0);
    let mv = |n: &str| m.vectors.get(n).cloned().unwrap_or_default();
    let lf = |n: &str| light.float(n).unwrap_or(0.0);
    let lv = |n: &str| light.vector(n).unwrap_or([0.0; 4]);
    let d = lv("dirLight_Direction");
    let (sun, bounce) = (light.sun, light.bounce);
    let v3 = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
    let [inv_wl, keys, scales] = scatter(radius, atmosphere);
    let halo_uv = mv("haloUVOffset");
    let halo_scale = m.float("haloScale").unwrap_or(1.0);
    let hl = mv("highlightColour");
    let vectors: [[f32; 4]; 27] = [
        v3(frame.camera_pos),
        frame.project_cam,
        frame.globe_r,
        frame.atmos_r,
        d,
        lv("dirLight_Colour"),
        v3(to_view(sun, &frame.view)),
        v3(to_view(bounce, &frame.view)),
        v3(frame.highlight_view),
        lv("rampColor0"),
        lv("rampColor1"),
        lv("rampColor2"),
        lv("globalSpecularColour"),
        [lf("rampIntensity"), lf("rampRimFalloff"), lf("rampRimInLight"), lf("rampRimInShadow")],
        [lf("rampBounceAtt"), lf("rampBounceSpread"), lf("globalLightingMult"), frame.game_time],
        [f("dirLight_Specular_Power"), f("dirLight_Fresnel"), f("reliefDepth"), f("heightToNormalScale")],
        [f("cloudAnimSpeed"), f("cloudIntensity"), f("cloudLightDstance"), f("cloudShadowOffsetScalar")],
        [f("cloudShadowIntensity"), f("seaSpecularFalloff"), f("atmosSkyMultiplier"), f("atmosGlobeMultiplier")],
        inv_wl,
        keys,
        scales,
        // TODO(guess): the relief loops' counts are the shader's int literals [15, 32] in register order (i30 = 15 binary steps, i31 = 32 cone steps; `globe_default.fxo` `forward` PS, the int list of the shader's definition table); settle: the effect loader's int-literal upload.
        [4.5, 5.0, 32.0, 15.0],
        [
            hl.first().copied().unwrap_or(0.0),
            hl.get(1).copied().unwrap_or(0.0),
            hl.get(2).copied().unwrap_or(0.0),
            hl.get(3).copied().unwrap_or(0.0),
        ],
        [f("highlightRippleFreq"), f("highlightLineWidth"), f("highlightLineSharpness"), f("highlightAnimSpeed")],
        [f("highlightCursorCore"), f("highlightCursorExtent"), 0.0, 0.0],
        [
            halo_uv.first().copied().unwrap_or(0.0),
            halo_uv.get(1).copied().unwrap_or(0.0),
            f("haloIntensity"),
            halo_scale,
        ],
        [0.0; 4],
    ];
    let mut out = Vec::with_capacity(UNIFORM_FLOATS);
    for m in [&frame.w_inv, &frame.world_view, &frame.proj, &frame.view_inverse] {
        out.extend(m.iter().flatten());
    }
    out.extend(vectors.iter().flatten());
    out
}
