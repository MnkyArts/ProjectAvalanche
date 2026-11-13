use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::command::Invocation;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Listen {
    #[default]
    All,
    Ipv4,
    Local,
}

impl Listen {
    pub const ALL: [(Self, &'static str); 3] = [
        (Self::All, "Every network, IPv6 and IPv4"),
        (Self::Ipv4, "Every network, IPv4 only"),
        (Self::Local, "This computer only"),
    ];

    pub fn bind(self) -> &'static str {
        match self {
            Self::All => "::",
            Self::Ipv4 => "0.0.0.0",
            Self::Local => "127.0.0.1",
        }
    }

    pub fn join_address(port: u16) -> std::net::SocketAddr {
        std::net::SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), port)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostSettings {
    pub folder: Option<PathBuf>,
    pub name: String,
    pub port: u16,
    pub max_players: u16,
    pub password: String,
    pub drop: String,
    pub listen: Listen,
    pub lan: bool,
    pub public: bool,
    pub port_mapping: bool,
    pub written: Option<u64>,
    pub map: Option<HostMap>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMap {
    pub id: String,
    pub mode: String,
}

impl Default for HostSettings {
    fn default() -> Self {
        Self {
            folder: None,
            name: "Project Avalanche server".into(),
            port: 27450,
            max_players: 8,
            password: String::new(),
            drop: "exp_alps_vorassay_drop_3".into(),
            listen: Listen::All,
            lan: true,
            public: true,
            port_mapping: false,
            written: None,
            map: None,
        }
    }
}

impl HostSettings {
    pub fn folder(&self) -> PathBuf {
        self.folder_in(&crate::settings::app_dir())
    }

    pub fn folder_in(&self, app_dir: &Path) -> PathBuf {
        self.folder.clone().unwrap_or_else(|| app_dir.join("Server"))
    }

    pub fn config_path(&self) -> PathBuf {
        self.folder().join("server.toml")
    }

    pub fn data_dir(&self) -> PathBuf {
        self.folder().join("data")
    }
}

pub fn problems(h: &HostSettings) -> Vec<String> {
    let mut out = Vec::new();
    let name = h.name.trim();
    if name.is_empty() || name.chars().count() > 40 || name != h.name || name.chars().any(char::is_control) {
        out.push("Name: 1 to 40 characters, no spaces at either end".into());
    }
    if h.port == 0 || h.port == u16::MAX {
        out.push("Port: 1 to 65534 (the query takes the next one)".into());
    }
    if !(1..=64).contains(&h.max_players) {
        out.push("Players: 1 to 64".into());
    }
    if h.password.len() > 64 || h.password.chars().any(char::is_control) {
        out.push("Password: at most 64 bytes, no control characters".into());
    }
    let drop_ok = (1..=64).contains(&h.drop.len())
        && h.drop.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    match &h.map {
        Some(m) => {
            let id_ok = m.id.parse::<ssx_content::ContentId>().is_ok_and(|id| id.kind() == ssx_content::Kind::Map);
            let mode_ok = ssx_content::map::Mode::parse(&m.mode).is_some()
                || m.mode.parse::<ssx_content::ContentId>().is_ok_and(|g| g.kind() == ssx_content::Kind::Gamemode);
            if !id_ok || !mode_ok {
                out.push("Map: choose one of the list and one of its modes".into());
            }
        }
        None if !drop_ok => out.push("Drop: choose one of the list".into()),
        None => {}
    }
    if h.port_mapping && h.listen == Listen::Local {
        out.push("Map the ports: a server for this computer only needs no mapping".into());
    }
    out
}

pub fn config_text(h: &HostSettings, map: Option<&HostedMap>) -> Result<String, String> {
    use toml::{Table, Value};
    let mut server = Table::new();
    server.insert("name".into(), Value::String(h.name.clone()));
    server.insert("bind".into(), Value::String(h.listen.bind().into()));
    server.insert("port".into(), Value::Integer(i64::from(h.port)));
    server.insert("max_players".into(), Value::Integer(i64::from(h.max_players)));
    server.insert("password".into(), Value::String(h.password.clone()));
    let mut game = Table::new();
    let mut resources = None;
    match (&h.map, map) {
        (Some(m), Some(hosted)) if hosted.map.id == m.id && hosted.mode() == m.mode => {
            game.insert("map".into(), Value::String(m.id.clone()));
            game.insert("mode".into(), Value::String(hosted.server_mode().into()));
            if let Some(g) = &hosted.gamemode {
                game.insert("gamemode".into(), Value::String(g.id.clone()));
            }
            let start = hosted.packages().into_iter().map(|(_, name)| Value::String(name)).collect();
            let mut r = Table::new();
            r.insert("start".into(), Value::Array(start));
            resources = Some(r);
        }
        (Some(m), _) => {
            return Err(format!(
                "the map {} in the mode {} is not installed (the Mods page installs packages)",
                m.id, m.mode
            ));
        }
        (None, _) => {
            game.insert("drop".into(), Value::String(h.drop.clone()));
        }
    }
    let mut discovery = Table::new();
    let public = h.public && h.listen != Listen::Local;
    discovery.insert("query".into(), Value::Boolean(h.lan || public));
    discovery.insert("lan".into(), Value::Boolean(h.lan && h.listen != Listen::Local));
    discovery.insert("public".into(), Value::Boolean(public));
    let mut mapping = Table::new();
    mapping.insert("enabled".into(), Value::Boolean(h.port_mapping));
    let mut stop = Table::new();
    stop.insert("round_end".into(), Value::Integer(i64::from(HOST_ROUND_END_SECS)));
    let mut root = Table::new();
    root.insert("server".into(), Value::Table(server));
    root.insert("game".into(), Value::Table(game));
    root.insert("discovery".into(), Value::Table(discovery));
    root.insert("port_mapping".into(), Value::Table(mapping));
    root.insert("stop".into(), Value::Table(stop));
    if let Some(r) = resources {
        root.insert("resources".into(), Value::Table(r));
    }
    let body = toml::to_string(&root).map_err(|e| e.to_string())?;
    Ok(format!("{HEADER}{body}"))
}

const HEADER: &str = "# Written by the Project Avalanche launcher's Host page. The page writes this file again when its settings are\n\
                      # saved there; a file changed by hand is kept until the page is told to replace it.\n\n";

pub fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xCBF2_9CE4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01B3))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Written {
    Yes(u64),
    EditedByHand,
}

