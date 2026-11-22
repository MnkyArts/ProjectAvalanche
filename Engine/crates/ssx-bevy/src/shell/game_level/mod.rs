use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bevy::camera::Hdr;
use bevy::prelude::{
    App, ButtonInput, Commands, DetectChangesMut, Entity, FixedUpdate, Has, IntoScheduleConfigs, KeyCode, Msaa, Mut,
    PostUpdate, Query, Res, ResMut, Resource, Update, With, Without, World, info, resource_exists, warn,
};
use bevy::ui::IsDefaultUiCamera;
use ssx_formats::csvdb::Geotag;
use ssx_physics::event::EventResult;
use ssx_ui::game::fe::Launch;
use ssx_ui::game::fe::host::Step;
use ssx_ui::game::fe::pause::GameRequest;
use ssx_ui::game::feed::HudFeed;
use ssx_ui::game::input::Button;

use crate::camera::FlyCam;
use crate::ride::{Ride, RideStepSet};
use crate::shell::game_mode::{self, GameUiRes};
use crate::ui;

mod frame;
mod pause;
mod prompt_player;
mod results;
mod step;

pub use frame::level_frame;
pub use pause::pause_frame;
pub use results::stats_from_ride;
pub use step::nis_state;
use step::{feed_step, prompt_step};

pub struct LevelOptions {
    pub launch: Launch,
    pub stat_names: std::sync::Arc<std::collections::BTreeMap<u32, String>>,
    pub big_root: PathBuf,
    pub shots: Vec<(f32, PathBuf)>,
    pub stop_after: Option<f32>,
    pub script: String,
    pub shot_dir: PathBuf,
    pub debug_hud: bool,
    pub autoplay: bool,
    pub follow_prompts: bool,
    pub win_after: Option<f32>,
}

pub struct LevelLoad {
    opts: LevelOptions,
    rivals: ssx_ui::game::ride::DropRivals,
    geotags: Vec<Geotag>,
    script: Vec<(f64, Step)>,
    rival_values: Vec<u32>,
    bot_scores: Vec<u32>,
}

impl LevelLoad {
    pub fn load(opts: LevelOptions, data: &crate::shell::level::LevelData) -> Result<LevelLoad> {
        let def = data
            .ride
            .as_ref()
            .filter(|_| opts.launch.map.is_none())
            .and_then(|r| r.ride().world().events.iter().find(|e| e.key == opts.launch.drop).cloned());
        let rivals = ssx_ui::game::ride::drop_rivals(&opts.big_root, def.as_ref());
        for p in &rivals.problems {
            warn!("level: {p}");
        }
        let geotags = if opts.launch.map.is_some() {
            Vec::new()
        } else {
            match ssx_assets::track_objects::geotags::load_rows(&opts.big_root, &opts.launch.track) {
                Ok(rows) => rows.unwrap_or_default(),
                Err(e) => {
                    warn!("level: the geotags of {} did not load: {e:#}", opts.launch.track);
                    Vec::new()
                }
            }
        };
        let script = ssx_ui::game::fe::host::parse_script(&opts.script).context("the level's script")?;
        let rival_values = def.as_ref().map_or_else(Vec::new, |d| d.rivals.iter().map(|r| r.value()).collect());
        let bot_scores = match (def.as_ref(), data.ride.as_ref()) {
            (Some(d), Some(r)) => ssx_ui::game::ride::bot_scores(d, r.ride().session.bots.as_ref()),
            _ => Vec::new(),
        };
        Ok(LevelLoad { opts, rivals, geotags, script, rival_values, bot_scores })
    }
}

#[cfg(feature = "scripts")]
impl LevelGame {
    pub(crate) fn script_view(&self) -> (&HudFeed, &Launch) {
        (&self.feed, &self.launch)
    }

    pub(crate) fn script_ghost(&self, id: u64, seconds: f32) -> Option<(usize, [f32; 3], [f32; 3], bool)> {
        let (k, r) = self.ghosts.replays.iter().find(|(_, r)| u64::from(r.id) == id)?;
        if r.finished(seconds) {
            return None;
        }
        let pose = r.pose(seconds);
        let air = pose.flags & ssx_formats::ghost::FLAG_IN_AIR != 0;
        Some((*k, r.position(seconds), pose.velocity, air))
    }
}

