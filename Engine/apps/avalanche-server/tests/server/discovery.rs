use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use avalanche_master::{MasterOptions, MasterThread};
use avalanche_server::config::Config;
use avalanche_server::discovery::Extras;
use avalanche_server::discovery::query::TRACKED;
use avalanche_server::{ServerOptions, ServerThread};
use ssx_net_join::fetch_info;
use ssx_net_proto::query::{
    CHALLENGE_SIZE, Challenge, LAN_PORT, LanAnswer, LanProbe, QUERY_ANSWER_SIZE, QueryAnswer, QueryRequest, info_hash,
};

use crate::common::temp_dir;

static NEXT: AtomicU32 = AtomicU32::new(0);

fn next_ip() -> IpAddr {
    let n = NEXT.fetch_add(1, Ordering::Relaxed) + 1;
    IpAddr::V4(Ipv4Addr::new(127, 0, 40 + (n / 250) as u8, (n % 250) as u8 + 1))
}

fn server_on(ip: IpAddr, edit: impl FnOnce(&mut Config), extras: Extras) -> ServerThread {
    let mut config = crate::common::defaults();
    config.bind = ip;
    config.port = 0;
    edit(&mut config);
    let options = ServerOptions {
        config,
        data_dir: temp_dir("discovery"),
        console: false,
        logging: false,
        log_to_file: false,
        signals: false,
        conditions: None,
        relay_rates: None,
        extend: Some(Box::new(move |app| {
            app.insert_resource(extras);
        })),
        rules: avalanche_server::rules::RulesSource::Unchecked,
    };
    avalanche_server::spawn(options).expect("the server starts")
}

fn udp(from: IpAddr) -> UdpSocket {
    let s = UdpSocket::bind(SocketAddr::new(from, 0)).expect("udp");
    s.set_read_timeout(Some(Duration::from_millis(400))).expect("timeout");
    s
}

fn ask(s: &UdpSocket, to: SocketAddr, bytes: &[u8]) -> Option<Vec<u8>> {
    s.send_to(bytes, to).expect("send");
    let mut buf = [0u8; 2048];
    match s.recv_from(&mut buf) {
        Ok((n, from)) if from == to => Some(buf[..n].to_vec()),
        _ => None,
    }
}

#[test]
fn the_query_answers_its_asker_never_larger_and_limited() {
    let ip = next_ip();
    let server = server_on(ip, |c| c.discovery.query = true, Extras::default());
    let game = server.started.addr;
    let query = SocketAddr::new(ip, game.port() + 1);
    let me = udp(next_ip());
    let request = QueryRequest::Challenge.encode();
    let c = ask(&me, query, &request).expect("a challenge");
    assert!(c.len() == CHALLENGE_SIZE && c.len() <= request.len());
    let token = Challenge::decode(&c).expect("reads").token;
    let q = QueryRequest::Query { token, sequence: 77 }.encode();
    let a = ask(&me, query, &q).expect("the state");
    assert!(a.len() == QUERY_ANSWER_SIZE && a.len() <= q.len());
    let a = QueryAnswer::decode(&a).expect("reads");
    let info = fetch_info(game).expect("/info");
    assert_eq!((a.sequence, u32::from(a.max_players), a.mode, a.password), (77, info.max_players, 1, false));
    assert_eq!(a.info_hash, info_hash(&info), "the query and /info describe one server");
    for _ in 0..8 {
        me.send_to(&q, query).expect("send");
    }
    let mut buf = [0u8; 256];
    let mut answered = 0;
    while me.recv_from(&mut buf).is_ok() {
        answered += 1;
    }
    assert_eq!(answered, 3);
    let other = udp(next_ip());
    assert_eq!(ask(&other, query, &q), None, "another address's token");
    assert_eq!(ask(&other, query, &[0u8; 63]), None);
    assert_eq!(ask(&other, query, b"\xFF\xFF\xFF\xFFTSource Engine Query\0"), None);
    server.stop();
    server.join().expect("stops");
}

#[test]
fn forged_tokens_from_many_addresses_do_not_shut_an_honest_asker_out() {
    let ip = next_ip();
    let server = server_on(ip, |c| c.discovery.query = true, Extras::default());
    let query = SocketAddr::new(ip, server.started.addr.port() + 1);
    let forged = QueryRequest::Query { token: [0x5A; 16], sequence: 1 }.encode();
    for n in 0..TRACKED + 200 {
        let from = Ipv4Addr::new(127, 120 + (n / 62_500) as u8, (n / 250 % 250) as u8, (n % 250) as u8 + 1);
        let s = UdpSocket::bind(SocketAddr::new(IpAddr::V4(from), 0)).expect("a source");
        s.send_to(&forged, query).expect("send");
    }
    let me = udp(next_ip());
    let challenge = (0..3).find_map(|_| ask(&me, query, &QueryRequest::Challenge.encode()));
    let token = Challenge::decode(&challenge.expect("a challenge beside the forged tokens")).expect("reads").token;
    let answer = (0..2).find_map(|_| ask(&me, query, &QueryRequest::Query { token, sequence: 9 }.encode()));
    assert_eq!(QueryAnswer::decode(&answer.expect("the state")).expect("reads").sequence, 9);
    let status = server.started.discovery.lines().join("\n");
    assert!(!status.contains("bad token 0)"), "{status}");
    server.stop();
    server.join().expect("stops");
}

