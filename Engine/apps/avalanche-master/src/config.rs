use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use ssx_net_proto::master::MAX_LIST_SERVERS;
use toml::Spanned;

use crate::forwarded::{MAX_TRUSTED, Network, Trusted};
use crate::names::{HostName, MAX_TRUSTED_HOSTS};

// PORT(choice): 27900, away from the game ports (27450 and up, each with its query port + 1) and LAN discovery's 35450;
// a public master behind a name usually takes 443 (the operator's choice); the original has no master server.
pub const DEFAULT_PORT: u16 = 27900;
// PORT(choice): 4 096 servers by default (MTA's master lists a few thousand); tuning: the size of real masters' lists;
// the original has no master server.
pub const DEFAULT_MAX_SERVERS: usize = 4096;
pub const DEFAULT_MAX_PER_ADDRESS: usize = 16;
pub const MAX_PER_ADDRESS: usize = 256;
const MAX_FILE: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub path: Option<PathBuf>,
    pub bind: IpAddr,
    pub port: u16,
    pub max_servers: usize,
    pub max_per_address: usize,
    pub allow_private: bool,
    pub tls: Option<Tls>,
    pub blocked_servers: Option<PathBuf>,
    pub blocked_resources: Option<PathBuf>,
    pub trusted: Trusted,
    pub trusted_hosts: Vec<HostName>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tls {
    pub certificate: PathBuf,
    pub key: PathBuf,
}

impl Config {
    pub fn defaults() -> Self {
        Self {
            path: None,
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: DEFAULT_PORT,
            max_servers: DEFAULT_MAX_SERVERS,
            max_per_address: DEFAULT_MAX_PER_ADDRESS,
            allow_private: false,
            tls: None,
            blocked_servers: None,
            blocked_resources: None,
            trusted: Trusted::default(),
            trusted_hosts: Vec::new(),
        }
    }

