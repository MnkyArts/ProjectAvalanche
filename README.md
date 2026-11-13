# Project Avalanche

[![build](https://github.com/MnkyArts/ProjectAvalanche/actions/workflows/build.yml/badge.svg)](https://github.com/MnkyArts/ProjectAvalanche/actions/workflows/build.yml)
[![Download for Windows](https://img.shields.io/badge/download-Windows-0078D4?logo=windows&logoColor=white)](https://github.com/MnkyArts/ProjectAvalanche/releases/latest/download/ProjectAvalanche-windows-x64.zip)
[![Download for Linux](https://img.shields.io/badge/download-Linux-FCC624?logo=linux&logoColor=black)](https://github.com/MnkyArts/ProjectAvalanche/releases/latest/download/ProjectAvalanche-linux-x64.tar.gz)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.com/invite/XrFGQEfwdJ)

A ground-up native PC port of **SSX (2012)**, rebuilt in Rust on the [Bevy](https://bevy.org) engine with help from Claude — no
emulation, no recompilation. The original formats are reverse-engineered from the retail Xbox 360 disc and
re-implemented as independent Rust code, and so is the game's behaviour, read from the game's own code.

**Status:** work in progress. The game runs in one window: the boot movies, the menus and a new profile, World
Tour and Explore, the drops with the rider physics decoded from the game's code, the HUD, the music, speech and
sound effects, the results, and the saved progress. Nobody has played the whole game through yet, so expect bugs
and unfinished parts. The launcher sets the game data up from your own disc image. What is not decoded yet is
approximated and marked in the code
(see [Open questions in the code](#open-questions-in-the-code)).

**You need your own copy of the game:** an ISO image dumped from your retail Xbox 360 disc of SSX (2012). No
game data is included in this repository or in its builds.

**Servers, scripts and mods:** the [wiki](https://github.com/MnkyArts/ProjectAvalanche/wiki) covers hosting a server,
writing Luau scripts and making mods, maps and gamemodes.

## Play on Windows

1. Download the Windows build (the `ProjectAvalanche-windows-x64` artifact of a CI run, or a release) and
   unpack it anywhere: `Project Avalanche.exe`, `avalanche-game.exe` (the game), `avalanche-movie.exe` (the game's
   decoder of mods' movies, which it starts in a sandbox of its own) and the `fonts` folder must stay in the same
   folder.
2. Start **Project Avalanche.exe**. On the first start, click the large **SET UP GAME DATA** tile, then
   **Select SSX (2012) Xbox 360 ISO…**, and pick your own disc image (an `.iso` dumped from the retail Xbox 360
   disc). The image is only read, never changed.
3. Click **Extract game data**. The game's archives and movies are extracted once (about 4.7 GB) to
   `%LOCALAPPDATA%\Project Avalanche\GameData` (or a folder you choose); you can cancel and resume later.
4. Click the large **PLAY SSX (2012)** tile (the launcher returns to it when the setup is done). The game starts
   in its own window; progress is saved to `%LOCALAPPDATA%\Project Avalanche\Saves\ssxvprofile`. The launcher
   stays open and shows the game's output under **Extras, Output**.
5. **Extras, Tools** starts the asset viewer (fly camera) or a test ride on a single track, drop and rider:
   development tools that need `asset-viewer.exe`, built from the source, next to the launcher; it is not in
   the download.

Every choice is remembered in `%LOCALAPPDATA%\Project Avalanche\settings.json`. The launcher's pictures come
from your own game data once it is set up; your own PNGs in `%LOCALAPPDATA%\Project Avalanche\LauncherArt\`
replace them.

## Play on Linux

1. Download the Linux build (the `ProjectAvalanche-linux-x64` artifact of a CI run, or a release) and
   unpack `ProjectAvalanche-linux-x64.tar.gz` into a folder of its own (it has no top-level folder:
   `mkdir ProjectAvalanche && tar -xzf ProjectAvalanche-linux-x64.tar.gz -C ProjectAvalanche`):
   `project-avalanche` (the launcher), `avalanche-game` (the game), `avalanche-movie` (the game's decoder of mods'
   movies) and the `fonts` folder must stay in the same folder.
2. Start `./project-avalanche` and set up the game data as on Windows (steps 2 to 4 above). The data goes to
   `~/.local/share/project-avalanche/GameData` (`$XDG_DATA_HOME/project-avalanche`; `PROJECT_AVALANCHE_HOME`
   overrides it), the progress to `~/.local/share/project-avalanche/Saves/ssxvprofile`.

It needs a GPU with a Vulkan driver, ALSA for sound (PipeWire and PulseAudio provide it), Wayland or X11, and the
XDG desktop portal for the ISO file picker. The build is made on Ubuntu 24.04 (x86-64) and runs on distributions
with that glibc or newer.

## Updates

The launcher checks the project's [latest release](https://github.com/MnkyArts/ProjectAvalanche/releases/latest) when
it starts; a newer version is shown under **Extras, Updates** (the running version is in the footer and on
**Extras, About**). It downloads the release's archive, checks it against the SHA-256 GitHub publishes, unpacks it
beside the launcher and restarts. Your settings, saves and game data stay where they are; only the launcher, the
game, the server and the `fonts` folder's files are replaced. `--no-update-check` (or `SSX_NO_UPDATE_CHECK=1`) turns
the check off.

**After an update from an older version.** Game data set up by v0.2.0 or earlier has no content index, which joining a
server needs: the launcher says so and **Settings, Game Data, Repair game data** reads your disc image again, keeps
every file that is already right and writes only what is missing or differs. An installation updated by launcher
0.2.0 lacks `avalanche-movie` and the `fonts` folder: **Extras, Updates, Repair the installation** downloads this
version's release again and puts them back the same verified way.

The downloads above always take the latest release: the Windows `.zip` for a manual install, the Linux
`.tar.gz`; the launcher uses its own `ProjectAvalanche-windows-x64.tar.gz` /
`ProjectAvalanche-linux-x64.tar.gz` assets.

## Host a server

**On your computer:** the launcher's **HOST** page sets up a server folder (the rules it needs come from your own game
data, about 21 MB), starts the dedicated server as its own program, shows its output, joins it with the game and stops
it with the launcher. It is on the game's Internet tab (the project's master server) unless you switch off **Listed on
the public server list**; friends on your network find it in the LAN tab, and anyone can join by your address.

**On a server machine:** the dedicated server (`ProjectAvalanche-server-windows-x64` / `ProjectAvalanche-server-linux-x64`,
or `cargo build --release -p avalanche-server`) runs headless: no GPU, sound or display.

1. `avalanche-server setup /path/to/your/ssx.iso --out data` takes the rules from your own disc image (21 MB of it).
2. Copy `server.toml.example` to `server.toml`, edit it, and run `avalanche-server check`.
3. Start `avalanche-server`. It listens on port 27450 (UDP for the game, TCP for the join) and 27451 (UDP, the query);
   `bind = "::"` listens on IPv6 and IPv4; `[port_mapping] enabled = true` maps the ports on your router (off by
   default). It is listed on the public server list unless `[discovery] public = false` (then it sends the master
   server no heartbeat; every server follows the master's resource blocklist). It makes its keys in `data/` at the first start and logs to `data/logs/server.log`.

A systemd unit, a Windows service note and a Docker example are in `Engine/assets/server/deploy/` (the systemd unit
and the Windows note are in the server downloads too).

**A master server** (the list of public servers; built from the source, not in the downloads): on
[Coolify](https://coolify.io), create a resource from this repository, keep the build pack **Docker Compose** (or
choose **Dockerfile**), set the domain, and press Deploy. `docker-compose.yaml` and `Dockerfile` at the root are
configured for Coolify's proxy, which holds the certificate. On a plain host, `Engine/assets/master/deploy/` has a
systemd unit, a certbot hook and a Compose file for direct TLS.

## Controls

An Xbox-style gamepad with the game's own layout is recommended: the game was made for a pad. The keyboard
works too; the launcher lists the keys under **Settings, Controls**:

| | Keys |
|---|---|
| In the menus | Enter = START, Space = A, Backspace = B, X, Y, arrow keys = D-pad, Q / E = LB / RB, W A S D = left stick (the globe), F1 skips a movie |
| On the mountain | arrow keys or W A S D = left stick (steer, tuck, brake), Space = A (jump), I / J / L / K = Y / X / B / A, Shift = RT, Ctrl = LT, E = RB, Backspace = LB (rewind), Esc = START (pause) |

## Build from source

You need Rust (stable, 1.97.1 or newer, from [rustup](https://rustup.rs)), a C++ compiler (Luau, the scripts' engine,
is built from source) and [NASM](https://nasm.us) 2.14 or newer on the `PATH` (the mod movies' AV1 decoder assembles
its x86-64 kernels with it). On Linux, Bevy also needs a few system libraries; on Debian or Ubuntu (`build-essential`
brings the C++ compiler):

```sh
sudo apt-get install build-essential pkg-config libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev \
  libgtk-3-dev nasm
```

On Windows: `winget install NASM.NASM` (or `choco install nasm`), then add `C:\Program Files\NASM` to the `PATH`.

Then, in `Engine/`:

```sh
cargo build --release -p avalanche-game -p avalanche-launcher -p ssx-movie   # the game, the launcher, the movie decoder
cargo build --release -p asset-viewer                           # optional: the asset viewer
cargo build --release -p avalanche-server                       # optional: the dedicated server (headless)
target/release/project-avalanche                                # the launcher: set up the game data, then Play
```

The launcher expects `avalanche-game` (and `asset-viewer`) next to it in `target/release/`, and the game its movie
decoder `avalanche-movie` (without it a mod's movie is refused with that reason). On Linux it keeps
its data in `~/.local/share/project-avalanche` (`$XDG_DATA_HOME/project-avalanche`; `PROJECT_AVALANCHE_HOME`
overrides it). Once the data is set up, the game also starts on its own:
`target/release/avalanche-game --big-root <data folder>/GameData/big` (`--help` lists the options).

The checks the CI runs on every push (it also builds the Windows and the Linux download; a `v*` tag makes a
release with both):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p ssx-movie --all-targets --features sandbox-probe -- -D warnings
cargo test --workspace
cargo test -p ssx-movie --features sandbox-probe --test sandbox   # the movie decoder's sandbox (Linux x64, Windows)
```

## Open questions in the code

Comments marked `TODO(guess)` mark behaviour that is approximated rather than decoded from the original game;
each names the game-code address or the open question that would settle it. Contributions that decode one are
welcome. Comments marked `PORT(choice)` record the port's own decisions where the original has nothing to decode
(the PC's input and windows, the launcher, mods and online play), each with what the console had instead.

## Contributing

Bug reports, accuracy reports and decoded guesses are welcome: see [CONTRIBUTING.md](CONTRIBUTING.md), the
[Code of Conduct](CODE_OF_CONDUCT.md) and the [security policy](SECURITY.md). Questions and help with the setup:
the [Discord server](https://discord.com/invite/XrFGQEfwdJ).

## Layout

The Rust workspace is in `Engine/`:

| Path | |
|---|---|
| `apps/game` | the game, `avalanche-game`: movies, front end, levels, HUD and audio in one window |
| `apps/launcher` | the launcher, `project-avalanche`: the game data setup from your disc image, the options, Play |
| `apps/asset-viewer` | the asset viewer, `asset-viewer`: a track's terrain and objects with a fly camera, test rides (a development tool) |
| `apps/avalanche-server` | the dedicated multiplayer server, `avalanche-server`: headless, the join over HTTP, the game over UDP, `setup` of the rules from your own disc image, `check`, IPv6, port mapping (opt-in) |
| `apps/avalanche-master` | the master server, `avalanche-master`: the list of public servers (authenticated heartbeats, a reachability check, expiry, blocklists); the project runs it at `https://masterlist.liamrbsn.com`, the one master every program uses (no setting names another) |
| `crates/ssx-core` | the foundations every other crate shares: the world's conventions and units, common identifiers, how text from outside the program is shown (no Bevy) |
| `crates/ssx-files` | the files the programs write, replaced whole or not at all across a crash or a power cut: a partial file synced, renamed over the old one, the folder synced |
| `crates/ssx-formats` | parsers for the disc's file formats |
| `crates/ssx-assets` | the game data: extraction from the disc image and conversion |
| `crates/ssx-physics` | the collision world and the rider's physics |
| `crates/ssx-rules` | the game's rules data for the simulation and the dedicated server: the tracks' vault records, placements, locators and trigger volumes, the riders' tuning, the events and the trick table |
| `crates/ssx-lua` | a Lua 5.1 implementation for the game's scripts |
| `crates/ssx-ui` | the Scaleform player: the menus and the HUD |
| `crates/ssx-bevy` | the game on Bevy: rendering, audio, the game shell |
| `crates/ssx-content` | the content system for mods: content ids, the layered registry, package manifests (`resource.toml`), record patches, the packages' checks |
| `crates/ssx-movie` | the mods' movies: AV1 pictures through rav1d in a program of its own (`avalanche-movie`) inside the operating system's sandbox, Vorbis sound through symphonia, read by `ssx-content`'s WebM reader |
| `crates/ssx-gameplay` | the multiplayer rules the server applies: instances and their rounds, the chat's and the poses' checks |
| `crates/ssx-script` | the mods' Luau scripts: the sandboxed runtime on `mlua` (Luau), the budgets, the API, the client host the game runs |
| `crates/ssx-net-proto` | the multiplayer wire: the pose sample, the join's sealed request and answer, the keys, the names |
| `crates/ssx-net` | the multiplayer network layer on lightyear: the protocol, the server's sessions and guards, the client |
| `crates/ssx-net-join` | the client's join: the player's identity key, the servers' keys trusted on first use, the join over HTTP |
| `crates/ssx-net-ops` | the dedicated server's operation: the ban store and the operator's mutes (`bans.toml`), the access control list (`acl.toml`) with its groups and resources' permission requests, the console's command grammar; every file read strictly and written whole |
| `crates/ssx-net-lan` | the machine's network interfaces for LAN discovery (broadcast addresses, on-link checks) |
| `crates/ssx-net-portmap` | the dedicated server's port mapping on the router: UPnP IGD, PCP and NAT-PMP (opt-in), every answer bounded and checked |
| `vendor/` | eight third-party crates (lightyear's, bevy_replicon) as published, with the fixes their files mark "Project Avalanche patch" (`vendor/PATCHES.md`); the workspace uses them through `[patch.crates-io]` |

## AI Disclaimer

AI tools were used to assist with parts of the development and documentation of this project.

- Findings are verified against the retail disc wherever possible, including whole-disc verification for every parser, cross-checks against the game's code, and comparisons with reference decoders. However, errors may still exist. Treat the code and documentation as works in progress. Assumptions that have not yet been fully decoded are marked with `TODO(guess)` in the code.
- All analysis was performed using the maintainer's own copy of the game. No original game data, assets, or proprietary code are included in this repository or its builds.

### A Final Note

The use of AI in this project is intentional, and we have no plans to apologize for it or justify our choice of development tools.

If AI involvement is a dealbreaker for you, that's perfectly fine. There are other projects to explore, and you're always welcome to create your own alternative.

We welcome constructive feedback, bug reports, and technical discussions. Personal opinions about which tools developers should or shouldn't use, however, are not a project concern.

Nobody is required to use this port, and nobody needs anyone else's approval to build it. :)

## Credits

Built on the work of others. Their code was read as a lead or used as a reference to check against; every claim
was re-checked on the retail disc and the port's code is our own.

SSX community:

- [SSX-Library](https://github.com/GlitcherOG/SSX-Library) and
  [SSX Collection Multitool](https://github.com/GlitcherOG/SSX-Collection-Multitool) (GlitcherOG and
  contributors): the community's research into the SSX (2012) formats on the PS3 disc — the BIG archives, the
  GEOM models, the attribute vaults, the skinned models' scene files (CRSF).
- [bigfile](https://github.com/SSXModding/bigfile) and [SSXTools](https://github.com/SSXModding/SSXTools)
  ([SSXModding](https://github.com/SSXModding)): EA BIG archives and RefPack compression.
- The SSX wiki at [ssx.computernewb.com](https://ssx.computernewb.com/wiki/Formats/Common:BIG): the BIG
  format.

Xbox 360:

- [Xenia](https://github.com/xenia-canary/xenia-canary) (xenia-canary): the GPU's texture tiling and formats,
  XPR, LZX, XEX loading, the XMA packet headers, the kernel import tables, and the PowerPC / VMX128 opcode
  tables our disassembler uses.
- [Velocity](https://github.com/hetelek/Velocity) (hetelek): STFS and XEX notes.
- [extract-xiso](https://github.com/XboxDev/extract-xiso) (XboxDev): the lead for our XDVDFS disc reader, and
  its cross-check.
- [XeXtractor](https://github.com/XboxChef/XeXtractor) (XboxChef): the XEX2 structures, the base file's
  decryption and decompression.

Audio and video:

- vgmstream: the EA audio containers (SNR / SNS, ABK / AST, SBR / SBS, the Pathfinder MPF / MUS), EALayer3
  and the EA movies' audio; through it, Zench's ealayer3.
- FFmpeg: the EA movie container, the VP6 decoder and its codec tables, the WMA Pro / XMA decoder (read for
  the algorithm); and the reference our movie and MP3 decoding was checked against.
- mpg123, minimp3 and nanomp3 (a minimp3 port): reference decoders for our MPEG audio decoding.
- symphonia (`symphonia-bundle-mp3`): its tables were the lead when transcribing the MP3 layer III tables.

Flash and Scaleform:

- The SWF 10 file format specification (Adobe): the tags, the shapes and the AVM1 actions.
- Ruffle: its AVM1 implementation as reading material; Ruffle's and the JPEXS Free Flash Decompiler's tag
  lists for Scaleform's own tags.

Other references:

- Lua 5.1 (Lua.org, PUC-Rio): the reference manual and the sources our Lua implementation follows; the
  `lua5.1` interpreter produced reference outputs for the tests.
- Microsoft's DDS documentation (`DDS_HEADER`, `DDS_PIXELFORMAT`).
- libmspack: the LZX conventions our LZX decoder was compared with; the `lzxd` crate: the cross-check of our
  LZX decoder.

Reverse engineering:

- Ghidra (NSA) for the analysis of the game's executable, served to our tools by
  [GhidraMCP](https://github.com/bethington/ghidra-mcp).
- [ImHex](https://github.com/WerWolv/ImHex) (WerWolv) and [Kaitai Struct](https://github.com/kaitai-io/kaitai_struct):
  hex and structure work.
- [skate-3-rust-engine](https://github.com/SK8-ENGINE/skate-3-rust-engine) (SK8-ENGINE): ideas from a
  comparable Rust / Bevy port of an EA Xbox 360 game; the Skate 3 tools it carries were leads for the
  animation and collision formats — the ABIN animation importer from
  [skate-3-trick-toolkit](https://github.com/chasmlol/skate-3-trick-toolkit) (chasmlol) and the collision mesh
  reader of the Skate 3 Custom Engine Layer contributors.

The engine:

- [Bevy](https://bevy.org), egui / eframe (the launcher), and the other Rust crates listed in
  `Engine/Cargo.lock`.
- The launcher's font is Saira Condensed, under the SIL Open Font License
  (`Engine/apps/launcher/assets/fonts/OFL.txt`).
- The fallback fonts the game and the launcher draw other scripts with are the Noto fonts (Noto Sans, Arabic,
  Hebrew, Devanagari, Thai and Noto Sans CJK JP), under the SIL Open Font License (`Engine/assets/fonts/OFL.txt`,
  their sources in `Engine/assets/fonts/fonts.toml`).

## License

The port's own code is under the [MIT License](LICENSE). That covers our code only: SSX (2012) and everything on
its disc belong to Electronic Arts and are not part of this repository or its builds; the launcher's font (Saira
Condensed) is under the SIL Open Font License (`Engine/apps/launcher/assets/fonts/OFL.txt`), and so are the Noto
fallback fonts (`Engine/assets/fonts/OFL.txt`); the Rust crates we build on keep their own licenses.

Unofficial project, not affiliated with Electronic Arts. SSX is a trademark of Electronic Arts.