#[test]
fn lan_probes_are_answered_with_the_game_port() {
    let ip = next_ip();
    let server = server_on(ip, |c| c.discovery.lan = true, Extras::default());
    let lan = SocketAddr::new(ip, LAN_PORT);
    let me = udp(next_ip());
    let probe = LanProbe { protocol: 99 }.encode();
    let a = ask(&me, lan, &probe).expect("an answer");
    assert!(a.len() <= probe.len());
    let a = LanAnswer::decode(&a).expect("reads");
    assert_eq!((a.protocol, a.port), (ssx_net_proto::PROTOCOL_VERSION, server.started.addr.port()));
    assert_eq!(ask(&me, lan, &probe[..63]), None, "a probe of another size");
    let mut bad = probe.clone();
    bad[60] = 1;
    assert_eq!(ask(&me, lan, &bad), None, "padding that is not zero");
    server.stop();
    server.join().expect("stops");
}

fn master(tls: Option<avalanche_master::config::Tls>) -> MasterThread {
    let mut config = avalanche_master::config::Config::defaults();
    config.bind = Ipv4Addr::LOCALHOST.into();
    config.port = 0;
    config.allow_private = true;
    config.tls = tls;
    avalanche_master::spawn(MasterOptions {
        config,
        data_dir: std::env::temp_dir(),
        console: false,
        logging: false,
        log_to_file: false,
        signals: false,
    })
    .expect("the master starts")
}

fn listed(m: &MasterThread) -> Vec<(String, u16, String, String)> {
    let r = m.started.master.registry.lock().expect("the registry");
    r.iter()
        .map(|l| (l.ip.to_string(), l.beat.port, l.beat.region.clone(), ssx_net_proto::keys::to_hex(&l.key)))
        .collect()
}

fn test_tls(dir: &std::path::Path) -> (avalanche_master::config::Tls, rustls::pki_types::CertificateDer<'static>) {
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
    let mut ca = CertificateParams::new(Vec::<String>::new()).expect("params");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(ca, KeyPair::generate().expect("a key")).expect("the CA");
    let key = KeyPair::generate().expect("a key");
    let leaf =
        CertificateParams::new(vec!["127.0.0.1".to_string()]).expect("params").signed_by(&key, &ca).expect("signed");
    let (cert, key_path) = (dir.join("chain.pem"), dir.join("key.pem"));
    std::fs::write(&cert, leaf.pem()).expect("write");
    std::fs::write(&key_path, key.serialize_pem()).expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).expect("mode");
    }
    (avalanche_master::config::Tls { certificate: cert, key: key_path }, ca.der().clone())
}

fn listed_within(m: &MasterThread, key: &str) -> bool {
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        if listed(m).iter().any(|s| s.3 == key) {
            return true;
        }
        if Instant::now() > end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn a_public_server_is_listed_on_its_master_over_http_and_https_and_leaves_it() {
    let dir = temp_dir("tls");
    let (tls, ca) = test_tls(&dir);
    let plain = master(None);
    let secure = master(Some(tls));
    for (m, url) in [
        (&plain, format!("http://{}", plain.started.addr)),
        (&secure, format!("https://127.0.0.1:{}", secure.started.addr.port())),
    ] {
        let ip = next_ip();
        let extras = Extras { roots: vec![ca.clone()], ..Extras::default() };
        let server = server_on(
            ip,
            |c| {
                c.discovery = crate::common::listed_on(&url);
                c.discovery.region = "eu-central".into();
            },
            extras,
        );
        let key = server.started.identity_key.to_hex();
        assert!(listed_within(m, &key), "listed on {url} within 15 s");
        assert_eq!(listed(m), [(ip.to_string(), server.started.addr.port(), "eu-central".to_string(), key.clone())]);
        let said = server.started.discovery.master.lock().map(|m| m.clone()).unwrap_or_default();
        assert_eq!(said, Some((url.clone(), "listed".to_string())), "the status names the master");
        server.stop();
        server.join().expect("stops");
        std::thread::sleep(Duration::from_millis(1200));
        assert!(listed(m).is_empty(), "the stop's leave from {url}");
    }
    for m in [plain, secure] {
        m.stop();
        m.join().expect("stops");
    }
}

const CHILD: &str = "SSX_SERVER_DEFAULT_DISCOVERY_CHILD";

#[test]
fn child_runs_a_server_with_the_default_discovery() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let mut config = Config::defaults();
    config.bind = Ipv4Addr::new(127, 0, 39, 1).into();
    config.port = 0;
    let options = ServerOptions {
        config,
        data_dir: temp_dir("default-discovery"),
        console: false,
        logging: false,
        log_to_file: false,
        signals: false,
        conditions: None,
        relay_rates: None,
        extend: None,
        rules: avalanche_server::rules::RulesSource::Unchecked,
    };
    let server = avalanche_server::spawn(options).expect("the server starts");
    println!("KEY={}", server.started.identity_key.to_hex());
    let mut rest = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut rest);
    server.stop();
    server.join().expect("stops");
}

