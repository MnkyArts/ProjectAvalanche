use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol as SockProtocol, Socket, Type};

use crate::exchange::{NATPMP_SCHEDULE, PCP_SCHEDULE, exchange, local_toward, socket};
use crate::{Config, Error, ErrorKind, Method, Port, SsdpTarget, Targets, http, igd, natpmp, pcp, ssdp};

pub enum Session {
    Upnp { service: igd::Service, client: Ipv4Addr, lease: u32 },
    Pcp { client: Ipv4Addr, nonces: Vec<(Port, [u8; 12])> },
    NatPmp,
}

pub struct Made {
    pub session: Session,
    pub external: Option<IpAddr>,
    pub lifetime: u32,
}

impl Session {
    pub fn method(&self) -> Method {
        match self {
            Session::Upnp { .. } => Method::Upnp,
            Session::Pcp { .. } => Method::Pcp,
            Session::NatPmp => Method::NatPmp,
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Session::Upnp { service, .. } => {
                let kind = service.service_type.rsplit(':').nth(1).unwrap_or_default();
                format!(" ({kind}, control {})", service.control.addr)
            }
            Session::Pcp { .. } | Session::NatPmp => String::new(),
        }
    }
}

// PORT(choice): a permanent mapping added again every 30 minutes, a granted lifetime renewed at half and at least a
// second apart; tuning: real gateways that forget mappings (a restart) and grant short ones; the original has no
// dedicated server.
pub fn renew_after(lifetime: u32) -> Duration {
    if lifetime == 0 {
        return Duration::from_secs(1800);
    }
    Duration::from_secs(u64::from(lifetime / 2).max(1))
}

pub fn start(method: Method, gateway: Ipv4Addr, cfg: &Config, targets: &Targets) -> Result<Made, Error> {
    match method {
        Method::NatPmp => natpmp_map(gateway, cfg, targets),
        Method::Pcp => {
            let client = local_toward(cfg.local, gateway, targets.pmp_port)?;
            let mut nonces = Vec::new();
            for p in &cfg.ports {
                let mut n = [0u8; 12];
                getrandom::fill(&mut n).map_err(|e| Error::refused(format!("PCP: the system's random source: {e}")))?;
                nonces.push((*p, n));
            }
            pcp_map(gateway, cfg, targets, client, nonces)
        }
        Method::Upnp => upnp_start(gateway, cfg, targets),
    }
}

pub fn renew(session: Session, gateway: Ipv4Addr, cfg: &Config, targets: &Targets) -> Result<Made, Error> {
    match session {
        Session::NatPmp => natpmp_map(gateway, cfg, targets),
        Session::Pcp { client, nonces } => pcp_map(gateway, cfg, targets, client, nonces),
        Session::Upnp { service, client, lease } => upnp_add_all(gateway, cfg, targets, service, client, lease),
    }
}

pub fn remove(
    session: &Session,
    gateway: Ipv4Addr,
    cfg: &Config,
    targets: &Targets,
    ports: &[Port],
    until: Instant,
) -> Vec<String> {
    let mut failed = Vec::new();
    let quick = [Duration::from_millis(250), Duration::from_millis(500)];
    for p in ports {
        if Instant::now() >= until {
            failed.push(format!("{p}: no time left"));
            continue;
        }
        let r = match session {
            Session::NatPmp => natpmp_one(gateway, cfg, targets, *p, 0, &quick).map(|_| ()),
            Session::Pcp { client, nonces } => match nonces.iter().find(|(q, _)| q == p) {
                Some((_, n)) => {
                    let req = PcpRequest { client: *client, nonce: *n, port: *p, lifetime: 0 };
                    pcp_one(gateway, cfg, targets, &req, &quick).map(|_| ())
                }
                None => Ok(()),
            },
            Session::Upnp { service, .. } => {
                let body = igd::delete_port_mapping(&service.service_type, p.protocol, p.port);
                upnp_soap(cfg.local, gateway, service, "DeletePortMapping", body)
                    .map(|_| ())
                    .map_err(SoapError::into_error)
            }
        };
        if let Err(e) = r {
            failed.push(format!("{p}: {e}"));
        }
    }
    failed
}