pub fn write_config(path: &Path, text: &str, last: Option<u64>, replace: bool) -> Result<Written, String> {
    let dir = path.parent().ok_or("the server file has no folder")?;
    match std::fs::symlink_metadata(dir) {
        Ok(m) if !m.is_dir() => return Err(format!("{} is not a folder (or is a link)", dir.display())),
        Ok(_) => {}
        Err(_) => ssx_files::create_dir_all(dir).map_err(|e| e.to_string())?,
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if !m.is_file() => return Err(format!("{} is not a file (or is a link)", path.display())),
        Ok(_) if !replace => {
            let have = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            if Some(fnv(&have)) != last && have != text.as_bytes() {
                return Ok(Written::EditedByHand);
            }
        }
        _ => {}
    }
    ssx_files::replace(path, text.as_bytes(), &ssx_files::Options::new())
        .map_err(|e| format!("writing {}: {}", path.display(), e.io()))?;
    Ok(Written::Yes(fnv(text.as_bytes())))
}

pub fn setup_from_extraction(h: &HostSettings, game_data: &Path, map: Option<&InstalledMap>) -> Invocation {
    let mut args: Vec<OsString> =
        vec!["setup".into(), "--from-extraction".into(), game_data.into(), "--out".into(), h.data_dir().into()];
    args.extend(setup_config(h));
    args.extend(setup_classes(map));
    Invocation { program: crate::command::server_exe(), args, env: Vec::new() }
}

pub fn setup_from_image(h: &HostSettings, iso: &Path, map: Option<&InstalledMap>) -> Invocation {
    let mut args: Vec<OsString> = vec!["setup".into(), iso.into(), "--out".into(), h.data_dir().into()];
    args.extend(setup_config(h));
    args.extend(setup_classes(map));
    Invocation { program: crate::command::server_exe(), args, env: Vec::new() }
}

fn setup_config(h: &HostSettings) -> [OsString; 2] {
    ["--config".into(), h.config_path().into()]
}

fn setup_classes(map: Option<&InstalledMap>) -> Vec<OsString> {
    match map {
        Some(m) if m.layout => vec!["--with".into(), "verify,collision".into()],
        Some(_) => vec!["--with".into(), "verify".into()],
        None => Vec::new(),
    }
}

pub fn check(h: &HostSettings) -> Invocation {
    let args: Vec<OsString> =
        vec!["check".into(), "--config".into(), h.config_path().into(), "--data".into(), h.data_dir().into()];
    Invocation { program: crate::command::server_exe(), args, env: Vec::new() }
}

pub fn run(h: &HostSettings) -> Invocation {
    let args: Vec<OsString> = vec![
        "run".into(),
        "--stop-with-input".into(),
        "--config".into(),
        h.config_path().into(),
        "--data".into(),
        h.data_dir().into(),
    ];
    Invocation { program: crate::command::server_exe(), args, env: Vec::new() }
}

// PORT(choice): 2 000 lines (the viewer's output list's), 400 characters a line, 6 s for the server's own stop (its 2 s
// grace, the master's goodbye at most 3 s, a second's margin); tuning: a busy server's stop measured; the original has
// no launcher.
const LINES: usize = 2000;
const LINE: usize = 400;
pub const STOP_WAIT: Duration = Duration::from_secs(6 + HOST_ROUND_END_SECS as u64 + 5 + 2);
// PORT(choice): 15 s (the server's own default is 60 s; a player stopping a server of their own, or closing the
// launcher, waits at most STOP_WAIT, 28 s); tuning: players' stops of hosted servers; the original has no launcher.
pub const HOST_ROUND_END_SECS: u32 = 15;

pub struct Process {
    pub lines: Arc<Mutex<Vec<String>>>,
    pub status: Arc<Mutex<Option<String>>>,
    pub pid: u32,
    stdin: Option<ChildStdin>,
    kill_at: Arc<Mutex<Option<Instant>>>,
}

