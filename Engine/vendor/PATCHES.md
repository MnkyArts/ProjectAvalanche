# Patched third-party crates

Each directory here is a crate exactly as published on crates.io (the `.crate` contents: sources, `Cargo.toml`,
`Cargo.toml.orig`, `Cargo.lock`, `.cargo_vcs_info.json`) plus the patches listed below, and nothing else. The
workspace uses them through `[patch.crates-io]` in `Engine/Cargo.toml`; they are not workspace members (`exclude`), so
our formatting, lints and tests do not run on them. Every patched place carries a comment that starts with
`Project Avalanche patch` and names its patch (T1…T24 below).

**Licenses.** lightyear and bevy_replicon are MIT OR Apache-2.0. lightyear's published crates carry no license file;
each lightyear directory holds `LICENSE-MIT` and `LICENSE-APACHE` from the lightyear repository at the commit the
0.31.0 crates were published from (`8a926d6fece6a0cf04c2747de90c01258524fe0d`, per each crate's
`.cargo_vcs_info.json`). bevy_replicon 0.45.2's crate carries its own two license files (published from
`7af0114ca0763e076dc95302c5a095e6eff384e3`). Apache-2.0 §4(b) asks modified files to say so: the patch comments and
this file do. rav1d 1.1.0 is BSD-2-Clause; its crate carries `COPYING` (VideoLAN, the dav1d authors, the Internet
Security Research Group; published from `782dab2135ea64a057c097088a13eb8ed3cc3320`).
gilrs 0.11.2 is MIT OR Apache-2.0; its published crate omits the root license files, so its directory also carries
`LICENSE-MIT` and `LICENSE-APACHE` from its published commit `07e286e24b046cf39e5c367daa2770b805a64692` in
`https://gitlab.com/gilrs-project/gilrs` (the commit recorded in `.cargo_vcs_info.json`). Its bundled
`SDL_GameControllerDB/LICENSE` is retained unchanged (zlib, Sam Lantinga). gilrs-core 0.6.8 (MIT OR Apache-2.0) was
published from the same commit and its crate omits the same files, so its directory carries the same two license
files. T23's `src/mapping/automatic.rs` is a translation of SDL 3.4.20 code (zlib): it is marked as altered and keeps
SDL's licence notice in full, as the zlib licence's conditions 2 and 3 ask.

**Decisions.** The user, 2026-10-23 (`Docs/architecture/multiplayer.md` §16, D18 and D19): patch lightyear in the
workspace, keep `std` maths, fix the crash bugs in our copy, report nothing upstream. Measurements:
`Docs/architecture/network.md` §7, §9; T9…T12 came with multiplayer phase 1 (round `net1`), T10…T12 from its audit of
lightyear's and replicon's panics and errors on peer input (`network.md` §12), each crash confirmed by a hostile case
against the upstream code first. rav1d: the user's decision of 2026-11-07 (taken as recommended, decision M-D9 in
`Docs/architecture/modding.md` §16), measured in `Docs/architecture/content.md` §20.9: the assembly on, 8-bit only.

**The export.** `Engine/tools/export/export.py` copies this folder verbatim into the game's repository, with the
`[patch.crates-io]` entries the exported packages need (comments are not stripped from third-party code; the license
files and this file are kept; `Docs/architecture/export.md`).

**Upgrading.** A lightyear upgrade replaces these directories with the new release's crates and applies each patch
again (or drops it if the release has an equivalent fix); the line numbers below are 0.31.0's.

| Crate | Patches |
|---|---|
| `lightyear_replication` 0.31.0 | T5, T7 |
| `lightyear_transport` 0.31.0 | T1, T3, T6, T13 |
| `lightyear_messages` 0.31.0 | T6 |
| `lightyear` 0.31.0 | T2 |
| `lightyear_netcode` 0.31.0 | T3, T8, T9, T15 |
| `lightyear_udp` 0.31.0 | T4, T11, T16 |
| `lightyear_core` 0.31.0 | T10 |
| `bevy_replicon` 0.45.2 | T12, T14 |
| `rav1d` 1.1.0 | T17, T18 |
| `gilrs` 0.11.2 | T19, T20, T22, T23, T24 |
| `gilrs-core` 0.6.8 | T21 |

## T1 — fragment reassembly is bounded (`lightyear_transport`)

**Why.** `FragmentReceiver::receive_fragment` (`src/channel/fragments/receive.rs:56`) created a `FragmentConstructor`
(`:83`, `new` at `:137`) for the first fragment of every message id, and `new` allocated `vec![false; num_fragments]`
(`:145`) and `vec![0; num_fragments × fragment_size]` (`:146`) with `num_fragments` straight from the packet (a u64
varint), before any check. One 30-byte packet from any connected peer declaring 2^40 fragments aborted the process
("memory allocation of 1099511627776 bytes failed"). Moderate counts on many message ids reserved memory without
limit, and reliable channels never expired an incomplete message (`expires_fragments`, `src/channel/receive.rs:179`;
the cleanup at `:143`).

**What.**
- `FragmentLimits` (new, `src/channel/fragments/receive.rs`; exported in the prelude): `max_message_size` (the
  largest declared message, fragment count × fragment size), `max_in_flight_bytes` (incomplete messages per channel
  and connection), `reliable_stale_after`. A Bevy resource that `TransportPlugin` initialises with a default (1 MiB,
  4 MiB, 10 s; a guess marker at the code states it) and an application replaces; `ssx-net` sets its protocol's limits
  (16 KiB, 64 KiB, 10 s; `crates/ssx-net/src/limits.rs`).
- `receive_fragment` checks the declared size against `max_message_size` and the channel's reserved bytes plus it
  against `max_in_flight_bytes` before creating a constructor; the new errors `FragmentedMessageTooLarge` and
  `FragmentBudgetExceeded` (`ChannelReceiveError`, `src/channel/receive.rs:16`). The receiver counts the bytes each
  constructor reserves and releases them when the message completes or expires (`cleanup`).