#[derive(Resource)]
pub struct LevelGame {
    launch: Launch,
    stat_names: std::sync::Arc<std::collections::BTreeMap<u32, String>>,
    badges_seen: usize,
    feed: HudFeed,
    ghosts: ssx_ui::game::ride::RivalGhosts,
    ghosts0: ssx_ui::game::ride::RivalGhosts,
    feed0: HudFeed,
    geotag_rows: Vec<Geotag>,
    fed: u64,
    fed_paused: u64,
    result: Option<EventResult>,
    results_posted: bool,
    up: bool,
    ready_frames: u32,
    spawned: Instant,
    shots: Vec<(f32, PathBuf)>,
    frame_ms: Vec<f64>,
    ui_ms: Vec<f64>,
    next_log: f32,
    release: Vec<(Button, u8)>,
    script_a_until: Option<f64>,
    script_back_until: Option<f64>,
    script_bumper_until: Option<(KeyCode, f64)>,
    stick_dpad: game_mode::StickDpad,
    requests: Vec<GameRequest>,
    restart_at: Option<u32>,
    restarts: u32,
    ui_frames: u32,
    script: Vec<(f64, Step)>,
    clock: f64,
    shot_dir: PathBuf,
    stop_after: Option<f32>,
    stopped: bool,
    ride_seconds: f32,
    save_due: bool,
    ghost_due: Vec<(u32, u32)>,
    paused: bool,
    sim_hold: Option<ssx_ui::game::sim_hold::SimHold>,
    autoplay: bool,
    popup_since: Option<f64>,
    popup_presses: u32,
    popup_text: Option<String>,
    flags: std::collections::BTreeSet<u32>,
    prompts: Option<prompt_player::PromptPlayer>,
    win_after: Option<f32>,
    rival_values: Vec<u32>,
    bot_scores: Vec<u32>,
    live: bool,
    live_rounds: bool,
}

pub struct KeptGhost {
    pub drop: u32,
    pub score: u32,
    pub file: Option<ssx_formats::ghost::GhostFile>,
}

