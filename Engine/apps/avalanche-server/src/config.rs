use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use ssx_net::limits::MAX_CLIENTS;
use ssx_net_proto::join::MAX_PASSWORD_BYTES;
use ssx_net_proto::name::server_name;
use ssx_net_proto::{GhostStyle, InstanceSpec, Mode, Rewinds};
use toml::Spanned;
use zeroize::Zeroizing;

mod discovery;
pub use discovery::Discovery;
use discovery::{RawDiscovery, parse_discovery};

pub const DEFAULT_PORT: u16 = ssx_net_proto::DEFAULT_PORT;
pub const DEFAULT_MAX_PLAYERS: u16 = 16;
pub const DEFAULT_NAME: &str = "Project Avalanche server";
// PORT(choice): the default drop is a name (the server reads no disc, so its default can only be a name) until phase
// 4's setup of the server's rules data, which knows the operator's drops; the original has no dedicated server.
pub const DEFAULT_DROP: &str = "exp_alps_vorassay_drop_3";
const MAX_FILE: u64 = 64 * 1024;

#[derive(Clone)]
pub struct Config {
    pub path: Option<PathBuf>,
    pub name: String,
    pub description: String,
    pub bind: IpAddr,
    pub port: u16,
    pub max_players: u16,
    pub password: Zeroizing<String>,
    pub instances: Vec<crate::game_config::GameConfig>,
    pub data_folder: PathBuf,
    pub validation: ssx_gameplay::validate::Policy,
    pub chat: bool,
    pub discovery: Discovery,
    pub port_mapping: PortMapping,
    pub resources: crate::resources::config::Resources,
    pub mods: crate::resources::config::Mods,
    pub operation: crate::operation::config::OperationConfig,
    pub notices: Vec<String>,
}