- Reliable channels expire an incomplete message after `reliable_stale_after` without a new fragment (unreliable ones
  keep upstream's 3 s, `DISCARD_AFTER`): `ChannelReceive::update` (`src/channel/receive.rs:143`) takes the limits;
  every received fragment records its time (`:162` passed `None` for reliable channels).
- `ChannelReceive::fragment_bytes_in_flight` (new, public): what a channel holds, for tests and diagnostics.
- `TransportPlugin::buffer_receive` (`src/plugin.rs:79`) reads the resource and passes it on; `buffer_recv` and
  `update` take it. Upstream's unit tests pass the default limits (`src/channel/fragments/receive.rs`,
  `src/channel/receive.rs`, `src/channel/builder.rs` test helpers) and initialise the resource where they run the
  receive system on a bare `World` (`src/plugin.rs` tests); two new tests: a declared size above the limit is refused
  with nothing reserved; partial messages are refused above the budget and freed by the cleanup.

## T2 — a protocol mismatch disconnects instead of panicking (`lightyear`)

**Why.** `ProtocolCheckPlugin::receive_verify_protocol` (`src/protocol.rs:58`) returned the mismatch as an error from an
observer (`:67`, `:70`, `:73`); Bevy's default error handler gives such errors the panic severity, so a server whose
message, component or channel registration differs from the client's (or one that forges the check) crashed every
client that connected.

**What.** The observer returns nothing; on a mismatch it logs an error, triggers the new `ProtocolMismatch { entity,
error }` (an `EntityEvent`, exported in the prelude with `ProtocolCheckError`, which gains `Clone, Copy, PartialEq, Eq`)
for the client link whose remote peer sent the check, and triggers lightyear's `Disconnect` on that link (netcode's
client disconnect: its packets tell the server). Bevy's global error handler is untouched.

## T3 — the server disconnects one client through netcode (`lightyear_netcode`)

**Why.** `NetcodeServer`'s server is crate-private; only `Stop` (`src/server_plugin.rs:428`) disconnected clients, all
of them. A server could only drop a client's link: netcode kept the session until its timeout and sent nothing, so the
client went on sending until it timed out.

**What.**
- A new observer `NetcodeServerPlugin::disconnect_client`: lightyear's `Disconnect` on a connected client link of a
  netcode server drops the payloads already queued on the link, calls the netcode server's `disconnect` for that
  client (its disconnect packets are queued, the session is forgotten) and inserts `Disconnecting` (the link is
  despawned in `Last`), as `Stop` does per client. Triggered before `PostUpdate`'s send, the disconnect packets leave in
  that frame (measured: the kicked client leaves `Connected` within 2 s, against its 3 s timeout before).