fn header_gamertag(name: &str) -> String {
    let name = name.split('\0').next().unwrap_or_default();
    // PORT(choice): the name is cut at a character boundary at most 0x30 bytes long, so the header holds valid UTF-8;
    // an Xbox gamertag is at most 15 ASCII characters, and the console cuts the bytes.
    let mut end = name.len().min(ssx_formats::ghost::GAMERTAG_MAX - 1);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

fn header_gear(setup: &ssx_ui::game::fe::setup::GameSetup) -> u16 {
    const HEAD_LAMP: usize = 6;
    const ICE_AXE: usize = 2;
    u16::from(!setup.gear[HEAD_LAMP].is_cleared()) | u16::from(!setup.gear[ICE_AXE].is_cleared()) << 1
}

// PORT(choice): a restart never ends the wait: the kept run's save waits on into the restarted run and takes its
// recording at that run's first halt (a pause, the failed results, the stop 3 s after its results) or at the level's
// exit, under the first run's score. The console's task polls +0x3C every 1 ms on a worker (0x82540F80…0x82540F9C:
// `FUN_82356C38` → SleepEx) while the restart (`GhostManager_OnRestart` 0x824B5B90) sets it to −1 (`FUN_82538200`) and
// back to 0 within one handler (`FUN_82538150`, after clearing the streams); the port's frame has no such thread, so
// the poll never lands between the two. Two kept runs waiting at once (the restarted run's own new best) take the same
// halt's recording and are saved in the order kept. The console had a thread race whose rare other side deflates the
// buffer while it is being cleared, and the two deflate jobs in its scheduler's (`[0x838AB2E4]`+4) worker order.
fn ghost_saves_due(due: &mut Vec<(u32, u32)>, halted: bool, leaving: bool) -> Vec<(u32, u32)> {
    if halted || leaving { std::mem::take(due) } else { Vec::new() }
}

impl LevelGame {
    pub fn is_up(&self) -> bool {
        self.up
    }

    pub fn takes_input(&self) -> bool {
        self.up && !self.results_posted
    }

    pub fn results_shown(&self) -> bool {
        self.results_posted
    }

    pub fn result(&self) -> Option<&EventResult> {
        self.result.as_ref()
    }

    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    pub fn test_run_done(&self) -> bool {
        self.shots.is_empty() && (self.script.is_empty() || !self.live)
    }

    #[cfg(feature = "net")]
    pub fn session_results_shown(&mut self, shown: bool) {
        self.results_posted = shown;
    }

    #[cfg(feature = "net")]
    pub fn set_session_hud(&mut self, hud: Option<ssx_ui::game::session_board::SessionHud>) {
        self.feed.session = hud;
    }

    pub fn ghost_pending(&self) -> bool {
        !self.ghost_due.is_empty()
    }

    pub fn take_save(&mut self) -> bool {
        std::mem::take(&mut self.save_due)
    }

    pub fn take_ghost(&mut self, ride: &Ride, gamertag: &str, leaving: bool) -> Vec<KeptGhost> {
        let rec = &ride.session.recorder;
        let due = ghost_saves_due(&mut self.ghost_due, rec.halted(), leaving);
        if leaving && !rec.halted() && !due.is_empty() {
            info!("level: the level is left before the ghost recorder stops: its recording is kept as it stands");
        }
        due.into_iter()
            .map(|(drop, score)| {
                let header = ssx_formats::ghost::UserHeader {
                    gamertag: header_gamertag(gamertag),
                    gear_flags: header_gear(&self.launch.setup),
                    score,
                    build: ssx_formats::ghost::RECORDER_BUILD,
                };
                KeptGhost { drop, score, file: rec.recording(header) }
            })
            .collect()
    }

    pub fn popup_presses(&self) -> u32 {
        self.popup_presses
    }

    pub fn request(&mut self, r: GameRequest) {
        self.requests.push(r);
    }

    pub fn ui_cpu(&mut self, d: Duration) {
        if self.up {
            self.ui_ms.push(d.as_secs_f64() * 1000.0);
        }
    }

    pub fn summary(&self) -> String {
        let n = self.frame_ms.len();
        let avg = self.frame_ms.iter().sum::<f64>() / n.max(1) as f64;
        let ui = self.ui_ms.iter().sum::<f64>() / self.ui_ms.len().max(1) as f64;
        let result = self.result.as_ref().map_or_else(
            || "no result".to_string(),
            |r| {
                format!(
                    "time {} ms, points {}, place {:?}, medal {:?}, failed {}",
                    r.time_ms,
                    r.points,
                    r.rank.map(|k| k + 1),
                    r.medal,
                    r.failed
                )
            },
        );
        format!(
            "{}: ride {:.1} s, {result}; frame time {avg:.2} ms average over {n} frames, the UI's CPU {ui:.2} ms",
            self.place(),
            self.ride_seconds
        )
    }

    pub fn place(&self) -> String {
        let on = if self.launch.track.is_empty() { "a new track" } else { self.launch.track.as_str() };
        format!("{} on {on}", self.launch.event)
    }
}

pub fn register(app: &mut App) {
    let up = resource_exists::<LevelGame>;
    app.add_systems(FixedUpdate, prompt_step.before(RideStepSet).run_if(up))
        .add_systems(FixedUpdate, feed_step.after(RideStepSet).run_if(up))
        .add_systems(Update, feed_step.after(RideStepSet).before(level_frame).run_if(up))
        .add_systems(Update, camera_type_to_fe.run_if(up))
        .add_systems(PostUpdate, sync_ui_camera.run_if(up));
    crate::shell::level::scoped::<LevelGame>(app);
    crate::shell::level::scoped::<CameraTypeLink>(app);
}

#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct CameraTypeLink {
    pub restore: Option<i32>,
    pub current: Option<i32>,
}

fn camera_type_to_fe(link: Option<Res<CameraTypeLink>>, mut ui: ResMut<GameUiRes>) {
    let Some(t) = link.and_then(|l| l.current) else { return };
    if let Some(fe) = ui.0.fe.as_mut()
        && fe.camera_type != Some(t)
    {
        info!("level: the camera's INT_CameraType is {t}");
        fe.camera_type = Some(t);
    }
}

pub fn end(world: &mut World) {
    world.resource_scope(|world, mut keys: Mut<ButtonInput<KeyCode>>| {
        if let Some(p) = world.get_resource_mut::<LevelGame>().as_mut().and_then(|l| l.prompts.as_mut()) {
            p.release(keys.bypass_change_detection());
        }
        if world.get_resource::<LevelGame>().is_some_and(|l| l.script_a_until.is_some()) {
            keys.bypass_change_detection().release(KeyCode::Space);
        }
        if world.get_resource::<LevelGame>().is_some_and(|l| l.script_back_until.is_some()) {
            keys.bypass_change_detection().release(KeyCode::KeyC);
        }
    });
}

