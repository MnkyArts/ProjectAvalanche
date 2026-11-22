use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Instant;

use bevy::app::ScheduleRunnerPlugin;
use bevy::ecs::schedule::SingleThreadedExecutor;
use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use ssx_net::harness::Conditions;
use ssx_net::lightyear::prelude::Linked;
use ssx_net::lightyear::prelude::endpoint::UdpEndpoint;
use ssx_net::lightyear::prelude::server::Start;
use ssx_net::server::{ChatConfig, spawn_udp_server};
use ssx_net::{NetServerPlugin, tick_duration};
use ssx_net_proto::keys::{ServerKey, ServerSecret};
use tokio::sync::Notify;

use crate::bridge::{Shared, join_queue};
use crate::config::Config;
use crate::http::{self, HttpState};
use crate::logging;
use crate::systems::{
    Bridge, ConsoleLines, Lifecycle, ServerFacts, TickTimes, console_commands, decide_joins, lifecycle, log_chat,
    log_disconnect, log_kick, log_leave, publish, report_suppressed, tick_ended, tick_started,
};
use crate::{bind, console, discovery, errors, keys, portmap};

const LOG_FILTER: &str = "info,lightyear_udp=warn,lightyear_netcode=warn,lightyear_transport=warn";

pub struct ServerOptions {
    pub config: Config,
    pub data_dir: PathBuf,
    pub console: bool,
    pub logging: bool,
    pub log_to_file: bool,
    pub signals: bool,
    pub conditions: Option<Conditions>,
    pub relay_rates: Option<ssx_net::server::RelayRates>,
    pub extend: Option<AppExtension>,
    pub rules: crate::rules::RulesSource,
}

pub type AppExtension = Box<dyn FnOnce(&mut App) + Send>;

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("{0}")]
    Keys(#[from] ssx_net_proto::keyfile::KeyFileError),
    #[error("the HTTP socket {addr} (TCP) does not open: {error}")]
    Http { addr: SocketAddr, error: std::io::Error },
    #[error("the game socket {addr} (UDP) does not open: {why}")]
    Udp { addr: SocketAddr, why: String },
    #[error("{0}")]
    Other(String),
}

#[derive(Clone)]
pub struct Started {
    pub addr: SocketAddr,
    pub identity_key: ServerKey,
    pub shared: Arc<Shared>,
    pub created_keys: Vec<PathBuf>,
    pub discovery: discovery::Status,
}

pub struct ServerThread {
    pub started: Started,
    handle: JoinHandle<Result<(), String>>,
}

impl ServerThread {
    pub fn stop(&self) {
        self.started.shared.stop_requested.store(true, Ordering::Release);
    }

    pub fn join(self) -> Result<(), String> {
        self.handle.join().map_err(|_| "the server thread panicked".to_string())?
    }
}

pub fn spawn(options: ServerOptions) -> Result<ServerThread, StartError> {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::Builder::new()
        .name("server".into())
        .spawn(move || {
            let result = run_with(options, |started| {
                let _ = tx.send(Ok(started.clone()));
            });
            if let Err(e) = &result {
                let _ = tx.send(Err(e.to_string()));
            }
            result.map_err(|e| e.to_string())
        })
        .map_err(|e| StartError::Other(e.to_string()))?;
    match rx.recv() {
        Ok(Ok(started)) => Ok(ServerThread { started, handle }),
        Ok(Err(e)) => Err(StartError::Other(e)),
        Err(_) => Err(StartError::Other("the server thread ended before it started".into())),
    }
}

pub fn run(options: ServerOptions) -> Result<(), StartError> {
    run_with(options, |started| {
        for path in &started.created_keys {
            info!("made {} (keep it; it is never replaced)", path.display());
        }
        info!("listening on {} (UDP and TCP); identity key {}", started.addr, started.identity_key.to_hex());
    })
}