- The receive system's despawn of a disconnected client (`:401`) becomes `try_despawn`: the link may already be gone.
- `lightyear_transport`'s `TransportPlugin::buffer_send` (`src/plugin.rs:400`) skips links that are `Disconnecting`:
  netcode's send passes a disconnecting link's queue on as it is (`src/server_plugin.rs:256` of netcode keeps it for the
  disconnect packets), so packets the transport still built for the link would have gone out unencrypted (from the
  source; upstream's `Stop` has the same path).

## T4 — the UDP endpoint forgets a link's address (`lightyear_udp`)

**Why.** `UdpEndpoint::connected_addresses` (`src/endpoint.rs:53`) gained an entry for every new source address and lost
it only when the same address sent again after its link was gone (`:217`): one entry per address ever seen, spoofed
sources included, for the endpoint's lifetime.

**What.** A new observer `UdpEndpointPlugin::forget_link` on the removal of `UdpLinkOfIO` (a link despawned, or no
longer this transport's) removes the address's entry when it still names that link. `UdpEndpoint::address_count`
(new) reports the entries.

## T5 — no forced `libm` maths (`lightyear_replication`)

**Why.** The crate depended on `bevy_transform` with the feature `libm` (`Cargo.toml:120`, `Cargo.toml.orig:59`), which
turns on `bevy_math/libm` and `glam/libm`. Cargo unifies features over every package of one build, so any build that
contained lightyear compiled glam's and `bevy_math`'s float functions from the `libm` crate instead of `std`: the
game's physics moved (6 of 9 ride fingerprints, `bot_race`; traced to `glam/libm` alone).

**What.** `bevy_transform` without the feature; the crate's `std` feature turns on `bevy_transform/std` (a math backend
for builds without the game's Bevy); a new feature `libm` forwards to `bevy_transform/libm` for builds that want it.
`cargo tree -e features -i glam` / `-i bevy_math` for the workspace, `ssx-net` alone, the game alone and the physics
with `ssx-net`: no `libm`, `std` on (`nostd-libm`, which `bevy_internal` turns on as before, is inert with `std`).

## T6 — receive errors reach the application (`lightyear_transport`, `lightyear_messages`)

**Why.** A packet the transport could not parse (`src/plugin.rs:284` of the transport) or a message the message layer
could not read (an unknown message id, bytes that do not deserialize; `src/receive.rs:691` of the messages crate) was
logged at error level and dropped: the application could not tell which link sent it, so a client sending them could
be neither counted nor disconnected, and the log grew with every packet.

**What.** Two new Bevy messages, written where the errors were only logged: `TransportReceiveError { entity, error }`
(`lightyear_transport`, `src/plugin.rs`; registered by `TransportPlugin`) and `MessageReceiveError { entity, error }`
(`lightyear_messages`, `src/plugin.rs`, written in `src/receive.rs`; registered by `MessagePlugin`); both exported in
the preludes. The log line stays. These packets arrive after the connection layer authenticated them, so the link's
own peer sent them: `ssx-net` disconnects it at the first.

## T7 — replicon's foreign channels are refused, not passed on (`lightyear_replication`)

**Why.** lightyear registers replicon's three channels both ways and hands every message it receives on them to
replicon by the channel's index in its own list: the server's `receive_server_packets` (`src/server.rs:187`) passed a
client's message on the update or mutation channel to `ServerMessages::insert_received` with index 1 or 2, the
client's `receive_client_packets` (`src/client.rs:148`) a server's message on the mutation-ack channel with index 2.
Replicon has one client channel (the acks) and two server channels by default, and `insert_received` panics for a
channel it has not registered: one message from any connected client crashed the server ("server should have a
receive channel with id 1"), one from any server crashed the client ("client should have a channel with id 2"; both
measured, `Game-Data/Converted/handoff/net0/hostile/run4_patched.txt`, `d19/replication_from_server_unpatched.txt`).

**What.** Both functions pass on only the channels replicon receives on that side (`RepliconChannels::client_channels`
on the server, `server_channels` on the client); a message on another one is dropped, logged, and reported as a
`TransportReceiveError` (T6) with `TransportError::ChannelIdNotFound` for its link. (`ssx-net` drained these channels
itself before T7 existed; it now relies on the report.)

## T8 — netcode's per-client state stays bounded (`lightyear_netcode`)

**Why.** A valid connect request makes the netcode server create a pending connection (`ConnectionCache::add`,
`src/server.rs:141`) before the client answers the challenge, and three things in it grew without bound or went
inconsistent (from the source; measured with T8's tests against upstream's behaviour,
`Game-Data/Converted/handoff/net0/d19/netcode_cache_upstream.txt`):
- The same request sent from another source address (another link) replaced the client id's entry but kept the earlier
  link's entity mapping (`:178`): one request replayed from 300 addresses left 300 mappings. The connect-token replay
  table does not stop it (`TokenEntries::find_or_insert` accepts a token it has seen; its address check is a comment).
  A later response from an earlier link then connected that link under the entry that names the newer one.
- A link whose request named another client id updated its entry in place but left it under the old client id, so
  that connection could not complete.
- A pending connection never expired: the timeout check skips connections that are not connected (`:851`), and
  `ConnectionCache::remove` (`:180`) refuses them.
- `Server::send_netcode_packets` (`:901`) drained a link's queue of netcode packets (challenges, denials) and kept the
  empty queue: one entry per link that was ever sent one.

**What.**
- `add` keeps one entry per link and per client id: the same link repeating its request for the same client id
  refreshes the entry as upstream did; any other request first forgets the link's earlier pending connection and the
  client id's (`ConnectionCache::remove_pending`, new). Requests for a connected link or client id are refused before
  `add`, as upstream refuses them, so a connected entry is never touched.
- `check_for_timeouts` forgets a pending connection `timeout` seconds (the token's) after the last request for it, and
  drops the link's netcode send queue with it and with a connected client that times out.
- `send_netcode_packets` removes the queue it sends.
- `Server::state_sizes` and `NetcodeServer::state_sizes` (new) report the entries (`ServerStateSizes`: connections,
  link entities, replay windows, send queues).
- Three new unit tests (`src/server.rs`): one request replayed from 300 links keeps one pending connection, and no
  queue is left once the challenges are sent; a link naming another client id replaces its pending connection; a
  pending connection expires after its timeout counted from the last request, and a token without a timeout keeps
  one. All three fail with upstream's `add`, timeout check and send queue.

What T8 does not change: the connect-token replay table (a token is accepted again from another address while it is
valid; with T8 that moves the pending connection to the newer link, it no longer leaves one behind); netcode's
challenge goes to whatever source address a request names (no amplification: a 1 078-byte request draws a smaller
challenge). Both belong to phase 1's join, which issues the tokens.

## T9 — a client can read a connect token's public fields (`lightyear_netcode`)

**Why.** Phase 1's join hands the client a connect token from a server it may not trust. The netcode client sends its
connection requests to the token's public server addresses for as long as the token is valid, and a token without a
timeout keeps a dead connection forever; upstream's `ConnectToken` (`src/token.rs`) exposes none of these fields, so a
client cannot check them before it uses the token.

**What.** Four read-only accessors after `expire_timestamp` (`src/token.rs:322`): `server_addresses()` (the public
addresses as a `Vec<SocketAddr>`), `protocol_id()`, `create_timestamp()`, `timeout_seconds()`. Nothing else changes.
`ssx_net::connect_token` uses them (`Docs/architecture/net-protocol.md` §3.4).

## T10 — tick deltas from a peer convert without a panic (`lightyear_core`, vendored for it)

**Why.** `TickDelta::to_duration` (`src/time.rs:313`) was `tick_duration.mul_f32(self.value.to_num())`, which panics
for a negative or a too-large product ("cannot convert float seconds to Duration"). Tick deltas come from peers: a
client's pong carrying a tick 2^31 away made `lightyear_sync`'s `process_pong` compute a negative delta and **any
server panicked** (exit 101); a server's `SenderMetadata` with an interval of 2^31 ticks panicked the client's
interpolation timeline the same way (both confirmed against upstream by the hostile cases `PongOverflow` and
`MetadataOverflowFromServer`, `crates/ssx-net/src/harness/`). `TickDelta::to_i32` (`:357`) was
`self.value.round().to_num()`, which overflows in debug builds for values within half a tick of the type's limit.

**What.** `to_duration` returns `Duration::ZERO` for a delta ≤ 0 (a negative delta has no `Duration`), `Duration::MAX`
when the product would exceed half of it, and upstream's `mul_f32` otherwise (the same value for every delta upstream
accepted). `to_i32` rounds with `saturating_round`. A new unit test (`conversions_of_any_delta_never_panic`): a delta of
2^31 ticks (which wraps negative), a negative one, a normal one (equal to upstream's), the type's maximum against
`Duration::MAX`, and `to_i32` at both limits and at halves.

## T11 — oversized datagrams on Windows do not stall the UDP read (`lightyear_udp`)

**Why.** The endpoint reads at most 1 472 bytes per datagram. Linux cuts a larger datagram and returns the rest; Windows
reports it as the error `WSAEMSGSIZE` (10040) after discarding it, and upstream's receive loops (`src/lib.rs:225`,
`src/endpoint.rs:315`) treated every error but `WouldBlock` as the end of the frame's reading: one oversized datagram
per frame from anyone delayed every other packet by a frame (from the source; not measured: no Windows run).

**What.** Both receive loops recycle the buffer and read the next datagram on `WSAEMSGSIZE` when built for Windows
(`cfg!(windows)`), as Linux behaves; the constant is defined beside them. The arm is compiled but never taken on Linux.

## T12 — a peer's replication data does not panic replicon and is reported (`bevy_replicon`, vendored for it)

**Why.** Three places of the client's replication receive (`src/client.rs`: `apply_removals` :628, `apply_changes`
:707 and :715, `apply_mutations` :951) took a size from the server's message and called `Bytes::split_to` with it,
which panics beyond the message's end, and looked up a component-function id from the message with
`ReplicationRegistry::get` (`src/shared/replication/registry.rs:163`), which panics on an unknown id: **a hostile
server crashed every client** with one message (confirmed against upstream by the hostile cases
`ReplicationOversizeFromServer` and `ReplicationUnknownFnsFromServer`). Every other refusal of the client
(`apply_replication`'s errors, :281…:365) and the server's refusal of a client's mutation acks that do not read
(`receive_acks`, `src/server.rs:407`, which also kept reading the rest) were only logged at debug level: the
application could not end the connection of a peer that sends what no correct peer sends.

**What.**
- `split_checked` (new, `src/client.rs`): the size checked against the remaining bytes before `split_to`, an error
  otherwise (as replicon's own `read_userdata` already did); used at the three sites.
- `ReplicationRegistry::try_get` (new): `None` for an unknown id; the three sites return an error instead of panicking.
- `ReplicationReceiveError { client: Option<Entity>, error: String }` (new Bevy message, `src/shared/receive_error.rs`,
  registered by the client and the server plugins): the client writes one (`client: None`) where it refused the
  server's data (four sites); `receive_acks` writes one with the client's entity and stops reading that message.
- `ssx-net` acts on it: the server kicks the client (`MalformedReplication`), a client leaves the server and says why
  (`Docs/architecture/network.md` §7).

## T13 — what a peer has not acknowledged is bounded (`lightyear_transport`)

**Why.** A reliable channel's sender kept every message until the peer acknowledged it
(`ReliableSendState::pending`, `src/channel/send_reliable.rs:46`), without a bound: a peer that keeps its connection
alive but never acknowledges made its sender hold every reliable message it was sent, for ever — the server's memory
grew with each control message and replicon update for such a client (found by phase 1's audit, `network.md` §11,
§12; reachable once phase 2 replicates). Past 65 536 held messages the wrapping `u16` message id would also have
replaced held messages in the map.

**What.**
- `ReliableLimits` (new Bevy resource beside `FragmentLimits`, `src/channel/fragments/receive.rs`; in the prelude):
  `max_unacked_messages` and `max_unacked_bytes` per reliable channel and connection; `TransportPlugin` initialises a
  default (4 096 messages, 4 MiB: below the 32 768 ids the wrapping id tells apart; a guess marker states it) and an
  application replaces it (`ssx-net`: 256 messages, 256 KiB; `crates/ssx-net/src/limits.rs`).
- `ReliableSendState` counts the bytes it holds (`pending_bytes`, kept by `buffer_send` and `receive_ack`; `held()`),
  `ChannelSend::reliable_held()` reads them.
- The transport's send (`TransportPlugin::buffer_send`, `src/plugin.rs`) does not buffer a reliable send that would
  exceed a limit: it stops that link's sends of the frame with `TransportError::UnacknowledgedLimit` and writes
  `TransportSendError { entity, error }` (new Bevy message, in the prelude), so the application learns which link.
- `ssx-net` acts on it: the server kicks the client (`Unacknowledged`), a client leaves the server and says why.
- Test `held_messages_and_bytes_follow_buffering_and_acks` (single and fragmented messages).

## T14 — what a server makes a client replicate is bounded (`bevy_replicon`)

**Why.** The client's replication receive spawned an entity for every new server entity a message named
(`apply_changes`, `src/client.rs`: the vacant entry's `world.spawn_empty()`; `WriteCtx::get_mapped`,
`src/shared/replication/registry/ctx.rs:91`, for an entity inside component data) and buffered every update and mutate
message it could not apply yet (`BufferedUpdates`, `BufferedMutations`), without a bound: a hostile server could make a
client spawn entities or hold messages without end (found by phase 1's audit, `network.md` §11).

**What.**
- `ReplicationLimits` (new Bevy resource, `src/shared/receive_error.rs`, initialised by the client plugin with a default
  of 65 536 entities and 4 096 buffered messages, a guess marker at the code): `max_entities`, `max_buffered_messages`;
  `ssx-net` sets 2 × its connection limit (128) and 600 (64 until round `net3`: the receive reads a frame's mutate
  messages into the buffer before it applies any, and the server sends an unacknowledged mutation again every tick, so
  a client whose frame stalls receives one a tick; `ssx-net/src/limits.rs`).
- `ServerEntityMap` gains a limit (`set_limit`, set from the resource at each receive; a `clear` keeps it), `at_limit`,
  and a refused flag: a new server entity beyond the limit is refused **before it is spawned** — `apply_changes`
  returns an error at the vacant entry, `get_mapped` returns `Entity::PLACEHOLDER` without mapping or reserving and sets
  the flag, which the caller turns into an error after that component's write (`apply_changes`, `apply_mutations`).
- An update or mutate message that would exceed `max_buffered_messages` is not buffered; it is reported.
- Every refusal goes through T12's `ReplicationReceiveError`, and the client leaves the server.
- Test `the_limit_counts_mapped_entities` (`src/shared/server_entity_map.rs`).

## T15 — the server tells a client's goodbye from its timeout (`lightyear_netcode`)

**Why.** The netcode server called its disconnect callback the same way for a client's disconnect packet
(`Server::process_packet`, `src/server.rs:583`), for a client that sent nothing within its timeout
(`check_for_timeouts`, `:858`) and for its own disconnect (`Server::disconnect`, `:1093`), and the server plugin gave
every one the same `Disconnected { reason: ByPeer(None) }` (`src/server_plugin.rs:399`): a server could not tell a
player who left from one whose connection failed (the dedicated server logged "the client left, or timed out"). The
client side tells them apart already (`src/client_plugin.rs:186`: `ByPeer` for the server's disconnect, `TransportError`
with the state's name for a timeout). Found by the window checks of multiplayer phase 2's pause menu (round `net2`).

**What.**
- `DisconnectCause` (new, `src/server.rs`; exported, a Bevy component): `ByClient`, `TimedOut`, `ByServer`; the
  disconnect callback (`Callback`, `ServerConfig::on_disconnect`) gets it as its third argument from the three callers.
- The server plugin puts the cause on the link entity beside its `Disconnected`, whose reason now says it as the client
  side does: `ByPeer(None)` for the client's disconnect (as before), `TransportError("ConnectionTimedOut")` for the
  timeout, `UserRequested(None)` for the server's own disconnect.
- `ssx-net` counts a rider's goodbye and timeout (`ServerStats::left`, `timed_out`; `crates/ssx-net/src/server/ends.rs`)
  and the dedicated server names them in its log and status.
- Test `the_disconnect_callback_tells_a_client_that_left_from_one_that_timed_out` (`src/server.rs`; a connected client's
  disconnect packet, its timeout, the server's disconnect), and `crates/ssx-net/tests/session.rs`
  (`the_server_tells_a_riders_goodbye_from_a_timeout`, over crossbeam).

## T16 — the UDP endpoint takes the application's socket (`lightyear_udp`)

**Why.** `UdpEndpointPlugin::link` (`src/endpoint.rs:131`) always made its socket with `std::net::UdpSocket::bind`, so an
application could set no option before the bind. A dedicated server on `::` needs `IPV6_V6ONLY` off to serve IPv6 and
IPv4 on one socket (multiplayer phase 4, round `net4a`, `Docs/architecture/net-protocol.md` §24): Windows' default is
on (IPv6 only), Linux's is the system's `net.ipv6.bindv6only` (usually off), and the option cannot be changed after
the bind.

**What.** `UdpEndpoint` gains a private `prebound: Option<std::net::UdpSocket>` (`None` by default) and `pub fn
use_socket(&mut self, socket)`; `link` takes that socket when there is one, else binds as before, then sets it
non-blocking and reads `LocalAddr` from it exactly as upstream. Nothing changes for an endpoint without a socket of
the application's. The dedicated server makes its game socket with `socket2` (`apps/avalanche-server/src/bind.rs`) and
hands it over before the start. Tests: the crate's own `link_takes_the_applications_socket` (`src/endpoint.rs`: the
socket taken, `LocalAddr` read from it) and `apps/avalanche-server/tests/server/ipv6.rs` (a join over `::1`, IPv4 peers
of an IPv6 socket); the crate's tests standalone: `Game-Data/Converted/handoff/net4a/runs/vendor_tests.txt`.

## T17 — rav1d builds its 10- and 12-bit contexts only with `bitdepth_16` (`rav1d`, vendored for it)

**Why.** The game builds rav1d 8-bit only (`bitdepth_8`; a mod movie is 8-bit) and, since 2026-11-07, with its
assembly (`asm`). That combination did not link: `Rav1dBitDepthDSPContext::get` (`src/internal.rs:186`) built the 10-
and 12-bit contexts (`Self::new::<BitDepth16>`) whatever the features, and their film grain names
`dav1d_{fgy_32x32xn,fguv_32x32xn_i4{20,22,44}}_16bpc_{ssse3,avx2,avx512icl}` and
`dav1d_generate_grain_{y,uv_4{20,22,44}}_16bpc_{ssse3,avx2}`, which `build.rs` assembles only with `bitdepth_16`
(rust-lld: those 20 symbols undefined in each program that links rav1d). Without `asm` it
linked, and a 10-bit stream reached rav1d's 16-bit Rust code (the game's own reader refuses such a stream first:
`ssx-content`'s AV1 check, "10-bit pictures (a movie is 8-bit)", and any sequence header but the track's).

**What.** The two statics and the `10` / `12` arms carry `#[cfg(feature = "bitdepth_16")]`. Without the feature `get`
answers `None` for them, which upstream's caller already handles (`rav1d_submit_frame`, `src/decode.rs:5019`: "Compiled
without support for {bpc}-bit decoding", `ENOPROTOOPT`); the other callers ask only for pictures that were decoded. With
`bitdepth_16` nothing changes. The game's binary holds no 16-bit code of rav1d's (`nm`: 1 596 `*_8bpc_avx2` kernels, 0
`_16bpc_`; the `bitdepth_16` build has 3 378).

## T18 — rav1d builds without warnings as a path dependency (`rav1d`)

**Why.** Cargo shows the warnings of a path dependency (a registry crate's are capped), so the vendored crate printed
upstream's own warnings in every build of the workspace, which is kept warning-free (`AGENTS.md`); rustc 1.99 with
`bitdepth_8` and `asm`: four unused 16-bit file lists in `build.rs`, a method and a struct nothing uses, three elided
lifetimes, and the derived equality of a function-pointer newtype (`unpredictable_function_pointer_comparisons`, once
per wrapped function). Each change keeps the behaviour; no lint is allowed.

**What.** `build.rs`: `x86_bpc16`, `x86_64_bpc16`, `arm_sve_bpc16`, `arm_bpc16` only with `bitdepth_16` (their one use
is so gated). `src/disjoint_mut.rs`: `Bounds::overlaps` only with `debug_assertions` or `test` (its users: `mod debug`
and the tests). `src/internal.rs`: `Rav1dFrameContextTaskThreadPendingTasks` removed (nothing constructed or named it).
`include/dav1d/picture.rs` (`with_offset`, `lf_offsets`) and `src/cdf.rs` (`cdf_write`): the elided lifetime written
`'_`. `src/wrap_fn_ptr.rs`: `Fn`'s `PartialEq` / `Eq` written out with `core::ptr::fn_addr_eq` (the same comparison
of addresses the derive made). Checked: `cargo build --release`, `cargo clippy --all-targets` (debug assertions on) and
the same for `x86_64-pc-windows-gnu` of `ssx-movie`: no warning.

## T19 — the Nacon GC-100 0110 uses its verified Linux controller mapping (`gilrs`, vendored for it)

**Why.** The user's physical Nacon GC-100 (`3285:0607`, input/USB device version `0110`, xpad) has GUID
`03000000853200000706000010010000`. gilrs 0.11.2's bundled database has the same USB device with version `0112`
(`03000000853200000706000012010000`), but its UUID lookup is exact. The actual v0.3.1 game logs a missing mapping;
the fallback maps native `ABS_Z` / `ABS_RZ` to `Axis::LeftZ` / `Axis::RightZ`, not to the analog trigger buttons
the game reads. Three physical press/release cycles per trigger reached 0…255. Simultaneous headless Bevy apps on
that same physical pad, one default and one exact-GUID mapped: the default trigger-button peaks stayed 0/0 with
0/0 complete cycles; the mapped peaks reached 1/1 with 4/4 complete cycles, including both held together. Evidence:
`Game-Data/Converted/handoff/lt-rt-20261110/`, `Docs/research/controller-input.md`. These are trigger-input proofs;
they do not claim that every controller binding or post-fix ride has already passed a physical test.

**What.** The published `.crate` was extracted unmodified first; its SHA-256 matches the registry checksum in the
workspace lockfile: `902fb00d3f6398e635be22e5c837b303c501835cca7ac11a47bba138f7aafdd8` (37 published files).
`SDL_GameControllerDB/gamecontrollerdb.txt` adds one Linux row for the observed **0110** GUID, copying the
existing **0112** row's bindings and preserving that original row and every other mapping. The preceding T19
comment identifies the modification. `lefttrigger:a2` and `righttrigger:a5` select this xpad layout's native
`ABS_Z` and `ABS_RZ`. The ordinary build script includes that Linux mapping, and the ordinary database selects it:
no runtime-library implementation change, process environment mutation, re-execution shim, new threshold or
global interpretation of arbitrary controllers' Z axes. Windows' mappings and native backend are unchanged.
The game and asset viewer receive the same dependency fix; Standard and Classic action layouts remain intact.

`src/mapping/mod.rs` adds two Linux regression tests using the device's native-code ordering. One verifies all
23 mapped native elements (including LT/RT as analog buttons, the four face buttons, shoulders, menu/thumb
buttons, both sticks, and D-pad axes/buttons), absence of trigger signals in the Z-axis slots, and the mapped
hat mask. The other checks that the original 0112 entry is preserved, neighboring GUIDs are not matched, and a
Windows mapping cannot override the Linux row. The actual normal mapping parser/database are tested, not a copy.

**Upgrade.** Replace the published gilrs crate on upgrade and reapply this one-row database addition only if the
upstream mapping still lacks the exact GUID; keep its focused regression coverage. A database update that still
contains only the 0112 GUID is not equivalent to this fix.

## T20 — gilrs uses the primitive floating-point maximum (`gilrs`)

**Why.** As a path dependency, gilrs exposes upstream's warnings instead of Cargo capping them for a registry
crate. On rustc 1.99 its import of `std::f32` makes the two `f32::MAX` uses in `src/ff/mod.rs` resolve to the
deprecated module constant. Both warnings are in the unmodified published source and are unrelated to T19.

**What.** Remove only `f32` from that import, retaining `std::fmt`. The same two expressions now resolve to the
primitive `f32` associated constant, whose value is identical. No gain-clamping behaviour changes and no lint
is suppressed. Its T20 comment identifies the modification.

## T21 — gilrs-core keeps the evdev facts SDL's automatic mapping reads (`gilrs-core`, vendored for it)

**Why.** T23 ports SDL's automatic gamepad mapping, which reads three facts per Linux device: the key and absolute-axis
capability bits (gilrs-core already keeps them), each absolute axis' `EVIOCGABS` answer (SDL 3.4.20
`GuessIfAxesAreDigitalHat`, `src/joystick/linux/SDL_sysjoystick.c:1192-1223`, tells a digital hat by its range;
`HandleHat`, 1730-1777, reads it by that range) and the device's kernel driver (the xpad d-pad rule, 2672-2688).
gilrs-core 0.6.8 used the `EVIOCGABS` answer only for its own normalisation and dropped it, and never asked udev for
the driver.

**What.** `src/platform/linux/gamepad.rs`: `AxesInfo` keeps each axis' `EvdevAbsInfo` (minimum, maximum, fuzz, flat,
resolution) where the ioctl succeeded (SDL skips an axis whose `EVIOCGABS` fails); the device's driver is read as
SDL's `SDL_UDEV_GetProductInfo` reads it (`src/core/linux/SDL_udev.c:279-287`: `udev_device_get_driver`, else the
`ID_USB_DRIVER` property; a name that is not UTF-8 counts as none), at enumeration and hotplug; without udev (the
inotify fallback) there is none, as in SDL without libudev. `udev.rs` adds `Device::driver`. The crate root exports
`EvdevAbsInfo`, `Gamepad::evdev_abs_info(code)`, `Gamepad::udev_driver()` and `EvCode::from_evdev(kind, code)` on
Linux only. Nothing else changes: no event, value or normalisation differs, other platforms compile none of it.

## T22 — gilrs looks a gamepad's mapping row up in SDL's order (`gilrs`)

**Why.** gilrs 0.11.2 finds a row only by the device's exact GUID. SDL 3.4.20 (`SDL_PrivateGetGamepadMappingForGUID`,
`src/joystick/SDL_gamepad.c:1426-1462`, its matcher 1372-1421) tries the exact GUID, then — for a GUID whose layout
carries a version (`SDL_JoystickGUIDUsesVersion`, `SDL_joystick.c:3132-3146`) — the first row in list order that
matches with the version cleared; and a row replaces an older one for the same GUID only at the same or a higher
priority, keeping its place in the list (`SDL_PrivateAddMappingForGUID`, 2133-2256, the replacement 2199-2246; the
priorities `SDL_GamepadMappingPriority`, 92-97: default, API, user). The Nacon GC-100's 0110 GUID is an example: its
0112 row would have matched without the version (T19 added the exact row before this was known).

