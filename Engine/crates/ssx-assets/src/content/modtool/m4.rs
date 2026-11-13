use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ssx_content::check::{Checked, Particles, Severity, locate_key};
use ssx_content::lighting::{FieldType, misfit};
use ssx_content::manifest::Manifest;
use ssx_content::particles::SprayTable;
use ssx_content::shader::HookFamily;
use ssx_content::sky::SkyKind;
use ssx_content::{BASE_NAMESPACE, ContentId, Kind, PORT_NAMESPACE};

use super::check::push;
use crate::vault_db::{fnv1, with_attribdb};

fn text(c: &Checked, f: &str) -> String {
    c.tree
        .as_ref()
        .and_then(|t| t.read(f, ssx_content::limits::MAX_RECORD_BYTES).ok())
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default()
}

fn known(id: &ContentId, m: &Manifest, base: Option<&BTreeSet<ContentId>>) -> Option<bool> {
    match id.namespace() {
        BASE_NAMESPACE => base.map(|b| b.contains(id)),
        PORT_NAMESPACE => Some(*id == ssx_content::sky::scattering_id()),
        ns if ns == m.resource.name => Some(m.add.contains_key(id)),
        ns if m.dependencies.require.iter().any(|d| d.name == ns) => None,
        _ => Some(false),
    }
}

fn check_id(c: &mut Checked, m: &Manifest, base: Option<&BTreeSet<ContentId>>, file: &str, key: &str, id: &ContentId) {
    if known(id, m, base) == Some(false) {
        let at = locate_key(&text(c, file), key);
        push(
            c,
            Severity::Error,
            file,
            at,
            format!(
                "{key} = {id}: no such id (the base game's, the port's, this package's [add], or a required package's)"
            ),
        );
    }
}

pub(super) fn named_ids(c: &Checked) -> Vec<ContentId> {
    let mut out = Vec::new();
    for (_, _, _, r) in &c.m4.lighting {
        out.extend(r.textures.values().cloned());
        out.extend(r.sky.clone());
    }
    for (_, _, _, r) in &c.m4.postfx {
        out.extend(r.cube.clone());
    }
    for (_, _, _, p) in &c.m4.particles {
        match p {
            Particles::System(s) => out.extend(system_textures(s)),
            Particles::Spray(t) => out.extend(t.slots.iter().flatten().flatten().filter_map(|s| s.particles.clone())),
            Particles::SprayPatch(t) => out.extend(patch_particles(t)),
        }
    }
    for (_, _, d) in &c.m4.skies {
        if let SkyKind::Dome { model, material } = &d.kind {
            out.extend([model.clone(), material.clone()]);
        }
    }
    out
}

fn patch_particles(t: &toml::Table) -> Vec<ContentId> {
    t.values()
        .filter_map(toml::Value::as_table)
        .flat_map(|row| row.values().filter_map(toml::Value::as_table))
        .filter_map(|slot| slot.get("particles").and_then(toml::Value::as_str).and_then(|p| p.parse().ok()))
        .collect()
}

fn merged_spray(big: &Path, id: &ContentId, patch: &toml::Table) -> Result<SprayTable, String> {
    let key = crate::content::particles::SPRAY_TABLES
        .iter()
        .find(|(n, _)| crate::content::particles::spray_id(n) == *id)
        .map(|(_, k)| *k)
        .ok_or_else(|| format!("{id}: the base game has no such spray table"))?;
    let set = with_attribdb(big, |db| crate::particles::load_snow_spray(db, key)).map_err(|e| format!("{e:#}"))?;
    let mut table = crate::content::particles::spray_record(&set).to_table();
    ssx_content::patch::merge(&mut table, patch);
    SprayTable::from_table(&table).map_err(|ps| ps.join("; "))
}

fn system_textures(s: &ssx_formats::lynx::ParticleSystem) -> Vec<ContentId> {
    let mut out = Vec::new();
    for a in s.effects.iter().flat_map(|e| &e.groups).flat_map(|g| &g.actions) {
        for p in a.params.iter().chain(a.states.iter().flat_map(|st| &st.params)) {
            if p.name == ssx_content::particles::TEXTURE_PARAMETER
                && let ssx_formats::lynx::Value::String(t) = &p.value
                && let Ok(id) = t.parse::<ContentId>()
            {
                out.push(id);
            }
        }
    }
    out
}

