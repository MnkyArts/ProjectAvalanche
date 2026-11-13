use std::path::Path;

use ssx_physics::Vec3;
use ssx_physics::event::EventDef;

const MEDAL_NAMES: [&str; 3] =
    ["TXT_EXPLORE_GHOST_MEDAL_GOLD", "TXT_EXPLORE_GHOST_MEDAL_SILVER", "TXT_EXPLORE_GHOST_MEDAL_BRONZE"];

#[derive(Clone, Debug, Default)]
pub struct DropRivals {
    pub rivals: Vec<(String, u32)>,
    pub replays: Vec<(usize, ssx_assets::ghosts::GhostReplay)>,
    pub rider_titles: std::collections::BTreeMap<u32, String>,
    pub problems: Vec<String>,
}

pub fn drop_rivals(big_root: &Path, def: Option<&EventDef>) -> DropRivals {
    let career = ssx_assets::career::CareerDb::load(big_root).ok();
    let rider_titles = career.iter().flat_map(|c| c.riders.iter().map(|r| (r.key, r.title.clone()))).collect();
    board_rivals(def, rider_titles, |id| ssx_assets::ghosts::GhostReplay::load(big_root, id))
}

pub fn board_rivals(
    def: Option<&EventDef>,
    rider_titles: std::collections::BTreeMap<u32, String>,
    mut load: impl FnMut(u32) -> anyhow::Result<ssx_assets::ghosts::GhostReplay>,
) -> DropRivals {
    let mut out = DropRivals { rider_titles, ..Default::default() };
    let Some(def) = def else { return out };
    for (slot, g) in def.ghost_board().into_iter().enumerate() {
        out.rivals.push((out.rider_titles.get(&g.character).cloned().unwrap_or_default(), g.value));
        let Some(row) = g.row.and_then(|i| def.rivals.get(i)) else { continue };
        match load(row.ghost_id as u32) {
            Ok(replay) => out.replays.push((slot, replay)),
            Err(e) => out.problems.push(format!("ghost {} (row {slot} of the board): {e:#}", row.ghost_id)),
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct RivalGhosts {
    pub replays: Vec<(usize, ssx_assets::ghosts::GhostReplay)>,
    finish: Vec<Option<u32>>,
    targets: Vec<(String, u32)>,
}

impl RivalGhosts {
    pub fn new(replays: Vec<(usize, ssx_assets::ghosts::GhostReplay)>) -> Self {
        RivalGhosts { finish: vec![None; replays.len()], replays, targets: Vec::new() }
    }

    pub fn with_targets(mut self, targets: Vec<(String, u32)>) -> Self {
        self.targets = targets;
        self
    }

    pub fn step(
        &mut self,
        seconds: f32,
        ms: u32,
        set: &ssx_physics::triggers::TriggerSet,
        loc: &mut crate::game::loc::Localization,
    ) -> (Vec<crate::game::hud::billboards::Ghost>, Vec<crate::game::leaderboard::RaceEntrant>) {
        let mut ghosts = Vec::new();
        let mut racers = Vec::new();
        for (k, (slot, g)) in self.replays.iter().enumerate() {
            let key = MEDAL_NAMES.get(*slot).copied().unwrap_or_default();
            let name = loc.text(key);
            let position = g.position(seconds);
            let active = !g.finished(seconds);
            let p = Vec3::from_array(position);
            if active
                && self.finish[k].is_none()
                && set
                    .touching(p, ssx_physics::triggers::PLAYER_TOUCH_RADIUS, ssx_physics::triggers::IG_GHOST)
                    .any(|i| set.volumes[i].name == "end")
            {
                self.finish[k] = Some(ms);
            }
            // TODO(guess): these entrants (the medal names, no picture) stand in where the session has no racers (the game's are kind 1 for explore requests too, `Racers_Activate` 0x82501874…0x825018A0: `GameMode_BuildExploreGhostRequests` 0x82480840 makes them racers of type 2 with request +0x80 = 1,
            // so the race leaderboard's query 0x824CB548 gives the character's name and `assets\gamertagIcons\%s.dds`); settle: load the racers' data here as well.
            racers.push(crate::game::leaderboard::RaceEntrant {
                name: name.clone(),
                picture: String::new(),
                active: crate::game::feed::RACER_PRESENT,
                finish_ms: self.finish[k],
                y: Some(position[1]),
            });
            if active {
                ghosts.push(crate::game::hud::billboards::Ghost {
                    id: u64::from(g.id),
                    name,
                    colour: ssx_assets::ghosts::EXPLORE_GHOST_TINT,
                    position,
                });
            }
        }
        for (key, value) in &self.targets {
            racers.push(crate::game::leaderboard::RaceEntrant {
                name: loc.text(key),
                picture: String::new(),
                active: true,
                finish_ms: Some(*value),
                y: None,
            });
        }
        (ghosts, racers)
    }
}

// PORT(choice): a map's targets stand where a drop's medal ghosts stand (the Explore leaderboard's rows for the
// trick-ghost and survive-to-death types, 0x824833E8; the race leaderboard's entrants for the race types, 0x824CC8B8,
// as finished at their value from the start, no replay to follow: no height, so no split callout against them,
// 0x824CB7C0 measuring from a racer's height); the ghost recorder (`GhostRecorder_Init` 0x824BE0D8) and writer
// (`GhostFile_WriteHeader` 0x8253DC98) ported would let a gamemode ship replays; the game has no maps.
pub fn target_rows(values: &[u32]) -> Vec<(String, u32)> {
    MEDAL_NAMES.iter().zip(values).map(|(k, &v)| (k.to_string(), v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_physics::event::{EventManager, EventResult, EventSetup, EventType, Rival};

    fn row(ghost_id: i32, character: u32, score: u32) -> Rival {
        let end_bits = [0; 3];
        Rival { starting_idx: 0, ghost_id, character, catchup: 0, score, tuned_score: u32::MAX, lap: 0, end_bits }
    }

    fn event(t: EventType, rivals: Vec<Rival>) -> EventDef {
        EventDef {
            name: "exp_test".into(),
            key: 7,
            drop_key: 0,
            event_type: Some(t),
            scripts: Vec::new(),
            rivals,
            required_equipment: Vec::new(),
            avalanche_scripts: Vec::new(),
            forced_rider: 0,
            forced_items: [0; 5],
            loadout: None,
            objectives: None,
            message: Default::default(),
            places: None,
            goal: None,
        }
    }

    fn replay(id: u32) -> ssx_assets::ghosts::GhostReplay {
        let header = ssx_formats::ghost::UserHeader { gamertag: String::new(), gear_flags: 0, score: 0, build: 0 };
        let (times, positions, scores) = (vec![0.0], vec![[0.0; 3]], vec![0.0]);
        ssx_assets::ghosts::GhostReplay { id, header, times, positions, scores, poses: Vec::new(), rejected: 0 }
    }

    fn titles() -> std::collections::BTreeMap<u32, String> {
        [(1, "A"), (2, "B"), (3, "C"), (ssx_physics::event::PADDING_CHARACTER, "MAC")]
            .map(|(k, t)| (k, t.to_string()))
            .into()
    }

    #[test]
    fn the_replays_keep_their_rows_when_one_does_not_load() {
        let def = event(EventType::RaceGhosts, vec![row(11, 1, 120_965), row(22, 2, 92_631), row(33, 3, 77_532)]);
        let load = |id: u32| if id == 33 { anyhow::bail!("missing") } else { Ok(replay(id)) };
        let r = board_rivals(Some(&def), titles(), load);
        let by_slot: Vec<(usize, u32)> = r.replays.iter().map(|(k, g)| (*k, g.id)).collect();
        assert_eq!(by_slot, [(1, 22), (2, 11)]);
        assert_eq!(r.rivals, [("C".into(), 77_532), ("B".into(), 92_631), ("A".into(), 120_965)]);
        assert_eq!(r.problems.len(), 1);
        let ghosts = RivalGhosts::new(r.replays);
        let names: Vec<&str> = ghosts.replays.iter().map(|(k, _)| MEDAL_NAMES[*k]).collect();
        assert_eq!(names, ["TXT_EXPLORE_GHOST_MEDAL_SILVER", "TXT_EXPLORE_GHOST_MEDAL_BRONZE"]);
    }

    #[test]
    fn padding_rows_are_macs_without_a_replay() {
        let def = event(EventType::RaceGhosts, vec![row(11, 1, 200_000)]);
        let r = board_rivals(Some(&def), titles(), |id| Ok(replay(id)));
        assert_eq!(r.rivals, [("MAC".into(), 180_000), ("A".into(), 200_000), ("MAC".into(), 240_000)]);
        assert_eq!(r.replays.iter().map(|(k, g)| (*k, g.id)).collect::<Vec<_>>(), [(1, 11)]);
    }

    fn rules() -> ssx_assets::career::Rules {
        use ssx_assets::career::{DropType, EventRule};
        let mut r =
            ssx_assets::career::Rules { explore_rewards: vec![[5000, 10000, 20000, 2500]; 2], ..Default::default() };
        let rule = EventRule {
            key: 7,
            name: "exp".into(),
            drop_type: DropType::Explore,
            event_type: EventType::RaceGhosts as i64,
            reward: 1,
            places: None,
            goal: None,
            cost: 0,
            peak: 0,
            range: 0,
        };
        r.events.insert(7, rule);
        r
    }

    #[test]
    fn the_session_the_results_and_run_end_give_one_medal() {
        let def = event(EventType::RaceGhosts, vec![row(11, 1, 120_965), row(22, 2, 92_631), row(33, 3, 77_532)]);
        let setup = EventSetup::new(Some(&def), None);
        for (ms, code) in [(77_532, 1), (92_631, 2), (93_000, 3), (101_598, 3), (121_000, 4)] {
            let s = setup.rank(ms, false, vec![77_532, 95_012, 100_755]);
            let r = EventResult {
                event_type: def.event_type,
                points: 0,
                time_ms: ms,
                descent_mm: 0,
                rewinds: 0,
                loops: 0,
                failed: false,
                value: ms,
                rivals: s.rivals,
                rank: s.rank,
                medal: s.medal,
                place: s.place,
                geotag_credits: 0,
                geotag_count: 0,
            };
            let ctx = super::super::ResultsContext { drop_key: 7, event_type: 6, ..Default::default() };
            let mut profile = ssx_assets::career::Profile::new();
            let screen = super::super::career_results(&ctx, &r, &mut profile, &rules()).medal;
            let session = r.medal.map(|m| m as i32);
            assert_eq!(screen.map(|m| 2 - m as i32), session, "{ms} ms");
            let run_end = r.run_end(&EventManager::new(setup.clone()).setup, &[]).expect("an Explore drop");
            assert_eq!(run_end.code, code, "{ms} ms");
        }
    }
}
