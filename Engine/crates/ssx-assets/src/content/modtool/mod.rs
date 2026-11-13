mod anims;
mod audio;
pub mod check;
pub mod data;
mod gamemode_script;
mod m10_templates;
mod m3_templates;
pub mod m4;
mod m4_templates;
mod m5_templates;
pub mod m6;
pub mod m6_templates;
mod m7_templates;
mod m9_templates;
mod movie;
pub mod pack;
mod prop;
mod records;
mod rider;
mod scoreboard_template;
mod scripts;
pub(crate) mod tracks;
mod trick_templates;
mod tricks;

mod m8_templates;
pub mod map_start;
mod maps;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssx_content::check::Severity;
use ssx_content::id::{BASE_NAMESPACE, PORT_NAMESPACE, valid_namespace};
use ssx_content::install::{Installed, install_folder, install_zip};
use ssx_content::path::check_package_path;

use super::mods::{EnabledEntry, EnabledList, MODS_DIR};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    TexturePack,
    Prop,
    Sky,
    Lighting,
    Shader,
    Sound,
    Music,
    Character,
    Gear,
    Board,
    Ui,
    Strings,
    Font,
    Script,
    Movie,
    Data,
    Map,
    Layout,
    Gamemode,
    Anim,
    Trick,
    Scoreboard,
}

impl Template {
    pub const fn name(self) -> &'static str {
        match self {
            Template::TexturePack => "texture-pack",
            Template::Prop => "prop",
            Template::Sky => "sky",
            Template::Lighting => "lighting",
            Template::Shader => "shader",
            Template::Sound => "sound",
            Template::Music => "music",
            Template::Character => "character",
            Template::Gear => "gear",
            Template::Board => "board",
            Template::Ui => "ui",
            Template::Strings => "strings",
            Template::Font => "font",
            Template::Script => "script",
            Template::Movie => "movie",
            Template::Data => "data",
            Template::Map => "map",
            Template::Layout => "layout",
            Template::Gamemode => "gamemode",
            Template::Anim => "anim",
            Template::Trick => "trick",
            Template::Scoreboard => "scoreboard",
        }
    }

    fn phase(self) -> Option<&'static str> {
        match self {
            Template::TexturePack
            | Template::Prop
            | Template::Sky
            | Template::Lighting
            | Template::Shader
            | Template::Sound
            | Template::Music
            | Template::Character
            | Template::Gear
            | Template::Board
            | Template::Ui
            | Template::Strings
            | Template::Font
            | Template::Script
            | Template::Movie
            | Template::Data
            | Template::Map
            | Template::Layout
            | Template::Anim
            | Template::Trick
            | Template::Gamemode
            | Template::Scoreboard => None,
        }
    }
}

fn manifest_text(name: &str) -> String {
    format!(
        r#"# The manifest of the package "{name}" (Project Avalanche; every key: Docs/architecture/content.md §2).
# Check it with `ssx-assets mod check <folder>`, pack it with `ssx-assets mod pack <folder>`.

[resource]
# The package's name: also the namespace of everything it adds ("{name}:<kind>/<path>").
name = "{name}"
# MAJOR.MINOR.PATCH
version = "0.1.0"
# One line each, shown on the launcher's Mods page.
title = "{name}"
authors = []
description = ""
# The game's API level the package needs.
api = 1
type = "mod"
# Among installed mods the higher priority wins on the same id; on equal priority, the later on the Mods page.
priority = 0

# Replace the game's textures by id. `ssx-assets content ids --kind texture` lists the ids; `ssx-assets content export
# <id>` writes one from your own disc as a starting point for your own work. Exported files themselves may not be
# shipped: `mod check` and `mod pack` refuse them, and every file of the disc.
[override]
# "ssx:texture/worlds/textures/rock_basalt_diff" = "textures/rock_basalt_diff.png"
"#
    )
}

const TEXTURES_TOML: &str = r#"# Settings for the image files of this folder, one table per file name (optional).
# kind = "color" | "normal" | "data": undeclared, a texture is sampled as the game samples the one it replaces
# (two-channel normal maps where the game's texture is one, raw colour channels elsewhere).
#
# ["rock_basalt_n.dds"]
# kind = "normal"
"#;