pub(super) fn check(
    c: &mut Checked,
    m: &Manifest,
    big: &Path,
    base: Option<&BTreeSet<ContentId>>,
    helper: Option<&Path>,
) {
    let content = crate::content::GameContent::of(big);
    if !c.m4.lighting.is_empty() {
        let schema = content.lighting_schema();
        let nodes = particle_nodes(big);
        for (id, file, patch, r) in c.m4.lighting.clone() {
            let t = text(c, &file);
            match &schema {
                Ok(s) => {
                    for (k, ty) in r.fields() {
                        if let Some(why) = misfit(k, ty, s) {
                            push(c, Severity::Error, &file, locate_key(&t, k), why);
                        }
                    }
                }
                Err(e) => {
                    push(c, Severity::Error, &file, (0, 0), format!("the disc's lighting presets cannot be read: {e}"))
                }
            }
            for (k, tid) in &r.textures {
                check_id(c, m, base, &file, k, tid);
            }
            if let Some(sky) = &r.sky {
                check_id(c, m, base, &file, "sky", sky);
            }
            if let (Some(n), Ok(nodes)) = (r.sun_node, &nodes)
                && !nodes.contains(&n)
            {
                push(
                    c,
                    Severity::Error,
                    &file,
                    locate_key(&t, "sun_node"),
                    format!("sun_node 0x{n:08x}: no ge_particlenode of the disc has this key"),
                );
            }
            if !patch && id.namespace() == BASE_NAMESPACE {
                let missing: Vec<&String> = schema
                    .as_ref()
                    .map(|s| s.keys().filter(|k| r.fields().all(|(f, _)| f != k.as_str())).collect())
                    .unwrap_or_default();
                if !missing.is_empty() {
                    let message = format!(
                        "a whole preset in place of {id}: {} fields the disc's presets carry are left out (the game reads them as 0); a patch changes fields of the disc's preset",
                        missing.len()
                    );
                    push(c, Severity::Warning, &file, (0, 0), message);
                }
            }
            if r.vectors.contains_key("dirLight_Direction")
                || r.floats.contains_key("isNightCondition")
                || r.floats.contains_key("dirLight_Offset")
                || r.floats.contains_key("headLampScaleFactor")
            {
                push(c, Severity::Note, &file, (0, 0), "sets the sun's direction or the night flag: simulation fields (the survival shade, the night conditions), not the base game's ride".to_string());
            }
        }
    }
    if !c.m4.postfx.is_empty() {
        let stacks = crate::postfx::stack_groups(big);
        for (_, file, _, r) in c.m4.postfx.clone() {
            let t = text(c, &file);
            match &stacks {
                Ok(stacks) => {
                    for b in &r.colour_cubes {
                        match stacks.iter().find(|(i, _, _)| *i == b.stack) {
                            Some((_, name, groups)) if !groups.contains(&b.group) => push(
                                c,
                                Severity::Error,
                                &file,
                                locate_key(&t, "colour_cubes"),
                                format!("colour_cubes: the stack {name} has no group {:?}", b.group),
                            ),
                            Some(_) => {}
                            None => push(
                                c,
                                Severity::Error,
                                &file,
                                locate_key(&t, "colour_cubes"),
                                format!("colour_cubes: no colour-cube stack {}", b.stack),
                            ),
                        }
                    }
                }
                Err(e) => push(
                    c,
                    Severity::Error,
                    &file,
                    (0, 0),
                    format!("the disc's colour-cube stacks cannot be read: {e:#}"),
                ),
            }
            if let Some(cube) = &r.cube {
                check_id(c, m, base, &file, "cube", cube);
                if cube.namespace() == m.resource.name
                    && !m.add.get(cube).is_some_and(|e| e.files().iter().all(|f| f.ends_with(".png")))
                {
                    push(
                        c,
                        Severity::Error,
                        &file,
                        locate_key(&t, "cube"),
                        format!("cube = {cube}: a postfx entry of a .png strip"),
                    );
                }
            }
        }
    }
    if !c.m4.particles.is_empty() {
        let nodes = particle_nodes(big);
        for (id, file, _, p) in c.m4.particles.clone() {
            let p = match p {
                Particles::SprayPatch(t) => match merged_spray(big, &id, &t) {
                    Ok(merged) => Particles::Spray(Box::new(merged)),
                    Err(why) => {
                        push(c, Severity::Error, &file, (0, 0), format!("merged onto the base game's table: {why}"));
                        continue;
                    }
                },
                p => p,
            };
            match p {
                Particles::System(s) => {
                    for tid in system_textures(&s) {
                        check_id(c, m, base, &file, ssx_content::particles::TEXTURE_PARAMETER, &tid);
                    }
                }
                Particles::SprayPatch(_) => {}
                Particles::Spray(t) => {
                    for slot in t.slots.iter().flatten().flatten() {
                        if let Ok(nodes) = &nodes
                            && !nodes.contains(&slot.node)
                        {
                            push(
                                c,
                                Severity::Error,
                                &file,
                                (0, 0),
                                format!("node 0x{:08x}: no ge_particlenode of the disc has this key", slot.node),
                            );
                        }
                        if let Some(pid) = &slot.particles {
                            check_id(c, m, base, &file, "particles", pid);
                        }
                    }
                }
            }
        }
    }
    for (_, file, d) in c.m4.skies.clone() {
        if let SkyKind::Dome { model, material } = &d.kind {
            check_id(c, m, base, &file, "model", model);
            check_id(c, m, base, &file, "material", material);
            if let Some((_, def)) = c.materials.get(material).cloned()
                && def.family != Some(ssx_content::material::Family::Sky)
            {
                push(
                    c,
                    Severity::Error,
                    &file,
                    locate_key(&text(c, &file), "material"),
                    format!("material = {material}: a dome's material is of the sky family"),
                );
            }
        }
    }
    compose_shaders(c, helper);
}

