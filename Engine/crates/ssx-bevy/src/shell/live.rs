use bevy::prelude::Resource;

use crate::ride::live::LiveRules;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveEnd {
    pub reason: Option<String>,
    #[cfg(feature = "net")]
    pub refusal: Option<ssx_ui::game::fe::ava::browser::RefusalCode>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LiveOrigin {
    #[default]
    Hub,
    #[cfg(feature = "net")]
    Browser,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LiveLevel {
    #[default]
    None,
    Loading,
    Up,
    Left,
    Away,
    Next,
}

#[derive(Resource, Debug, Default)]
pub struct LiveSession {
    pub rules: Option<LiveRules>,
    pub launch: Option<SessionLevel>,
    pub ridden: Option<SessionLevel>,
    pub relaunch: Option<SessionLevel>,
    pub end: Option<LiveEnd>,
    pub level: LiveLevel,
    pub server: Option<String>,
    pub origin: LiveOrigin,
    #[cfg(feature = "scripts")]
    pub serial: SessionSerial,
    #[cfg(feature = "scripts")]
    pub content: LiveContent,
}

#[cfg(feature = "scripts")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionSerial(u64);

#[cfg(feature = "scripts")]
impl SessionSerial {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[cfg(feature = "scripts")]
impl Default for SessionSerial {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        SessionSerial(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[cfg(feature = "scripts")]
#[derive(Clone, Debug)]
pub struct LiveContent {
    pub client_scripts: ssx_content::policy::ClientScripts,
    pub scripts: Vec<ServerScripts>,
    pub storage: Option<std::path::PathBuf>,
    pub scripts_allowed: bool,
    pub generation: u64,
    pub markers: bool,
}

#[cfg(feature = "scripts")]
impl Default for LiveContent {
    fn default() -> Self {
        LiveContent {
            client_scripts: ssx_content::policy::ClientScripts::SESSION_DEFAULT,
            scripts: Vec::new(),
            storage: None,
            scripts_allowed: false,
            generation: 0,
            markers: false,
        }
    }
}

#[cfg(feature = "scripts")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerScripts {
    pub name: String,
    pub folder: std::path::PathBuf,
    pub version: ssx_content::ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionLevel {
    Drop(String),
    #[cfg_attr(not(feature = "net"), allow(dead_code))]
    Map {
        id: String,
        event_type: u8,
        mode: String,
    },
}

impl core::fmt::Display for SessionLevel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Drop(d) => f.write_str(d),
            Self::Map { id, mode, .. } => write!(f, "the map {id} ({mode})"),
        }
    }
}

impl LiveSession {
    pub fn ending(&self) -> bool {
        self.end.is_some()
    }
}