pub fn new_package(name: &str, template: Template, out: &Path, data: &Path) -> Result<PathBuf> {
    check_name(name)?;
    if let Some(phase) = template.phase() {
        bail!(
            "the {} template comes with phase {phase}; this version has the texture-pack, prop, sky, lighting, shader, \
             sound, music, character, gear, board, ui, strings, font, script, movie, data, map, layout, gamemode, \
             anim, trick and scoreboard templates",
            template.name()
        );
    }
    if template == Template::Layout {
        bail!("the layout template sits on a disc drop: --drop <ui_drop name> (`ssx-assets content ids --kind drop`)");
    }
    let root = out.join(name);
    if matches!(template, Template::Character | Template::Gear | Template::Board) {
        m3_templates::skeleton(data)?;
    }
    if template == Template::Movie {
        crate::content::fmv::ffmpeg::check_ffmpeg()?;
    }
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    std::fs::create_dir(&root).with_context(|| format!("creating {} (it must not exist yet)", root.display()))?;
    std::fs::create_dir(root.join("textures"))?;
    std::fs::write(root.join("textures").join(ssx_content::texture::settings::SETTINGS_FILE), TEXTURES_TOML)?;
    match template {
        Template::Prop => prop::lay_out(&root, name)?,
        Template::Sky => m4_templates::sky(&root, name)?,
        Template::Lighting => m4_templates::lighting(&root, name)?,
        Template::Shader => m4_templates::shader(&root, name)?,
        Template::Sound => m5_templates::sound(&root, name)?,
        Template::Music => m5_templates::music(&root, name)?,
        Template::Character => m3_templates::character(&root, name, data)?,
        Template::Gear => m3_templates::gear(&root, name, data)?,
        Template::Board => m3_templates::board_package(&root, name, data)?,
        Template::Ui => m6_templates::ui(&root, name)?,
        Template::Strings => m6_templates::strings(&root, name)?,
        Template::Font => m6_templates::font(&root, name)?,
        Template::Script => m7_templates::script(&root, name)?,
        Template::Movie => {
            if let Err(e) = movie::template(&root, name) {
                std::fs::remove_dir_all(&root).ok();
                return Err(e);
            }
        }
        Template::Data => m9_templates::data(&root, name)?,
        Template::Map => m8_templates::map(&root, name)?,
        Template::Gamemode => m8_templates::gamemode(&root, name)?,
        Template::Anim => m10_templates::anim(&root, name)?,
        Template::Trick => trick_templates::trick(&root, name)?,
        Template::Scoreboard => scoreboard_template::scoreboard(&root, name)?,
        _ => std::fs::write(root.join(ssx_content::manifest::MANIFEST_FILE), manifest_text(name))?,
    }
    Ok(root)
}

fn check_name(name: &str) -> Result<()> {
    if !valid_namespace(name) || check_package_path(name, false).is_err() {
        bail!("{name:?}: a package name is lower case a-z, 0-9, `_`, `.`, `-` (and not a Windows device name)");
    }
    if name == BASE_NAMESPACE || name == PORT_NAMESPACE {
        bail!("{name:?} is reserved (the base game's and the port's namespaces)");
    }
    Ok(())
}

pub fn new_layout(name: &str, drop: &str, out: &Path, data: &Path) -> Result<PathBuf> {
    check_name(name)?;
    let files = m8_templates::layout(name, drop, &crate::setup::big_root(data))?;
    let root = out.join(name);
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    std::fs::create_dir(&root).with_context(|| format!("creating {} (it must not exist yet)", root.display()))?;
    std::fs::create_dir(root.join("textures"))?;
    std::fs::write(root.join("textures").join(ssx_content::texture::settings::SETTINGS_FILE), TEXTURES_TOML)?;
    for (rel, text) in files {
        let path = root.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, text)?;
    }
    Ok(root)
}

pub fn install(package: &Path, app_dir: &Path, replace: bool, disabled: bool) -> Result<Installed> {
    let mods = app_dir.join(MODS_DIR);
    let is_zip = package.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let meta = std::fs::metadata(package).with_context(|| format!("{}", package.display()))?;
    let done = if meta.is_dir() {
        install_folder(package, &mods, replace)
    } else if is_zip {
        install_zip(package, &mods, replace)
    } else {
        bail!("{}: a package is a folder or a .zip", package.display());
    }
    .map_err(|e| anyhow::Error::new(e).context(package.display().to_string()))?;
    let mut list = EnabledList::read(app_dir).map_err(anyhow::Error::msg)?.unwrap_or_default();
    let old = done.previous.as_ref().map(|(folder, _)| folder.clone());
    list.entries.retain(|e| Some(&e.folder) != old.as_ref() && !e.folder.eq_ignore_ascii_case(&done.folder));
    list.entries.push(EnabledEntry { folder: done.folder.clone(), enabled: !disabled });
    list.write(app_dir).map_err(anyhow::Error::msg)?;
    Ok(done)
}

pub fn install_report(done: &Installed, app_dir: &Path) -> Vec<String> {
    let r = &done.manifest.resource;
    let mut out = vec![format!(
        "installed {} {} into {} ({} files, {} bytes; content index {})",
        r.name,
        r.version,
        app_dir.join(MODS_DIR).join(&done.folder).display(),
        done.files,
        done.bytes,
        if done.indexed { "checked" } else { "absent" }
    )];
    if let Some((folder, _)) = &done.previous {
        out.push(format!("replaced the version installed in {folder}"));
    }
    for f in done.findings.iter().filter(|f| f.severity != Severity::Error) {
        out.push(f.to_string());
    }
    out
}