impl Process {
    pub fn spawn(ctx: &egui::Context, inv: &Invocation, log: &Path, console: bool) -> std::io::Result<Self> {
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::fs::File::create(log)?;
        writeln!(file, "$ {}", inv.display())?;
        let mut cmd = inv.command();
        cmd.stdin(if console { Stdio::piped() } else { Stdio::null() });
        cmd.stdout(Stdio::from(file.try_clone()?)).stderr(Stdio::from(file));
        cmd.env("NO_COLOR", "1");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn()?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(Mutex::new(None));
        let kill_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let exited = Arc::new(AtomicBool::new(false));
        let (done, flag, deadline, repaint) =
            (Arc::clone(&status), Arc::clone(&exited), Arc::clone(&kill_at), ctx.clone());
        std::thread::spawn(move || {
            let text = wait(&mut child, &deadline);
            flag.store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(300));
            if let Ok(mut d) = done.lock() {
                *d = Some(text);
            }
            repaint.request_repaint();
        });
        let (out, ctx, path) = (Arc::clone(&lines), ctx.clone(), log.to_path_buf());
        std::thread::spawn(move || tail(&path, &out, &exited, &ctx));
        Ok(Self { lines, status, pid, stdin, kill_at })
    }

    pub fn status(&self) -> Option<String> {
        self.status.lock().ok()?.clone()
    }

    pub fn running(&self) -> bool {
        self.status().is_none()
    }

    pub fn send(&mut self, line: &str) -> bool {
        let Some(stdin) = &mut self.stdin else { return false };
        let ok = stdin.write_all(format!("{line}\n").as_bytes()).and_then(|()| stdin.flush()).is_ok();
        if !ok {
            self.stdin = None;
        }
        ok
    }

    pub fn stop(&mut self) {
        self.send("stop");
        if let Ok(mut k) = self.kill_at.lock() {
            k.get_or_insert(Instant::now() + STOP_WAIT);
        }
    }

    pub fn kill(&mut self) {
        self.stdin = None;
        if let Ok(mut k) = self.kill_at.lock() {
            *k = Some(Instant::now());
        }
    }

    pub fn stop_and_wait(&mut self) {
        self.stop();
        let until = Instant::now() + STOP_WAIT + Duration::from_secs(1);
        while self.running() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stdin = None;
    }
}

