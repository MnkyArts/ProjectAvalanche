use std::ops::Range;
use std::path::PathBuf;

use serde::Deserialize;
use toml::Spanned;

use super::Mirror;
use super::mirror::MAX_MIRRORS;
use crate::id::valid_namespace;
use crate::policy::ClientMods;

// PORT(choice): "resources" below `--data` (FiveM's `resources/` beside its config); the original has no dedicated
// server.
pub const DEFAULT_FOLDER: &str = "resources";
// PORT(choice): 256 resources (`delivery::MAX_RESOURCES`) and 64-byte names, port caps; tuning: real servers; the
// original has no dedicated server.
pub const MAX_START: usize = super::MAX_RESOURCES;
pub const MAX_NAME: usize = super::MAX_NAME;
pub const MAX_FOLDER_PATH: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resources {
    pub folder: PathBuf,
    pub start: Vec<String>,
    pub mirrors: Vec<Mirror>,
}

impl Default for Resources {
    fn default() -> Self {
        Resources { folder: PathBuf::from(DEFAULT_FOLDER), start: Vec::new(), mirrors: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mods {
    pub client_mods: ClientMods,
    pub client_scripts: bool,
}

impl Default for Mods {
    // PORT(choice): `client_scripts = true` by default (the plan's example; the scripts draw and read only, locally);
    // tuning: what operators of race servers set once servers are public; the original has no dedicated server.
    fn default() -> Self {
        Mods { client_mods: ClientMods::Presentation, client_scripts: true }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawResources {
    folder: Option<Spanned<String>>,
    start: Option<Spanned<Vec<Spanned<String>>>>,
    files_url: Option<Spanned<Vec<Spanned<String>>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMods {
    client_mods: Option<Spanned<String>>,
    client_scripts: Option<Spanned<bool>>,
}

pub type At<'a, E> = &'a dyn Fn(Range<usize>, String) -> E;

fn q(s: &str) -> String {
    format!("{:?}", s.chars().take(40).collect::<String>())
}

pub fn parse_resources<E>(raw: RawResources, at: At<E>) -> Result<Resources, E> {
    let mut out = Resources::default();
    if let Some(folder) = raw.folder {
        let f = folder.get_ref();
        if f.is_empty() || f.len() > MAX_FOLDER_PATH || f.chars().any(char::is_control) {
            return Err(at(
                folder.span(),
                format!("folder: a path of 1 to {MAX_FOLDER_PATH} bytes, no control characters"),
            ));
        }
        out.folder = PathBuf::from(f);
    }
    if let Some(list) = raw.start {
        if list.get_ref().len() > MAX_START {
            return Err(at(list.span(), format!("start: at most {MAX_START} resources")));
        }
        for name in list.into_inner() {
            let n = name.get_ref();
            if n.len() > MAX_NAME || !valid_namespace(n) || n == "ssx" || n == "ava" {
                return Err(at(
                    name.span(),
                    format!("start: {} is not a resource name (1 to {MAX_NAME} of a-z 0-9 _ . -)", q(n)),
                ));
            }
            if out.start.contains(n) {
                return Err(at(name.span(), format!("start: {} twice", q(n))));
            }
            out.start.push(name.into_inner());
        }
    }
    if let Some(list) = raw.files_url {
        if list.get_ref().len() > MAX_MIRRORS {
            return Err(at(list.span(), format!("files_url: at most {MAX_MIRRORS} mirrors")));
        }
        for url in list.into_inner() {
            let m = Mirror::parse(url.get_ref()).map_err(|e| at(url.span(), format!("files_url: {e}")))?;
            if out.mirrors.contains(&m) {
                return Err(at(url.span(), format!("files_url: {m} twice")));
            }
            out.mirrors.push(m);
        }
    }
    Ok(out)
}

pub fn parse_mods<E>(raw: RawMods, at: At<E>) -> Result<Mods, E> {
    let mut out = Mods::default();
    if let Some(p) = raw.client_mods {
        out.client_mods = ClientMods::parse(p.get_ref()).ok_or_else(|| {
            at(p.span(), format!("client_mods = {}: \"presentation\", \"any\" or \"none\"", q(p.get_ref())))
        })?;
    }
    if let Some(s) = raw.client_scripts {
        out.client_scripts = s.into_inner();
    }
    Ok(out)
}
