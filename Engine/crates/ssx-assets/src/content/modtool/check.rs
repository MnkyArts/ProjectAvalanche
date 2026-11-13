use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::Result;
use ssx_content::check::{Checked, Severity, check_package};
use ssx_content::id::{BASE_NAMESPACE, PORT_NAMESPACE};
use ssx_content::limits::MAX_FILE_BYTES;
use ssx_content::manifest::{MANIFEST_FILE, Manifest};
use ssx_content::texture::{self, Shape, Want};
use ssx_content::{ContentHash, ContentId, Kind, PackageSource};

use super::super::export::{EXPORTS_FILE, parse_exports};
use super::super::mods::MODS_DIR;
use super::super::textures::DiscTextures;
use super::super::{DataDirs, catalogue, hash};

pub const MIN_REFUSED_BYTES: u64 = 64;

pub struct CheckOptions<'a> {
    pub data: &'a Path,
    pub app_dir: &'a Path,
    pub with: &'a [PathBuf],
    pub texture_limits: texture::Limits,
    pub shader_check: Option<&'a Path>,
    pub script_check: Option<&'a Path>,
    pub map_check: Option<&'a Path>,
}

pub struct Report {
    pub checked: Checked,
    pub hashed: usize,
    pub short: usize,
}

pub(super) fn push(c: &mut Checked, severity: Severity, file: &str, (line, column): (usize, usize), message: String) {
    c.findings.push(ssx_content::check::Finding {
        severity,
        file: file.to_string(),
        line,
        column,
        message: message.into(),
    });
}

pub fn check(dir: &Path, opts: &CheckOptions<'_>) -> Result<Report> {
    let mut c = check_package(dir, &opts.texture_limits);
    let (hashed, short) = refuse_base_bytes(&mut c, opts);
    if let Some(m) = c.manifest.clone() {
        check_ids(&mut c, &m, opts);
        super::tracks::check(&mut c, &m, dir, &crate::setup::big_root(opts.data), opts.app_dir);
        super::rider::check(&mut c, &m, dir, &crate::setup::big_root(opts.data), opts.app_dir);
        super::scripts::check(&mut c, &m, opts.script_check);
        let others = super::tricks::other_dirs(opts.app_dir, opts.with);
        super::map_start::check(&mut c, &crate::setup::big_root(opts.data), &others, opts.map_check);
        super::movie::check(&mut c, dir);
        super::data::check(&mut c, &m, dir, &crate::setup::big_root(opts.data));
        super::data::check_earlier(&mut c, &m, dir, &crate::setup::big_root(opts.data));
    }
    c.findings.sort_by(|a, b| (&a.file, a.line, a.column, a.severity).cmp(&(&b.file, b.line, b.column, b.severity)));
    Ok(Report { checked: c, hashed, short })
}

fn refuse_base_bytes(c: &mut Checked, opts: &CheckOptions<'_>) -> (usize, usize) {
    let mut known: HashMap<ContentHash, String> = HashMap::new();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    match hash::ensure_index(opts.data, threads) {
        Ok(index) => {
            for (path, (_, h)) in &index.entries {
                known.entry(*h).or_insert_with(|| format!("the disc's {path} ({})", hash::INDEX_FILE));
            }
        }
        Err(e) => push(
            c,
            Severity::Error,
            "",
            (0, 0),
            format!("the disc's content index cannot be read ({e:#}): disc bytes are not checked"),
        ),
    }
    let exports = opts.app_dir.join(EXPORTS_FILE);
    match std::fs::read_to_string(&exports) {
        Ok(text) => match parse_exports(&text) {
            Ok(rows) => {
                for r in rows {
                    known
                        .entry(r.hash)
                        .or_insert_with(|| format!("your export of {} ({}, {EXPORTS_FILE})", r.id, r.file));
                }
            }
            Err(e) => push(
                c,
                Severity::Error,
                "",
                (0, 0),
                format!("{}: {e}: exported bytes are not checked", exports.display()),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            push(c, Severity::Error, "", (0, 0), format!("{}: {e}: exported bytes are not checked", exports.display()))
        }
    }
    let Some(tree) = c.tree.take() else { return (0, 0) };
    let (mut hashed, mut short) = (0, 0);
    let files: Vec<String> = tree.files().map(str::to_string).collect();
    for f in files {
        if tree.size(&f).unwrap_or(0) < MIN_REFUSED_BYTES {
            short += 1;
            continue;
        }
        match tree.copy_to(&f, MAX_FILE_BYTES, &mut std::io::sink()) {
            Ok((_, hash)) => {
                hashed += 1;
                if let Some(what) = known.get(&hash) {
                    let message = format!(
                        "the same bytes as {what}: a mod may name the game's content by id, never contain it or an \
                         export of it"
                    );
                    push(c, Severity::Error, &f, (0, 0), message);
                }
            }
            Err(e) => push(c, Severity::Error, &f, (0, 0), e.to_string()),
        }
    }
    c.tree = Some(tree);
    (hashed, short)
}

struct Other {
    manifest: Manifest,
}

fn other_packages(c: &mut Checked, opts: &CheckOptions<'_>) -> BTreeMap<String, Other> {
    let mut out = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(opts.app_dir.join(MODS_DIR)) {
        for e in rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
            if let Ok(p) = PackageSource::open(&e.path()) {
                out.insert(p.manifest.resource.name.clone(), Other { manifest: p.manifest });
            }
        }
    }
    for dir in opts.with {
        match PackageSource::open(dir) {
            Ok(p) => {
                out.insert(p.manifest.resource.name.clone(), Other { manifest: p.manifest });
            }
            Err(e) => push(c, Severity::Error, "", (0, 0), format!("--with {}: {e}", dir.display())),
        }
    }
    out
}