fn run_with(options: ServerOptions, on_started: impl FnOnce(&Started)) -> Result<(), StartError> {
    let mut config = options.config;
    let rules_folder = matches!(options.rules, crate::rules::RulesSource::Config)
        .then(|| crate::rules::folder(&config, &options.data_dir));
    let settled = crate::resources::Settled::default();
    let (validation, rules, mut maps) = match options.rules {
        crate::rules::RulesSource::Config => {
            let (v, rules, maps) = crate::rules::prepare(&config, &options.data_dir, settled.clone());
            (Some(v), Some(rules), Some(maps))
        }
        crate::rules::RulesSource::Unchecked => (None, None, None),
        crate::rules::RulesSource::Custom(v) => (Some(v), None, None),
    };
    let blocklists = crate::blocklist::load(
        &crate::blocklist::Sources::of(&config, &options.data_dir),
        ssx_content::blocklist::now_secs(),
        None,
    )
    .map_err(StartError::Other)?;
    let scripts = crate::scripts::ServerScripts::spawn(&options.data_dir).map_err(StartError::Other)?;
    let resources = crate::resources::prepare(
        &config,
        &options.data_dir,
        rules_folder.as_deref(),
        std::sync::Arc::clone(&blocklists.blocked),
        settled,
        Some(scripts.handle.clone()),
    )
    .map_err(|e| StartError::Other(format!("resources: {e}")))?;
    let published = Arc::clone(&resources.published);
    if let Some(r) = &rules {
        r.check_drops(&config).map_err(StartError::Other)?;
    }
    match maps.as_mut() {
        Some(m) => m.check(&mut config).map_err(StartError::Other)?,
        None => {
            if let Some(level) = config.gamemode_level() {
                return Err(StartError::Other(format!(
                    "{level}: a gamemode's event type is its package's, which a server without its rules folder's \
                     check cannot read"
                )));
            }
        }
    }
    let keys = keys::load_or_create(&options.data_dir)?;
    let operation = Arc::new(crate::operation::Operation::load(&options.data_dir).map_err(StartError::Other)?);
    let icon = match crate::operation::config::icon_file(&config) {
        Some(path) => Some(crate::operation::icon::load(&path).map_err(StartError::Other)?),
        None => None,
    };
    let (listener, socket, picks) = bind::game_sockets(config.addr()).map_err(|e| match e {
        bind::PortError::Tcp(addr, error) => StartError::Http { addr, error },
        bind::PortError::Udp(addr, error) => StartError::Udp { addr, why: error.to_string() },
    })?;
    let local = listener.local_addr().map_err(|error| StartError::Http { addr: config.addr(), error })?;
    listener.set_nonblocking(true).map_err(|error| StartError::Http { addr: local, error })?;
    let shared = Arc::new(Shared::default());
    if let (Some(m), Ok(mut level)) = (maps.as_ref(), shared.level.lock()) {
        *level = Some(m.shown(&config.game().level));
    }
    let (joins_tx, joins_rx) = join_queue();

    let (_log_writers, log_file) = if options.logging {
        let path = options.log_to_file.then(|| options.data_dir.join(crate::logging::LOG_FOLDER).join("server.log"));
        let (writers, file) = logging::start(path.as_deref());
        (Some(writers), file)
    } else {
        (None, None)
    };
    let mut app = App::new();
    app.set_error_handler(errors::handler);
    if options.logging {
        if let Some(file) = log_file {
            app.insert_resource(file);
        }
        app.add_plugins(LogPlugin {
            filter: LOG_FILTER.into(),
            level: Level::INFO,
            custom_layer: logging::file_layer,
            fmt_layer: logging::stdout_layer,
        });
    }
    if let Some(r) = &rules {
        r.log_is_up();
    }
    if picks > 1 {
        info!("the game port {} after {picks} picks (UDP on the earlier picks was another process's)", local.port());
    }
    for notice in &config.notices {
        warn!("{notice}");
    }
    for line in config.discovery.lines() {
        info!("{}", line.trim_start());
    }
    app.add_plugins((MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(tick_duration())), StatesPlugin));
    app.add_plugins(NetServerPlugin);
    app.insert_resource(config.instance_config());
    app.insert_resource(ChatConfig { enabled: config.chat });
    app.insert_resource(ssx_net::server::OperatorGate(Arc::clone(&operation) as Arc<dyn ssx_net::server::Operator>));
    app.insert_resource(crate::operation::commands::OperationRes(Arc::clone(&operation)));
    app.insert_resource(crate::operation::stop::StopConfig { round_end_secs: config.operation.round_end_secs });
    if let Some(v) = validation {
        app.insert_resource(v);
    }
    if let Some(m) = maps {
        app.insert_resource(m);
    }
    if let Some(rates) = options.relay_rates {
        app.insert_resource(rates);
    }
    app.init_resource::<TickTimes>().init_resource::<Lifecycle>();
    app.insert_resource(crate::scripts::requests_at_start(&resources.running));
    app.insert_resource(resources).init_resource::<crate::resources::PermissionRequests>();
    app.add_systems(Update, crate::resources::run_queue.after(crate::systems::console_commands));
    app.add_systems(Update, crate::resources::publish_now.after(crate::resources::run_queue));
    app.add_systems(Update, crate::maps::follow_resources.after(crate::resources::run_queue));
    crate::scripts::register(&mut app, scripts);
    app.insert_resource(Bridge { shared: Arc::clone(&shared), joins: joins_rx });
    if options.console {
        let lines = console::spawn().map_err(|e| StartError::Other(format!("console: {e}")))?;
        app.insert_resource(ConsoleLines(std::sync::Mutex::new(lines)));
    }
    app.add_systems(First, tick_started).add_systems(Last, tick_ended);
    app.add_systems(Last, crate::stalls::report);
    app.add_observer(log_kick).add_observer(log_leave).add_observer(log_disconnect);
    app.add_systems(
        Update,
        (
            decide_joins,
            publish,
            console_commands,
            lifecycle,
            report_suppressed,
            log_chat,
            crate::rounds::log_rounds,
            crate::rules::log_validation,
        )
            .chain(),
    );
    app.add_systems(
        Update,
        (
            crate::operation::world::run_rcon,
            crate::operation::stop::end_rounds,
            crate::operation::world::publish,
            crate::operation::world::sweep,
        )
            .chain()
            .after(lifecycle),
    );
    if let Some(extend) = options.extend {
        extend(&mut app);
    }
    crate::blocklist::follow(&mut app, &config, &options.data_dir, blocklists, Arc::clone(&shared))
        .map_err(StartError::Other)?;
    app.finish();
    app.cleanup();
    one_thread(&mut app);

    let udp = SocketAddr::new(config.bind, local.port());
    let conditioner = options.conditions.map(|c| c.conditioner()).transpose().map_err(StartError::Other)?;
    if let (Some(c), Some(mut time)) =
        (options.conditions, app.world_mut().get_resource_mut::<ssx_net::server::ServerTime>())
    {
        time.set_receive_hold(c.latency + c.jitter);
    }
    let server = spawn_udp_server(app.world_mut(), udp, *keys.netcode, conditioner);
    match app.world_mut().get_mut::<UdpEndpoint>(server) {
        Some(mut endpoint) => endpoint.use_socket(socket),
        None => return Err(StartError::Other("the UDP server entity has no endpoint".into())),
    }
    app.insert_resource(ServerFacts {
        max_players: config.max_players,
        server_entity: server,
        started: Instant::now(),
    });
    app.world_mut().trigger(Start { entity: server });
    app.update();
    if app.world().get::<Linked>(server).is_none() {
        return Err(StartError::Udp { addr: udp, why: errors::last().join("; ") });
    }

    let identity_key = keys.identity_key();
    let sockets = discovery::bind(&config, local)?;
    let beat_identity =
        ServerSecret::from_bytes(keys.identity.to_bytes()).map_err(|e| StartError::Other(e.to_string()))?;
    let mut state = HttpState::new(&config, keys.identity, joins_tx, Arc::clone(&shared));
    state.resources = published;
    let state = Arc::new(state);
    {
        use crate::operation::http::OperationHttp;
        use crate::operation::world::{OperationHttpRes, RconQueueRes};
        let (ops, queue) = OperationHttp::new(Arc::clone(&operation), identity_key, config.operation.clone(), icon);
        let ops = Arc::new(ops);
        let _ = state.operation.set(Arc::clone(&ops));
        app.insert_resource(OperationHttpRes(ops));
        app.insert_resource(RconQueueRes(std::sync::Mutex::new(queue)));
        let acl = operation.acl();
        let operators = acl
            .groups()
            .iter()
            .flat_map(|g| g.objects.iter())
            .filter(|o| matches!(o, ssx_net_ops::acl::Object::Operator(_)))
            .count();
        info!(
            "operation: {} bans, {} mutes; the remote console {}; /metrics {}; {}",
            operation.bans().bans().len(),
            operation.bans().mutes().len(),
            if operators > 0 {
                format!("on ({operators} operator keys)")
            } else {
                "off (no operator key in acl.toml)".into()
            },
            if config.operation.metrics_enabled { "on" } else { "off" },
            config.operation.icon.as_ref().map_or("no icon".to_string(), |p| format!("the icon {}", p.display())),
        );
    }
    let extras = app.world_mut().remove_resource::<discovery::Extras>().unwrap_or_default();
    let found = discovery::Discovery::new(
        &config,
        beat_identity,
        local,
        Arc::clone(&state),
        Arc::clone(&shared),
        extras,
        sockets,
    )?;
    let mut discovery_status = found.status();
    let portmap_targets = app.world_mut().remove_resource::<portmap::PortmapTargets>().unwrap_or_default();
    let mapper = portmap::start(&config, local, portmap_targets.0).map_err(StartError::Other)?;
    discovery_status.portmap = mapper.as_ref().map(|m| m.status());
    app.insert_resource(discovery_status.clone());
    let shutdown = Arc::new(Notify::new());
    let first_mapping = mapper.as_ref().map(|m| m.first_attempt());
    let http_thread = {
        let (shutdown, shared) = (Arc::clone(&shutdown), Arc::clone(&shared));
        let signals = options.signals;
        std::thread::Builder::new()
            .name("http".into())
            .spawn(move || -> Result<(), String> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()
                    .map_err(|e| e.to_string())?;
                runtime.block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener).map_err(|e| e.to_string())?;
                    if signals {
                        tokio::spawn(watch_signals(shared));
                    }
                    let starter = tokio::spawn(async move {
                        if let Some(first) = first_mapping {
                            let _ = tokio::task::spawn_blocking(move || first.wait(portmap::MAPPING_WAIT)).await;
                        }
                        found.start()
                    });
                    http::run(listener, state, shutdown).await;
                    if let Ok(running) = starter.await {
                        running.stop().await;
                    }
                    Ok(())
                })
            })
            .map_err(|e| StartError::Other(e.to_string()))?
    };
    let started =
        Started { addr: local, identity_key, shared, created_keys: keys.created, discovery: discovery_status };
    on_started(&started);
    app.run();
    if let Some(m) = mapper {
        m.stop();
    }
    shutdown.notify_one();
    match http_thread.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(StartError::Other(format!("the HTTP thread: {e}"))),
        Err(_) => Err(StartError::Other("the HTTP thread panicked".into())),
    }
}

