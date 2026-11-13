use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budgets {
    pub call: u64,
    pub call_backstop: Duration,
    pub tick: Duration,
    pub tick_checkpoints: u64,
    pub tick_backstop: Duration,
    pub over_tick: u32,
    pub memory: usize,
    pub chain: u64,
    pub chain_backstop: Duration,
}

// PORT(choice): the checkpoint budgets: client 250 000 a call, 500 000 a chain, 50 000 a tick; server 1 250 000 / 2 500
// 000 / 100 000. They keep the plan's CPU budgets (client 10 ms a call, 20 ms a chain, 2 ms a tick; server 50 / 100 / 4
// ms) at 40 ns a checkpoint, a tight loop on the development machine's fastest core at idle (measured: 38 ns; table
// writes 52, the string matcher 40, mixed handler code 77, GC churn 91); the chain is twice a call so a callee's own
// runaway meets its own budget first; tuning: count the checkpoints of real gamemodes and HUD packages (M7b / M8) per
// call and per tick; the game runs no mod scripts.
const CLIENT_CALL: u64 = 250_000;
const CLIENT_CHAIN: u64 = 500_000;
const CLIENT_TICK: u64 = 50_000;
const SERVER_CALL: u64 = 1_250_000;
const SERVER_CHAIN: u64 = 2_500_000;
const SERVER_TICK: u64 = 100_000;

// PORT(choice): each backstop is 8 times the plan's CPU budget (client 80 ms a call, 160 ms a chain, 16 ms a tick;
// server 400 / 800 / 32 ms): on the development machine (6 P, 8 E, 2 low-power cores) the same work took up to 5.6
// times its idle P-core CPU time (median of 30) on a low-power core with every core busy, 11.5 times in one run; mixed
// handler code (77 ns a checkpoint) at a full call budget still meets the backstop first there; tuning: the same
// measurement on the slowest machines the game supports, and on Windows; the original runs no mod scripts.
const K: u32 = 8;

impl Budgets {
    pub const CLIENT: Budgets = Budgets {
        call: CLIENT_CALL,
        call_backstop: Duration::from_millis(10).saturating_mul(K),
        tick: Duration::from_millis(2),
        tick_checkpoints: CLIENT_TICK,
        tick_backstop: Duration::from_millis(2).saturating_mul(K),
        over_tick: 30,
        memory: 64 << 20,
        chain: CLIENT_CHAIN,
        chain_backstop: Duration::from_millis(20).saturating_mul(K),
    };
    pub const SERVER: Budgets = Budgets {
        call: SERVER_CALL,
        call_backstop: Duration::from_millis(50).saturating_mul(K),
        tick: Duration::from_millis(4),
        tick_checkpoints: SERVER_TICK,
        tick_backstop: Duration::from_millis(4).saturating_mul(K),
        over_tick: 30,
        memory: 256 << 20,
        chain: SERVER_CHAIN,
        chain_backstop: Duration::from_millis(100).saturating_mul(K),
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackageCaps {
    pub elements: usize,
    pub data_keys: usize,
    pub data_bytes: usize,
    pub handlers: usize,
    pub events: usize,
    pub timers: usize,
    pub commands: usize,
    pub exports: usize,
    pub latent: usize,
    pub queue_entries: usize,
    pub queue_bytes: usize,
    pub tick_messages: usize,
    pub tick_bytes: usize,
}

impl PackageCaps {
    pub const DEFAULT: PackageCaps = PackageCaps {
        elements: 4096,
        data_keys: 16384,
        data_bytes: 4 << 20,
        handlers: 1024,
        events: 512,
        timers: 1024,
        commands: 128,
        exports: 256,
        latent: 16,
        queue_entries: 1024,
        queue_bytes: 4 << 20,
        tick_messages: 1024,
        tick_bytes: 1 << 20,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageCaps {
    pub keys: usize,
    pub bytes: usize,
    pub flush_every: Duration,
}

impl StorageCaps {
    pub const SERVER: StorageCaps = StorageCaps { keys: 4096, bytes: 16 << 20, flush_every: Duration::from_secs(1) };
    pub const CLIENT: StorageCaps = StorageCaps { keys: 1024, bytes: 1 << 20, flush_every: Duration::from_secs(1) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostLimits {
    pub budgets: Budgets,
    pub game: super::game::GameCaps,
    pub package: PackageCaps,
    pub storage: StorageCaps,
    pub packages: usize,
    pub source_files: usize,
    pub source_bytes: usize,
    pub tree_depth: usize,
    pub event_depth: usize,
    pub call_depth: usize,
    pub min_timer: Duration,
    pub max_timer: Duration,
    pub timer_args: usize,
    pub outbox_messages: usize,
    pub outbox_bytes: usize,
    pub latent_bytes: usize,
    pub latent_chunk: usize,
    pub latent_rate: u32,
    pub latent_min_rate: u32,
    pub latent_max_rate: u32,
    pub latent_peer_transfers: usize,
    pub latent_peer_bytes: usize,
    pub latent_send_total: Option<u32>,
    pub client_data_keys: usize,
    pub client_data_bytes: usize,
    pub writable_keys: usize,
    pub mirror_elements: usize,
    pub mirror_data_keys: usize,
    pub mirror_data_bytes: usize,
    pub command_line: usize,
    pub command_args: usize,
}

pub const CLIENT_LATENT_TOTAL: u32 = 64 << 10;

impl HostLimits {
    const fn side(budgets: Budgets, storage: StorageCaps) -> Self {
        HostLimits {
            budgets,
            game: super::game::GAME_CAPS,
            package: PackageCaps::DEFAULT,
            storage,
            packages: 256,
            source_files: 256,
            source_bytes: 4 << 20,
            tree_depth: 64,
            event_depth: 16,
            call_depth: 8,
            min_timer: Duration::from_millis(50),
            max_timer: Duration::from_secs(24 * 3600),
            timer_args: 16,
            outbox_messages: 8192,
            outbox_bytes: 16 << 20,
            latent_bytes: crate::value::MAX_BYTES,
            latent_chunk: 16 << 10,
            latent_rate: 50_000,
            latent_min_rate: 500,
            latent_max_rate: 1_000_000,
            latent_peer_transfers: 16,
            latent_peer_bytes: 32 << 20,
            latent_send_total: None,
            client_data_keys: 1024,
            client_data_bytes: 1 << 20,
            writable_keys: 64,
            mirror_elements: 65536,
            mirror_data_keys: 262_144,
            mirror_data_bytes: 64 << 20,
            command_line: 1024,
            command_args: 32,
        }
    }
    pub const CLIENT: HostLimits =
        HostLimits { latent_send_total: Some(CLIENT_LATENT_TOTAL), ..Self::side(Budgets::CLIENT, StorageCaps::CLIENT) };
    pub const SERVER: HostLimits = Self::side(Budgets::SERVER, StorageCaps::SERVER);
}