fn particle_nodes(big: &Path) -> Result<BTreeSet<u32>, String> {
    with_attribdb(big, |db| Ok(db.collections_of_all(fnv1("ge_particlenode")).into_iter().map(|(k, _)| k).collect()))
        .map_err(|e| format!("{e:#}"))
}

pub fn helper_path(given: Option<&Path>) -> PathBuf {
    if let Some(p) = given {
        return p.to_path_buf();
    }
    let exe = if cfg!(windows) { "ssx-shader-check.exe" } else { "ssx-shader-check" };
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(exe))).unwrap_or_else(|| PathBuf::from(exe))
}

fn compose_shaders(c: &mut Checked, helper: Option<&Path>) {
    let hooks: BTreeMap<String, HookFamily> = c.m4.shaders.iter().map(|(_, f, a)| (f.clone(), a.family)).collect();
    if hooks.is_empty() {
        return;
    }
    let exe = helper_path(helper);
    let Some(root) = c.tree.as_ref().map(|t| t.root().to_path_buf()) else { return };
    for (file, family) in hooks {
        let run = std::process::Command::new(&exe).args(["--family", family.as_str()]).arg(root.join(&file)).output();
        match run {
            Ok(out) => {
                let lines = String::from_utf8_lossy(&out.stdout).to_string();
                for l in lines.lines() {
                    let parts: Vec<&str> = l.splitn(5, '\t').collect();
                    match parts.as_slice() {
                        ["composed", class, sets, distinct, bevy_too] => push(
                            c,
                            Severity::Note,
                            &file,
                            (0, 0),
                            format!(
                                "composed with its family and validated for {sets} def sets on {class} ({distinct} \
                                 distinct; {bevy_too} that Bevy 0.20's own module cannot compose either)"
                            ),
                        ),
                        ["analysed", class, frame, worst] => push(
                            c,
                            Severity::Note,
                            &file,
                            (0, 0),
                            format!(
                                "as composed with its family on {class} (the module that runs): {worst}; the entry \
                                 points' function memory at most {frame} bytes of WGSL's 8192"
                            ),
                        ),
                        ["refused", line, col, m] => push(
                            c,
                            Severity::Error,
                            &file,
                            (line.parse().unwrap_or(0), col.parse().unwrap_or(0)),
                            (*m).to_string(),
                        ),
                        ["error", m, ..] => {
                            push(c, Severity::Error, &file, (0, 0), format!("the composition check: {m}"))
                        }
                        _ => {}
                    }
                }
                if !out.status.success() && !lines.contains("refused") && !lines.contains("error") {
                    push(c, Severity::Error, &file, (0, 0), format!("the composition check failed: {}", out.status));
                }
            }
            Err(e) => push(
                c,
                Severity::Error,
                &file,
                (0, 0),
                format!(
                    "the composition of a shader hook needs {} ({e}): build it with `cargo build --release -p ssx-bevy --features shader-check --bin ssx-shader-check`, or name it with --shader-check",
                    exe.display()
                ),
            ),
        }
    }
}

pub fn schema_fields(content: &crate::content::GameContent) -> BTreeMap<String, FieldType> {
    content.lighting_schema().map(|s| s.as_ref().clone()).unwrap_or_default()
}

pub fn is_m4(id: &ContentId) -> bool {
    matches!(id.kind(), Kind::Sky | Kind::Lighting | Kind::Postfx | Kind::Particles)
}
