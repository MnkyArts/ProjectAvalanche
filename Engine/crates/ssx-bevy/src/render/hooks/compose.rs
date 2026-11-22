use std::collections::{BTreeMap, BTreeSet, HashMap};

use bevy::asset::AssetId;
use bevy::shader::{Shader, ShaderCache, ShaderCacheError, ShaderCacheSource, ShaderDefVal, ValidateShader};
use naga::valid::ModuleInfo;
use ssx_content::shader::{Accepted, HookFamily, Limits, check_composed, composed_source};
use wesl::mangler::{EscapeMangler, Mangler};
use wesl::syntax::{ModulePath, PathOrigin};

use super::sets::{self, Kind};

pub fn entries(family: HookFamily) -> &'static [(&'static str, Kind)] {
    match family {
        HookFamily::Object => &[
            ("render/objects/object.wesl", Kind::MainFragment),
            ("render/lighting/game_vertex.wesl", Kind::MainVertex),
            ("render/objects/object_prepass.wesl", Kind::ShadowPrepass),
            ("render/objects/object_prepass.wesl", Kind::CameraPrepass),
            ("render/objects/object_gbuffer.wesl", Kind::ShadowPrepass),
            ("render/objects/object_gbuffer.wesl", Kind::CameraPrepass),
        ],
        HookFamily::Sky => &[("render/sky/sky_dome.wesl", Kind::Own)],
    }
}

fn entry_hooks(entry: &str) -> &'static [&'static str] {
    match entry {
        "render/objects/object.wesl" | "render/objects/object_gbuffer.wesl" => &["surface"],
        "render/lighting/game_vertex.wesl" | "render/objects/object_prepass.wesl" => &["vertex_offset"],
        _ => &["surface", "vertex_offset"],
    }
}

pub fn hook_flags(accepted: &Accepted) -> Vec<ShaderDefVal> {
    [("SSX_HOOK_SURFACE", accepted.surface.is_some()), ("SSX_HOOK_VERTEX", accepted.vertex.is_some())]
        .into_iter()
        .filter(|(_, on)| *on)
        .map(|(f, _)| sets::flag(f))
        .collect()
}

pub fn def_key(defs: &[ShaderDefVal]) -> String {
    let mut parts: Vec<String> = defs
        .iter()
        .map(|d| match d {
            ShaderDefVal::Bool(n, true) => n.to_string(),
            ShaderDefVal::Bool(n, false) => format!("{n}=false"),
            ShaderDefVal::Int(n, v) => format!("{n}={v}"),
            ShaderDefVal::UInt(n, v) => format!("{n}={v}"),
        })
        .collect();
    parts.sort();
    parts.dedup();
    parts.join(" ")
}

fn module_path_of(asset_path: &str) -> ModulePath {
    let path = asset_path.trim_start_matches("embedded://");
    let path = path.strip_suffix(".wesl").unwrap_or(path);
    let mut parts = path.split('/').filter(|p| !p.is_empty()).map(str::to_string);
    let package = parts.next().unwrap_or_default();
    ModulePath { origin: PathOrigin::Package(package), components: parts.collect() }
}

pub fn hook_item(family: HookFamily, hash: u64) -> impl Fn(&str) -> Option<String> {
    let dirs: BTreeSet<&str> = entries(family).iter().map(|(e, _)| e.rsplit_once('/').map_or("", |(d, _)| d)).collect();
    let paths: Vec<ModulePath> =
        dirs.into_iter().map(|d| module_path_of(&variant_path(hash, &format!("{d}/hook.wesl")))).collect();
    move |name: &str| {
        let (path, item) = EscapeMangler.unmangle(name)?;
        paths.contains(&path).then_some(item)
    }
}

pub fn types_module(family: HookFamily) -> &'static str {
    match family {
        HookFamily::Object => "package::render::objects::hook_types",
        HookFamily::Sky => "package::render::sky::hook_types",
    }
}

pub fn variant_dir(hash: u64) -> String {
    format!("hooks/h{hash:016x}")
}

pub fn variant_path(hash: u64, entry: &str) -> String {
    format!("embedded://ssx_bevy/{}/{entry}", variant_dir(hash))
}

