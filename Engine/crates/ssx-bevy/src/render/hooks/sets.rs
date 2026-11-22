use std::collections::BTreeSet;

use bevy::shader::ShaderDefVal;

pub const CRATE: &str = "ssx_bevy";

pub fn strip_comments(src: &str) -> String {
    let (bytes, mut out, mut i, mut depth) = (src.as_bytes(), String::with_capacity(src.len()), 0, 0usize);
    while i < bytes.len() {
        if bytes[i..].starts_with(b"/*") {
            depth += 1;
            i += 2;
        } else if depth > 0 && bytes[i..].starts_with(b"*/") {
            depth -= 1;
            i += 2;
        } else if depth > 0 {
            i += 1;
        } else if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else {
            let ch = src[i..].chars().next().expect("a char");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

pub fn condition_flags(src: &str) -> BTreeSet<String> {
    let code = strip_comments(src);
    let mut flags = BTreeSet::new();
    for attr in ["@if", "@elif"] {
        for (at, _) in code.match_indices(attr) {
            let rest = code[at + attr.len()..].trim_start();
            let Some(rest) = rest.strip_prefix('(') else { continue };
            let (mut level, mut end) = (1, rest.len());
            for (k, ch) in rest.char_indices() {
                match ch {
                    '(' => level += 1,
                    ')' => {
                        level -= 1;
                        if level == 0 {
                            end = k;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            for word in rest[..end].split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && word != "true" && word != "false"
                {
                    flags.insert(word.to_string());
                }
            }
        }
    }
    flags
}

pub fn package_imports(src: &str, modules: &BTreeSet<String>) -> BTreeSet<String> {
    let code = strip_comments(src);
    let mut out = BTreeSet::new();
    for (at, _) in code.match_indices("package::") {
        let path: String = code[at + "package::".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
            .collect();
        let parts: Vec<&str> = path.split("::").filter(|p| !p.is_empty()).collect();
        if let Some(module) = (1..=parts.len())
            .rev()
            .map(|k| format!("{CRATE}/{}.wesl", parts[..k].join("/")))
            .find(|m| modules.contains(m))
        {
            out.insert(module);
        }
    }
    out
}

pub fn has_entry_point(src: &str) -> bool {
    let code = strip_comments(src);
    ["@vertex", "@fragment", "@compute"].iter().any(|e| code.contains(e))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    MainVertex,
    MainFragment,
    ShadowPrepass,
    CameraPrepass,
    Own,
}

pub fn kinds_of(module: &str) -> Option<&'static [Kind]> {
    let rel = module.strip_prefix(&format!("{CRATE}/")).expect("a crate module");
    Some(match rel {
        "render/lighting/game_vertex.wesl" => &[Kind::MainVertex],
        "render/terrain/proxy_vertex.wesl" => &[Kind::MainVertex],
        "render/terrain/terrain.wesl"
        | "render/objects/object.wesl"
        | "render/objects/foreground.wesl"
        | "render/terrain/proxy.wesl"
        | "ride/heli_translucent.wesl"
        | "rider/character.wesl"
        | "rider/ghost.wesl"
        | "render/geotag.wesl"
        | "particles/shadow_query.wesl" => &[Kind::MainFragment],
        "render/terrain/terrain_prepass.wesl"
        | "render/terrain/terrain_normal.wesl"
        | "render/objects/object_gbuffer.wesl"
        | "render/objects/object_prepass.wesl"
        | "rider/character_gbuffer.wesl" => &[Kind::ShadowPrepass, Kind::CameraPrepass],
        _ if rel.starts_with("render/post/")
            || rel.starts_with("render/sky/")
            || rel.starts_with("particles/render/")
            || rel.starts_with("ui/")
            || rel.ends_with(".wgsl") =>
        {
            &[Kind::Own]
        }
        _ => return None,
    })
}

pub fn flag(name: &str) -> ShaderDefVal {
    ShaderDefVal::Bool(name.to_string().into(), true)
}

pub fn uint(name: &str, value: u32) -> ShaderDefVal {
    ShaderDefVal::UInt(name.to_string().into(), value)
}

pub fn def_name(def: &ShaderDefVal) -> &str {
    match def {
        ShaderDefVal::Bool(n, _) | ShaderDefVal::Int(n, _) | ShaderDefVal::UInt(n, _) => n,
    }
}

pub fn base_defs(kind: Kind, binding_arrays: bool) -> Vec<ShaderDefVal> {
    let mut defs = match kind {
        Kind::MainVertex | Kind::MainFragment => vec![
            flag("MESH_PIPELINE"),
            flag("VERTEX_OUTPUT_INSTANCE_INDEX"),
            flag("VIEW_PROJECTION_NONSTANDARD"),
            flag("SHADOW_FILTER_METHOD_GAUSSIAN"),
            ShaderDefVal::Int("SCREEN_SPACE_SPECULAR_TRANSMISSION_BLUR_TAPS".into(), 4),
            flag("IRRADIANCE_VOLUMES_ARE_USABLE"),
            uint("MATERIAL_BIND_GROUP", 3),
        ],
        Kind::ShadowPrepass => vec![
            flag("PREPASS_PIPELINE"),
            flag("VERTEX_OUTPUT_INSTANCE_INDEX"),
            flag("DEPTH_PREPASS"),
            flag("VIEW_PROJECTION_ORTHOGRAPHIC"),
            uint("MATERIAL_BIND_GROUP", 3),
            uint("SSX_LIGHT_BINDING", 100),
        ],
        Kind::CameraPrepass => vec![
            flag("PREPASS_PIPELINE"),
            flag("VERTEX_OUTPUT_INSTANCE_INDEX"),
            flag("DEPTH_PREPASS"),
            flag("VIEW_PROJECTION_NONSTANDARD"),
            uint("MATERIAL_BIND_GROUP", 3),
            uint("SSX_LIGHT_BINDING", 100),
        ],
        Kind::Own => vec![],
    };
    if kind == Kind::MainVertex {
        defs.push(uint("SSX_LIGHT_BINDING", 100));
    }
    if binding_arrays {
        match kind {
            Kind::MainVertex | Kind::MainFragment => {
                defs.extend([flag("MULTIPLE_LIGHT_PROBES_IN_ARRAY"), flag("MULTIPLE_LIGHTMAPS_IN_ARRAY")])
            }
            Kind::ShadowPrepass | Kind::CameraPrepass => defs.push(flag("MULTIPLE_LIGHTMAPS_IN_ARRAY")),
            Kind::Own => {}
        }
    }
    defs
}

pub const MESH_DEFS: [&str; 9] = [
    "VERTEX_POSITIONS",
    "VERTEX_NORMALS",
    "VERTEX_UVS_A",
    "VERTEX_UVS_B",
    "VERTEX_TANGENTS",
    "VERTEX_COLORS",
    "SKINNED",
    "MORPH_TARGETS",
    "VISIBILITY_RANGE_DITHER",
];

pub const COMPRESSIBLE: [&str; 5] =
    ["VERTEX_POSITIONS", "VERTEX_NORMALS", "VERTEX_UVS_A", "VERTEX_UVS_B", "VERTEX_TANGENTS"];

pub struct Variation {
    pub add: &'static [&'static str],
    pub remove: &'static [&'static str],
}

pub const fn var(add: &'static [&'static str], remove: &'static [&'static str]) -> Variation {
    Variation { add, remove }
}

pub const MAIN_VARIATIONS: &[Variation] = &[
    var(&["MULTISAMPLED"], &[]),
    var(&["DEPTH_PREPASS"], &[]),
    var(&["NORMAL_PREPASS", "LOAD_PREPASS_NORMALS"], &[]),
    var(&["DEPTH_PREPASS", "NORMAL_PREPASS"], &[]),
    var(&["MOTION_VECTOR_PREPASS"], &[]),
    var(&["MOTION_VECTOR_PREPASS", "HAS_PREVIOUS_SKIN", "HAS_PREVIOUS_MORPH", "SKINNED", "MORPH_TARGETS"], &[]),
    var(&["DEFERRED_PREPASS"], &[]),
    var(&["SCREEN_SPACE_AMBIENT_OCCLUSION"], &[]),
    var(&["TONEMAP_IN_SHADER", "TONEMAP_METHOD_TONY_MC_MAPFACE"], &[]),
    var(&["TONEMAP_IN_SHADER", "TONEMAP_METHOD_AGX", "DEBAND_DITHER"], &[]),
    var(&["MAY_DISCARD"], &[]),
    var(&["ENVIRONMENT_MAP"], &[]),
    var(&["IRRADIANCE_VOLUME"], &[]),
    var(&["LIGHTMAP", "LIGHTMAP_BICUBIC_SAMPLING"], &[]),
    var(&["TEMPORAL_JITTER"], &[]),
    var(&["DISTANCE_FOG"], &[]),
    var(&["ATMOSPHERE"], &[]),
    var(&["OIT_ENABLED"], &[]),
    var(&["CONTACT_SHADOWS"], &[]),
    var(&["VIEW_TRANSMISSION_TEXTURE"], &[]),
    var(&["PREMULTIPLY_ALPHA", "BLEND_PREMULTIPLIED_ALPHA"], &[]),
    var(&["PREMULTIPLY_ALPHA", "BLEND_MULTIPLY"], &[]),
    var(&["ALPHA_TO_COVERAGE"], &[]),
    var(&["SHADOW_FILTER_METHOD_HARDWARE_2X2"], &["SHADOW_FILTER_METHOD_GAUSSIAN"]),
    var(&["SHADOW_FILTER_METHOD_TEMPORAL"], &["SHADOW_FILTER_METHOD_GAUSSIAN"]),
    var(&["VIEW_PROJECTION_PERSPECTIVE"], &["VIEW_PROJECTION_NONSTANDARD"]),
    var(&["VIEW_PROJECTION_ORTHOGRAPHIC"], &["VIEW_PROJECTION_NONSTANDARD"]),
    var(&["CLUSTERED_DECALS_ARE_USABLE"], &[]),
    var(&["BLUE_NOISE_TEXTURE", "DFG_LUT", "AREA_LIGHT_LUTS"], &[]),
    var(
        &[
            "PBR_TRANSMISSION_TEXTURES_SUPPORTED",
            "PBR_MULTI_LAYER_MATERIAL_TEXTURES_SUPPORTED",
            "PBR_ANISOTROPY_TEXTURE_SUPPORTED",
            "PBR_SPECULAR_TEXTURES_SUPPORTED",
        ],
        &[],
    ),
];

pub const PREPASS_VARIATIONS: &[Variation] = &[
    var(&["MAY_DISCARD"], &[]),
    var(&["BLEND_ALPHA"], &[]),
    var(&["BLEND_PREMULTIPLIED_ALPHA"], &[]),
    var(&["LIGHTMAP", "LIGHTMAP_BICUBIC_SAMPLING"], &[]),
    var(
        &[
            "MOTION_VECTOR_PREPASS",
            "MOTION_VECTOR_PREPASS_OR_DEFERRED_PREPASS",
            "PREPASS_FRAGMENT",
            "HAS_PREVIOUS_SKIN",
            "SKINNED",
        ],
        &[],
    ),
    var(
        &[
            "MOTION_VECTOR_PREPASS",
            "MOTION_VECTOR_PREPASS_OR_DEFERRED_PREPASS",
            "PREPASS_FRAGMENT",
            "HAS_PREVIOUS_MORPH",
            "MORPH_TARGETS",
        ],
        &[],
    ),
    var(&["VIEW_PROJECTION_PERSPECTIVE"], &["VIEW_PROJECTION_ORTHOGRAPHIC"]),
    var(&["VIEW_PROJECTION_NONSTANDARD"], &["VIEW_PROJECTION_ORTHOGRAPHIC"]),
];

pub const CAMERA_PREPASS_VARIATIONS: &[Variation] = &[
    var(&["MAY_DISCARD"], &[]),
    var(&["LIGHTMAP", "LIGHTMAP_BICUBIC_SAMPLING"], &[]),
    var(&["VIEW_PROJECTION_PERSPECTIVE"], &["VIEW_PROJECTION_NONSTANDARD"]),
];

pub struct Unproducible {
    pub why: &'static str,
    pub add: Vec<&'static str>,
    pub remove: Vec<&'static str>,
}

pub struct Module {
    pub path: String,
    pub kind: Kind,
    pub dims: Vec<Vec<String>>,
    pub contract: Vec<String>,
    pub unproducible: Vec<Unproducible>,
    pub vertex_input: bool,
}

pub fn mesh_contract(path: &str) -> &'static [&'static str] {
    match path {
        "ssx_bevy/render/terrain/terrain.wesl"
        | "ssx_bevy/render/terrain/terrain_prepass.wesl"
        | "ssx_bevy/render/terrain/terrain_normal.wesl" => &["VERTEX_COLORS", "VERTEX_UVS_A", "VERTEX_UVS_B"],
        _ => &[],
    }
}

pub fn counterpart(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::MainVertex => Some("bevy_pbr/render/mesh.wesl"),
        Kind::MainFragment => Some("bevy_pbr/render/pbr.wesl"),
        Kind::ShadowPrepass | Kind::CameraPrepass => Some("bevy_pbr/prepass/prepass.wesl"),
        Kind::Own => None,
    }
}