**What.** `src/mapping/mod.rs`, Linux only: `MappingDb` records each row's origin (`Origin`: the bundled rows as
SDL's DEFAULT, rows the application adds as API, `SDL_GAMECONTROLLERCONFIG`'s as USER) and the GUIDs' list order;
`insert_from` keeps a row against a lower-priority one and keeps its place when replaced; `lookup` answers exact, then
without the version in list order. `src/mapping/select.rs` reads SDL's GUID layout (`sdl_guid`: the standard form of
`SDL_CreateJoystickGUID`, `SDL_joystick.c:2933-2975`; vendor and product as `SDL_GetJoystickGUIDInfo`, 2860-2921).
The previous exact `get` stays for the other platforms.

## T23 — a Linux gamepad no row names gets SDL's automatic mapping (`gilrs`)

**Why.** A player's pad without a row in the bundled database (Linux) got gilrs' default mapping, which turns
`ABS_Z` / `ABS_RZ` into `Axis::LeftZ` / `Axis::RightZ`: the game, which reads `LeftTrigger2` / `RightTrigger2`, had dead
analog triggers (`Docs/research/controller-input.md`). T19 fixed one pad. SDL itself does not need a row for such a
pad: when its lookup misses, SDL 3.4.20 builds a mapping from the device's evdev capabilities
(`SDL_PrivateGetGamepadMapping`, `src/joystick/SDL_gamepad.c:2368-2392` → `SDL_PrivateGenerateAutomaticGamepadMapping`,
2306-2366 → `LINUX_JoystickGetGamepadMapping`, `src/joystick/linux/SDL_sysjoystick.c:2287-2820`).

