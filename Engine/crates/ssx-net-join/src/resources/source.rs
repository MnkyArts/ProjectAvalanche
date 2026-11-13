use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use ssx_content::ContentHash;
use ssx_content::delivery::Mirror;
use ssx_content::delivery::mirror::Host;
use ssx_net_proto::BUILD_ID;
use ureq::Agent;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

// PORT(choice): 5 s to connect, 2 s for a mirror's name (the join's and the masters' bounds); tuning: downloads of
// players on public servers; the original has no dedicated server.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HEADERS: usize = 8 * 1024;

#[derive(Clone, Debug)]
pub enum Source {
    Mirror(usize, Mirror),
    Server(SocketAddr),
}

impl Source {
    pub fn url(&self, hash: &ContentHash) -> String {
        match self {
            Source::Mirror(_, m) => m.file_url(hash),
            Source::Server(s) => format!("http://{s}/files/{hash}"),
        }
    }

    pub fn report_name(&self) -> String {
        match self {
            Source::Mirror(n, _) => format!("mirror {n}"),
            Source::Server(_) => "server".into(),
        }
    }

    pub fn shown(&self) -> String {
        match self {
            Source::Mirror(_, m) => format!("the mirror {m}"),
            Source::Server(s) => format!("the server {s}"),
        }
    }
}

pub fn local_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || (o[0] == 100 && (o[1] & 0xC0) == 64)
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => local_address(IpAddr::V4(v4)),
            None => {
                let s = v6.segments();
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (s[0] & 0xFE00) == 0xFC00
                    || (s[0] & 0xFFC0) == 0xFE80
            }
        },
    }
}

#[derive(Debug)]
struct MirrorResolver {
    mirror: Mirror,
    local_ok: bool,
    cancel: Option<Arc<AtomicBool>>,
}

impl Resolver for MirrorResolver {
    fn resolve(&self, uri: &Uri, _: &Config, _: NextTimeout) -> Result<ResolvedSocketAddrs, ureq::Error> {
        if uri.authority().map(|a| a.as_str()) != Some(self.mirror.authority().as_str()) {
            return Err(ureq::Error::HostNotFound);
        }
        let found = match &self.mirror.host {
            Host::Ip(ip) => SocketAddr::new(*ip, self.mirror.port),
            Host::Name(name) => crate::browse::resolve::resolve(
                name,
                self.mirror.port,
                RESOLVE_TIMEOUT,
                self.cancel.as_deref(),
                crate::browse::resolve::system,
            )
            .map_err(|e| ureq::Error::Io(std::io::Error::other(e)))?,
        };
        if local_address(found.ip()) && !self.local_ok {
            let why = format!("a public server's mirror at {} (a local address) is not used", found.ip());
            return Err(ureq::Error::Io(std::io::Error::other(why)));
        }
        let mut out = self.empty();
        out.push(found);
        Ok(out)
    }
}

fn config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .max_response_header_size(MAX_HEADERS)
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .user_agent(BUILD_ID)
}

pub fn agent(source: &Source, server: SocketAddr, cancel: Option<Arc<AtomicBool>>) -> Agent {
    match source {
        Source::Server(_) => config().build().into(),
        Source::Mirror(_, m) => {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let tls =
                TlsConfig::builder().root_certs(RootCerts::Specific(crate::browse::masters::system_roots())).build();
            let resolver = MirrorResolver { mirror: m.clone(), local_ok: local_address(server.ip()), cancel };
            Agent::with_parts(config().tls_config(tls).build(), DefaultConnector::new(), resolver)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addresses() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.1.1", "100.64.0.1", "0.0.0.0", "::1"]
        {
            assert!(local_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["fd00::1", "fe80::1", "::ffff:192.168.0.1", "255.255.255.255", "224.0.0.1"] {
            assert!(local_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "100.128.0.1", "172.32.0.1", "2001:4860::8888", "::ffff:8.8.8.8"] {
            assert!(!local_address(ip.parse().unwrap()), "{ip}");
        }
    }
}