pub fn module_model(path: &str, kind: Kind, mut flags: BTreeSet<String>) -> Module {
    let mut unproducible = Vec::new();
    let mut dims: Vec<Vec<String>> = Vec::new();
    if kind != Kind::Own {
        flags.extend(MESH_DEFS.iter().map(|d| d.to_string()));
    }
    flags.remove("VERTEX_UVS");
    if kind != Kind::Own && flags.remove("VERTEX_OUTPUT_INSTANCE_INDEX") {
        unproducible.push(Unproducible {
            why: "Bevy 0.20's mesh and prepass pipelines always set VERTEX_OUTPUT_INSTANCE_INDEX \
                  (mesh.rs 3401, prepass/mod.rs 449)",
            add: vec![],
            remove: vec!["VERTEX_OUTPUT_INSTANCE_INDEX"],
        });
    }
    if flags.remove("NO_ARRAY_TEXTURES_SUPPORT") {
        unproducible.push(Unproducible {
            why: "NO_ARRAY_TEXTURES_SUPPORT is a global def of wasm32 WebGL builds only (pipeline_cache.rs 249)",
            add: vec!["NO_ARRAY_TEXTURES_SUPPORT"],
            remove: vec![],
        });
    }
    if kind == Kind::MainVertex && flags.remove("SSX_LOCAL_LIGHTS") {
        unproducible.push(Unproducible {
            why: "specialize_game_vertex sets SSX_LOCAL_LIGHTS on the fragment stage only (render/lighting/game.rs)",
            add: vec!["SSX_LOCAL_LIGHTS"],
            remove: vec![],
        });
    }
    if flags.contains("SSX_RAW_NORMAL") && flags.contains("SSX_TERRAIN_RIPPLE") {
        flags.remove("SSX_RAW_NORMAL");
        flags.remove("SSX_TERRAIN_RIPPLE");
        dims.push(vec!["SSX_RAW_NORMAL".to_string(), "SSX_TERRAIN_RIPPLE".to_string()]);
        for one in ["SSX_RAW_NORMAL", "SSX_TERRAIN_RIPPLE"] {
            unproducible.push(Unproducible {
                why: "the terrain's specialize sets SSX_RAW_NORMAL and SSX_TERRAIN_RIPPLE together",
                add: vec![one],
                remove: vec![],
            });
        }
    }
    if kind == Kind::ShadowPrepass {
        for derived in
            ["NORMAL_PREPASS_OR_DEFERRED_PREPASS", "MOTION_VECTOR_PREPASS_OR_DEFERRED_PREPASS", "PREPASS_FRAGMENT"]
        {
            flags.remove(derived);
        }
        flags.extend(["NORMAL_PREPASS", "MOTION_VECTOR_PREPASS", "UNCLIPPED_DEPTH_ORTHO_EMULATION"].map(String::from));
    }
    if kind == Kind::CameraPrepass {
        for derived in [
            "NORMAL_PREPASS_OR_DEFERRED_PREPASS",
            "MOTION_VECTOR_PREPASS_OR_DEFERRED_PREPASS",
            "PREPASS_FRAGMENT",
            "MOTION_VECTOR_PREPASS",
            "UNCLIPPED_DEPTH_ORTHO_EMULATION",
        ] {
            flags.remove(derived);
        }
        flags.insert("NORMAL_PREPASS".to_string());
    }
    let contract: Vec<String> = mesh_contract(path).iter().map(|c| c.to_string()).collect();
    for c in &contract {
        flags.remove(c);
    }
    dims.extend(flags.into_iter().map(|f| vec![f]));
    dims.sort();
    let vertex_input = kind == Kind::MainVertex || path.ends_with("terrain_prepass.wesl");
    Module { path: path.to_string(), kind, dims, contract, unproducible, vertex_input }
}