#[test]
fn a_default_server_heartbeats_to_the_master_the_build_names() {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    let m = master(None);
    let url = format!("http://{}", m.started.addr);
    let mut child = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["discovery::child_runs_a_server_with_the_default_discovery", "--exact", "--nocapture"])
        .args(["--test-threads=1"])
        .env(CHILD, "1")
        .env("SSX_DEV_MASTER", &url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the child starts");
    let stdout = child.stdout.take().expect("its output");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some((_, key)) = line.split_once("KEY=") {
                let _ = tx.send(key.to_string());
            }
        }
    });
    let key = rx.recv_timeout(Duration::from_secs(30));
    let on = key.as_ref().is_ok_and(|k| listed_within(&m, k));
    drop(child.stdin.take());
    let end = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break Some(status);
        }
        if Instant::now() > end {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let key = key.expect("the child's server started");
    assert!(on, "a default server listed on SSX_DEV_MASTER's {url} within 15 s");
    assert!(status.is_some_and(|s| s.success()), "the child stops: {status:?}");
    assert!(!listed(&m).iter().any(|s| s.3 == key), "the stop's leave");
    m.stop();
    m.join().expect("stops");
}

#[test]
fn a_busy_query_port_fails_the_start_and_a_refusing_master_is_told_in_the_status() {
    let ip = next_ip();
    let port =
        std::net::TcpListener::bind(SocketAddr::new(ip, 0)).expect("a free port").local_addr().expect("addr").port();
    let _held = UdpSocket::bind(SocketAddr::new(ip, port + 1)).expect("the query port held");
    let mut config = crate::common::defaults();
    config.bind = ip;
    config.port = port;
    config.discovery.query = true;
    let options = ServerOptions {
        config,
        data_dir: temp_dir("discovery"),
        console: false,
        logging: false,
        log_to_file: false,
        signals: false,
        conditions: None,
        relay_rates: None,
        extend: None,
        rules: avalanche_server::rules::RulesSource::Unchecked,
    };
    let e = avalanche_server::spawn(options).err().expect("a busy query port fails the start");
    assert!(e.to_string().contains(&format!("{}", port + 1)), "{e}");

    let mut mc = avalanche_master::config::Config::defaults();
    mc.bind = Ipv4Addr::LOCALHOST.into();
    mc.port = 0;
    let m = avalanche_master::spawn(MasterOptions {
        config: mc,
        data_dir: std::env::temp_dir(),
        console: false,
        logging: false,
        log_to_file: false,
        signals: false,
    })
    .expect("the master starts");
    let url = format!("http://{}", m.started.addr);
    let server = server_on(next_ip(), |c| c.discovery = crate::common::listed_on(&url), Extras::default());
    let end = Instant::now() + Duration::from_secs(10);
    let said = loop {
        let last = server.started.discovery.master.lock().map(|m| m.clone()).unwrap_or_default();
        if let Some((_, text)) = last {
            break text;
        }
        assert!(Instant::now() < end, "no word from the master within 10 s");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(said.contains("403") && said.contains("public addresses only") && said.contains("loopback"), "{said}");
    server.stop();
    server.join().expect("stops");
    m.stop();
    m.join().expect("stops");
}

#[test]
fn a_listed_join_uses_the_listed_key_and_leaves_the_trusted_keys_alone() {
    use ssx_net_join::{JoinFailure, JoinParams, PlayerIdentity, Trust, join_listed};
    let server = server_on(next_ip(), |_| {}, Extras::default());
    let addr = server.started.addr;
    let home = temp_dir("listed");
    let identity = PlayerIdentity::load_or_create(&home).expect("identity");
    let content = super::common::player_content(addr);
    let params = JoinParams { name: "Listed Rider", password: b"", cancel: None, content };
    let granted = join_listed(addr, &identity, &server.started.identity_key, &params).expect("granted");
    assert_eq!(granted.trust, Trust::Listed);
    assert!(!home.join(ssx_net_join::KNOWN_SERVERS_FILE).exists(), "nothing pinned");
    let before = server.started.shared.http_requests.load(Ordering::Relaxed);
    let other = ssx_net_proto::keys::ServerSecret::generate().public();
    match join_listed(addr, &identity, &other, &params) {
        Err(JoinFailure::NotListedKey { listed, offered, .. }) => {
            assert_eq!((listed, offered), (other.to_hex(), server.started.identity_key.to_hex()));
        }
        other => panic!("{:?}", other.map(|g| g.trust)),
    }
    let asked = server.started.shared.http_requests.load(Ordering::Relaxed) - before;
    assert_eq!(asked, 1, "/info only: no challenge, no join");
    server.stop();
    server.join().expect("stops");
}