pub fn variant_modules(
    family: HookFamily,
    hash: u64,
    text: &str,
    source_of: &dyn Fn(&str) -> Option<String>,
) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let hook = composed_source(types_module(family), text);
    let mut dirs = BTreeSet::new();
    for (entry, _) in entries(family) {
        let path = variant_path(hash, entry);
        if !out.iter().any(|(p, _)| *p == path) {
            out.push((path, source_of(entry)?));
        }
        dirs.insert(entry.rsplit_once('/').map_or("", |(d, _)| d));
    }
    for d in dirs {
        out.push((variant_path(hash, &format!("{d}/hook.wesl")), hook.clone()));
    }
    Some(out)
}

#[derive(Clone, Debug)]
pub struct DeviceCaps {
    pub name: String,
    pub features: wgpu::Features,
    pub downlevel: wgpu::DownlevelFlags,
    pub binding_arrays: bool,
    pub globals: Vec<ShaderDefVal>,
}

impl DeviceCaps {
    pub fn of(name: &str, features: wgpu::Features, limits: &wgpu::Limits, downlevel: wgpu::DownlevelFlags) -> Self {
        let array_len = (16 + bevy::pbr::MAX_VIEW_LIGHT_PROBES) as u32;
        let binding_arrays = limits.max_binding_array_elements_per_shader_stage >= array_len
            && limits.max_binding_array_sampler_elements_per_shader_stage >= array_len
            && features.contains(
                wgpu::Features::TEXTURE_BINDING_ARRAY
                    | wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
            );
        let storage = limits.max_storage_buffers_per_shader_stage;
        let globals = vec![
            ShaderDefVal::UInt("AVAILABLE_STORAGE_BUFFER_BINDINGS".into(), storage),
            ShaderDefVal::Bool("AVAILABLE_STORAGE_BUFFER_BINDINGS__GE_3".into(), storage >= 3),
            ShaderDefVal::Bool("AVAILABLE_STORAGE_BUFFER_BINDINGS__GE_6".into(), storage >= 6),
        ];
        DeviceCaps { name: name.to_string(), features, downlevel, binding_arrays, globals }
    }

    fn capabilities(&self) -> naga::valid::Capabilities {
        wgpu_naga_bridge::features_to_naga_capabilities(self.features, self.downlevel)
    }
}

fn composed_wgsl(_: &(), source: ShaderCacheSource, _: &ValidateShader) -> Result<String, ShaderCacheError> {
    match source {
        ShaderCacheSource::Wgsl(wgsl) => Ok(wgsl),
        ShaderCacheSource::SpirV(_) => Err(ShaderCacheError::ProcessShaderError("SPIR-V".into())),
    }
}

pub fn validate(wgsl: &str, caps: &DeviceCaps) -> Result<(), String> {
    parse_validated(wgsl, caps).map(|_| ())
}

pub fn parse_validated(wgsl: &str, caps: &DeviceCaps) -> Result<(naga::Module, ModuleInfo), String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| e.emit_to_string(wgsl))?;
    let mut v = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), caps.capabilities());
    let info = v.validate(&module).map_err(|e| e.emit_to_string(wgsl))?;
    Ok((module, info))
}

const PREPASS_VIEW_BINDINGS: [u32; 4] = [0, 1, 2, 14];

pub fn view_bindings_fit(kind: Kind, wgsl: &str) -> Result<(), String> {
    if !matches!(kind, Kind::ShadowPrepass | Kind::CameraPrepass) {
        return Ok(());
    }
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| e.emit_to_string(wgsl))?;
    module_bindings_fit(kind, &module)
}

pub fn module_bindings_fit(kind: Kind, module: &naga::Module) -> Result<(), String> {
    if !matches!(kind, Kind::ShadowPrepass | Kind::CameraPrepass) {
        return Ok(());
    }
    for (_, g) in module.global_variables.iter() {
        if let Some(b) = &g.binding
            && b.group == 0
            && !PREPASS_VIEW_BINDINGS.contains(&b.binding)
        {
            return Err(format!(
                "error: `{}` binds group 0, binding {}, which the prepass view layout does not have",
                g.name.as_deref().unwrap_or("?"),
                b.binding
            ));
        }
    }
    Ok(())
}

pub struct Composer {
    cache: ShaderCache<String, ()>,
    shaders: HashMap<String, (AssetId<Shader>, Shader)>,
}

