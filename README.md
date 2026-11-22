<h1 align="center">
  <a href="https://projavalanche.com/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/logo-dark.svg">
      <source media="(prefers-color-scheme: light)" srcset="docs/logo-light.svg">
      <img alt="Project Avalanche, the SSX 2012 PC port" src="docs/logo-light.svg" width="600">
    </picture>
  </a>
</h1>

<p align="center"><b>SSX (2012) on PC.</b> A native Windows and Linux port of EA's snowboarding game, written in Rust.</p>

<p align="center">
  <a href="https://github.com/MnkyArts/ProjectAvalanche/releases/latest/download/ProjectAvalanche-windows-x64.zip"><img alt="Download SSX 2012 PC port for Windows" src="https://img.shields.io/badge/download-Windows-0078D4?logo=windows&logoColor=white"></a>
  <a href="https://github.com/MnkyArts/ProjectAvalanche/releases/latest/download/ProjectAvalanche-linux-x64.tar.gz"><img alt="Download SSX 2012 PC port for Linux" src="https://img.shields.io/badge/download-Linux-FCC624?logo=linux&logoColor=black"></a>
  <a href="https://projavalanche.com/"><img alt="Website: projavalanche.com" src="https://img.shields.io/badge/website-projavalanche.com-0A9AC0"></a>
  <a href="https://discord.com/invite/XrFGQEfwdJ"><img alt="Discord" src="https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white"></a>
  <a href="https://github.com/MnkyArts/ProjectAvalanche/actions/workflows/build.yml"><img alt="Build" src="https://github.com/MnkyArts/ProjectAvalanche/actions/workflows/build.yml/badge.svg"></a>
</p>

