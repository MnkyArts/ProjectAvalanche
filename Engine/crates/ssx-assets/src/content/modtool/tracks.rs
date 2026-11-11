use std::collections::BTreeSet;
use std::path::Path;

use ssx_content::check::{Checked, Severity};
use ssx_content::id::BASE_NAMESPACE;
use ssx_content::manifest::{MANIFEST_FILE, Manifest};
use ssx_content::{ContentId, Kind, PackageSource};

use super::check::push;
use crate::content::GameContent;
use crate::content::models::model_id;
use crate::track_objects::load_track_objects;

pub(crate) fn tracks(big: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(big.join("cache1/data/xenon/attribdb"))
        .map(|d| {
            d.flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix("_track.vlt").map(str::to_string)).collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

pub(super) fn check(c: &mut Checked, m: &Manifest, dir: &Path, big: &Path, app_dir: &Path) {
    let objects = m.overrides.keys().any(|id| id.namespace() == BASE_NAMESPACE && id.kind() == Kind::Object);
    let meshes: BTreeSet<&ContentId> =
        m.overrides.keys().filter(|id| id.namespace() == BASE_NAMESPACE && id.kind() == Kind::Model).collect();
    if !objects && meshes.is_empty() {
        return;
    }
    if c.errors() > 0 {
        let message = "the tracks' budgets are measured once the package has no errors".to_string();
        return push(c, Severity::Note, MANIFEST_FILE, (0, 0), message);
    }
    let source = match PackageSource::open(dir) {
        Ok(s) => s,
        Err(e) => return push(c, Severity::Error, MANIFEST_FILE, (0, 0), e.to_string()),
    };
    let content = GameContent::of(big);
    content.mount_client_mods_except(app_dir, Some(&m.resource.name));
    let all: BTreeSet<ContentId> = m.add.keys().chain(m.overrides.keys()).cloned().collect();
    content.swap_dev_package(None, source, all);
    let mut measured = 0;
    for track in tracks(big) {
        let Ok(objs) = load_track_objects(big, &track, None) else { continue };
        let packages = content.track_packages(&track);
        let reaches_mesh = !meshes.is_empty()
            && objs
                .objects
                .iter()
                .flat_map(|o| o.lods.iter().flatten())
                .any(|mesh| model_id(&mesh.geometry).is_some_and(|id| meshes.contains(&id)));
        let mut outcomes: Vec<_> = packages.outcomes.clone();
        if reaches_mesh {
            outcomes.extend(packages.meshes(&content, &objs).outcomes.iter().cloned());
        }
        if outcomes.is_empty() {
            continue;
        }
        measured += 1;
        for o in outcomes.iter().filter(|o| o.source == m.resource.name) {
            let Some(why) = &o.refused else { continue };
            let at = m.positions.entries.get(&o.id).copied().unwrap_or((0, 0));
            let placements = if o.placements > 0 { format!(" at {} placements", o.placements) } else { String::new() };
            let message = format!("{}: on {track}{placements} the disc's is used instead: {why}", o.id);
            push(c, Severity::Warning, MANIFEST_FILE, at, message);
        }
    }
    let message = format!("the tracks' package decisions measured on {measured} tracks its overrides reach");
    push(c, Severity::Note, MANIFEST_FILE, (0, 0), message);
}