    pub fn behind_proxy(&self) -> bool {
        !self.trusted.is_empty() || !self.trusted_hosts.is_empty()
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    master: Option<RawMaster>,
    tls: Option<RawTls>,
    blocklists: Option<RawBlocklists>,
    proxy: Option<RawProxy>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProxy {
    trusted: Option<Spanned<Vec<Spanned<String>>>>,
    trusted_hosts: Option<Spanned<Vec<Spanned<String>>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMaster {
    bind: Option<Spanned<String>>,
    port: Option<Spanned<i64>>,
    max_servers: Option<Spanned<i64>>,
    max_servers_per_address: Option<Spanned<i64>>,
    allow_private_addresses: Option<Spanned<bool>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTls {
    certificate: Spanned<String>,
    key: Spanned<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlocklists {
    server_keys: Option<Spanned<String>>,
    resources: Option<Spanned<String>>,
}

fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset.min(text.len())).unwrap_or(text);
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

fn file_path(
    base: &Path,
    raw: &Spanned<String>,
    at: &dyn Fn(Range<usize>, String) -> ConfigError,
) -> Result<PathBuf, ConfigError> {
    let p = raw.get_ref();
    if p.is_empty() || p.contains('\0') {
        return Err(at(raw.span(), "a file's path".into()));
    }
    Ok(base.join(p))
}

fn ranged(
    v: &Spanned<i64>,
    range: std::ops::RangeInclusive<i64>,
    what: &str,
    at: &dyn Fn(Range<usize>, String) -> ConfigError,
) -> Result<i64, ConfigError> {
    let n = *v.get_ref();
    if !range.contains(&n) {
        return Err(at(v.span(), format!("{what} = {n}: {} to {}", range.start(), range.end())));
    }
    Ok(n)
}

pub fn parse(path: &Path, text: &str) -> Result<Config, ConfigError> {
    let at = |span: Range<usize>, message: String| {
        let (line, column) = position(text, span.start);
        ConfigError::At { path: path.to_path_buf(), line, column, message }
    };
    let raw: RawFile = toml::from_str(text).map_err(|e| at(e.span().unwrap_or(0..0), e.message().to_string()))?;
    let base = path.parent().unwrap_or(Path::new("")).to_path_buf();
    let mut c = Config::defaults();
    c.path = Some(path.to_path_buf());
    if let Some(m) = raw.master {
        if let Some(bind) = m.bind {
            c.bind = bind
                .get_ref()
                .parse::<IpAddr>()
                .map_err(|_| at(bind.span(), format!("bind = {:?}: an IPv4 or IPv6 address", bind.get_ref())))?;
            if c.bind.is_multicast() {
                return Err(at(bind.span(), "bind: a unicast address (or 0.0.0.0 / ::)".into()));
            }
        }
        if let Some(port) = m.port {
            c.port = ranged(&port, 1..=65535, "port", &at)? as u16;
        }
        if let Some(max) = m.max_servers {
            c.max_servers = ranged(&max, 1..=MAX_LIST_SERVERS as i64, "max_servers", &at)? as usize;
        }
        if let Some(max) = m.max_servers_per_address {
            c.max_per_address = ranged(&max, 1..=MAX_PER_ADDRESS as i64, "max_servers_per_address", &at)? as usize;
        }
        if let Some(allow) = m.allow_private_addresses {
            c.allow_private = allow.into_inner();
        }
    }
    if let Some(t) = raw.tls {
        c.tls = Some(Tls { certificate: file_path(&base, &t.certificate, &at)?, key: file_path(&base, &t.key, &at)? });
    }
    if let Some(b) = raw.blocklists {
        c.blocked_servers = b.server_keys.as_ref().map(|p| file_path(&base, p, &at)).transpose()?;
        c.blocked_resources = b.resources.as_ref().map(|p| file_path(&base, p, &at)).transpose()?;
    }
    let (trusted, hosts) = raw.proxy.map_or((None, None), |p| (p.trusted, p.trusted_hosts));
    if let Some(trusted) = trusted {
        if trusted.get_ref().len() > MAX_TRUSTED {
            return Err(at(trusted.span(), format!("trusted: at most {MAX_TRUSTED} networks")));
        }
        let mut networks: Vec<Network> = Vec::new();
        for raw in trusted.get_ref() {
            let network = Network::parse(raw.get_ref()).map_err(|e| at(raw.span(), format!("trusted: {e}")))?;
            if networks.contains(&network) {
                return Err(at(raw.span(), format!("trusted: {network} twice")));
            }
            networks.push(network);
        }
        c.trusted = Trusted::new(networks);
    }
    if let Some(hosts) = hosts {
        if hosts.get_ref().len() > MAX_TRUSTED_HOSTS {
            return Err(at(hosts.span(), format!("trusted_hosts: at most {MAX_TRUSTED_HOSTS} names")));
        }
        for raw in hosts.get_ref() {
            let name = HostName::parse(raw.get_ref()).map_err(|e| at(raw.span(), format!("trusted_hosts: {e}")))?;
            if c.trusted_hosts.contains(&name) {
                return Err(at(raw.span(), format!("trusted_hosts: {name} twice")));
            }
            c.trusted_hosts.push(name);
        }
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> String {
        match parse(Path::new("conf/master.toml"), text) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("accepted: {text}"),
        }
    }

    #[test]
    fn values_defaults_and_paths() {
        let c = parse(
            Path::new("conf/master.toml"),
            "[master]\nbind = \"127.0.0.1\"\nport = 28000\nmax_servers = 200\nmax_servers_per_address = 128\n\
             allow_private_addresses = true\n[tls]\ncertificate = \"tls/chain.pem\"\nkey = \"tls/key.pem\"\n\
             [blocklists]\nserver_keys = \"blocked.txt\"\n",
        )
        .expect("valid");
        assert_eq!(
            (c.addr().to_string(), c.max_servers, c.max_per_address, c.allow_private),
            ("127.0.0.1:28000".into(), 200, 128, true)
        );
        assert_eq!(c.tls.as_ref().map(|t| t.key.clone()), Some(PathBuf::from("conf/tls/key.pem")));
        assert_eq!((c.blocked_servers, c.blocked_resources), (Some(PathBuf::from("conf/blocked.txt")), None));
        let d = parse(Path::new("m.toml"), "").expect("empty");
        assert_eq!(Config { path: None, ..d }, Config::defaults());
        assert!(!Config::defaults().allow_private, "a public master by default");
    }

    #[test]
    fn unknown_keys_and_ranges_are_refused_with_the_line() {
        assert!(err("[master]\nport = 0\n").starts_with("conf/master.toml:2:8: port = 0"));
        assert!(err("[master]\nport = 65536\n").contains("1 to 65535"));
        assert!(err("[master]\nmax_servers = 10001\n").contains("1 to 10000"));
        assert!(err("[master]\nmax_servers_per_address = 0\n").contains("max_servers_per_address"));
        assert!(err("[master]\nbind = \"master.example.org\"\n").contains("IPv4 or IPv6"));
        assert!(err("[master]\nbind = \"ff02::1\"\n").contains("unicast"));
        assert!(err("[master]\nallow_private_addresses = \"yes\"\n").starts_with("conf/master.toml:2:"));
        assert!(err("[master]\nexpire_after = 10\n").starts_with("conf/master.toml:2:1"));
        assert!(err("[tls]\ncertificate = \"c.pem\"\n").contains("key"));
        assert!(err("[tls]\ncertificate = \"\"\nkey = \"k\"\n").contains("path"));
        assert!(err("[blocklists]\nkeys = \"k\"\n").starts_with("conf/master.toml:2:1"));
        assert!(err("[server]\nname = \"x\"\n").starts_with("conf/master.toml:1:"));
    }

    #[test]
    fn the_trusted_proxies() {
        let c = parse(Path::new("m.toml"), "[proxy]\ntrusted = [\"10.0.1.0/24\", \"fd00:1::/64\", \"172.18.0.2\"]\n")
            .expect("valid");
        let shown: Vec<String> = c.trusted.networks().iter().map(ToString::to_string).collect();
        assert_eq!(shown, ["10.0.1.0/24", "fd00:1::/64", "172.18.0.2/32"]);
        for empty in ["", "[proxy]\n", "[proxy]\ntrusted = []\n"] {
            assert!(parse(Path::new("m.toml"), empty).expect("valid").trusted.is_empty(), "{empty:?}: no proxy");
        }
        assert!(err("[proxy]\ntrusted = [\"10.0.1.5/24\"]\n").starts_with("conf/master.toml:2:12: trusted:"));
        assert!(err("[proxy]\ntrusted = [\"10.0.0.0/8\", \"10.0.0.0/8\"]\n").contains("twice"));
        assert!(err("[proxy]\ntrusted = [\"0.0.0.0/0\"]\n").contains("every address"));
        assert!(err("[proxy]\ntrusted = \"10.0.0.0/8\"\n").starts_with("conf/master.toml:2:"));
        assert!(err("[proxy]\ntrusted = [1]\n").starts_with("conf/master.toml:2:"));
        assert!(err("[proxy]\ntrusted_proxies = []\n").starts_with("conf/master.toml:2:1"));
        let many: Vec<String> = (0..=MAX_TRUSTED).map(|i| format!("\"10.0.{i}.0/24\"")).collect();
        assert!(err(&format!("[proxy]\ntrusted = [{}]\n", many.join(", "))).contains("at most 64"));
    }

    #[test]
    fn the_trusted_names() {
        let c =
            parse(Path::new("m.toml"), "[proxy]\ntrusted_hosts = [\"coolify-proxy\", \"Traefik\"]\n").expect("valid");
        let names: Vec<&str> = c.trusted_hosts.iter().map(HostName::as_str).collect();
        assert_eq!(names, ["coolify-proxy", "traefik"]);
        assert!(c.trusted.is_empty() && c.behind_proxy(), "names alone put the master behind a proxy");
        let both = parse(Path::new("m.toml"), "[proxy]\ntrusted = [\"10.0.9.0/24\"]\ntrusted_hosts = [\"p\"]\n");
        assert!(both.expect("valid").behind_proxy());
        for none in ["", "[proxy]\n", "[proxy]\ntrusted_hosts = []\n"] {
            assert!(!parse(Path::new("m.toml"), none).expect("valid").behind_proxy(), "{none:?}");
        }
        assert!(err("[proxy]\ntrusted_hosts = [\"10.0.0.1\"]\n").starts_with("conf/master.toml:2:18: trusted_hosts:"));
        assert!(err("[proxy]\ntrusted_hosts = [\"coolify-proxy:8080\"]\n").contains("letters, digits"));
        assert!(err("[proxy]\ntrusted_hosts = [\"p\", \"P\"]\n").contains("p twice"));
        assert!(err("[proxy]\ntrusted_hosts = \"coolify-proxy\"\n").starts_with("conf/master.toml:2:"));
        assert!(err("[proxy]\ntrusted_hosts = [7]\n").starts_with("conf/master.toml:2:"));
        assert!(err("[proxy]\ntrusted_names = [\"p\"]\n").starts_with("conf/master.toml:2:1"));
        let many: Vec<String> = (0..=MAX_TRUSTED_HOSTS).map(|i| format!("\"proxy{i}.lan\"")).collect();
        assert!(err(&format!("[proxy]\ntrusted_hosts = [{}]\n", many.join(", "))).contains("at most 8 names"));
    }
}