fn natpmp_one(
    gateway: Ipv4Addr,
    cfg: &Config,
    targets: &Targets,
    p: Port,
    lifetime: u32,
    schedule: &[Duration],
) -> Result<(u16, u32), Error> {
    let s = socket(cfg.local, gateway, targets.pmp_port)?;
    let external = if lifetime == 0 { 0 } else { p.port };
    let req = natpmp::map_request(p.protocol, p.port, external, lifetime);
    exchange(&s, &req, schedule, &format!("NAT-PMP {p} at {gateway}"), |d| {
        match natpmp::map_answer(d, p.protocol, p.port)? {
            natpmp::Answer::Mapped { external, lifetime, .. } => Ok(Some((external, lifetime))),
            natpmp::Answer::Refused { code, .. } => {
                Err(Error::refused(format!("the gateway refused {p}: {} ({code})", natpmp::result_text(code))))
            }
            natpmp::Answer::Address { .. } => Ok(None),
        }
    })
}

fn natpmp_map(gateway: Ipv4Addr, cfg: &Config, targets: &Targets) -> Result<Made, Error> {
    let s = socket(cfg.local, gateway, targets.pmp_port)?;
    let what = format!("NAT-PMP at {gateway}");
    let external =
        exchange(&s, &natpmp::address_request(), &NATPMP_SCHEDULE, &what, |d| match natpmp::address_answer(d)? {
            natpmp::Answer::Address { external, .. } => Ok(Some(external)),
            natpmp::Answer::Refused { code, .. } => Err(Error::refused(format!(
                "the gateway refused the address request: {} ({code})",
                natpmp::result_text(code)
            ))),
            natpmp::Answer::Mapped { .. } => Ok(None),
        })?;
    let mut granted = u32::MAX;
    let mut made: Vec<Port> = Vec::new();
    for p in &cfg.ports {
        let r = natpmp_one(gateway, cfg, targets, *p, cfg.lifetime, &NATPMP_SCHEDULE);
        let r = match r {
            Ok((ext, life)) => {
                made.push(*p);
                if ext == p.port { Ok(life) } else { Err(same_port(*p, ext)) }
            }
            Err(e) => Err(e),
        };
        match r {
            Ok(life) => granted = granted.min(life),
            Err(e) => {
                let _ = remove(&Session::NatPmp, gateway, cfg, targets, &made, Instant::now() + Duration::from_secs(2));
                return Err(e);
            }
        }
    }
    Ok(Made { session: Session::NatPmp, external: Some(IpAddr::V4(external)), lifetime: granted })
}

fn same_port(p: Port, ext: u16) -> Error {
    Error::refused(format!("the gateway gave {} port {ext} for {p}: the port must be the same outside", p.protocol))
}

struct PcpRequest {
    client: Ipv4Addr,
    nonce: [u8; 12],
    port: Port,
    lifetime: u32,
}

fn pcp_one(
    gateway: Ipv4Addr,
    cfg: &Config,
    targets: &Targets,
    r: &PcpRequest,
    schedule: &[Duration],
) -> Result<(u16, u32, IpAddr), Error> {
    let s = socket(cfg.local, gateway, targets.pmp_port)?;
    let p = r.port;
    let external = if r.lifetime == 0 { 0 } else { p.port };
    let req = pcp::map_request(r.client, r.nonce, p.protocol, p.port, external, r.lifetime);
    exchange(&s, &req, schedule, &format!("PCP {p} at {gateway}"), |d| {
        match pcp::map_answer(d, r.nonce, p.protocol, p.port)? {
            pcp::Answer::Mapped { lifetime, external_port, external, .. } => {
                let ip = external.to_ipv4_mapped().map_or(IpAddr::V6(external), IpAddr::V4);
                Ok(Some((external_port, lifetime, ip)))
            }
            pcp::Answer::Refused { code, .. } => {
                Err(Error::refused(format!("the gateway refused {p}: {} ({code})", pcp::result_text(code))))
            }
            pcp::Answer::NatPmpOnly => Err(Error::refused("the gateway speaks NAT-PMP, not PCP")),
        }
    })
}

