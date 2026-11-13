use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use bevy::prelude::*;
use ssx_assets::content::GameContent;
use ssx_assets::content::server::SessionMount;
use ssx_assets::folder::State;
use ssx_content::data::SimContentId;
use ssx_core::text::shown;
use ssx_net_join::browse::{ContentCheck, ResourceJoin};
use ssx_net_join::resources::{CACHE_FOLDER, Fetched, Progress, ResourceOptions};
use ssx_ui::game::fe::ava::resource_settings::ResourceSettings;

use super::live::LiveSession;
#[cfg(feature = "scripts")]
use super::live::{LiveContent, ServerScripts};

#[cfg(feature = "scripts")]
pub const SCRIPT_DATA: &str = "ScriptData";

#[derive(Default)]
struct Slot {
    mount: Option<SessionMount>,
    closed: bool,
    serial: u64,
}

static MOUNT_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Resource, Clone, Debug)]
pub struct SessionRoot(pub std::path::PathBuf);

pub struct MountSlot {
    content: Arc<GameContent>,
    slot: Arc<Mutex<Slot>>,
}

impl MountSlot {
    pub fn checker(&self, data: std::path::PathBuf) -> ContentCheck {
        let (content, slot) = (self.content.clone(), self.slot.clone());
        Box::new(move |f: &Fetched| check(&content, &data, &slot, f))
    }

    pub fn switched_off(&self) -> (u64, Vec<(String, Vec<ssx_content::ContentId>)>) {
        let s = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        (s.serial, s.mount.as_ref().map(|m| m.switched_off.clone()).unwrap_or_default())
    }

    pub fn serial(&self) -> u64 {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).serial
    }

    #[cfg(test)]
    pub fn mounted(&self) -> bool {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).mount.is_some()
    }
}

impl Drop for MountSlot {
    fn drop(&mut self) {
        let mount = {
            let mut s = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
            s.closed = true;
            s.mount.take()
        };
        if let Some(m) = mount {
            self.content.end_session(m);
            info!("content: the server's resources are taken down; the player's own content is back");
        }
    }
}

pub fn resource_join(big_root: &Path, app_dir: &Path, settings: ResourceSettings) -> (ResourceJoin, MountSlot) {
    let content = GameContent::of(big_root);
    let slot: Arc<Mutex<Slot>> = Arc::default();
    let data = data_folder(big_root).to_path_buf();
    let check: ContentCheck = {
        let (content, slot) = (content.clone(), slot.clone());
        Box::new(move |f: &Fetched| check(&content, &data, &slot, f))
    };
    let release: Box<dyn FnMut() + Send> = {
        let (content, slot) = (content.clone(), slot.clone());
        Box::new(move || take_down(&content, &slot))
    };
    let options = ResourceOptions {
        cache: app_dir.join(CACHE_FOLDER),
        quota: settings.cache_bytes(),
        limit: settings.download_limit(),
        blocklists: ssx_content::blocklist::Follow::Player(app_dir.to_path_buf()),
    };
    (ResourceJoin { options, progress: Arc::new(Progress::default()), check, release }, MountSlot { content, slot })
}

fn data_folder(big_root: &Path) -> &Path {
    big_root.parent().unwrap_or(big_root)
}

pub fn join_data_check(big_root: &Path) -> Result<(), ssx_assets::folder::Incomplete> {
    let data = data_folder(big_root);
    let state = ssx_assets::folder::state(data);
    if state.is_complete() {
        return Ok(());
    }
    Err(ssx_assets::folder::Incomplete { data: data.to_path_buf(), state })
}

pub fn incomplete_text(state: &State, text: &mut dyn FnMut(&str, &[&str]) -> String) -> String {
    const REPAIR: &str = "TXT_AVA_DATA_TODO_REPAIR";
    const SETUP: &str = "TXT_AVA_DATA_TODO_SETUP";
    let (what, todo) = match state {
        State::Complete { .. } | State::Missing => ("TXT_AVA_DATA_MISSING", SETUP),
        State::Unfinished => ("TXT_AVA_DATA_UNFINISHED", SETUP),
        State::NoMovies => ("TXT_AVA_DATA_NO_MOVIES", REPAIR),
        State::NoIndex => ("TXT_AVA_DATA_NO_INDEX", REPAIR),
        State::IndexDamaged(_) => ("TXT_AVA_DATA_INDEX_DAMAGED", REPAIR),
        State::MarkerDamaged(_) => ("TXT_AVA_DATA_MARKER_DAMAGED", REPAIR),
        State::FilesDamaged(_) => ("TXT_AVA_DATA_FILES_DAMAGED", REPAIR),
    };
    let (what, todo) = (text(what, &[]), text(todo, &[]));
    text("TXT_AVA_JOIN_DATA_INCOMPLETE", &[&what, &todo])
}