impl Composer {
    pub fn new(shaders: &[(String, AssetId<Shader>, Shader)], globals: &[ShaderDefVal]) -> Self {
        let mut cache = ShaderCache::new((), composed_wgsl);
        let mut by_path = HashMap::new();
        for (path, id, shader) in shaders {
            let mut shader = shader.clone();
            shader.shader_defs.splice(0..0, globals.iter().cloned());
            cache.set_shader(*id, shader.clone());
            by_path.insert(path.clone(), (*id, shader));
        }
        Composer { cache, shaders: by_path }
    }

    pub fn compose(&mut self, path: &str, defs: &[ShaderDefVal]) -> Result<String, String> {
        let (id, shader) = self.shaders.get(path).ok_or_else(|| format!("{path}: not a registered shader"))?.clone();
        let out = self.cache.get(0, id, defs).map(|wgsl| wgsl.as_ref().clone()).map_err(|e| e.to_string());
        self.cache.set_shader(id, shader);
        out
    }
}

#[derive(Clone, Debug)]
pub enum Scope {
    Full,
    Mesh { always: BTreeSet<&'static str>, optional: BTreeSet<&'static str> },
    Exact(Vec<(String, Vec<ShaderDefVal>)>),
}

fn module_flags(path: &str, sources: &BTreeMap<String, String>) -> BTreeSet<String> {
    let modules: BTreeSet<String> = sources.keys().cloned().collect();
    let mut seen = BTreeSet::from([path.to_string()]);
    let mut todo = vec![path.to_string()];
    let mut flags = BTreeSet::new();
    while let Some(p) = todo.pop() {
        let Some(src) = sources.get(&p) else { continue };
        flags.extend(sets::condition_flags(src));
        for import in sets::package_imports(src, &modules) {
            if seen.insert(import.clone()) {
                todo.push(import);
            }
        }
    }
    flags
}

#[derive(Clone, Debug, Default)]
pub struct CheckStats {
    pub sets: usize,
    pub validated: usize,
    pub bevy_too: usize,
    pub analysed: Option<Accepted>,
    pub entry_frame: u64,
    pub entry_work: u64,
    pub admitted: Vec<(String, String)>,
}

pub fn error_key(error: &str) -> String {
    error
        .lines()
        .map(str::trim)
        .find(|l| l.contains("error:") || l.starts_with("Shader validation error") || l.contains("parsing error"))
        .unwrap_or_else(|| error.lines().next().unwrap_or(""))
        .chars()
        .take(160)
        .collect()
}