fn pcp_map(
    gateway: Ipv4Addr,
    cfg: &Config,
    targets: &Targets,
    client: Ipv4Addr,
    nonces: Vec<(Port, [u8; 12])>,
) -> Result<Made, Error> {
    let mut granted = u32::MAX;
    let mut external = None;
    let mut made: Vec<Port> = Vec::new();
    for (p, n) in &nonces {
        let req = PcpRequest { client, nonce: *n, port: *p, lifetime: cfg.lifetime };
        let r = match pcp_one(gateway, cfg, targets, &req, &PCP_SCHEDULE) {
            Ok((ext, life, ip)) => {
                made.push(*p);
                if ext == p.port { Ok((life, ip)) } else { Err(same_port(*p, ext)) }
            }
            Err(e) => Err(e),
        };
        match r {
            Ok((life, ip)) => {
                granted = granted.min(life);
                external = Some(ip);
            }
            Err(e) => {
                let s = Session::Pcp { client, nonces: nonces.clone() };
                let _ = remove(&s, gateway, cfg, targets, &made, Instant::now() + Duration::from_secs(2));
                return Err(e);
            }
        }
    }
    Ok(Made { session: Session::Pcp { client, nonces }, external, lifetime: granted })
}

const SSDP_WAIT: Duration = Duration::from_millis(ssdp::MX as u64 * 1000 + 500);

fn ssdp_search(gateway: Ipv4Addr, cfg: &Config, targets: &Targets) -> Result<ssdp::Location, Error> {
    let s =
        Socket::new(Domain::IPV4, Type::DGRAM, Some(SockProtocol::UDP)).map_err(|e| Error::io("a UDP socket", e))?;
    let local = cfg.local.unwrap_or(Ipv4Addr::UNSPECIFIED);
    s.bind(&std::net::SocketAddr::new(local.into(), 0).into()).map_err(|e| Error::io("binding for SSDP", e))?;
    let target = match &targets.ssdp {
        SsdpTarget::Multicast => {
            let iface = local_toward(cfg.local, gateway, 1900)?;
            s.set_multicast_if_v4(&iface).map_err(|e| Error::io("SSDP's interface", e))?;
            s.set_multicast_ttl_v4(2).map_err(|e| Error::io("SSDP's TTL", e))?;
            ssdp::MULTICAST
        }
        SsdpTarget::Unicast(a) => *a,
    };
    let socket: UdpSocket = s.into();
    let search = ssdp::search(target);
    let until = Instant::now() + SSDP_WAIT;
    let mut buf = [0u8; ssdp::MAX_DATAGRAM + 1];
    let mut ignored: Option<String> = None;
    for i in 0..2 {
        if i == 1 {
            std::thread::sleep(Duration::from_millis(100));
        }
        socket.send_to(search.as_bytes(), target).map_err(|e| Error::io("sending the SSDP search", e))?;
    }
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        socket.set_read_timeout(Some(left)).map_err(|e| Error::io("SSDP", e))?;
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                if from.ip() != IpAddr::V4(gateway) {
                    ignored = Some(format!("an answer from {}, not the gateway", from.ip()));
                    continue;
                }
                match ssdp::answer(buf.get(..n).unwrap_or_default()) {
                    Ok(loc) if *loc.addr.ip() == gateway => return Ok(loc),
                    Ok(loc) => ignored = Some(format!("a location on {}, not the gateway", loc.addr.ip())),
                    Err(e) => ignored = Some(e.message),
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
            Err(e) => return Err(Error::io("SSDP", e)),
        }
    }
    let why = ignored.map_or("nothing came".to_string(), |w| format!("the last answer ignored: {w}"));
    Err(Error {
        kind: ErrorKind::Io,
        message: format!("SSDP: no gateway answered within {:.1} s ({why})", SSDP_WAIT.as_secs_f64()),
    })
}