**What.** `src/mapping/automatic.rs` translates that rule (it carries SDL's licence notice and SDL's line numbers at
each step): no `BTN_GAMEPAD`, no mapping (2350); SDL's button, axis and hat indices (`ConfigJoystick`, 1225-1345; a hat
whose axes span −1…1 is digital, 1192-1223); face buttons with Sony's west/north swap; Back, Start, the stick clicks,
Guide; the shoulders `BTN_TL`/`BTN_TR`, else digital hat 1, else analog `ABS_HAT1Y`/`ABS_HAT1X`; the triggers
`ABS_HAT2Y`/`ABS_BRAKE`/`ABS_Z` and `ABS_HAT2X`/`ABS_GAS`/`ABS_RZ`, else digital hat 2, else `BTN_TL2`/`BTN_TR2`
(2515-2600); the d-pad `BTN_DPAD_*`, else digital hat 0, else analog hat 0, else xpad's `BTN_TRIGGER_HAPPY1…4`
(2658-2688); the sticks `ABS_X`/`ABS_Y` and `ABS_RX`/`ABS_RY`, else `ABS_Z`/`ABS_RZ` (2725-2732); the paddles
`BTN_GRIPR`/`GRIPL`/`GRIPR2`/`GRIPL2`, else on a Microsoft pad `BTN_TRIGGER_HAPPY5…8` (2739-2795), and `KEY_RECORD` as
misc1 (2797-2803). The elements are written in SDL's order (its row string,
`SDL_gamepad.c:2743`), and an input bound to two elements drives only the first, as SDL's bindings do
(`HandleJoystickAxis` / `HandleJoystickButton`, 247-321). A digital hat is read as `HandleHat` reads it (1730-1777: the
range in thirds, growing extremes), its directions on hat 0 as the d-pad axes, which gilrs' d-pad filter turns into the
d-pad buttons, bound to the native `BTN_DPAD_*` codes as gilrs' parser binds a row's `h0.*`.
`src/mapping/select.rs` chooses on Linux: a row (T22), else this mapping, else gilrs' default mapping, and logs one
line per gamepad: its name, GUID, vendor and product, and which of the three it got (the row's origin and whether it
matched only without the version; the automatic mapping as SDL's elements string, so a player can paste it).
`Gamepad::mapping_source()` answers the new `MappingSource::Automatic` for it, not `SdlMappings`.
`src/gamepad.rs` applies the selection and the digital hats' values.

Port choices, each marked `PORT(choice):` at its place: an analog hat 0 used as the d-pad becomes gilrs' `DPadX` /
`DPadY` (SDL binds both directions to the whole axis, `dpup:a7,dpdown:a7`, which reads up and left as held at rest);
SDL's hints are at their defaults (`SDL_JOYSTICK_LINUX_HAT_DEADZONES` on, `SDL_JOYSTICK_LINUX_DIGITAL_HATS` off); the
name is cut at SDL's 127 bytes on a character boundary; a device without `BTN_GAMEPAD` keeps gilrs' default mapping (SDL
on Linux has no default mapping and would not open it as a gamepad; gilrs keeps offering it as before). Unlike SDL, the
generated row is not added to the mapping list (SDL adds it at DEFAULT priority, where a later API or user row for that
GUID replaces it; gilrs' `set_mapping` replaces a gamepad's mapping directly). Not ported: SDL's name-CRC GUIDs for
devices without a vendor (gilrs' GUIDs carry no name CRC) and the rows' `crc:` field (no Linux row has one).

## T24 — the bundled SDL_GameControllerDB is SDL_GameControllerDB c0f11d40 (`gilrs`)

**Why.** gilrs 0.11.2 bundles an older copy; the newer one has rows for 32 more Linux pads.

**What.** `SDL_GameControllerDB/gamecontrollerdb.txt` is upstream commit `c0f11d40b91f57853ce9714c703e119d8243f91c`
(2026-11-09, `https://github.com/mdqinc/SDL_GameControllerDB`; the file's SHA-256
`a2299ef257b68c62b9fcce39fc59b5fb3a5a7068214f766a96c21595b55a11c3`) with T19's comment and row put back before the 0112
row (upstream still lacks the 0110 GUID): SHA-256 `d414808d077f6dbe74bfe43636402452872ae8c3cde0043e766d257149394a4b`.
`README.md` is upstream's at that commit; `LICENSE`, `duplicates.py`, `mapping_guide.png` and `check.yml` were already
identical. Linux rows 718 → 749 (rows with a GUID 717 → 748): 32 added, 40 changed, 1 removed upstream (ODROID Go 2,
`19000000010000000100000001010000`); no Linux row has a `crc:` field or a hat above 0. The new test
`every_bundled_linux_row_is_read` parses every Linux row with gilrs' own parser (749 of 749; before the refresh the same
count gave 718 of 718).

## Tests of the patched crates

gilrs (2026-11-10, `Game-Data/Converted/handoff/nacon-v032/vendor-tests.log`):
`cargo test --manifest-path vendor/gilrs-0.11.2/Cargo.toml --lib --locked` from `Engine/`, standalone target
`.build/nacon-gilrs-tests`: **13 passed, 0 failed, 0 ignored**, warning-free after T20. The two T19 regressions are
included. A scratch copy with the original published database restored fails the new exact-GUID test at the
missing-row assertion; restoring the patched database passes. No live vendor source was reverted for that negative
proof. Physical Bevy trigger-input A/B evidence predates the patch and is described under T19; the root session
owns the final actual-game and complete-binding acceptance.

gilrs and gilrs-core (2026-11-13, round `triggers`, `Game-Data/Converted/handoff/triggers/vendor_tests.txt`; the
script `vendor_tests.sh` beside it), each from its folder, standalone target: gilrs-core
`cargo test --lib --locked`: **1 passed**; gilrs `cargo test --lib --config
'patch.crates-io.gilrs-core.path="<Engine>/vendor/gilrs-core-0.6.8"'` (its published lock names the registry
gilrs-core; the script restores both published `Cargo.lock` files after the run): **26 passed, 0 failed**,
warning-free. The release repository's Linux check runs the same two commands (its step "Bundled controller
mappings (gilrs)"); until 2026-11-13 it still ran gilrs with `--locked` and without the patch, which builds it against
the registry's gilrs-core and fails on T23 (the v0.4.0 snapshot run 43473803067). New: T23's nine tests on synthetic capability sets (the xpad layout as SDL writes it, with its triggers
on the trigger buttons; no `BTN_GAMEPAD`; no `ABS_RX`/`ABS_RY`; only `BTN_TL2`/`BTN_TR2`; hats 2 as triggers, digital
and analog; hat 1 as the shoulders; Sony's face buttons, xpad's d-pad buttons and the Xbox paddles; the hat guess and
`HandleHat`; the name), T22's three (the lookup order, the priorities, the GUID reading) and T24's whole-database read
(749 of 749 Linux rows). Before the refresh the same read gave 718 of 718. T21 has no unit test of its own (its facts
come from ioctls and udev); the `/dev/uinput` pads in `Docs/research/controller-input.md` exercise it on devices the
kernel made.

Each crate's own tests, run standalone with the features the workspace uses, each against the other patched crates
(`Game-Data/Converted/handoff/net0/d19/vendor_tests.txt`): `lightyear_transport` 76 + 1 (85 + 1 with `compression_lz4`;
T1's two new tests included), `lightyear_messages` 25 + 3, `lightyear_netcode` 32 + 9 (T8's three new tests included; 1
ignored upstream), `lightyear_udp` 5, `lightyear_replication` 61 + 3 + 6 (7 ignored upstream), `lightyear` 1 + 4 (3
ignored upstream): all pass. One upstream test depends on wall-clock time: `lightyear_messages`'
`host_client_drops_queued_messages_after_a_timeline_error` (`src/plugin.rs:816`) fails when more than one 10 ms tick
passes between its two `app.update()` calls (the local timeline advances, its message waits for a tick its test timeline
never reaches); unchanged by the patches, measured on a loaded machine at the same rate with and without them
(`d19/messages_flake.txt`), and left as published. The patches' behaviour against a running server and client is in
`crates/ssx-net/tests/hostile.rs` and `examples/net_hostile.rs`.

Phase 1 (round `net1`, all eight crates, each against the other seven; `Game-Data/Converted/handoff/net1/records/
vendor_tests.txt`, the script beside it in `tools/vendor_tests.sh`): `lightyear_core` 45 (T10's test included; 2
doc-tests ignored upstream), `lightyear_netcode` 32 + 9 (1 ignored upstream), `lightyear_udp` 5, `bevy_replicon` 149 +
63, `lightyear_transport` 76 + 1, `lightyear_messages` 25 + 3, `lightyear_replication` 61 + 3 + 6 (7 ignored upstream),
`lightyear` 1 + 4 (3 ignored upstream): all pass. The crashes T10 and T12 fix were each confirmed against the upstream
crates by a hostile case before the patch (`handoff/net1/records/hostile/upstream_core_replicon.txt`) and survive after
it (`patched_T10_T12.txt`).

Phase 2 (round `net2`, the same script and features, all eight crates each against the other seven;
`Game-Data/Converted/handoff/net2/records/checks/vendor_tests.txt`): `lightyear_core` 45 (2 doc-tests ignored upstream),
`lightyear_netcode` 32 + 9 (1 ignored upstream), `lightyear_udp` 5, `bevy_replicon` 150 + 63 (T14's test included),
`lightyear_transport` 77 + 1 (T13's test included), `lightyear_messages` 25 + 3, `lightyear_replication` 61 + 3 + 6 (7
ignored upstream), `lightyear` 1 + 4 (3 ignored upstream): all pass. The transport's first run did not compile its tests
(T13's test named `vec!` in the `no_std` crate; the upstream plugin tests, which run the send and receive systems
without `TransportPlugin`, lacked the new `ReliableLimits` resource): T13's test uses `alloc::vec!`, and each of those
tests initialises `ReliableLimits` beside T1's `FragmentLimits`, as `TransportPlugin` does; the rerun is the record's
last line. T13's and T14's behaviour against a running server and client is in `crates/ssx-net/tests/instance.rs`
(`a_connection_that_never_acknowledges_is_kicked`, `a_flood_of_players_stops_at_the_fragment_limit_or_the_entity_cap`).
T15 (after the round's commit, its window check): `lightyear_netcode` 33 + 9 (T15's test included; 1 ignored upstream)
and `lightyear` 1 + 4 (3 ignored upstream) again, each against the other patched crates
(`records/checks/vendor_tests_t15.txt`): all pass.

rav1d (2026-11-07, `Game-Data/Converted/handoff/orchestrator/md9/`): its own tests from its folder, `cargo +stable test
--locked --release --no-default-features --features bitdepth_8,asm` (`+stable`: the crate's `rust-toolchain.toml`
pins `nightly-2025-05-01`, which rustup would otherwise install): 6 + 2 doc-tests pass (the test build prints three
"unnecessary `unsafe` block" warnings of upstream's in `cfg(test)` code; the workspace's build has none). The patched
decoder's pictures are the unpatched one's to the byte: the 65 s 1080p M-D9 movie (6 065 280 000 bytes of YUV, 1 and 4
threads) and the template's movie hash the same without the assembly, with it on the published crate (`bitdepth_16`),
and with it on the patched one; the decoder's sandbox proof and its system-call list pass unchanged (`content.md`
§20.9, §20.13).