pub fn check_variant(
    composer: &mut Composer,
    family: HookFamily,
    hash: u64,
    accepted: &Accepted,
    sources: &BTreeMap<String, String>,
    caps: &DeviceCaps,
    scope: &Scope,
) -> Result<CheckStats, Vec<String>> {
    let mut stats = CheckStats::default();
    let mut failures = Vec::new();
    let mut validated: HashMap<u64, Result<(), String>> = HashMap::new();
    let hook_defs = hook_flags(accepted);
    let item = hook_item(family, hash);
    let limits = Limits::default();
    for (index, (entry, kind)) in entries(family).iter().enumerate() {
        if let Scope::Exact(wanted) = scope
            && !wanted.iter().any(|(e, _)| e == entry)
        {
            continue;
        }
        if matches!(scope, Scope::Exact(_)) && entries(family).iter().position(|(e, _)| e == entry) != Some(index) {
            continue;
        }
        let own = format!("{}/{entry}", sets::CRATE);
        let mut flags = module_flags(&own, sources);
        flags.remove("SSX_HOOK_SURFACE");
        flags.remove("SSX_HOOK_VERTEX");
        flags.remove("SSX_INVARIANT_POSITION");
        let mut module = sets::module_model(&own, *kind, flags);
        if let Scope::Mesh { always, optional } = scope {
            module.dims.retain(|d| !d.iter().any(|f| sets::MESH_DEFS.contains(&f.as_str())));
            if *kind != Kind::Own {
                module.dims.extend(optional.iter().map(|a| vec![a.to_string()]));
                module.contract = always.iter().map(|a| a.to_string()).collect();
            }
            module.dims.sort();
        }
        let path = variant_path(hash, entry).trim_start_matches("embedded://").to_string();
        let combos = 1u64 << module.dims.len();
        let compressed: &[bool] = if module.vertex_input { &[false, true] } else { &[false] };
        let mut sets_of = Vec::new();
        let exact: Vec<Vec<ShaderDefVal>> = match scope {
            Scope::Exact(wanted) => wanted.iter().filter(|(e, _)| e == entry).map(|(_, d)| d.clone()).collect(),
            _ => Vec::new(),
        };
        for bits in (0..combos).filter(|_| exact.is_empty()) {
            for &c in compressed {
                if let Some(defs) = sets::combination(&module, caps.binding_arrays, bits, c) {
                    sets_of.push(defs);
                }
            }
        }
        if matches!(scope, Scope::Full) {
            for v in sets::variations(*kind) {
                if v.add.contains(&"CLUSTERED_DECALS_ARE_USABLE") && !caps.binding_arrays {
                    continue;
                }
                for bits in [0, combos - 1] {
                    if let Some(defs) = sets::combination(&module, caps.binding_arrays, bits, false) {
                        sets_of.push(sets::with(defs, v.add, v.remove));
                    }
                }
            }
        }
        let all: Vec<Vec<ShaderDefVal>> = if exact.is_empty() {
            sets_of
                .into_iter()
                .map(|mut defs| {
                    defs.extend(hook_defs.iter().cloned());
                    defs
                })
                .collect()
        } else {
            exact
        };
        let expected: Vec<&str> = entry_hooks(entry)
            .iter()
            .copied()
            .filter(|h| if *h == "surface" { accepted.surface.is_some() } else { accepted.vertex.is_some() })
            .collect();
        let mut found: BTreeSet<&str> = BTreeSet::new();
        for defs in all {
            stats.sets += 1;
            let result = composer.compose(&path, &defs).and_then(|wgsl| {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                wgsl.hash(&mut h);
                let key = h.finish();
                if let Some(known) = validated.get(&key) {
                    return known.clone();
                }
                stats.validated += 1;
                let r = parse_validated(&wgsl, caps).and_then(|(module, info)| {
                    module_bindings_fit(*kind, &module)?;
                    let c = check_composed(family, &module, &info, &item, &limits)
                        .map_err(|ps| format!("error: the hook as composed: {}", ps.join("; ")))?;
                    for h in &expected {
                        if if *h == "surface" { c.accepted.surface.is_some() } else { c.accepted.vertex.is_some() } {
                            found.insert(h);
                        }
                    }
                    stats.analysed = Some(match &stats.analysed {
                        Some(a) => a.max(&c.accepted),
                        None => c.accepted.clone(),
                    });
                    stats.entry_frame = stats.entry_frame.max(c.entry_frames.values().copied().max().unwrap_or(0));
                    stats.entry_work = stats.entry_work.max(c.entry_work.values().copied().max().unwrap_or(0));
                    Ok(())
                });
                validated.insert(key, r.clone());
                r
            });
            if result.is_ok() {
                stats.admitted.push((entry.to_string(), def_key(&defs)));
            }
            let Err(e) = result else { continue };
            let key = error_key(&e);
            let bevy_too = sets::counterpart(*kind).is_some_and(|c| {
                composer
                    .compose(c, &defs)
                    .and_then(|wgsl| validate(&wgsl, caps))
                    .err()
                    .is_some_and(|b| error_key(&b) == key)
            });
            if bevy_too {
                stats.bevy_too += 1;
            } else if failures.len() < 4 {
                failures.push(format!("{entry} [{}] {}: {}", caps.name, sets::describe(&defs), summary(&e)));
            }
        }
        if !matches!(scope, Scope::Exact(_)) {
            for h in expected.iter().filter(|h| !found.contains(*h)) {
                failures.push(format!("{entry} [{}]: the hook's `{h}` is in no composed module of it", caps.name));
            }
        }
    }
    if failures.is_empty() { Ok(stats) } else { Err(failures) }
}

fn summary(report: &str) -> String {
    let lines: Vec<&str> = report.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let head = lines.iter().find(|l| l.starts_with("error")).or(lines.first()).copied().unwrap_or("");
    let notes: Vec<&str> = lines.iter().filter_map(|l| l.strip_prefix("= note:")).map(str::trim).collect();
    if notes.is_empty() { head.to_string() } else { format!("{head} ({})", notes.join("; ")) }
}