Project Avalanche is an unofficial, open-source PC port of SSX (2012), EA's snowboarding game for Xbox 360 and
PlayStation 3. It is a reimplementation, not an emulator, a decompilation or a static recompilation: the game's file
formats and behaviour were reverse-engineered from the retail Xbox 360 disc and rebuilt as new code in Rust on the
[Bevy](https://bevy.org) engine. It runs natively on Windows and Linux and sets up its game data from your own Xbox 360
disc image.

The port is built with help from Claude and is playable from start to finish: the boot movies, the menus and a new
profile, World Tour and Explore, the rider physics decoded from the game's own code, the HUD, the music, speech and
sound effects, the results and your saved progress. Players have finished all nine World Tour chapters and ridden
every track on the disc in Explore. The DLC tracks have seen less play. Each release fixes details that still differ
from the console, so please report what you find on the
[issue tracker](https://github.com/MnkyArts/ProjectAvalanche/issues). Behaviour that isn't decoded yet is
approximated and marked in the code (see [Open questions in the code](#open-questions-in-the-code)).

**You need your own copy of the game:** an ISO image dumped from your retail Xbox 360 disc of SSX (2012). No game
data is included in this repository or in its downloads.

**Website:** [projavalanche.com](https://projavalanche.com/) has the [download page](https://projavalanche.com/download/),
a guide to [playing SSX 2012 on PC](https://projavalanche.com/play-ssx-2012-on-pc/), the
[changelog](https://projavalanche.com/changelog/) and the [FAQ](https://projavalanche.com/faq/). The
[wiki](https://github.com/MnkyArts/ProjectAvalanche/wiki) covers hosting a server, writing Luau scripts and making
mods, maps and gamemodes.

## Features

- The whole career (World Tour), and Explore on every track of the disc with medals, leaderboards and your own
  personal best ghost.
- Rider physics decoded from the original executable: carving, tricks, boost and rewinds, checked against the
  console.
- Online drops on the official servers, or on a dedicated server you host yourself (Windows or Linux, headless).
  Servers on your own network show up in the LAN tab.
- The DLC (Mount Eddie, Japan and the bonus packs), title updates 1.02 and 3.02 and the level-15 update, all from
  your own dumps.
- Mods: custom maps, resources and gamemodes with Luau scripts.
- A launcher that sets the game up from your ISO, extracts it once (about 4.7 GB) and keeps itself up to date.
- Gamepad or keyboard, and Discord Rich Presence (you can switch it off).

## SSX 2012 decomp, recomp or emulator?

If you have looked for a way to play SSX (2012) on PC, you have probably come across emulators and the idea of an
SSX 2012 decomp or recomp. Project Avalanche is a different kind of port. The table shows how they compare:

| | How it works | EA's code in the download | Native PC program |
|---|---|---|---|
| Emulator (Xenia, RPCS3) | runs the unchanged console game in software that imitates the Xbox 360 or the PS3 | the console executable, as it is | no, a console is emulated |
| Decomp (decompilation) | turns the game's machine code back into C or C++ source, function by function | source reconstructed from EA's code | yes, once the source is ported |
| Recomp (static recompilation) | translates the console's PowerPC code into C++ ahead of time | EA's code in translated form | yes |
| **Project Avalanche** (reimplementation) | reverse-engineers the formats and the behaviour, then writes a new program that reads your disc's data | **none**: new Rust code under the MIT License | yes, on Windows and Linux |

The original game is still the reference. We read its executable in Ghidra and its scripts and shaders from the
disc to learn how it behaves, then write that behaviour as our own code. That is also why the port can have things
the console version never had: a Linux version, servers you host yourself and mods. More on the website:
[SSX 2012 decomp](https://projavalanche.com/ssx-2012-decomp/) and
[SSX 2012 recomp](https://projavalanche.com/ssx-2012-recomp/).

## Play SSX 2012 on Windows

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

## Play SSX 2012 on Linux

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

## SSX 2012 DLC, title update and level 15

The port plays the DLC, the title update and the level-15 update from **your own** copies of them. Nothing is
downloaded and no game content is shared. All three are in the launcher under **Settings, Game Data** (scroll down
past the disc image and the language). Set the game data up first: the DLC and title-update buttons stay grey until
then. Changes apply the next time the game starts, and nothing needs to be extracted again.

**DLC** (section *Downloadable content*): the Japan range, Mount Eddie, the Presell Pack, and the Eddie and Mac packs.

1. Dump the DLC from your Xbox 360: the content packages (`LIVE`, `CON` or `PIRS` files), as their folder (for
   example `DLC- 4541096D`) or a `.zip` of it.
2. Click **Install folder…** or **Install .zip…** and pick it. One folder or zip can hold several packages; a title
   update inside it is installed as a title update.
3. Each package is checked (its STFS hash tree) and copied to the `DLC` folder in the launcher's data folder
   (`%LOCALAPPDATA%\Project Avalanche\DLC` on Windows, `~/.local/share/project-avalanche/DLC` on Linux). Your
   originals are never changed. A damaged package is not installed, and a package whose data does not decode is
   listed but not loaded.

Each installed package has a switch to turn it off and a **Remove** button (it goes to the system trash). The DLC's
tracks, riders, outfits, boards and texts then show up in the game as on the console. The DLC's Global Events needed
EA's servers and cannot be played.

**Title update** (section *Title update*): versions **3.02** (July 2012) and **1.02** (February 2012). Install it with
**Install package…** (a single file), **Install folder…** or **Install .zip…**. You can install both, but only one
applies: the newest one that is switched on, because each is a complete rebuild, not an add-on. 3.02 brings the
patched menus, scripts and texts, the drop screen's **Freeride** (the run loops through the helicopter pickup), the
3-2-1 GO! heat and a New Zealand track fix. 1.02 changes only code. They go to `DLC/Title Updates` in the same folder.

**Level 15** (section *Level 15 data (tuner pack and profile)*): the level-15 update was not a title update but a
tuner pack that a level-15 profile carries in its save. Click **Install a level-15 save or tuner pack…** and pick the
save (`ssxvprofile`) or the pack. You get 15 rider levels instead of 10, the store's level tables and the changed
drops. The pack is kept as `Tuner/tuner.bin`; a save is also copied over your profile (`Saves/ssxvprofile`) after the
launcher asks you, so back up your own profile first if you want to keep it. **Remove…** deletes only the tuner
file, not the profile.

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

## Online play and dedicated servers

Online drops run on dedicated servers. Join one from the game's Internet tab (the public server list), from the LAN
tab, or by its address. Players and servers need the same version, because the network protocol changes between
releases.

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

## Mods

Custom maps, resources and gamemodes are packages with a `resource.toml`, scripted in Luau. You can install mods for
yourself, and a server can run them for its players. The wiki covers
[installing mods](https://github.com/MnkyArts/ProjectAvalanche/wiki/Installing-Mods),
[making a mod](https://github.com/MnkyArts/ProjectAvalanche/wiki/Making-a-Mod),
[a map](https://github.com/MnkyArts/ProjectAvalanche/wiki/Making-a-Map) and
[a gamemode](https://github.com/MnkyArts/ProjectAvalanche/wiki/Making-a-Gamemode), and
[resources on a server](https://github.com/MnkyArts/ProjectAvalanche/wiki/Server-Resources).

## Discord Rich Presence

While the game runs and the Discord app is open on the same computer, your Discord profile shows what you are doing
in Project Avalanche: the menus, the mode and event, the drop and its mountain (or the map), a timer and your
result, with buttons to get the game and to join its Discord server. The game talks only to the Discord app on your
computer and sends nothing else: not your name, your save, your rider or the server you play on. It is on by
default; **Settings, Discord** turns it off (the game then does not connect to Discord at all). A change applies the
next time the game starts.

## Controls

An Xbox-style gamepad with the game's own layout is recommended: the game was made for a pad. The keyboard
works too; the launcher lists the keys under **Settings, Controls**:

| | Keys |
|---|---|
| In the menus | Enter = START, Space = A, Backspace = B, X, Y, arrow keys = D-pad, Q / E = LB / RB, W A S D = left stick (the globe), F1 skips a movie |
| On the mountain | arrow keys or W A S D = left stick (steer, tuck, brake), Numpad 8 / 4 / 6 / 2 = right stick, Numpad 5 = right stick click (classic controls: oxygen), Space = A (jump), I / J / L / K = Y / X / B / A, Shift = RT, Ctrl = LT, E = RB (standard controls: wingsuit and oxygen), Backspace = LB (rewind), Esc = START (pause) |

## SSX 2012 on PC: questions

### Can I play SSX 2012 on PC?

Yes. SSX (2012) came out only on Xbox 360 and PlayStation 3, and EA never released a PC or Steam version. Project
Avalanche runs it natively on Windows and Linux. You need an ISO image of your own retail Xbox 360 disc; the
launcher sets the game data up from it.

### Should I use Xenia or RPCS3 instead?

An emulator such as Xenia (Xbox 360) or RPCS3 (PlayStation 3) runs the console version inside an emulated console.
Project Avalanche runs SSX (2012) as a native Windows or Linux program, with online play on dedicated servers and
mods. It reads an Xbox 360 disc image; the PlayStation 3 version is not used.

### Is there an SSX 2012 decomp or recomp?

Project Avalanche is neither. It contains no decompiled EA code and no PowerPC code translated from the Xbox 360
executable. If you were looking for a decomp or a recomp to play the game natively on PC, this gives you that result
without shipping EA's code. The table in [SSX 2012 decomp, recomp or emulator?](#ssx-2012-decomp-recomp-or-emulator)
shows the difference.

### Is this related to the SSX Tricky or SSX 3 decomps?

No. The decompilation and recompilation projects for the earlier SSX games on PlayStation 2 are separate efforts.
Project Avalanche covers SSX (2012) only.

### Which systems does it run on?

Windows x64 and Linux x86-64, plus a headless dedicated server for both. There is no macOS build at the moment.

### Is Project Avalanche affiliated with EA?

No. It is an independent community project, not affiliated with, endorsed or sponsored by Electronic Arts.

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

The checks the CI runs (a snapshot also builds the Windows and the Linux download, and a `v*` tag releases
both):

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

## Repository layout

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
| `crates/ssx-discord` | Discord Rich Presence: the Discord app's local connection (a Unix socket or a Windows named pipe), the activity the game shows, a thread of its own within Discord's rate limit; every reply read as hostile input; off unless the launcher's switch is on |
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
  contributors): the community's research into the SSX (2012) formats on the PS3 disc: the BIG archives, the
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
  animation and collision formats: the ABIN animation importer from
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