fn check_ids(c: &mut Checked, m: &Manifest, opts: &CheckOptions<'_>) {
    let others = other_packages(c, opts);
    let mut required: BTreeMap<&str, &Manifest> = BTreeMap::new();
    for dep in &m.dependencies.require {
        let at = m.positions.requires.get(&dep.name).copied().unwrap_or((0, 0));
        match others.get(&dep.name) {
            Some(o) if dep.req.matches(o.manifest.resource.version) => {
                required.insert(&dep.name, &o.manifest);
            }
            Some(o) => {
                let v = &o.manifest.resource.version;
                push(
                    c,
                    Severity::Error,
                    MANIFEST_FILE,
                    at,
                    format!("requires {} {}; the one found is {v}", dep.name, dep.req),
                );
            }
            None => {
                let message = format!("requires {}, which is neither installed nor given with --with", dep.name);
                push(c, Severity::Error, MANIFEST_FILE, at, message);
            }
        }
    }
    let targets: Vec<(&ContentId, (usize, usize), &str)> = m
        .overrides
        .keys()
        .map(|id| (id, m.positions.entries.get(id).copied().unwrap_or((0, 0)), "[override]"))
        .chain(m.patches.keys().map(|id| (id, m.positions.patches.get(id).copied().unwrap_or((0, 0)), "[patch]")))
        .collect();
    let m4_ids: Vec<ContentId> = super::m4::named_ids(c).into_iter().chain(super::audio::named_ids(c)).collect();
    let base_kinds: BTreeSet<Kind> = targets
        .iter()
        .map(|(id, ..)| *id)
        .chain(c.references.iter().map(|r| &r.id))
        .chain(m4_ids.iter())
        .filter(|id| id.namespace() == BASE_NAMESPACE)
        .map(ContentId::kind)
        .collect();
    let base = base_ids(c, opts, &base_kinds);
    let disc = base_kinds.contains(&Kind::Texture).then(|| DiscTextures::index(&crate::setup::big_root(opts.data)));
    for (id, at, section) in targets {
        let ns = id.namespace();
        let known = if ns == BASE_NAMESPACE {
            base.as_ref().map(|b| b.contains(id))
        } else if ns == PORT_NAMESPACE {
            use ssx_content::Source;
            Some(
                *id == ssx_content::sky::scattering_id()
                    || super::m6::is_port_id(id)
                    || super::super::markers::PortMarkers.has(id)
                    || super::super::fmv::port::is_slot(id),
            )
        } else if ns == m.resource.name {
            Some(m.add.contains_key(id))
        } else if let Some(o) = required.get(ns) {
            Some(o.add.contains_key(id))
        } else {
            let message = format!("{section} {id}: names {ns}'s content, but [dependencies] does not require {ns}");
            push(c, Severity::Error, MANIFEST_FILE, at, message);
            continue;
        };
        match known {
            Some(true) => {
                if let Some(note) = super::super::fmv::port::slot_note(id) {
                    push(c, Severity::Note, MANIFEST_FILE, at, format!("{section} {id}: {note}"));
                }
            }
            Some(false) => {
                let whose = match ns {
                    BASE_NAMESPACE => "the base game has no such id (`ssx-assets content ids` lists them)".to_string(),
                    PORT_NAMESPACE => "the port's own content is `ava:sky/scattering`, its built views \
                                       (`ava:ui/views/ava_*`), strings (`ava:string/txt_ava_*`), markers \
                                       (`ava:marker/{ring, cylinder, arrow, chevron, beam, gate}`) and its movie \
                                       slot (`ava:movie/fe_background`)"
                        .to_string(),
                    _ => format!("{ns}'s [add] has no such id"),
                };
                push(c, Severity::Error, MANIFEST_FILE, at, format!("{section} {id}: {whose}"));
            }
            None => {}
        }
    }
    super::records::check(c, m, &crate::setup::big_root(opts.data), base.as_ref());
    super::maps::check(c, &crate::setup::big_root(opts.data), base.as_ref());
    super::anims::check(c, &crate::setup::big_root(opts.data));
    super::tricks::check(c, &crate::setup::big_root(opts.data), &super::tricks::other_dirs(opts.app_dir, opts.with));
    super::m4::check(c, m, &crate::setup::big_root(opts.data), base.as_ref(), opts.shader_check);
    super::audio::check(c, m, &crate::setup::big_root(opts.data), base.as_ref());
    super::m6::check(c, m, &crate::setup::big_root(opts.data));
    if let Some(Ok(disc)) = &disc {
        fit_base_textures(c, m, disc);
    } else if let Some(Err(e)) = &disc {
        push(
            c,
            Severity::Error,
            "",
            (0, 0),
            format!("the disc's textures cannot be indexed ({e:#}): texture shapes are not checked"),
        );
    }
}