pub fn combination(module: &Module, binding_arrays: bool, bits: u64, compressed: bool) -> Option<Vec<ShaderDefVal>> {
    let mut on: BTreeSet<&str> = module.contract.iter().map(String::as_str).collect();
    for (k, dim) in module.dims.iter().enumerate() {
        if bits >> k & 1 == 1 {
            on.extend(dim.iter().map(String::as_str));
        }
    }
    if module.kind == Kind::CameraPrepass {
        let normal = on.contains("NORMAL_PREPASS");
        if !normal && (on.contains("VERTEX_NORMALS") || on.contains("VERTEX_TANGENTS")) {
            return None;
        }
        if normal {
            on.insert("NORMAL_PREPASS_OR_DEFERRED_PREPASS");
            on.insert("PREPASS_FRAGMENT");
        }
    }
    if module.kind == Kind::ShadowPrepass {
        let normal = on.contains("NORMAL_PREPASS");
        if !normal && (on.contains("VERTEX_NORMALS") || on.contains("VERTEX_TANGENTS")) {
            return None;
        }
        if normal {
            on.insert("NORMAL_PREPASS_OR_DEFERRED_PREPASS");
        }
        if on.contains("MOTION_VECTOR_PREPASS") {
            on.insert("MOTION_VECTOR_PREPASS_OR_DEFERRED_PREPASS");
        }
        if normal || on.contains("MOTION_VECTOR_PREPASS") || on.contains("UNCLIPPED_DEPTH_ORTHO_EMULATION") {
            on.insert("PREPASS_FRAGMENT");
        }
    }
    let produced =
        |flag: &str, mesh: &str| !on.contains(flag) || (on.contains("MOTION_VECTOR_PREPASS") && on.contains(mesh));
    if !produced("HAS_PREVIOUS_SKIN", "SKINNED") || !produced("HAS_PREVIOUS_MORPH", "MORPH_TARGETS") {
        return None;
    }
    if on.contains("VERTEX_UVS_A") || on.contains("VERTEX_UVS_B") {
        on.insert("VERTEX_UVS");
    }
    let mut defs = base_defs(module.kind, binding_arrays);
    defs.extend(on.iter().map(|d| flag(d)));
    if compressed {
        defs.extend(COMPRESSIBLE.iter().filter(|a| on.contains(*a)).map(|a| flag(&format!("{a}_COMPRESSED"))));
    }
    Some(defs)
}