fn note_data_folder(root: Res<SessionRoot>) {
    let data = data_folder(&root.0);
    match ssx_assets::folder::state(data) {
        state if state.is_complete() => info!("game data: {} is {state}", data.display()),
        state => warn!("game data: {}", ssx_assets::folder::Incomplete { data: data.to_path_buf(), state }),
    }
}

fn take_down(content: &GameContent, slot: &Mutex<Slot>) {
    let mount = slot.lock().unwrap_or_else(PoisonError::into_inner).mount.take();
    if let Some(m) = mount {
        content.end_session(m);
        info!(
            "content: the server's resources changed: their mount taken down, to be mounted as the server has them now"
        );
    }
}

fn check(
    content: &Arc<GameContent>,
    data: &Path,
    slot: &Mutex<Slot>,
    f: &Fetched,
) -> Result<Option<SimContentId>, String> {
    take_down(content, slot);
    let mount = ssx_assets::content::server::mount_fetched(content, &f.packages, f.manifest.client_mods)?;
    for (name, ids) in &mount.switched_off {
        match ids.is_empty() {
            true => info!("content: the server's policy switches the client mod {} off", shown(name)),
            false => info!(
                "content: the server's policy switches off {} simulation ids of the client mod {} ({}{})",
                ids.len(),
                shown(name),
                ids.iter().take(4).map(ToString::to_string).collect::<Vec<_>>().join(", "),
                if ids.len() > 4 { ", …" } else { "" }
            ),
        }
    }
    let id = match f.manifest.levels.is_empty() {
        true => Ok(None),
        false => ssx_assets::content::server::content_index(data)
            .and_then(|index| content.check_levels(&index, &f.manifest.levels))
            .map(Some),
    };
    let mut s = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if s.closed || id.is_err() {
        drop(s);
        content.end_session(mount);
        return id.and_then(|_| Err("cancelled".into()));
    }
    s.mount = Some(mount);
    s.serial = MOUNT_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    id
}

#[cfg(feature = "scripts")]
pub fn live_content(f: &Fetched, server_key: &str, app_dir: &Path, allowed: bool) -> Result<LiveContent, String> {
    let key = ssx_net_proto::keys::ServerKey::from_hex(server_key).map_err(|e| format!("the server's key: {e}"))?;
    let scripts = f
        .manifest
        .resources
        .iter()
        .filter(|r| r.scripts)
        .filter_map(|r| {
            let folder = &f.packages.iter().find(|(n, _)| *n == r.name)?.1;
            let files: Vec<(&str, ssx_content::ContentHash)> =
                r.files.iter().map(|x| (x.path.as_str(), x.hash)).collect();
            let version = ssx_content::delivery::files_hash(&files);
            Some(ServerScripts { name: r.name.clone(), folder: folder.clone(), version })
        })
        .collect();
    use ssx_content::policy::ClientScripts;
    let client_scripts = if f.manifest.client_scripts { ClientScripts::Allowed } else { ClientScripts::Refused };
    Ok(LiveContent {
        client_scripts,
        scripts,
        storage: Some(app_dir.join(SCRIPT_DATA).join(key.to_hex())),
        scripts_allowed: allowed,
        generation: 1,
        markers: false,
    })
}

#[derive(Resource)]
pub struct SessionContent {
    pub slot: MountSlot,
    pub fetched: Box<Fetched>,
    pub server: std::net::SocketAddr,
    #[cfg(feature = "scripts")]
    pub server_key: String,
    pub options: ResourceOptions,
}

pub fn stored_resource_settings() -> ResourceSettings {
    use ssx_ui::game::fe::ava::port_settings::{PortSettings, read_file};
    let path = ssx_assets::setup::app_dir().join("settings.json");
    read_file(&path)
        .ok()
        .map(|m| PortSettings::from_json(&serde_json::Value::Object(m)).0.resources)
        .unwrap_or_default()
}