enum SoapError {
    Fault { code: u32, description: String },
    Other(Error),
}

impl SoapError {
    fn into_error(self) -> Error {
        match self {
            SoapError::Fault { code, description } => {
                Error::refused(format!("{code} {description} ({})", igd::fault_text(code)))
            }
            SoapError::Other(e) => e,
        }
    }
}

fn upnp_soap(
    local: Option<Ipv4Addr>,
    gateway: Ipv4Addr,
    service: &igd::Service,
    action: &str,
    body: String,
) -> Result<Vec<(String, String)>, SoapError> {
    if *service.control.addr.ip() != gateway {
        return Err(SoapError::Other(Error::malformed("UPnP: the control URL is not on the gateway")));
    }
    let soap_action = igd::soap_action(&service.service_type, action);
    let headers = [("Content-Type", "text/xml; charset=\"utf-8\""), ("SOAPAction", soap_action.as_str())];
    let r = http::request(local, service.control.addr, "POST", &service.control.path, &headers, body.as_bytes())
        .map_err(SoapError::Other)?;
    match igd::answer(r.status, &r.body, action).map_err(SoapError::Other)? {
        igd::Answer::Done(args) => Ok(args),
        igd::Answer::Fault { code, description } => Err(SoapError::Fault { code, description }),
    }
}

fn upnp_start(gateway: Ipv4Addr, cfg: &Config, targets: &Targets) -> Result<Made, Error> {
    let location = ssdp_search(gateway, cfg, targets)?;
    let description = http::request(cfg.local, location.addr, "GET", &location.path, &[], &[])?;
    if description.status != 200 {
        return Err(Error::refused(format!("UPnP: the description answers {}", description.status)));
    }
    let service = igd::service(&description.body, &location, gateway)?;
    let client = local_toward(cfg.local, gateway, location.addr.port())?;
    upnp_add_all(gateway, cfg, targets, service, client, cfg.lifetime)
}

fn upnp_add_all(
    gateway: Ipv4Addr,
    cfg: &Config,
    targets: &Targets,
    service: igd::Service,
    client: Ipv4Addr,
    mut lease: u32,
) -> Result<Made, Error> {
    let mut made: Vec<Port> = Vec::new();
    for p in &cfg.ports {
        let add = |lease: u32| {
            let body =
                igd::add_port_mapping(&service.service_type, p.protocol, p.port, client, &cfg.description, lease);
            upnp_soap(cfg.local, gateway, &service, "AddPortMapping", body)
        };
        let r = match add(lease) {
            Err(SoapError::Fault { code: 725, .. }) if lease != 0 => {
                lease = 0;
                add(0)
            }
            r => r,
        };
        if let Err(e) = r {
            let session = Session::Upnp { service: service.clone(), client, lease };
            let _ = remove(&session, gateway, cfg, targets, &made, Instant::now() + Duration::from_secs(2));
            let e = e.into_error();
            return Err(Error { kind: e.kind, message: format!("UPnP AddPortMapping {p}: {}", e.message) });
        }
        made.push(*p);
    }
    let external = upnp_soap(
        cfg.local,
        gateway,
        &service,
        "GetExternalIPAddress",
        igd::get_external_address(&service.service_type),
    )
    .map_err(SoapError::into_error)
    .and_then(|a| igd::external_address(&a))
    .ok()
    .map(IpAddr::V4);
    Ok(Made { session: Session::Upnp { service, client, lease }, external, lifetime: lease })
}
