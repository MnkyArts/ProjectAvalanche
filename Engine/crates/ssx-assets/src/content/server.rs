use std::collections::BTreeSet;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use ssx_content::data::SimContentId;
use ssx_content::delivery::{Difference, LevelId, LevelName};
use ssx_content::policy::ClientMods;
use ssx_content::registry::SetAside;
use ssx_content::{ContentId, ContentIndex, Layer, PackageSource};

use super::data::SIM_KINDS;
use super::gamemodes::{ModeChoice, finish_from_record};
use super::store::GameContent;

pub fn mount_fetched(
    content: &GameContent,
    packages: &[(String, std::path::PathBuf)],
    policy: ClientMods,
) -> Result<SessionMount, String> {
    use ssx_core::text::{MESSAGE_CHARS, quoted, shown, shown_max};
    let mut sources = Vec::with_capacity(packages.len());
    for (name, dir) in packages {
        let src = PackageSource::open_as(dir, ssx_content::package::Origin::Server)
            .map_err(|e| format!("the server's {}: {e}", shown(name)))?;
        if src.manifest.resource.name != *name {
            return Err(format!(
                "the server's {}: its resource.toml names it {}",
                shown(name),
                quoted(&src.manifest.resource.name)
            ));
        }
        if let Some(why) = super::mods::package_rules(&src) {
            return Err(format!("the server's {}: {why}", shown(name)));
        }
        sources.push(src);
    }
    let manifests: Vec<ssx_content::manifest::Manifest> = sources.iter().map(|s| s.manifest.clone()).collect();
    if let Err(errors) = ssx_content::deps::start_order(&manifests) {
        let first = errors.first().map(ToString::to_string).unwrap_or_default();
        return Err(format!("the server's resources do not start together: {}", shown_max(&first, MESSAGE_CHARS)));
    }
    Ok(content.mount_session(policy, sources))
}

pub struct SessionMount {
    policy_before: ClientMods,
    aside: Option<SetAside>,
    pub resources: Vec<String>,
    pub switched_off: Vec<(String, Vec<ContentId>)>,
}

impl GameContent {
    pub fn mount_session(&self, policy: ClientMods, sources: Vec<PackageSource>) -> SessionMount {
        let policy_before = self.sim_policy();
        let mut changed = BTreeSet::new();
        let mut switched_off = Vec::new();
        let resources: Vec<String> = sources.iter().map(|s| s.manifest.resource.name.clone()).collect();
        self.warm_for(&sources.iter().map(|s| s as &dyn ssx_content::Source).collect::<Vec<_>>());
        let aside = {
            let mut registry = self.registry.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.unmount(Layer::Server);
            let mut mods: Vec<String> = Vec::new();
            for m in registry.mounts().filter(|m| m.layer == Layer::ClientMods) {
                if !mods.iter().any(|n| n == m.name) {
                    mods.push(m.name.to_string());
                }
            }
            for name in &mods {
                let ids = registry.ids_of(Layer::ClientMods, name);
                match policy {
                    ClientMods::None => {
                        switched_off.push((name.clone(), Vec::new()));
                        changed.extend(ids);
                    }
                    ClientMods::Presentation => {
                        let sim: Vec<ContentId> = ids.into_iter().filter(|id| SIM_KINDS.contains(&id.kind())).collect();
                        if !sim.is_empty() {
                            switched_off.push((name.clone(), sim));
                        }
                    }
                    ClientMods::Any => {}
                }
            }
            let aside = (policy == ClientMods::None).then(|| registry.set_aside(Layer::ClientMods));
            for s in sources {
                let (name, priority) = (s.manifest.resource.name.clone(), s.manifest.resource.priority);
                registry.mount(Layer::Server, priority, Box::new(s));
                changed.extend(registry.ids_of(Layer::Server, &name));
            }
            self.mount_records(&mut registry);
            self.set_session_policy(Some(policy));
            aside
        };
        self.set_sim_policy(policy);
        self.clear_track_decisions();
        self.note_change(changed);
        SessionMount { policy_before, aside, resources, switched_off }
    }

    pub fn end_session(&self, mount: SessionMount) {
        let mut changed = BTreeSet::new();
        {
            let mut registry = self.registry.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            for name in &mount.resources {
                changed.extend(registry.ids_of(Layer::Server, name));
            }
            registry.unmount(Layer::Server);
            if let Some(aside) = mount.aside {
                registry.put_back(aside);
                for (name, _) in &mount.switched_off {
                    changed.extend(registry.ids_of(Layer::ClientMods, name));
                }
            }
            self.set_session_policy(None);
        }
        self.set_sim_policy(mount.policy_before);
        self.clear_track_decisions();
        self.note_change(changed);
    }

    pub fn without_session<R>(&self, f: impl FnOnce(&ssx_content::registry::Registry) -> R) -> Option<R> {
        let registry = self.registry.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.session_policy().is_some() || registry.mounts().any(|m| m.layer == Layer::Server) {
            return None;
        }
        Some(f(&registry))
    }

    pub fn swap_session_resource(&self, mount: &mut SessionMount, name: &str, source: Option<PackageSource>) {
        let mut changed = BTreeSet::new();
        if let Some(s) = &source {
            self.warm_for(&[s]);
        }
        {
            let mut registry = self.registry.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            changed.extend(registry.ids_of(Layer::Server, name));
            registry.unmount_named(Layer::Server, name);
            mount.resources.retain(|r| r != name);
            if let Some(s) = source {
                let priority = s.manifest.resource.priority;
                registry.mount(Layer::Server, priority, Box::new(s));
                changed.extend(registry.ids_of(Layer::Server, name));
                mount.resources.push(name.to_string());
            }
            self.mount_records(&mut registry);
        }
        self.clear_track_decisions();
        self.note_change(changed);
    }
}