fn wait(child: &mut Child, kill_at: &Mutex<Option<Instant>>) -> String {
    loop {
        match child.try_wait() {
            Ok(Some(s)) if s.success() => return "exited normally".into(),
            Ok(Some(s)) => {
                return match s.code() {
                    Some(code) => format!("exited with code {code}"),
                    None => format!("ended: {s}"),
                };
            }
            Ok(None) => {}
            Err(e) => return format!("could not be waited for: {e}"),
        }
        if kill_at.lock().ok().and_then(|k| *k).is_some_and(|t| Instant::now() >= t) {
            let _ = child.kill();
            let _ = child.wait();
            return "stopped (killed: it did not end in time)".into();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn tail(path: &Path, lines: &Mutex<Vec<String>>, exited: &AtomicBool, ctx: &egui::Context) {
    let Ok(file) = std::fs::File::open(path) else { return };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        let finished = exited.load(Ordering::Acquire);
        let read = (&mut reader).take(64 * 1024).read_until(b'\n', &mut line);
        match read {
            Ok(n) if n > 0 && (line.ends_with(b"\n") || line.len() >= 64 * 1024) => {
                push(lines, &line);
                line.clear();
                ctx.request_repaint();
            }
            Ok(_) if !finished => std::thread::sleep(Duration::from_millis(100)),
            Ok(_) | Err(_) => {
                if !line.is_empty() {
                    push(lines, &line);
                }
                return;
            }
        }
    }
}

pub fn compact(line: &str) -> String {
    let Some((stamp, rest)) = line.split_once(char::is_whitespace) else { return line.to_string() };
    let time =
        stamp.get(11..19).filter(|t| stamp.len() > 20 && stamp.ends_with('Z') && t.as_bytes().get(2) == Some(&b':'));
    let Some((level, rest)) = rest.trim_start().split_once(char::is_whitespace) else { return line.to_string() };
    let Some((_target, message)) = rest.trim_start().split_once(": ") else { return line.to_string() };
    match (time, level) {
        (Some(t), "INFO") => format!("{t} UTC {message}"),
        (Some(t), "WARN" | "ERROR") => format!("{t} UTC {level} {message}"),
        _ => line.to_string(),
    }
}

fn push(lines: &Mutex<Vec<String>>, raw: &[u8]) {
    let text = ssx_core::text::shown_max(String::from_utf8_lossy(raw).trim_end(), LINE).to_string();
    if let Ok(mut l) = lines.lock() {
        if l.len() >= LINES {
            l.remove(0);
        }
        l.push(text);
    }
}

const TUTORIAL_DROPS: [u32; 2] = [0xC145_699A, 0xB0F3_51A2];

pub fn drops(big_root: &Path) -> Result<Vec<String>, String> {
    let db = ssx_assets::career::CareerDb::load_disc(big_root).map_err(|e| format!("{e:#}"))?;
    let mut v: Vec<String> = db
        .events
        .iter()
        .filter(|e| !TUTORIAL_DROPS.contains(&e.key) && db.event_named(&e.name).is_some_and(|f| f.key == e.key))
        .map(|e| e.name.clone())
        .collect();
    v.sort();
    v.dedup();
    Ok(v)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledMap {
    pub id: String,
    pub title: String,
    pub modes: Vec<String>,
    pub layout: bool,
    pub packages: Vec<(String, String)>,
    pub intro: ssx_content::map::Intro,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledGamemode {
    pub id: String,
    pub title: String,
    pub event_type: ssx_content::gamemode::EventType,
    pub maps: Vec<String>,
    pub any_map: bool,
    pub packages: Vec<(String, String)>,
}

impl InstalledGamemode {
    pub fn takes(&self, m: &InstalledMap) -> bool {
        self.names(&m.id) && ssx_content::gamemode::event_takes_start(self.event_type, m.intro).is_ok()
    }

    fn names(&self, id: &str) -> bool {
        self.any_map || self.maps.iter().any(|m| m == id)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostMaps {
    pub maps: Vec<InstalledMap>,
    pub gamemodes: Vec<InstalledGamemode>,
    pub unusable: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedMap {
    pub map: InstalledMap,
    pub gamemode: Option<InstalledGamemode>,
    builtin: Option<String>,
}

impl HostedMap {
    pub fn mode(&self) -> &str {
        match (&self.gamemode, &self.builtin) {
            (Some(g), _) => &g.id,
            (None, Some(b)) => b,
            (None, None) => "",
        }
    }

    pub fn server_mode(&self) -> &str {
        use ssx_content::gamemode::EventType;
        match &self.gamemode {
            None => self.mode(),
            Some(g) => match g.event_type {
                EventType::RaceClock | EventType::RaceGhosts | EventType::RaceBots => "race",
                EventType::TrickScore | EventType::TrickBots | EventType::TrickGhosts => "trick",
                EventType::SurviveToBottom | EventType::SurviveToDeath => "survive",
                EventType::FreeRide => "freeride",
            },
        }
    }

    pub fn packages(&self) -> Vec<(String, String)> {
        let mut out = self.map.packages.clone();
        for p in self.gamemode.iter().flat_map(|g| &g.packages) {
            if !out.iter().any(|(_, n)| *n == p.1) {
                out.push(p.clone());
            }
        }
        out
    }
}

impl HostMaps {
    pub fn modes_of(&self, m: &InstalledMap) -> Vec<(String, String)> {
        let built_in = m.modes.iter().map(|x| (x.clone(), x.clone()));
        let gamemodes = self.gamemodes.iter().filter(|g| g.takes(m)).map(|g| {
            let event = ssx_content::gamemode::event_name(g.event_type);
            (g.id.clone(), format!("{} ({}, {event})", g.title, g.id))
        });
        built_in.chain(gamemodes).collect()
    }

    pub fn hosted(&self, choice: &HostMap) -> Result<HostedMap, String> {
        let shown = |s: &str| ssx_core::text::shown(s).to_string();
        let map = self.maps.iter().find(|x| x.id == choice.id).ok_or_else(|| {
            format!("the map {} is not installed (the Mods page installs packages)", shown(&choice.id))
        })?;
        if map.modes.contains(&choice.mode) {
            return Ok(HostedMap { map: map.clone(), gamemode: None, builtin: Some(choice.mode.clone()) });
        }
        let Some(g) = self.gamemodes.iter().find(|g| g.id == choice.mode) else {
            return Err(format!(
                "the map offers the modes {}, and no installed gamemode is {}",
                map.modes.join(", "),
                shown(&choice.mode)
            ));
        };
        if !g.names(&map.id) {
            return Err(format!("the gamemode {} does not take the map {} (its [[gamemode.map]] list)", g.id, map.id));
        }
        if let Err(e) = ssx_content::gamemode::event_takes_start(g.event_type, map.intro) {
            return Err(format!("the gamemode {} on the map {}: {e}", g.id, map.id));
        }
        Ok(HostedMap { map: map.clone(), gamemode: Some(g.clone()), builtin: None })
    }
}

// PORT(choice): 256 maps and 32 packages (port caps: round net6's delivery carries at most 256 resources a server —
// `ssx_content::delivery::MAX_RESOURCES` —, the test maps' requirements are 0 and 1); tuning: players' mods folders
// once maps are shared; the original has no launcher.
pub const MAX_HOST_MAPS: usize = 256;
const MAX_MAP_PACKAGES: usize = 32;

pub fn installed_maps(app_dir: &Path) -> HostMaps {
    use ssx_assets::content::mods::{MODS_DIR, scan};
    let (_, rows) = scan(app_dir);
    let mods = app_dir.join(MODS_DIR);
    let packages: Vec<(&str, &ssx_content::Manifest)> = rows
        .iter()
        .filter(|r| r.present)
        .filter_map(|r| r.package.as_ref().ok().map(|m| (r.folder.as_str(), m)))
        .collect();
    let mut out = HostMaps::default();
    for &(folder, m) in &packages {
        let maps = m.add.iter().chain(m.overrides.iter()).filter(|(id, _)| id.kind() == ssx_content::Kind::Map);
        for (id, entry) in maps {
            if out.maps.len() + out.gamemodes.len() + out.unusable.len() >= MAX_HOST_MAPS {
                out.unusable.push(format!("more than {MAX_HOST_MAPS} maps and gamemodes: the rest are not listed"));
                return sorted(out);
            }
            let shown = ssx_core::text::shown(&id.to_string()).to_string();
            let record = entry.files().first().copied().ok_or_else(|| "no map.toml".to_string());
            let toml = record.and_then(|f| read_map_toml(&mods.join(folder), f));
            let order = start_order(&packages, folder, m);
            match (toml, order) {
                (Ok(t), Ok(packages)) => out.maps.push(InstalledMap {
                    id: id.to_string(),
                    title: t.title,
                    modes: t.modes.iter().map(|x| x.name().to_string()).collect(),
                    layout: t.base.is_some(),
                    packages,
                    intro: t.intro,
                }),
                (Err(e), _) | (_, Err(e)) => out.unusable.push(format!("{shown}: {}", ssx_core::text::message(&e))),
            }
        }
        let gamemodes =
            m.add.iter().chain(m.overrides.iter()).filter(|(id, _)| id.kind() == ssx_content::Kind::Gamemode);
        for (id, entry) in gamemodes {
            if out.maps.len() + out.gamemodes.len() + out.unusable.len() >= MAX_HOST_MAPS {
                out.unusable.push(format!("more than {MAX_HOST_MAPS} maps and gamemodes: the rest are not listed"));
                return sorted(out);
            }
            let shown = ssx_core::text::shown(&id.to_string()).to_string();
            let record = entry.files().first().copied().ok_or_else(|| "no gamemode.toml".to_string());
            let toml = record.and_then(|f| read_gamemode_toml(&mods.join(folder), f));
            match (toml, start_order(&packages, folder, m)) {
                (Ok(t), Ok(packages)) => out.gamemodes.push(InstalledGamemode {
                    id: id.to_string(),
                    title: t.title,
                    event_type: t.event_type,
                    maps: t.maps.iter().map(|e| e.map.to_string()).collect(),
                    any_map: t.any_map,
                    packages,
                }),
                (Err(e), _) | (_, Err(e)) => out.unusable.push(format!("{shown}: {}", ssx_core::text::message(&e))),
            }
        }
    }
    sorted(out)
}

fn sorted(mut m: HostMaps) -> HostMaps {
    m.maps.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));
    m.gamemodes.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));
    m
}

fn read_map_toml(root: &Path, file: &str) -> Result<ssx_content::map::MapToml, String> {
    let bytes = read_record(root, file)?;
    ssx_content::map::parse_toml(&bytes).map_err(|p| problems_text(file, &p))
}

fn read_gamemode_toml(root: &Path, file: &str) -> Result<ssx_content::gamemode::GamemodeToml, String> {
    let bytes = read_record(root, file)?;
    ssx_content::gamemode::parse_toml(&bytes).map_err(|p| problems_text(file, &p))
}

fn read_record(root: &Path, file: &str) -> Result<Vec<u8>, String> {
    let source = ssx_content::PackageSource::open(root).map_err(|e| e.to_string())?;
    source.tree().read(file, ssx_content::limits::MAX_RECORD_BYTES).map_err(|e| e.to_string())
}

fn problems_text(file: &str, problems: &[(usize, usize, String)]) -> String {
    problems.iter().map(|(line, col, m)| format!("{file}:{line}:{col}: {m}")).collect::<Vec<_>>().join("; ")
}

fn start_order(
    packages: &[(&str, &ssx_content::Manifest)],
    folder: &str,
    m: &ssx_content::Manifest,
) -> Result<Vec<(String, String)>, String> {
    fn visit(
        packages: &[(&str, &ssx_content::Manifest)],
        folder: &str,
        m: &ssx_content::Manifest,
        path: &mut Vec<String>,
        out: &mut Vec<(String, String)>,
    ) -> Result<(), String> {
        let name = &m.resource.name;
        if out.iter().any(|(_, n)| n == name) {
            return Ok(());
        }
        if path.contains(name) {
            return Err(format!("{name} requires itself through {}", path.join(" → ")));
        }
        if path.len() >= MAX_MAP_PACKAGES {
            return Err(format!("more than {MAX_MAP_PACKAGES} packages deep"));
        }
        path.push(name.clone());
        for dep in &m.dependencies.require {
            let found =
                packages.iter().find(|(_, p)| p.resource.name == dep.name && dep.req.matches(p.resource.version));
            let Some(&(f, p)) = found else {
                return Err(format!("{name} requires {} {}, which is not installed", dep.name, dep.req));
            };
            visit(packages, f, p, path, out)?;
        }
        path.pop();
        if out.len() >= MAX_MAP_PACKAGES {
            return Err(format!("more than {MAX_MAP_PACKAGES} packages"));
        }
        out.push((folder.to_string(), name.clone()));
        Ok(())
    }
    let mut out = Vec::new();
    visit(packages, folder, m, &mut Vec::new(), &mut out)?;
    Ok(out)
}

pub fn resources_dir(h: &HostSettings) -> PathBuf {
    h.data_dir().join("resources")
}

pub fn stage_map(h: &HostSettings, map: &HostedMap, app_dir: &Path) -> Result<Vec<String>, String> {
    let mods = app_dir.join(ssx_assets::content::mods::MODS_DIR);
    let to = resources_dir(h);
    ssx_files::create_dir_all(&to).map_err(|e| e.to_string())?;
    let mut done = Vec::new();
    for (folder, name) in &map.packages() {
        let installed =
            ssx_content::install::install_folder(&mods.join(folder), &to, true).map_err(|e| format!("{name}: {e}"))?;
        if installed.folder != *name {
            return Err(format!("{folder}: the package in it is {}, not {name}", installed.folder));
        }
        if let Some((_, aside)) = installed.previous {
            std::fs::remove_dir_all(&aside).map_err(|e| format!("{}: {e}", aside.display()))?;
        }
        done.push(format!(
            "{name} {} ({} files, {} bytes)",
            installed.manifest.resource.version, installed.files, installed.bytes
        ));
    }
    Ok(done)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Prepare,
    Start,
    Status,
    Stop,
}

impl Action {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "prepare" => Some(Self::Prepare),
            "start" => Some(Self::Start),
            "status" => Some(Self::Status),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shown {
    #[default]
    Server,
    Setup,
    Check,
}

#[derive(Default)]
pub struct HostPage {
    pub server: Option<Process>,
    pub setup: Option<Process>,
    pub check: Option<Process>,
    pub shown: Shown,
    pub note: Option<(bool, String)>,
    pub edited_by_hand: bool,
    pub drops: Option<Result<Vec<String>, String>>,
    pub loading_drops: Option<crate::jobs::Background<Result<Vec<String>, String>>>,
    pub maps: Option<HostMaps>,
    pub loading_maps: Option<crate::jobs::Background<HostMaps>>,
    pub staging: Option<crate::jobs::Background<Result<Vec<String>, String>>>,
    pub script: std::collections::VecDeque<Action>,
    pub scripted: Option<(Action, Instant)>,
}

impl HostPage {
    pub fn server_running(&self) -> bool {
        self.server.as_ref().is_some_and(Process::running)
    }

    pub fn busy(&self) -> bool {
        self.setup.as_ref().is_some_and(Process::running)
            || self.check.as_ref().is_some_and(Process::running)
            || self.staging.is_some()
    }

    pub fn scripted_done(&self) -> bool {
        let Some((action, at)) = self.scripted else { return true };
        if at.elapsed() > Duration::from_secs(60) {
            return true;
        }
        let server_said = |what: &str| {
            self.server.as_ref().is_some_and(|p| p.lines.lock().is_ok_and(|l| l.iter().any(|x| x.contains(what))))
        };
        match action {
            Action::Prepare => self.staging.is_none() && !self.setup.as_ref().is_some_and(Process::running),
            Action::Start => server_said("listening on") || !self.server_running(),
            Action::Status => server_said("uptime ") || !self.server_running(),
            Action::Stop => !self.server_running(),
        }
    }

    pub fn lines(&self) -> Vec<String> {
        let p = match self.shown {
            Shown::Server => &self.server,
            Shown::Setup => &self.setup,
            Shown::Check => &self.check,
        };
        p.as_ref().and_then(|p| p.lines.lock().ok().map(|l| l.clone())).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_is_written_the_one_way_and_hand_edits_are_kept() {
        let dir = std::env::temp_dir().join(format!("avalanche-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut h =
            HostSettings { folder: Some(dir.clone()), name: "Ann \"the\" Rider's".into(), ..HostSettings::default() };
        h.password = "a\"b\\c".into();
        let text = config_text(&h, None).expect("toml");
        let parsed: toml::Table = text.parse().expect("reads back");
        assert_eq!(parsed["server"]["name"].as_str(), Some("Ann \"the\" Rider's"));
        assert_eq!(parsed["server"]["password"].as_str(), Some("a\"b\\c"));
        let discovery = |h: &HostSettings| -> toml::Table {
            let t: toml::Table = config_text(h, None).expect("toml").parse().expect("reads back");
            t["discovery"].as_table().expect("[discovery]").clone()
        };
        let listed = discovery(&h);
        assert_eq!((listed["public"].as_bool(), listed["query"].as_bool()), (Some(true), Some(true)));
        assert!(!listed.contains_key("masters"), "{listed:?}");
        let unlisted = discovery(&HostSettings { public: false, lan: false, ..h.clone() });
        assert_eq!((unlisted["public"].as_bool(), unlisted["query"].as_bool()), (Some(false), Some(false)));
        let local = discovery(&HostSettings { listen: Listen::Local, lan: false, ..h.clone() });
        assert_eq!((local["public"].as_bool(), local["query"].as_bool()), (Some(false), Some(false)));
        assert_eq!(parsed["stop"]["round_end"].as_integer(), Some(i64::from(HOST_ROUND_END_SECS)));
        let path = h.config_path();
        let Written::Yes(hash) = write_config(&path, &text, None, false).unwrap() else { panic!() };
        assert_eq!(write_config(&path, &text, Some(hash), false).unwrap(), Written::Yes(hash));
        std::fs::write(&path, "[server]\nname = \"mine\"\n").unwrap();
        assert_eq!(write_config(&path, &text, Some(hash), false).unwrap(), Written::EditedByHand);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[server]\nname = \"mine\"\n", "kept");
        assert_eq!(write_config(&path, &text, Some(hash), true).unwrap(), Written::Yes(hash), "replaced when asked");
        assert!(problems(&h).is_empty());
        h.name = " padded".into();
        h.port = 65535;
        h.drop = "../x".into();
        assert_eq!(problems(&h).len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_installed_map_is_hosted_with_its_packages() {
        use ssx_assets::content::modtool::{Template, new_package};
        let app = std::env::temp_dir().join(format!("avalanche-host-maps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app);
        let mods = app.join(ssx_assets::content::mods::MODS_DIR);
        for name in ["tpl", "dep", "lone"] {
            new_package(name, Template::Map, &mods, Path::new("")).expect("the map template");
        }
        let require = |name: &str, req: &str| {
            let path = mods.join(name).join(ssx_content::manifest::MANIFEST_FILE);
            let text = std::fs::read_to_string(&path).expect("manifest");
            std::fs::write(&path, format!("{text}\n[dependencies]\nrequire = [\"{req}\"]\n")).expect("manifest");
        };
        require("dep", "tpl >=0.1.0");
        require("lone", "missing >=1.0.0");
        let found = installed_maps(&app);
        let ids: Vec<&str> = found.maps.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["dep:map/course", "tpl:map/course"], "{found:#?}");
        assert_eq!(found.unusable.len(), 1, "{found:#?}");
        let why = &found.unusable[0];
        assert!(why.starts_with("lone:map/course: lone requires missing ") && why.ends_with("not installed"), "{why}");
        let dep = &found.maps[0];
        let order: Vec<&str> = dep.packages.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(order, ["tpl", "dep"]);
        assert_eq!(
            (dep.title.as_str(), dep.modes.clone(), dep.layout),
            ("Template slope", vec!["race".into(), "freeride".into()], false)
        );
        let mut h = HostSettings { folder: Some(app.join("Server")), ..HostSettings::default() };
        h.map = Some(HostMap { id: dep.id.clone(), mode: "race".into() });
        assert!(problems(&h).is_empty(), "{:?}", problems(&h));
        let hosted = found.hosted(h.map.as_ref().expect("a map")).expect("hosted");
        assert_eq!((hosted.mode(), hosted.server_mode()), ("race", "race"));
        let parsed: toml::Table = config_text(&h, Some(&hosted)).expect("toml").parse().expect("reads back");
        assert_eq!(parsed["game"]["map"].as_str(), Some("dep:map/course"));
        assert_eq!(parsed["game"]["mode"].as_str(), Some("race"));
        assert!(parsed["game"].get("drop").is_none());
        assert!(parsed["game"].get("gamemode").is_none());
        let start: Vec<&str> =
            parsed["resources"]["start"].as_array().expect("start").iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(start, ["tpl", "dep"]);
        assert!(config_text(&h, None).is_err(), "a map not installed");
        for _ in 0..2 {
            let done = stage_map(&h, &hosted, &app).expect("staged");
            assert_eq!(done.len(), 2, "{done:?}");
        }
        let laid: Vec<String> = std::fs::read_dir(resources_dir(&h))
            .expect("resources")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(laid, ["dep", "tpl"], "no copy left aside");
        let args = |inv: Invocation| inv.args.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert!(
            args(setup_from_image(&h, Path::new("/x.iso"), Some(dep))).ends_with(&["--with".into(), "verify".into()])
        );
        assert_eq!(args(setup_from_image(&h, Path::new("/x.iso"), None)).len(), 6);
        let config = h.config_path().to_string_lossy().into_owned();
        assert!(args(setup_from_image(&h, Path::new("/x.iso"), None)).ends_with(&["--config".into(), config]));
        h.map = Some(HostMap { id: "dep:object/x".into(), mode: "race".into() });
        assert_eq!(problems(&h).len(), 1);
        h.map = Some(HostMap { id: dep.id.clone(), mode: "chess".into() });
        assert_eq!(problems(&h).len(), 1);
        std::fs::remove_dir_all(&app).expect("cleaned");
    }

    #[test]
    fn an_installed_map_is_hosted_under_an_installed_gamemode() {
        use ssx_assets::content::modtool::{Template, new_package};
        let app = std::env::temp_dir().join(format!("avalanche-host-gamemodes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app);
        let mods = app.join(ssx_assets::content::mods::MODS_DIR);
        new_package("tpl", Template::Map, &mods, Path::new("")).expect("the map template");
        for name in ["gm", "anyone"] {
            new_package(name, Template::Gamemode, &mods, Path::new("")).expect("the gamemode template");
        }
        let gm_toml = mods.join("gm").join("map.toml");
        let text = std::fs::read_to_string(&gm_toml).expect("gm's map.toml");
        assert_eq!(text.matches("intro = \"spawn\"").count(), 1, "{text}");
        std::fs::write(&gm_toml, text.replace("intro = \"spawn\"", "intro = \"heli\"")).expect("gm's map.toml");
        let record = mods.join("anyone").join(ssx_content::gamemode::template::GAMEMODE_PATH);
        std::fs::write(&record, "[gamemode]\ntitle = \"Anyone\"\nevent = \"survive_to_death\"\nany_map = true\n")
            .expect("gamemode.toml");
        let manifest = mods.join("anyone").join(ssx_content::manifest::MANIFEST_FILE);
        let text = std::fs::read_to_string(&manifest).expect("manifest");
        std::fs::write(&manifest, format!("{text}\n[dependencies]\nrequire = [\"tpl >=0.1.0\"]\n")).expect("manifest");
        let found = installed_maps(&app);
        assert!(found.unusable.is_empty(), "{found:#?}");
        let ids: Vec<&str> = found.gamemodes.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["anyone:gamemode/gully_race", "gm:gamemode/gully_race"], "{found:#?}");
        let tpl = found.maps.iter().find(|m| m.id == "tpl:map/course").expect("tpl's map");
        let gm_map = found.maps.iter().find(|m| m.id == "gm:map/course").expect("gm's map");
        let values = |m: &InstalledMap| found.modes_of(m).into_iter().map(|(v, _)| v).collect::<Vec<_>>();
        assert_eq!(values(tpl), ["race", "freeride"]);
        assert_eq!(values(gm_map), ["race", "freeride", "anyone:gamemode/gully_race", "gm:gamemode/gully_race"]);
        assert_eq!(found.modes_of(gm_map)[3].1, "Gully Race (gm:gamemode/gully_race, race_ghosts)");
        let mut h = HostSettings { folder: Some(app.join("Server")), ..HostSettings::default() };
        h.map = Some(HostMap { id: gm_map.id.clone(), mode: "gm:gamemode/gully_race".into() });
        assert!(problems(&h).is_empty(), "{:?}", problems(&h));
        let hosted = found.hosted(h.map.as_ref().expect("a map")).expect("hosted");
        assert_eq!(hosted.server_mode(), "race");
        let parsed: toml::Table = config_text(&h, Some(&hosted)).expect("toml").parse().expect("reads back");
        assert_eq!(parsed["game"]["map"].as_str(), Some("gm:map/course"));
        assert_eq!(parsed["game"]["mode"].as_str(), Some("race"));
        assert_eq!(parsed["game"]["gamemode"].as_str(), Some("gm:gamemode/gully_race"));
        h.map = Some(HostMap { id: gm_map.id.clone(), mode: "anyone:gamemode/gully_race".into() });
        let hosted = found.hosted(h.map.as_ref().expect("a map")).expect("hosted");
        assert_eq!(hosted.server_mode(), "survive", "survive to death is a survive instance");
        let parsed: toml::Table = config_text(&h, Some(&hosted)).expect("toml").parse().expect("reads back");
        let start: Vec<&str> =
            parsed["resources"]["start"].as_array().expect("start").iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(start, ["gm", "tpl", "anyone"]);
        let done = stage_map(&h, &hosted, &app).expect("staged");
        assert_eq!(done.len(), 3, "{done:?}");
        let e = found.hosted(&HostMap { id: tpl.id.clone(), mode: "gm:gamemode/gully_race".into() }).unwrap_err();
        assert!(e.contains("does not take the map tpl:map/course"), "{e}");
        let e = found.hosted(&HostMap { id: tpl.id.clone(), mode: "anyone:gamemode/gully_race".into() }).unwrap_err();
        assert!(e.contains("intro = \"heli\""), "{e}");
        let e = found.hosted(&HostMap { id: tpl.id.clone(), mode: "x:gamemode/none".into() }).unwrap_err();
        assert!(e.contains("no installed gamemode is"), "{e}");
        assert!(config_text(&h, None).is_err(), "a gamemode not installed");
        std::fs::remove_dir_all(&app).expect("cleaned");
    }

    #[test]
    fn log_lines_are_compacted_and_others_kept() {
        let l = "2026-10-29T13:13:23.599393Z  INFO avalanche_server::app: listening on 127.0.0.1:27470 (UDP and TCP)";
        assert_eq!(compact(l), "13:13:23 UTC listening on 127.0.0.1:27470 (UDP and TCP)");
        let w = "2026-10-29T13:13:23.599393Z  WARN avalanche_server::portmap: port mapping: off: no gateway";
        assert_eq!(compact(w), "13:13:23 UTC WARN port mapping: off: no gateway");
        for other in ["$ avalanche-server run", "  extraction 1/342 files", "", "setup refused: x"] {
            assert_eq!(compact(other), other);
        }
    }

    #[test]
    fn the_server_is_started_one_argument_per_word() {
        let h = HostSettings { folder: Some(PathBuf::from("/srv/my server; rm -rf ~")), ..HostSettings::default() };
        let r = run(&h);
        assert_eq!(r.args[0], "run");
        assert_eq!(r.args[1], "--stop-with-input");
        assert_eq!(PathBuf::from(&r.args[3]), PathBuf::from("/srv/my server; rm -rf ~/server.toml"), "one argument");
        assert_eq!(r.args.len(), 6);
    }
}