pub fn with(defs: Vec<ShaderDefVal>, add: &[&str], remove: &[&str]) -> Vec<ShaderDefVal> {
    let mut out: Vec<ShaderDefVal> =
        defs.into_iter().filter(|d| !remove.contains(&def_name(d)) && !add.contains(&def_name(d))).collect();
    out.extend(add.iter().map(|d| flag(d)));
    if add.contains(&"TONEMAP_IN_SHADER") {
        out.push(uint("TONEMAPPING_LUT_TEXTURE_BINDING_INDEX", bevy::pbr::TONEMAPPING_LUT_TEXTURE_BINDING_INDEX));
        out.push(uint("TONEMAPPING_LUT_SAMPLER_BINDING_INDEX", bevy::pbr::TONEMAPPING_LUT_SAMPLER_BINDING_INDEX));
    }
    out
}

pub fn variations(kind: Kind) -> &'static [Variation] {
    match kind {
        Kind::MainVertex | Kind::MainFragment => MAIN_VARIATIONS,
        Kind::ShadowPrepass => PREPASS_VARIATIONS,
        Kind::CameraPrepass => CAMERA_PREPASS_VARIATIONS,
        Kind::Own => &[],
    }
}

pub fn describe(defs: &[ShaderDefVal]) -> String {
    let mut names: Vec<String> = defs
        .iter()
        .map(|d| match d {
            ShaderDefVal::Bool(n, true) => n.to_string(),
            ShaderDefVal::Bool(n, false) => format!("!{n}"),
            ShaderDefVal::Int(n, v) => format!("{n}={v}"),
            ShaderDefVal::UInt(n, v) => format!("{n}={v}"),
        })
        .collect();
    names.sort();
    names.join(" ")
}