pub fn start(world: &mut World, load: LevelLoad) {
    let LevelLoad { opts, rivals, geotags, script, rival_values, bot_scores } = load;
    let event_type =
        world.get_resource::<Ride>().and_then(|r| r.session.event.setup.event_type).map_or(-1, |t| t as i32);
    let event_type = if opts.launch.map.is_some() { opts.launch.event_type as i32 } else { event_type };
    let mut badges_seen = 0;
    let mut flags = std::collections::BTreeSet::new();
    if world.contains_resource::<Ride>() {
        world.resource_scope(|world, mut ride: Mut<Ride>| {
            if let Some(fe) = world.resource::<GameUiRes>().0.fe.as_ref() {
                ssx_ui::game::stats::into_session(&mut ride.session.stats, &fe.profile, &opts.stat_names);
                ride.session.seed_named_flags(fe.profile.named_flags.iter().copied());
                flags = fe.profile.named_flags.clone();
                info!("level: the flag store: {} permanent named flags from the profile", flags.len());
                badges_seen = ride.session.stats.awarded[2].len();
                info!(
                    "level: the stats store: {} persistent stats, {} badges from the profile",
                    ride.session.stats.maps[ssx_physics::stats::PERSISTENT].len(),
                    ride.session.stats.awarded[2].len()
                );
            }
            ride.build_scripts_ahead();
        });
    }
    let launched = ssx_ui::game::ride::game_setup(&opts.launch.setup);
    let in_session = world.get_resource::<crate::shell::live::LiveSession>().is_some();
    let handed = match (&opts.launch.setup.presentation, in_session) {
        (Some(p), true) => ssx_ui::game::ride::game_setup(p),
        _ => launched.clone(),
    };
    if let Some(ride) = world.get_resource::<Ride>()
        && launched.character != 0
        && ride.game_setup.as_ref().is_none_or(|g| g.items != handed.items)
        && std::env::var("SSX_RIDE_LOADOUT").is_err()
        && std::env::var("SSX_RIDE_GEAR").is_err()
    {
        warn!("level: the ride was not handed the launch's game setup (`RideOptions::setup`): the event's loadout");
    }
    let live_rules = world.get_resource::<crate::shell::live::LiveSession>().and_then(|l| l.rules);
    let rivals = match live_rules {
        Some(rules) if !rules.explore_ghosts => {
            info!("level: a network session without the event's explore ghosts: no medal ghosts");
            ssx_ui::game::ride::DropRivals { rider_titles: rivals.rider_titles, ..Default::default() }
        }
        _ => rivals,
    };
    let mut feed = HudFeed::new(&opts.launch.rider);
    feed.explore.rivals = rivals.rivals;
    if let Some(fe) = world.resource_mut::<GameUiRes>().0.fe.as_mut() {
        feed.explore.pictures = fe.career.riders.iter().map(|r| (r.title.clone(), r.icon.clone())).collect();
        // PORT(choice): a launch that passed no Explore drop screen (the hub's, a network session's round, the
        // `--launch` test option) has the game mode's leaderboard list built here as the drop screen builds it
        // (0x82481A50: 0x8247C308 empties it, 0x82481540 the medal ghosts, 0x82480D70 the own best of 0x824A7B90);
        // the console launches an Explore drop only from its drop screen, whose selection built the list for it.
        if fe.explore_list.drop != opts.launch.drop {
            fe.explore_list = ssx_ui::game::fe::explore::ExploreList::build(&fe.profile, opts.launch.drop);
        }
    }
    feed.rider_titles = rivals.rider_titles;
    let mut ghosts = ssx_ui::game::ride::RivalGhosts::new(rivals.replays);
    let board = world.get_resource::<Ride>().and_then(|r| r.map.as_ref()?.mode.as_ref()).and_then(|m| {
        (m.gamemode.is_some() && m.rules.rivals() != ssx_content::gamemode::Rivals::None)
            .then(|| m.rules.board_values())
    });
    if let Some(values) = board {
        let rows = ssx_ui::game::ride::target_rows(&values);
        info!("level: the map's gamemode's board {values:?} as the HUD's rows");
        feed.explore.rivals = rows.clone();
        ghosts = ghosts.with_targets(rows);
    }
    let nis = world.get_resource::<Ride>().is_some_and(|r| ssx_ui::game::feed::nis_is_active(&r.state, &r.session));
    let free_ride =
        opts.launch.free_ride || world.get_resource::<Ride>().is_some_and(|r| r.session.event.setup.free_ride);
    world.resource_mut::<GameUiRes>().set_drop(opts.launch.drop, event_type, free_ride, nis);
    if world.contains_resource::<Ride>() {
        world.resource_scope(|world, mut ui: Mut<GameUiRes>| {
            let ride = world.resource::<Ride>();
            feed.score_manager_feed(&mut ui.0, &ride.state, &ride.session, ride.world());
        });
    }
    world.resource_mut::<GameUiRes>().fe_call(|fe, _| ssx_ui::game::fe::flow::simulation_begins(fe));
    let saved = world.resource::<GameUiRes>().0.fe.as_ref().map_or(-1, |f| f.profile.camera_type());
    info!("level: the profile's camera type (CTYP) {saved}");
    world.insert_resource(CameraTypeLink::default());
    if let Some(mut ride) = world.get_resource_mut::<Ride>() {
        ride.hold = true;
    }
    let cams: Vec<Entity> = world.query_filtered::<Entity, With<FlyCam>>().iter(world).collect();
    match cams.first() {
        Some(&c) => _ = world.entity_mut(c).insert((ui::UiCamera, IsDefaultUiCamera)),
        None => warn!("level: no 3-D camera for the HUD"),
    }
    if opts.debug_hud {
        info!("level: the viewer's debug text is on (--debug-hud)");
    }
    let live_rules = world.get_resource::<crate::shell::live::LiveSession>().and_then(|l| l.rules);
    let live = live_rules.is_some();
    let live_rounds = live_rules.is_some_and(|r| r.rounds);
    if live {
        info!("level: a network session's level (the pause, the popups and the results as multiplayer has them)");
    }
    world.insert_resource(LevelGame {
        live,
        live_rounds,
        launch: opts.launch,
        stat_names: opts.stat_names,
        badges_seen,
        feed0: feed.clone(),
        feed,
        ghosts0: ghosts.clone(),
        ghosts,
        geotag_rows: geotags,
        fed: 0,
        fed_paused: 0,
        result: None,
        results_posted: false,
        up: false,
        ready_frames: 0,
        spawned: Instant::now(),
        shots: opts.shots,
        frame_ms: Vec::new(),
        ui_ms: Vec::new(),
        next_log: 10.0,
        release: Vec::new(),
        script_a_until: None,
        script_back_until: None,
        script_bumper_until: None,
        stick_dpad: Default::default(),
        requests: Vec::new(),
        restart_at: None,
        restarts: 0,
        ui_frames: 0,
        script,
        clock: 0.0,
        shot_dir: opts.shot_dir,
        stop_after: opts.stop_after,
        stopped: false,
        ride_seconds: 0.0,
        save_due: false,
        ghost_due: Vec::new(),
        paused: false,
        sim_hold: None,
        autoplay: opts.autoplay,
        popup_since: None,
        popup_presses: 0,
        popup_text: None,
        flags,
        prompts: opts.follow_prompts.then(prompt_player::PromptPlayer::default),
        win_after: opts.win_after,
        rival_values,
        bot_scores,
    });
}

