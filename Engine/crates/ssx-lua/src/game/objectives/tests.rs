use std::collections::BTreeMap;

use super::binder::load_batch;
use super::*;

#[derive(Default)]
struct TestWorld {
    states: BTreeMap<(String, u16), GsValue>,
    touching: Vec<String>,
    effects: Vec<(String, Effect)>,
}

impl World for TestWorld {
    fn game_state(&mut self, entity: &Name, id: u16) -> Option<GsValue> {
        self.states.get(&(entity.text.to_string(), id)).copied()
    }
    fn touching_name(&mut self, volume: &Name) -> Option<bool> {
        Some(self.touching.iter().any(|t| volume.is(t)))
    }
    fn effect(&mut self, objective: &Name, e: &Effect) {
        self.effects.push((objective.text.to_string(), e.clone()));
    }
}

fn load(src: &str) -> (Runner, TestWorld) {
    let mut r = Runner { level_active: true, ..Runner::default() };
    r.log = Some(Vec::new());
    let (r, w, rep) =
        load_batch(r, SetId::Level, &[("t.lua".into(), src.as_bytes().to_vec())], TestWorld::default(), None);
    assert!(rep.scripts.iter().all(|s| s.1.is_none()), "{:?}", rep.scripts);
    assert!(rep.problems.is_empty(), "{:?}", rep.problems);
    (r, w)
}

fn gs_id(name: &str) -> u16 {
    crate::game::bindings::GAME_STATE.iter().position(|n| *n == name).expect("id") as u16
}

fn sets(w: &TestWorld) -> Vec<String> {
    w.effects
        .iter()
        .filter_map(|(_, e)| match e {
            Effect::SetGameState { entity, id, value } => Some(format!("{entity}.{}={value:?}", game_state_name(*id))),
            _ => None,
        })
        .collect()
}

#[test]
fn a_timed_wait_compares_before_adding() {
    let (mut r, mut w) = load(
        r#"BeginObjective("T")
            WAIT_Time(0.25)
            DO_SetGameStateValue("Player1", BLN_AllowRaceEnd, true)
        EndObjective()"#,
    );
    let mut when = None;
    for k in 1..=6 {
        r.update(0.1, &mut w);
        if when.is_none() && !w.effects.is_empty() {
            when = Some(k);
        }
    }
    assert_eq!(when, Some(4));
    assert_eq!(sets(&w), ["Player1.BLN_AllowRaceEnd=Bool(true)"]);
    assert!(r.objective("T").expect("T").complete());
}

#[test]
fn a_restart_ends_the_update_and_begins_again() {
    let (mut r, mut w) = load(
        r#"BeginObjective("Death")
            WAIT_TriggerVolumeByName("death")
            DO_SetGameStateValue("Player1", BLN_TriggerDeathByDeathPlane, true)
            DO_RestartObjective()
        EndObjective()"#,
    );
    r.update(1.0 / 30.0, &mut w);
    assert!(w.effects.is_empty());
    w.touching.push("death".into());
    r.update(1.0 / 30.0, &mut w);
    r.update(1.0 / 30.0, &mut w);
    assert_eq!(sets(&w).len(), 2, "once per update while inside");
    let log = r.log.as_ref().expect("log");
    assert_eq!(log.iter().filter(|e| e.what == Logged::Restarted).count(), 2);
}

#[test]
fn the_combinators_follow_the_nodes() {
    let (mut r, mut w) = load(
        r#"BeginObjective("A")
            NOT()
            WAIT_Time(0.0)
            DO_SetGameStateValue("Player1", BLN_AllowRaceEnd, true)
            WAIT_TriggerVolumeByName("x")
            AND()
            NOT()
            WAIT_TriggerVolumeByName("y")
            DO_SetGameStateValue("Player1", BLN_AllowRaceEnd, false)
            WAIT_TriggerVolumeByName("y")
            OR()
            WAIT_TriggerVolumeByName("z")
            DO_SetGameStateValue("Player1", BLN_NisActive, false)
        EndObjective()"#,
    );
    r.update(0.1, &mut w);
    assert_eq!(sets(&w), ["Player1.BLN_AllowRaceEnd=Bool(true)"]);
    w.touching = vec!["x".into(), "y".into()];
    r.update(0.1, &mut w);
    assert_eq!(sets(&w).len(), 1, "x ∧ ¬y fails");
    w.touching = vec!["x".into()];
    r.update(0.1, &mut w);
    assert_eq!(sets(&w).len(), 2, "x ∧ ¬y holds");
    w.touching = vec!["z".into()];
    r.update(0.1, &mut w);
    assert_eq!(sets(&w).len(), 3, "y ∨ z holds on z");
    assert!(r.objective("A").expect("A").complete());
}