impl Config {
    pub fn defaults() -> Self {
        Self {
            path: None,
            name: DEFAULT_NAME.into(),
            description: String::new(),
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: DEFAULT_PORT,
            max_players: DEFAULT_MAX_PLAYERS,
            password: Zeroizing::new(String::new()),
            instances: vec![crate::game_config::GameConfig { spec: default_game(), capacity: None, rounds: None }],
            data_folder: PathBuf::from(crate::rules::DEFAULT_DATA_FOLDER),
            validation: ssx_gameplay::validate::Policy::default(),
            chat: true,
            discovery: Discovery::default(),
            port_mapping: PortMapping::default(),
            resources: Default::default(),
            mods: Default::default(),
            operation: crate::operation::config::OperationConfig::default(),
            notices: Vec::new(),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind, self.port)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{path}: {error}")]
    Io { path: PathBuf, error: std::io::Error },

    #[error("{path}: larger than {MAX_FILE} bytes")]
    TooLarge { path: PathBuf },

    #[error("{path}:{line}:{column}: {message}")]
    At { path: PathBuf, line: usize, column: usize, message: String },
}

pub(crate) fn default_game() -> InstanceSpec {
    InstanceSpec {
        level: ssx_net_proto::instance::Level::Drop(DEFAULT_DROP.into()),
        mode: Mode::Freeride,
        rewinds: Rewinds::On,
        ghosts: false,
        ghost_style: GhostStyle::Character,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    server: Option<RawServer>,
    game: Option<Spanned<crate::game_config::RawGame>>,
    instance: Option<Spanned<Vec<Spanned<crate::game_config::RawGame>>>>,
    data: Option<crate::rules::RawData>,
    validation: Option<crate::rules::RawValidation>,
    chat: Option<RawChat>,
    discovery: Option<RawDiscovery>,
    port_mapping: Option<RawPortMapping>,
    resources: Option<crate::resources::config::RawResources>,
    mods: Option<crate::resources::config::RawMods>,
    metrics: Option<crate::operation::config::RawMetrics>,
    stop: Option<crate::operation::config::RawStop>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChat {
    enabled: Option<Spanned<bool>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServer {
    name: Option<Spanned<String>>,
    description: Option<Spanned<String>>,
    bind: Option<Spanned<String>>,
    port: Option<Spanned<i64>>,
    max_players: Option<Spanned<i64>>,
    password: Option<Spanned<String>>,
    icon: Option<Spanned<String>>,
}

fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

pub fn load(path: &Path) -> Result<Config, ConfigError> {
    use std::io::Read;
    let io = |error| ConfigError::Io { path: path.to_path_buf(), error };
    let mut text = String::new();
    match std::fs::File::open(path) {
        Ok(file) => {
            file.take(MAX_FILE + 1).read_to_string(&mut text).map_err(io)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::defaults()),
        Err(e) => return Err(io(e)),
    }
    if text.len() as u64 > MAX_FILE {
        return Err(ConfigError::TooLarge { path: path.to_path_buf() });
    }
    parse(path, &text)
}

pub fn parse(path: &Path, text: &str) -> Result<Config, ConfigError> {
    let at = |span: Range<usize>, message: String| {
        let (line, column) = position(text, span.start);
        ConfigError::At { path: path.to_path_buf(), line, column, message }
    };
    let raw: RawFile = toml::from_str(text).map_err(|e| at(e.span().unwrap_or(0..0), e.message().to_string()))?;
    let mut config = Config::defaults();
    config.path = Some(path.to_path_buf());
    if let Some(instances) = crate::game_config::instances(raw.game, raw.instance, &at)? {
        config.instances = instances;
    }
    if let Some(d) = raw.data {
        config.data_folder = crate::rules::parse_data(d, &at)?;
    }
    if let Some(v) = raw.validation {
        config.validation = crate::rules::parse_validation(v, &at)?;
    }
    if let Some(enabled) = raw.chat.and_then(|c| c.enabled) {
        config.chat = enabled.into_inner();
    }
    if let Some(d) = raw.discovery {
        config.discovery = parse_discovery(d, &at, &mut config.notices)?;
    }
    if let Some(r) = raw.resources {
        config.resources = crate::resources::config::parse_resources(r, &at)?;
    }
    if let Some(m) = raw.mods {
        config.mods = crate::resources::config::parse_mods(m, &at)?;
    }
    let port_mapping = raw.port_mapping;
    crate::operation::config::parse_tables(raw.metrics, raw.stop, &mut config.operation, &at)?;
    let Some(server) = raw.server else { return finish(config, port_mapping, &at) };
    if let Some(name) = server.name {
        let span = name.span();
        let checked = server_name(name.get_ref()).map_err(|e| at(span.clone(), format!("name: {e}")))?;
        if checked != *name.get_ref() {
            return Err(at(span, "name: no spaces at either end and no runs of spaces".into()));
        }
        config.name = checked;
    }
    if let Some(description) = server.description {
        ssx_net_proto::info::description(description.get_ref())
            .map_err(|e| at(description.span(), format!("description: {e}")))?;
        config.description = description.into_inner();
    }
    if let Some(bind) = server.bind {
        config.bind = bind
            .get_ref()
            .parse::<IpAddr>()
            .map_err(|_| at(bind.span(), format!("bind = {:?}: an IPv4 or IPv6 address", bind.get_ref())))?;
        if config.bind.is_multicast() {
            return Err(at(bind.span(), "bind: a unicast address (or 0.0.0.0 / ::)".into()));
        }
    }
    if let Some(port) = server.port {
        config.port = u16::try_from(*port.get_ref())
            .ok()
            .filter(|&p| p != 0)
            .ok_or_else(|| at(port.span(), format!("port = {}: 1 to 65535", port.get_ref())))?;
    }
    if let Some(max) = server.max_players {
        config.max_players = u16::try_from(*max.get_ref())
            .ok()
            .filter(|&m| m >= 1 && usize::from(m) <= MAX_CLIENTS)
            .ok_or_else(|| at(max.span(), format!("max_players = {}: 1 to {MAX_CLIENTS}", max.get_ref())))?;
    }
    if let Some(password) = server.password {
        let p = password.get_ref();
        if p.len() > MAX_PASSWORD_BYTES || p.chars().any(char::is_control) {
            return Err(at(
                password.span(),
                format!("password: at most {MAX_PASSWORD_BYTES} bytes, no control characters"),
            ));
        }
        config.password = Zeroizing::new(p.clone());
    }
    if let Some(icon) = server.icon {
        config.operation.icon = Some(crate::operation::config::parse_icon(icon, &at)?);
    }
    finish(config, port_mapping, &at)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortMapping {
    pub enabled: bool,
    pub methods: Vec<ssx_net_portmap::Method>,
    pub gateway: Option<Ipv4Addr>,
    pub lifetime: u32,
}

pub const DEFAULT_LIFETIME: u32 = 7200;
// PORT(choice): a day at most (IGD 2 takes up to 604 800 s, NAT-PMP and PCP what the gateway grants); tuning: gateways
// that refuse long leases; the original has no dedicated server.
pub const LIFETIMES: std::ops::RangeInclusive<i64> = 120..=86_400;

impl Default for PortMapping {
    fn default() -> Self {
        PortMapping {
            enabled: false,
            methods: vec![ssx_net_portmap::Method::Upnp, ssx_net_portmap::Method::Pcp, ssx_net_portmap::Method::NatPmp],
            gateway: None,
            lifetime: DEFAULT_LIFETIME,
        }
    }
}

impl PortMapping {
    pub fn describe(&self) -> String {
        if !self.enabled {
            return "off".into();
        }
        let methods: Vec<&str> = self.methods.iter().map(|m| m.name()).collect();
        let gateway = self.gateway.map_or("the default route's gateway".to_string(), |g| g.to_string());
        format!("on: {} at {gateway}, {} s, renewed at half", methods.join(", "), self.lifetime)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPortMapping {
    enabled: Option<Spanned<bool>>,
    methods: Option<Spanned<Vec<Spanned<String>>>>,
    gateway: Option<Spanned<String>>,
    lifetime: Option<Spanned<i64>>,
}

fn finish(
    mut config: Config,
    port_mapping: Option<RawPortMapping>,
    at: &dyn Fn(Range<usize>, String) -> ConfigError,
) -> Result<Config, ConfigError> {
    if let Some(raw) = port_mapping {
        config.port_mapping = parse_port_mapping(raw, config.bind, at)?;
    }
    Ok(config)
}

fn parse_port_mapping(
    r: RawPortMapping,
    bind: IpAddr,
    at: &dyn Fn(Range<usize>, String) -> ConfigError,
) -> Result<PortMapping, ConfigError> {
    let mut out = PortMapping::default();
    let mut enabled_at = 0..0;
    if let Some(e) = r.enabled {
        enabled_at = e.span();
        out.enabled = e.into_inner();
    }
    if let Some(list) = r.methods {
        if list.get_ref().is_empty() {
            return Err(at(list.span(), "methods: at least one of \"upnp\", \"pcp\", \"natpmp\"".into()));
        }
        out.methods.clear();
        for m in list.into_inner() {
            let method = ssx_net_portmap::Method::parse(m.get_ref()).ok_or_else(|| {
                at(
                    m.span(),
                    format!(
                        "methods: {:?}: \"upnp\", \"pcp\" or \"natpmp\"",
                        m.get_ref().chars().take(32).collect::<String>()
                    ),
                )
            })?;
            if out.methods.contains(&method) {
                return Err(at(m.span(), format!("methods: {} twice", method.name())));
            }
            out.methods.push(method);
        }
    }
    if let Some(g) = r.gateway
        && !g.get_ref().is_empty()
    {
        {
            let ip: Ipv4Addr = g.get_ref().parse().map_err(|_| {
                at(
                    g.span(),
                    format!(
                        "gateway = {:?}: an IPv4 address (or \"\" for the default route's)",
                        g.get_ref().chars().take(40).collect::<String>()
                    ),
                )
            })?;
            if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
                return Err(at(g.span(), format!("gateway = \"{ip}\": a unicast address")));
            }
            out.gateway = Some(ip);
        }
    }
    if let Some(l) = r.lifetime {
        let v = *l.get_ref();
        if !LIFETIMES.contains(&v) {
            return Err(at(l.span(), format!("lifetime = {v}: {} to {} seconds", LIFETIMES.start(), LIFETIMES.end())));
        }
        out.lifetime = u32::try_from(v).map_err(|_| at(l.span(), "lifetime: out of range".into()))?;
    }
    let ipv6_only = matches!(bind, IpAddr::V6(v6) if !v6.is_unspecified() && v6.to_ipv4_mapped().is_none());
    if out.enabled && ipv6_only {
        return Err(at(
            enabled_at,
            format!(
                "enabled = true: the server binds the IPv6 address {bind} alone; port mapping maps IPv4 ports on a NAT gateway"
            ),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_net_proto::instance::drop_name;

    fn err(text: &str) -> String {
        match parse(Path::new("server.toml"), text) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("accepted: {text}"),
        }
    }

    #[test]
    fn values_and_defaults() {
        let c = parse(
            Path::new("s.toml"),
            "[server]\nname = \"Alps Night Riders\"\ndescription = \"Freeride, all welcome\"\nbind = \"127.0.0.1\"\nport = 27500\nmax_players = 8\n",
        )
        .expect("valid");
        assert_eq!(c.description, "Freeride, all welcome");
        assert_eq!(
            (c.name.as_str(), c.addr().to_string(), c.max_players),
            ("Alps Night Riders", "127.0.0.1:27500".into(), 8)
        );
        assert!(c.password.is_empty());
        let d = parse(Path::new("s.toml"), "").expect("empty");
        assert_eq!((d.port, d.max_players), (DEFAULT_PORT, DEFAULT_MAX_PLAYERS));
        assert_eq!(*d.game(), default_game());
        assert_eq!(d.instances.len(), 1);
        assert_eq!(drop_name(DEFAULT_DROP), Ok(DEFAULT_DROP));
        let g = parse(
            Path::new("s.toml"),
            "[game]\ndrop = \"exp_himwest_makalu_drop_1_trick\"\nmode = \"freeride\"\nrewinds = 3\nghosts = \"on\"\nghost_style = \"hologram\"\n",
        )
        .expect("valid [game]");
        assert_eq!(
            (g.game().level.name(), g.game().rewinds, g.game().ghosts, g.game().ghost_style),
            ("exp_himwest_makalu_drop_1_trick", Rewinds::Limit(3), true, GhostStyle::Hologram)
        );
        let off = parse(Path::new("s.toml"), "[game]\nrewinds = \"off\"\n").expect("off");
        assert_eq!(off.game().rewinds, Rewinds::Off);
        assert!(d.chat, "the chat is on by default");
        assert!(!parse(Path::new("s.toml"), "[chat]\nenabled = false\n").expect("[chat]").chat);
        assert!(parse(Path::new("s.toml"), "[chat]\n").expect("an empty [chat]").chat);
    }

    #[test]
    fn unknown_keys_and_ranges_are_refused_with_the_line() {
        assert_eq!(err("[server]\nport = 27450\nregion = \"eu\"\n").split(": ").next(), Some("server.toml:3:1"));
        assert!(err("[game]\nrewinds = \"sometimes\"\n").starts_with("server.toml:2:11: rewinds"));
        assert!(err("[game]\nrewinds = 0\n").contains("1 to 99"));
        assert!(err("[game]\nrewinds = 100\n").contains("1 to 99"));
        assert!(err("[game]\ndrop = \"Exp_Alps\"\n").starts_with("server.toml:2:8: drop"));
        assert!(err("[game]\ndrop = \"../../etc\"\n").contains("drop"));
        assert!(err("[game]\nmode = \"races\"\n").contains("\"race\""));
        assert!(err("[game]\nghosts = \"yes\"\n").contains("ghosts"));
        assert!(err("[game]\nghosts = true\n").starts_with("server.toml:2:"));
        assert!(err("[game]\nghost_style = \"ghost\"\n").contains("hologram"));
        assert!(err("[game]\npose_rate_hz = 10\n").starts_with("server.toml:2:1"));
        assert!(err("[server]\n\nport = 0\n").starts_with("server.toml:3:8: port = 0"));
        assert!(err("[server]\nport = 70000\n").contains("1 to 65535"));
        assert!(err("[server]\nmax_players = 65\n").starts_with("server.toml:2:15: max_players = 65: 1 to 64"));
        assert!(err("[server]\nmax_players = -1\n").contains("max_players"));
        assert!(err("[server]\nname = \" padded\"\n").starts_with("server.toml:2:8"));
        assert!(err("[server]\nname = \"日本\"\n").contains("cannot draw"));
        assert!(err(&format!("[server]\nname = \"{}\"\n", "N".repeat(41))).contains("at most 40"));
        assert!(err("[server]\ndescription = \"two\\nlines\"\n").starts_with("server.toml:2:15: description"));
        assert!(err(&format!("[server]\ndescription = \"{}\"\n", "d".repeat(251))).contains("at most 250"));
        assert!(err("[server]\nbind = \"localhost\"\n").contains("IPv4 or IPv6"));
        assert!(err("[server]\nbind = \"224.0.0.1\"\n").contains("unicast"));
        assert!(err(&format!("[server]\npassword = \"{}\"\n", "x".repeat(65))).contains("password"));
        assert!(err("[server]\npassword = \"a\\u0007\"\n").contains("password"));
        assert!(err("[server]\nport = \"27450\"\n").starts_with("server.toml:2:"));
        assert!(err("[server]\nport = 1\nport = 2\n").starts_with("server.toml:3:"));
        assert!(err("[chat]\nenabled = \"no\"\n").starts_with("server.toml:2:11"));
        assert!(err("[chat]\nenabled = 0\n").starts_with("server.toml:2:11"));
        assert!(err("[chat]\nrate = 5\n").starts_with("server.toml:2:1"));
        assert!(err("[chat]\nenabled = true\nenabled = false\n").starts_with("server.toml:3:"));
    }

    #[test]
    fn port_mapping_is_off_by_default_and_checked() {
        use ssx_net_portmap::Method;
        assert_eq!(parse(Path::new("s.toml"), "").expect("empty").port_mapping, PortMapping::default());
        assert!(!PortMapping::default().enabled);
        let c = parse(
            Path::new("s.toml"),
            "[port_mapping]\nenabled = true\nmethods = [\"natpmp\", \"upnp\"]\ngateway = \"192.168.1.1\"\nlifetime = 3600\n",
        )
        .expect("valid");
        let m = c.port_mapping;
        assert!(m.enabled);
        assert_eq!(
            (m.methods, m.gateway, m.lifetime),
            (vec![Method::NatPmp, Method::Upnp], Some(Ipv4Addr::new(192, 168, 1, 1)), 3600)
        );
        assert!(err("[port_mapping]\nenabled = \"yes\"\n").starts_with("server.toml:2:11"));
        assert!(err("[port_mapping]\nmethods = []\n").starts_with("server.toml:2:11: methods: at least one"));
        assert!(err("[port_mapping]\nmethods = [\"igd\"]\n").starts_with("server.toml:2:12: methods: \"igd\""));
        assert!(err("[port_mapping]\nmethods = [\"pcp\", \"pcp\"]\n").contains("pcp twice"));
        assert!(err("[port_mapping]\nmethods = \"pcp\"\n").starts_with("server.toml:2:"));
        assert!(err("[port_mapping]\ngateway = \"router.lan\"\n").starts_with("server.toml:2:11: gateway"));
        assert!(err("[port_mapping]\ngateway = \"::1\"\n").contains("IPv4 address"));
        assert!(err("[port_mapping]\ngateway = \"255.255.255.255\"\n").contains("unicast"));
        assert!(err("[port_mapping]\nlifetime = 60\n").starts_with("server.toml:2:12: lifetime = 60: 120 to 86400"));
        assert!(err("[port_mapping]\nlifetime = 86401\n").contains("120 to 86400"));
        assert!(err("[port_mapping]\nlifetime = \"1h\"\n").starts_with("server.toml:2:"));
        assert!(err("[port_mapping]\nport = 5351\n").starts_with("server.toml:2:1"));
        assert!(
            err("[server]\nbind = \"::1\"\n[port_mapping]\nenabled = true\n")
                .starts_with("server.toml:4:11: enabled = true: the server binds the IPv6 address ::1 alone")
        );
        assert!(
            parse(Path::new("s.toml"), "[server]\nbind = \"::\"\n[port_mapping]\nenabled = true\n").is_ok(),
            "the dual stack maps its IPv4"
        );
    }

    #[test]
    fn data_folder_and_validation_policy_are_checked() {
        use ssx_gameplay::validate::{Action, Check, Policy};
        let d = parse(Path::new("s.toml"), "").expect("empty");
        assert_eq!((d.data_folder.as_path(), d.validation), (Path::new("big"), Policy::default()));
        let c = parse(
            Path::new("s.toml"),
            "[data]\nfolder = \"/srv/ssx/big\"\n[validation]\nspeed = \"warn\"\nclock = \"kick\"\n",
        )
        .expect("valid");
        assert_eq!(c.data_folder, PathBuf::from("/srv/ssx/big"));
        assert_eq!(
            (c.validation.of(Check::Speed), c.validation.of(Check::Clock), c.validation.of(Check::Step)),
            (Action::Warn, Action::Kick, Action::Invalidate)
        );
        assert!(err("[data]\nfolder = \"\"\n").starts_with("server.toml:2:10: folder"));
        assert!(err("[data]\nfolder = \"a\\u0000b\"\n").contains("control characters"));
        assert!(err("[data]\nfolders = \"x\"\n").starts_with("server.toml:2:1"));
        assert!(err("[validation]\nspeed = \"ban\"\n").starts_with("server.toml:2:9: speed = \"ban\""));
        assert!(err("[validation]\nteleport = \"kick\"\n").starts_with("server.toml:2:1"));
        assert!(err("[validation]\nclock = true\n").starts_with("server.toml:2:9"));
    }
}