// PORT(choice): one thread for every schedule. At 0 riders a frame is about a millisecond of short systems, and Bevy's
// multi-threaded executor (the default with its `multi_threaded` feature) handed them to its compute threads and
// waited on each: 80-175 thread switches a frame, more than half of the idle CPU on the pool's threads; the official
// servers (4 vCPUs, four servers) took 15-30 % of a core each idle and logged frames of 250-528 ms. Measured (round
// srv2, the four servers on one laptop core, 300 s): CPU 4.5 -> 1.6 % of a core each, switches 5 240 -> 133 a second,
// a frame's work p50 1.0-5.9 -> 0.4-1.3 ms, p99 6.3-11.4 -> 1.8-2.9 ms; on two cores beside a busy process 4.5 ->
// 1.4 %, p50 13-14 -> 2-3 ms. The server's systems are short and mostly ordered one after another; the network's I/O,
// the scripts and the HTTP server have threads of their own. The original has no dedicated server.
fn one_thread(app: &mut App) {
    for (_, schedule) in app.world_mut().resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(SingleThreadedExecutor::new());
    }
}

async fn watch_signals(shared: Arc<Shared>) {
    let mut asked = false;
    loop {
        if wait_for_signal().await.is_err() {
            warn!("the signal handlers could not be installed: stop the server with the console's `stop`");
            return;
        }
        if asked {
            error!("a second stop signal: ending at once");
            std::process::exit(1);
        }
        asked = true;
        info!("stop signal: stopping (a second signal ends the server at once)");
        shared.stop_requested.store(true, Ordering::Release);
    }
}

#[cfg(unix)]
async fn wait_for_signal() -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
    Ok(())
}

#[cfg(windows)]
async fn wait_for_signal() -> std::io::Result<()> {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_shutdown};
    let (mut c, mut b, mut close, mut shutdown) = (ctrl_c()?, ctrl_break()?, ctrl_close()?, ctrl_shutdown()?);
    tokio::select! {
        _ = c.recv() => {}
        _ = b.recv() => {}
        _ = close.recv() => {}
        _ = shutdown.recv() => {}
    }
    Ok(())
}