fn base_ids(c: &mut Checked, opts: &CheckOptions<'_>, kinds: &BTreeSet<Kind>) -> Option<BTreeSet<ContentId>> {
    if kinds.is_empty() {
        return Some(BTreeSet::new());
    }
    let (big, disc) = (opts.data.join("big"), opts.data.join("disc"));
    let only = (kinds.len() == 1).then(|| kinds.first().copied()).flatten();
    match catalogue(&DataDirs { big: &big, disc: &disc }, only) {
        Ok(ids) => Some(ids.rows.into_iter().map(|(id, _)| id).collect()),
        Err(e) => {
            let message = format!(
                "the base game's ids cannot be listed from {} ({e:#}): ids are not checked",
                opts.data.display()
            );
            push(c, Severity::Error, "", (0, 0), message);
            None
        }
    }
}

fn fit_base_textures(c: &mut Checked, m: &Manifest, disc: &DiscTextures) {
    for (id, t) in c.textures.clone() {
        if id.namespace() != BASE_NAMESPACE || !m.overrides.contains_key(&id) {
            continue;
        }
        let at = m.positions.entries.get(&id).copied().unwrap_or((0, 0));
        let layout = match disc.layout(&id) {
            Ok(Some(l)) => l,
            Ok(None) => continue,
            Err(e) => {
                push(c, Severity::Error, MANIFEST_FILE, at, format!("{id}: the disc's texture: {e:#}"));
                continue;
            }
        };
        // TODO(guess): how the game samples the texture is taken from the disc's format (a DXN map as a two-channel normal map, the rest as raw texels), as M1a's consumers ask (terrain normal slots, `normalTexture`, the ghosts' map: NORMAL; every other use PLANE); a mismatch is only a warning, since the run-time resolve decides per use; settle: a map of every id's uses from the vault materials and the consumers' parameters.
        let want = match layout {
            (Shape::Volume, _) => {
                let message = format!(
                    "{id}: the post-FX colour cube is a volume, the postfx kind's data, not a texture a file replaces"
                );
                push(c, Severity::Error, MANIFEST_FILE, at, message);
                continue;
            }
            (Shape::Cube, _) => Want::CUBE,
            (_, true) => Want::NORMAL,
            (_, false) => Want::PLANE,
        };
        if t.shape != want.shape {
            let message =
                format!("{id}: {}: the game's texture is a {:?}, the file a {:?}", t.file, want.shape, t.shape);
            push(c, Severity::Error, MANIFEST_FILE, at, message);
        } else if let Err(e) = texture::fit(t.shape, t.kind, &want) {
            push(c, Severity::Warning, MANIFEST_FILE, at, format!("{id}: {}: {e} (the disc stores it so)", t.file));
        }
    }
}