#[test]
fn flags_overrides_and_completion() {
    let (mut r, mut w) = load(
        r#"BeginObjective("First")
            DO_SetTemporaryNamedFlag("Temp", true)
            DO_SetPermanentNamedFlag("Kept", true)
        EndObjective()
        BeginObjective("Second")
            WAIT_ObjectiveComplete("First")
            WAIT_CheckNamedFlag("Temp")
            DO_SetGameStateValue("Player1", BLN_AllowRaceEnd, true)
        EndObjective()
        BeginObjective("Replaced")
            DO_SetGameStateValue("Player1", BLN_NisActive, false)
        EndObjective()
        OverrideObjective("Replaced")
            DO_SetTemporaryNamedFlag("Kept", true)
        EndObjective()"#,
    );
    assert_eq!(r.sets[1].len(), 3, "the override keeps its place");
    r.update(0.1, &mut w);
    assert_eq!(sets(&w), ["Player1.BLN_AllowRaceEnd=Bool(true)"], "Second after First in one update; no IsAlive");
    assert!(r.flags.check(&Name::new("Kept")) && r.flags.permanent().any(|f| f == "Kept"), "kept its kind");
    r.unload_level(&mut w);
    assert_eq!(r.flags.names(), ["Kept"]);
    assert!(r.sets[1].is_empty());
}

#[test]
fn comparisons_by_type() {
    assert!(Cmp::LessThanOrEqualTo.f32(f32::NAN, 1.0) && Cmp::NotEqualTo.f32(f32::NAN, f32::NAN));
    assert!(!Cmp::LessThan.f32(f32::NAN, 1.0) && Cmp::GreaterThanOrEqualTo.f32(f32::NAN, 1.0));
    assert!(Cmp::LessThan.i32(-1, 0) && !Cmp::LessThan.u32(u32::MAX, 0));
    assert_eq!(fnv1(b"Player1"), 0xC88C_300F);
    assert_eq!(game_state_type(gs_id("VEC_CameraOrientationTrigger") as i32), Some(GsType::Vector));
    assert_eq!(game_state_type(gs_id("QUAT_LevelStartOrientation") as i32), Some(GsType::Quat));
}

#[test]
fn named_flags_by_hash_are_the_profiles_permanent_ones() {
    let mut f = NamedFlags::default();
    let (seen, other) = (Name::new("TutorialHintSeen"), Name::new("Other"));
    f.set_permanent_hash(seen.hash, true);
    f.set(&other, true, true);
    assert!(f.check(&seen) && f.check(&other));
    assert_eq!(f.permanent_hashes(), [seen.hash], "the temporary flag is not saved");
    f.set(&seen, true, true);
    assert_eq!(f.permanent_hashes(), [seen.hash], "a present flag keeps its kind");
    f.set_permanent_hash(other.hash, true);
    assert_eq!(f.permanent_hashes(), [seen.hash], "… the temporary one too");
    f.clear_temporary();
    assert!(!f.check(&other));
    f.set(&seen, false, false);
    assert!(f.permanent_hashes().is_empty() && !f.check(&seen));
}

#[test]
fn the_goofy_grab_is_the_stance_mirror() {
    use super::binder::mirrored_grab;
    for (a, b) in [(3, 4), (5, 9), (6, 13), (8, 11), (2, 10)] {
        assert_eq!((mirrored_grab(a), mirrored_grab(b)), (b, a));
    }
    for g in [1, 7, 12] {
        assert_eq!(mirrored_grab(g), g);
    }
    for g in [0, -1, 14, i32::MAX] {
        assert_eq!(mirrored_grab(g), g);
    }
}

#[test]
fn location_stat_keys_hash_the_decimal_location() {
    for (name, location) in [("HasCollectedThing", "Loc_A"), ("x", ""), ("Stat", "Cliff_07"), ("", "q")] {
        let l = Name::new(location);
        let key = StatKey { name: Name::new(name), scope: 0, location: Some(l.clone()) };
        assert_eq!(key.key(), fnv1(format!("{name}{}", l.hash).as_bytes()));
    }
    for hash in [0, 7, 10, 4_294_967_295] {
        let key = StatKey { name: Name::new("S"), scope: 0, location: Some(Name { hash, text: "".into() }) };
        assert_eq!(key.key(), fnv1(format!("S{hash}").as_bytes()));
    }
    assert_eq!(fnv1_continue(fnv1(b"ab"), b"cd"), fnv1(b"abcd"));
}