pub fn end_with_session(mut commands: Commands, live: Option<Res<LiveSession>>, held: Option<Res<SessionContent>>) {
    if held.is_some() && live.is_none() {
        commands.remove_resource::<SessionContent>();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Follow {
    Nothing,
    Everything,
    Since(u64),
}

fn to_follow(before: Option<u64>, was_held: bool, generation: u64, held: bool) -> Follow {
    match (was_held != held, before) {
        (true, _) => Follow::Everything,
        (false, Some(b)) if held && b != generation => Follow::Since(b),
        _ => Follow::Nothing,
    }
}

pub fn follow_mount(
    mut seen: Local<(Option<u64>, bool)>,
    root: Option<Res<SessionRoot>>,
    held: Option<Res<SessionContent>>,
    mut game: Option<ResMut<crate::shell::game_mode::GameUiRes>>,
    scene: Option<ResMut<crate::ui::UiScene>>,
) {
    let Some(root) = root else { return };
    let content = GameContent::of(&root.0);
    let generation = content.generation();
    let (before, was_held) = std::mem::replace(&mut *seen, (Some(generation), held.is_some()));
    let ids = match to_follow(before, was_held, generation, held.is_some()) {
        Follow::Nothing => return,
        Follow::Everything => None,
        Follow::Since(b) => content.changes_since(b),
    };
    let changed =
        |kinds: &[ssx_content::Kind]| ids.as_ref().is_none_or(|ids| ids.iter().any(|i| kinds.contains(&i.kind())));
    use ssx_content::Kind;
    let riders = changed(&[Kind::Character, Kind::Gear, Kind::Board]);
    if riders && let Some(fe) = game.as_mut().and_then(|g| g.fe.as_mut()) {
        match fe.reload_riders(&root.0) {
            Ok(()) => info!("session: the riders' records read again ({} riders)", fe.career.riders.len()),
            Err(e) => warn!("session: the riders' records not read again: {e:#}"),
        }
    }
    let Some(mut scene) = scene else { return };
    if changed(&[Kind::Font]) {
        let n = crate::render::reload::ui_content::apply_fonts(&mut scene, game.as_deref_mut().map(|g| &mut g.0));
        info!("session: the fonts converted again, {n} text fields laid out anew");
    }
    if (riders || changed(&[Kind::String]))
        && let Some(game) = game.as_mut()
    {
        match game.reload_strings(scene.lib.files()) {
            Ok(n) => info!("session: the string table read again ({n} package strings)"),
            Err(e) => warn!("session: the string table not read again: {e:#}"),
        }
    }
}

pub struct SessionContentPlugin;

impl Plugin for SessionContentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<super::session_follow::Follow>();
        app.add_systems(Last, end_with_session).add_systems(Update, follow_mount);
        app.add_systems(Update, note_data_folder.run_if(resource_added::<SessionRoot>));
        app.add_systems(Update, super::session_follow::follow_changes.before(follow_mount));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_content::delivery::{ClientManifest, File, FileRole, FileStore, Resource};
    use ssx_content::policy::ClientMods;
    use ssx_content::{ContentHash, ContentId, Layer};

    #[test]
    fn a_refused_join_says_what_is_wrong_and_what_repairs_it() {
        let mut keys = |key: &str, args: &[&str]| format!("{key}({})", args.join("|"));
        let cases = [
            (State::NoIndex, "TXT_AVA_DATA_NO_INDEX", "TXT_AVA_DATA_TODO_REPAIR"),
            (State::NoMovies, "TXT_AVA_DATA_NO_MOVIES", "TXT_AVA_DATA_TODO_REPAIR"),
            (State::IndexDamaged("line 3".into()), "TXT_AVA_DATA_INDEX_DAMAGED", "TXT_AVA_DATA_TODO_REPAIR"),
            (State::MarkerDamaged("not JSON".into()), "TXT_AVA_DATA_MARKER_DAMAGED", "TXT_AVA_DATA_TODO_REPAIR"),
            (State::FilesDamaged("1 file: 1 missing".into()), "TXT_AVA_DATA_FILES_DAMAGED", "TXT_AVA_DATA_TODO_REPAIR"),
            (State::Unfinished, "TXT_AVA_DATA_UNFINISHED", "TXT_AVA_DATA_TODO_SETUP"),
            (State::Missing, "TXT_AVA_DATA_MISSING", "TXT_AVA_DATA_TODO_SETUP"),
        ];
        for (state, what, todo) in cases {
            let text = incomplete_text(&state, &mut keys);
            assert_eq!(text, format!("TXT_AVA_JOIN_DATA_INCOMPLETE({what}()|{todo}())"), "{state:?}");
        }
    }

    fn fetched(dir: &Path, scripts: bool) -> Fetched {
        let files: [(&str, &[u8], FileRole); 3] = [
            (
                "resource.toml",
                b"[resource]\nname = \"srv\"\nversion = \"1.0.0\"\napi = 1\n[add]\n\"model/m\" = \"models/m.gltf\"\n",
                FileRole::Manifest,
            ),
            (
                "models/m.gltf",
                br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"m.bin","byteLength":4}]}"#,
                FileRole::Content,
            ),
            ("models/m.bin", &[0u8; 4], FileRole::Data),
        ];
        let store = FileStore::open(&dir.join("cache")).unwrap();
        let mut listed = Vec::new();
        for (path, bytes, role) in files {
            let hash = ContentHash::of(bytes);
            store.put(&hash, bytes.len() as u64, &mut &bytes[..]).unwrap();
            listed.push(File { path: path.into(), hash, size: bytes.len() as u64, role, lazy: false });
        }
        let mount = store.mount().unwrap();
        let triples: Vec<(&str, ContentHash, u64)> = listed.iter().map(|f| (f.path.as_str(), f.hash, f.size)).collect();
        let laid = mount.lay_out(&store, "srv", &triples).unwrap();
        let manifest = ClientManifest {
            client_mods: ClientMods::Presentation,
            client_scripts: false,
            mirrors: Vec::new(),
            content_id: None,
            resources: vec![Resource {
                name: "srv".into(),
                version: "1.0.0".parse().unwrap(),
                title: "Server pack".into(),
                group: 0,
                scripts,
                files: listed,
            }],
            levels: Vec::new(),
        };
        Fetched {
            manifest,
            manifest_hash: ContentHash::of(b"manifest"),
            mount,
            packages: vec![("srv".into(), laid)],
            fetched_bytes: 0,
            cached_bytes: 0,
            content_id: None,
        }
    }

    fn server_id(content: &GameContent) -> bool {
        let id: ContentId = "srv:model/m".parse().unwrap();
        content.with_registry(|r| r.winner(&id).is_some_and(|w| w.layer == Layer::Server))
    }

    #[test]
    fn the_session_mounts_the_servers_resources_and_its_end_takes_them_down() {
        let dir = std::env::temp_dir().join(format!("ssx-session-content-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let big_root = dir.join("data/big");
        std::fs::create_dir_all(&big_root).unwrap();
        let f = fetched(&dir, true);
        let content = GameContent::of(&big_root);
        let (mut join, slot) = resource_join(&big_root, &dir, ResourceSettings::default());
        assert_eq!(join.options.cache, dir.join(CACHE_FOLDER));
        assert_eq!(join.options.limit, 2 << 30);
        assert_eq!((join.check)(&f), Ok(None), "no level stated: no id");
        assert!(slot.mounted() && server_id(&content));
        assert_eq!(content.session_policy(), Some(ClientMods::Presentation));
        #[cfg(feature = "scripts")]
        {
            let live = live_content(&f, &"ab".repeat(32), &dir, true).unwrap();
            assert_eq!(live.client_scripts, ssx_content::policy::ClientScripts::Refused);
            assert_eq!(live.scripts.len(), 1);
            assert_eq!(live.storage, Some(dir.join(SCRIPT_DATA).join("ab".repeat(32))));
            assert!(live_content(&f, "../../escape", &dir, true).is_err(), "a key that is not one names no folder");
        }
        assert!(slot.serial() > 0, "a mount kept has its serial");
        assert_eq!(slot.switched_off(), (slot.serial(), Vec::new()), "no client mod mounted");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(SessionContentPlugin);
        let held = SessionContent {
            slot,
            fetched: Box::new(f),
            server: std::net::SocketAddr::from(([127, 0, 0, 1], 9)),
            #[cfg(feature = "scripts")]
            server_key: "ab".repeat(32),
            options: join.options.clone(),
        };
        app.insert_resource(LiveSession::default()).insert_resource(held);
        app.update();
        let held = app.world().resource::<SessionContent>();
        assert!(server_id(&content) && held.slot.mounted(), "kept while the session lasts");
        assert_eq!(held.fetched.packages.len(), 1);
        app.world_mut().remove_resource::<LiveSession>();
        app.update();
        assert!(!app.world().contains_resource::<SessionContent>());
        assert!(!server_id(&content) && content.session_policy().is_none(), "the session's end takes it down");
        let f = fetched(&dir.join("again"), false);
        let (mut join, slot) = resource_join(&big_root, &dir, ResourceSettings::default());
        drop(slot);
        assert_eq!((join.check)(&f), Err("cancelled".into()));
        assert!(!server_id(&content) && content.session_policy().is_none());
        drop(f);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_server_telling_of_changes_gets_one_reader_at_a_time() {
        use crate::shell::session_follow::Follow as Readers;
        let dir = std::env::temp_dir().join(format!("ssx-session-follow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let big_root = dir.join("data/big");
        std::fs::create_dir_all(&big_root).unwrap();
        let f = fetched(&dir, false);
        let (mut join, slot) = resource_join(&big_root, &dir, ResourceSettings::default());
        assert_eq!((join.check)(&f), Ok(None));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let held = SessionContent {
            slot,
            fetched: Box::new(f),
            server: listener.local_addr().unwrap(),
            #[cfg(feature = "scripts")]
            server_key: "ab".repeat(32),
            options: join.options.clone(),
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(SessionContentPlugin);
        app.insert_resource(LiveSession::default()).insert_resource(held);
        app.insert_resource(SessionRoot(big_root)).insert_resource(ssx_net::ServerSaid::default());
        let tell = |app: &mut App, k: u8| {
            app.world_mut().resource_mut::<ssx_net::ServerSaid>().resources_changed = Some([k; 32]);
            app.update();
        };
        tell(&mut app, 1);
        assert_eq!(app.world().resource::<Readers>().starts, 1, "the first change is read");
        for k in [2, 3, 1, 4, 5] {
            tell(&mut app, k);
        }
        assert_eq!(app.world().resource::<Readers>().starts, 1, "one reader at a time");
        assert!(app.world().resource::<LiveSession>().end.is_none());
        drop(listener);
        let until = |app: &mut App, what: &dyn Fn(&World) -> bool| {
            let t = std::time::Instant::now();
            while !what(app.world()) && t.elapsed() < std::time::Duration::from_secs(30) {
                app.update();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            what(app.world())
        };
        assert!(
            until(&mut app, &|w| w.resource::<Readers>().starts == 2),
            "the newest change after the first's answer"
        );
        assert!(app.world().resource::<LiveSession>().end.is_none(), "a superseded reader's failure ends nothing");
        tell(&mut app, 5);
        assert_eq!(app.world().resource::<Readers>().starts, 2, "the change being read is not read again");
        assert!(until(&mut app, &|w| w.resource::<LiveSession>().end.is_some()), "a change that cannot be followed");
        let end = app.world().resource::<LiveSession>().end.clone().unwrap();
        assert!(end.reason.as_deref().is_some_and(|r| r.contains("cannot follow")), "{end:?}");
        assert_eq!(app.world().resource::<Readers>().starts, 2, "changes 2, 3 and 4 never read, 1 and 5 once");
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_front_end_follows_the_mount_only() {
        assert_eq!(to_follow(None, false, 3, false), Follow::Nothing, "the first frame of single player");
        assert_eq!(to_follow(Some(3), false, 4, false), Follow::Nothing, "a change outside a session");
        assert_eq!(to_follow(Some(3), false, 4, true), Follow::Everything, "the join's mount");
        assert_eq!(to_follow(Some(4), true, 4, true), Follow::Nothing);
        assert_eq!(to_follow(Some(4), true, 6, true), Follow::Since(4), "a restarted resource");
        assert_eq!(to_follow(Some(6), true, 7, false), Follow::Everything, "the session's end");
    }
}