type SceneCamera<'a> = (Has<Hdr>, &'a Msaa);
type UiCameraSettings<'a> = (Entity, Has<Hdr>, &'a mut Msaa);

fn sync_ui_camera(
    mut commands: Commands,
    scene: Query<SceneCamera, With<FlyCam>>,
    mut ui_cams: Query<UiCameraSettings, (With<ui::UiCamera>, Without<FlyCam>)>,
) {
    let Some((hdr, msaa)) = scene.iter().next() else { return };
    for (e, ui_hdr, mut ui_msaa) in &mut ui_cams {
        if *ui_msaa != *msaa {
            *ui_msaa = *msaa;
        }
        match (hdr, ui_hdr) {
            (true, false) => _ = commands.entity(e).insert(Hdr),
            (false, true) => _ = commands.entity(e).remove::<Hdr>(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod ghost_tests {
    use super::ghost_saves_due;

    #[test]
    fn a_kept_ghost_waits_for_the_halt_through_a_restart() {
        let mut due = vec![(0xA1, 500)];
        assert!(ghost_saves_due(&mut due, false, false).is_empty());
        assert_eq!(due, vec![(0xA1, 500)]);
        due.push((0xA1, 700));
        assert_eq!(ghost_saves_due(&mut due, true, false), vec![(0xA1, 500), (0xA1, 700)]);
        assert!(due.is_empty());
        let mut due = vec![(0xB2, 9)];
        assert_eq!(ghost_saves_due(&mut due, false, true), vec![(0xB2, 9)]);
        assert!(ghost_saves_due(&mut due, true, true).is_empty());
    }
}