// PORT(choice): 8 (a popup's lines); tuning: the "content differs" popup in use; the original has no dedicated server.
pub const MAX_NAMED_DIFFERENCES: usize = 8;

pub fn content_index(data: &Path) -> Result<ContentIndex, String> {
    crate::folder::content_index(data).map_err(|e| e.to_string())
}

impl GameContent {
    pub fn level_id(self: &Arc<Self>, index: &ContentIndex, level: &str) -> Result<LevelId, String> {
        let (id, rows) = self.session_level_rows(index, level)?;
        LevelId::from_rows(level, id, &rows)
    }

    pub fn check_levels(self: &Arc<Self>, index: &ContentIndex, levels: &[LevelId]) -> Result<SimContentId, String> {
        let basis = self.session_policy().unwrap_or_else(|| self.sim_policy()).id_basis();
        self.with_sim_policy(basis, || self.check_levels_now(index, levels))
    }

    fn check_levels_now(self: &Arc<Self>, index: &ContentIndex, levels: &[LevelId]) -> Result<SimContentId, String> {
        let mut ours = Vec::with_capacity(levels.len());
        let mut differ: Vec<String> = Vec::new();
        let mut count = 0usize;
        for theirs in levels {
            let mine = self.session_level_rows(index, &theirs.level)?;
            if mine.0 == theirs.id {
                ours.push((theirs.level.clone(), mine.0));
                continue;
            }
            let found = theirs.differences(&mine.1);
            let found = if found.is_empty() { vec![Difference::Files] } else { found };
            for d in found {
                count += 1;
                if differ.len() < MAX_NAMED_DIFFERENCES {
                    differ.push(format!("{}: {d}", ssx_core::text::shown(&theirs.level)));
                }
            }
            ours.push((theirs.level.clone(), mine.0));
        }
        if count > 0 {
            let more = count - differ.len();
            let tail = if more > 0 { format!("; and {more} more") } else { String::new() };
            return Err(format!("content differs from the server's: {}{tail}", differ.join("; ")));
        }
        ssx_content::delivery::content_id(&ours).ok_or_else(|| "the server names a level twice".to_string())
    }

    fn session_level_rows(
        self: &Arc<Self>,
        index: &ContentIndex,
        level: &str,
    ) -> Result<(SimContentId, Vec<ssx_content::data::SimRow>), String> {
        let shown = ssx_core::text::shown(level);
        let sid = match LevelName::parse(level).map_err(|e| format!("{shown}: {e}"))? {
            LevelName::Drop(drop) => {
                let id = ContentId::from_str(&format!("ssx:drop/{drop}")).map_err(|e| format!("{shown}: {e}"))?;
                let base = self.layout_base(&id).map_err(|e| format!("{shown}: {e:#}"))?;
                self.sim_content_id_with(index, &base.track, drop)
            }
            LevelName::Map { id, mode } => {
                let map = self.map(&id).map_err(|e| format!("{shown}: {e}"))?;
                let objects: Vec<_> =
                    self.map_objects(&map.value).map_err(|e| format!("{shown}: {e}"))?.into_values().collect();
                let base = map.value.toml.base.as_ref().map(|b| self.layout_base(b)).transpose();
                let base = base.map_err(|e| format!("{shown}: {e:#}"))?;
                let choice = ModeChoice::parse(mode).map_err(|e| format!("{shown}: {e}"))?;
                let has_finish = finish_from_record(&map.value.toml).unwrap_or(true);
                let mode = self.resolve_mode(&map, &choice, has_finish).map_err(|e| format!("{shown}: {e}"))?;
                self.map_sim_content_id_with(index, &map, base.as_ref(), &objects, Some(&mode))
            }
        };
        let sid = sid.map_err(|e| format!("{shown}: {e:#}"))?;
        Ok((sid.id, sid.rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_read_without_a_session_only_while_none_is_mounted() {
        let dir = std::env::temp_dir().join(format!("ssx-without-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let srv = dir.join("srv");
        std::fs::create_dir_all(srv.join("models")).unwrap();
        let manifest =
            "[resource]\nname = \"srv\"\nversion = \"1.0.0\"\napi = 1\n[add]\n\"model/m\" = \"models/m.gltf\"\n";
        std::fs::write(srv.join("resource.toml"), manifest).unwrap();
        std::fs::write(srv.join("models/m.gltf"), br#"{"asset":{"version":"2.0"},"buffers":[]}"#).unwrap();
        let content = GameContent::of(&dir.join("data/big"));
        let id: ContentId = "srv:model/m".parse().unwrap();
        assert_eq!(content.without_session(|r| r.has(&id)), Some(false));
        let source = PackageSource::open_as(&srv, ssx_content::package::Origin::Server).unwrap();
        let mount = content.mount_session(ClientMods::Presentation, vec![source]);
        assert!(content.with_registry(|r| r.has(&id)), "the session's package is mounted");
        assert_eq!(content.without_session(|r| r.has(&id)), None, "nothing is read while it is");
        content.end_session(mount);
        assert_eq!(content.without_session(|r| r.has(&id)), Some(false), "read again after the session's end");
        let mount = content.mount_session(ClientMods::None, Vec::new());
        assert_eq!(content.without_session(|_| ()), None, "a set-aside of the client mods is a session's too");
        content.end_session(mount);
        assert_eq!(content.without_session(|_| ()), Some(()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
