use std::fmt;
use std::net::IpAddr;
use std::path::Path;

use ssx_net_proto::rcon::OperatorKey;

use crate::range::Range;
use crate::rules::FileError;

mod check;
mod edit;
mod file;

pub use check::{Source, Verdict};
pub use edit::{Access, AclError};

pub const ROOTS: [&str; 4] = ["command", "function", "resource", "general"];
// PORT(choice): 256 groups, 4 096 objects and 1 024 rights a group, 4 096 requests; far above MTA's default acl.xml (a
// handful of groups and ACLs of a few hundred rights, as recalled); tuning: the ACLs of long-running servers; the
// original has no dedicated server.
pub const MAX_GROUPS: usize = 256;
pub const MAX_OBJECTS: usize = 4096;
pub const MAX_RIGHTS: usize = 1024;
pub const MAX_REQUESTS: usize = 4096;
pub const MAX_RIGHT_BYTES: usize = 128;
pub const MAX_SEGMENTS: usize = 8;
pub const MAX_SEGMENT_BYTES: usize = 32;
pub const MAX_GROUP_BYTES: usize = 32;
pub const MAX_RESOURCE_BYTES: usize = 64;

pub const RESOURCE_DENIED: [&str; 13] = [
    "function.load",
    "function.loadstring",
    "function.fetchRemote",
    "function.callRemote",
    "function.startResource",
    "function.stopResource",
    "function.restartResource",
    "function.acl",
    "general.ModifyOtherObjects",
    "function.kickPlayer",
    "function.banPlayer",
    "function.mutePlayer",
    "function.executeCommand",
];
pub const RESOURCE_ALLOWED: [&str; 1] = ["function"];

pub fn right(text: &str) -> Result<&str, String> {
    let shown = ssx_core::text::quoted(text);
    if text.is_empty() || text.len() > MAX_RIGHT_BYTES {
        return Err(format!("{shown} is not a right (1 to {MAX_RIGHT_BYTES} bytes)"));
    }
    let segments: Vec<&str> = text.split('.').collect();
    if segments.len() > MAX_SEGMENTS
        || !segments.iter().all(|s| {
            !s.is_empty() && s.len() <= MAX_SEGMENT_BYTES && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
    {
        return Err(format!("{shown} is not a right (segments of A-Z, a-z, 0-9, _ joined by dots)"));
    }
    if !ROOTS.contains(&segments[0]) {
        return Err(format!("{shown} does not start with command, function, resource or general"));
    }
    Ok(text)
}

pub fn resource(text: &str) -> Result<&str, String> {
    let ok = !text.is_empty()
        && text.len() <= MAX_RESOURCE_BYTES
        && text != "."
        && text != ".."
        && text.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-'));
    if !ok {
        return Err(format!("{} is not a resource name (a-z, 0-9, _ . -)", ssx_core::text::quoted(text)));
    }
    Ok(text)
}

pub fn group_name(text: &str) -> Result<&str, String> {
    let ok = !text.is_empty()
        && text.len() <= MAX_GROUP_BYTES
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if !ok {
        return Err(format!("{} is not a group name (A-Z, a-z, 0-9, _ . -)", ssx_core::text::quoted(text)));
    }
    Ok(text)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Object {
    Operator(OperatorKey),
    Player([u8; 32]),
    Players,
    Ip(Range),
    Resource(String),
    Resources,
}

impl Object {
    pub fn parse(text: &str) -> Result<Self, String> {
        let shown = ssx_core::text::quoted(text);
        let (kind, rest) = text.split_once('.').ok_or_else(|| format!("{shown} is not kind.name"))?;
        let object = match (kind, rest) {
            ("operator", "*") => return Err("operator.* is refused: name each operator's key".into()),
            ("operator", hex) => Self::Operator(OperatorKey::from_hex(hex).map_err(|e| format!("{shown}: {e}"))?),
            ("player", "*") => Self::Players,
            ("player", hex) => Self::Player(crate::rules::player_id(hex)?),
            ("ip", r) => Self::Ip(Range::parse(r)?),
            ("resource", "*") => Self::Resources,
            ("resource", name) => Self::Resource(resource(name)?.to_string()),
            _ => return Err(format!("{shown} is not operator.<key>, player.<id>, ip.<range> or resource.<name>")),
        };
        if object.to_string() != text {
            return Err(format!("{shown}: write it as {object}"));
        }
        Ok(object)
    }

    pub fn holds(&self, subject: &Subject<'_>) -> bool {
        match (self, subject) {
            (Self::Operator(k), Subject::Operator(o)) => k == *o,
            (Self::Player(p), Subject::Player { id, .. }) => p == *id,
            (Self::Players, Subject::Player { .. }) => true,
            (Self::Ip(r), Subject::Player { ip: Some(ip), .. }) => r.contains(*ip),
            (Self::Resource(n), Subject::Resource(r)) => n == r,
            (Self::Resources, Subject::Resource(_)) => true,
            _ => false,
        }
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Operator(k) => write!(f, "operator.{}", k.to_hex()),
            Self::Player(id) => write!(f, "player.{}", ssx_net_proto::keys::to_hex(id)),
            Self::Players => f.write_str("player.*"),
            Self::Ip(r) => write!(f, "ip.{r}"),
            Self::Resource(n) => write!(f, "resource.{n}"),
            Self::Resources => f.write_str("resource.*"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Subject<'a> {
    Console,
    Operator(&'a OperatorKey),
    Player { id: &'a [u8; 32], ip: Option<IpAddr> },
    Resource(&'a str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub objects: Vec<Object>,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestState {
    Pending,
    Allowed,
    Denied,
}

impl RequestState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Allowed => "allowed",
            Self::Denied => "denied",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub resource: String,
    pub right: String,
    pub state: RequestState,
    pub asked: u64,
    pub by: String,
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Acl {
    groups: Vec<Group>,
    requests: Vec<Request>,
}

impl Acl {
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    pub fn requests(&self) -> &[Request] {
        &self.requests
    }

    pub fn is_operator(&self, key: &OperatorKey) -> bool {
        self.groups.iter().any(|g| g.objects.iter().any(|o| matches!(o, Object::Operator(k) if k == key)))
    }

    pub fn has_operators(&self) -> bool {
        self.groups.iter().any(|g| g.objects.iter().any(|o| matches!(o, Object::Operator(_))))
    }

    pub fn parse(text: &str) -> Result<Self, FileError> {
        file::parse(text)
    }

    pub fn to_toml(&self) -> String {
        file::write(self)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let shown = path.display().to_string();
        match crate::rules::read_file(path) {
            Ok(None) => Ok(Self::default()),
            Ok(Some(text)) => Self::parse(&text).map_err(|e| ssx_core::text::located(&shown, e).to_string()),
            Err(e) => Err(ssx_core::text::located(&shown, e).to_string()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        ssx_files::replace(path, self.to_toml().as_bytes(), &ssx_files::Options::new()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
