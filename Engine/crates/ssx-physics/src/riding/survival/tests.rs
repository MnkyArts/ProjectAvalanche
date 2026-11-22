use super::*;
use crate::death::{DeathState, Vitals};
use crate::testutil::{survival, tuning};

fn props() -> ResolvedCharacterProperties {
    ResolvedCharacterProperties::resolve(&tuning().cp_max).expect("test properties")
}

const RUN: RunFlags = RunFlags { started: true, finished: false, nis: false };

#[test]
fn impact_damage_is_quantised_and_capped() {
    let t = survival();
    assert_eq!(impact_damage(&t, 0.0, Vec3::ZERO), 0.0);
    assert!((impact_damage(&t, 0.2, Vec3::X * 0.2) - 0.2).abs() < 1e-6);
    assert_eq!(impact_damage(&t, 2.0, Vec3::ZERO), 0.395);
}

#[test]
fn armour_absorbs_then_health_takes_and_regenerates() {
    let (t, r, d) = (survival(), props(), DropSurvival::default());
    let mut s = SurvivalState::default();
    s.init_outputs(&t, &r);
    assert_eq!(s.armour, 1.0, "an armour item (R.PhysicalDamageModifier 0.15 > 0)");
    s.ticks_since_impact_damage = 8.0;
    let hit = ImpactInputs { this_frame: true, speed: 2.0, ..ImpactInputs::default() };
    s.update_health(&t, &r, &d, RUN, hit, 2.0);
    assert!((s.armour - (1.0 - 0.15 * 0.395)).abs() < 1e-6 && s.health == 1.0);
    s.armour = 0.0;
    s.ticks_since_impact_damage = 8.0;
    s.update_health(&t, &r, &d, RUN, hit, 2.0);
    assert!((s.health - 0.605).abs() < 1e-6 && s.ticks_since_damaged == 0.0);
    let h0 = s.health;
    for _ in 0..3 {
        s.update_health(&t, &r, &d, RUN, ImpactInputs::default(), 2.0);
    }
    assert!((s.health - (h0 + 2.0 * 0.006_666_667 * 3.0)).abs() < 1e-5, "{}", s.health);
}

#[test]
fn minimum_health_impact_bail_and_permanent_death() {
    let (t, r) = (survival(), props());
    let mut d = DropSurvival::default();
    let mut s = SurvivalState { armour: 0.0, health: 0.1, ticks_since_impact_damage: 8.0, ..Default::default() };
    let hit = ImpactInputs { this_frame: true, speed: 2.0, ..ImpactInputs::default() };
    s.update_health(&t, &r, &d, RUN, hit, 2.0);
    assert!(s.at_min_health && s.trigger_impact_bail && s.health > 0.0);
    d.death_permanent = true;
    let mut s = SurvivalState { armour: 0.0, health: 0.1, ticks_since_impact_damage: 8.0, ..Default::default() };
    s.update_health(&t, &r, &d, RUN, hit, 2.0);
    assert_eq!(s.health, 0.0);
    assert!((s.health_max - (1.0 - 0.395 * 0.25)).abs() < 1e-6, "the permanent loss lowers the maximum");
    let mut death = DeathState::default();
    let v = Vitals { health: s.health, death_permanent: true, ..Vitals::default() };
    let msg = death.update_vitals(&v, 2.0).expect("MsgRiderDead");
    assert!(msg.by_health && !death.is_alive() && death.time_since_death == 2.0);
}

#[test]
fn shade_freezes_and_the_sun_warms_a_thermal_suit() {
    let (t, d) = (survival(), DropSurvival::default());
    let mut bare = props();
    (bare.temperature_loss_modifier, bare.temperature_gain_modifier) = (1.0, 0.0);
    let mut s = SurvivalState { shade: 0.0, ..Default::default() };
    s.update_temperature(&t, &bare, &d, RUN, 0.0, 2.0);
    assert!((s.temperature - 0.996).abs() < 1e-6 && (s.temperature_max - 0.996).abs() < 1e-6);
    let suit = props();
    let mut s = SurvivalState { shade: 0.0, ..Default::default() };
    s.update_temperature(&t, &suit, &d, RUN, 0.0, 2.0);
    s.shade = 1.0;
    s.update_temperature(&t, &suit, &d, RUN, 0.0, 2.0);
    assert!((s.temperature - s.temperature_max).abs() < 1e-6 && s.temperature_max > 0.9999, "{s:?}");
    let mut death = DeathState::default();
    let air = Vitals { temperature: 0.0, in_air: true, ..Vitals::default() };
    assert_eq!(death.update_vitals(&air, 2.0), None);
    let ground = Vitals { temperature: 0.0, ..Vitals::default() };
    assert!(death.update_vitals(&ground, 2.0).is_some_and(|m| m.by_freezing));
}

#[test]
fn low_oxygen_drains_and_the_tank_refills() {
    let (t, r, d) = (survival(), props(), DropSurvival::default());
    let mut s = SurvivalState { low_oxygen: true, ..Default::default() };
    s.init_outputs(&t, &r);
    s.update_oxygen(&t, &r, &d, RUN, 30.0, false, 0.0, 2.0);
    assert!((s.oxygen - (1.0 - 2.0 * 0.0105 * 0.1)).abs() < 1e-6, "{}", s.oxygen);
    let o = s.oxygen;
    s.oxygen_pad.use_oxygen = true;
    s.update_oxygen(&t, &r, &d, RUN, 30.0, false, 0.0, 2.0);
    assert!(s.oxygen > o && s.oxygen_tank < 1.0 && s.oxygen_use_requested);
    let mut calm = SurvivalState::default();
    calm.init_outputs(&t, &r);
    calm.update_oxygen(&t, &r, &d, RUN, 0.0, false, 0.0, 2.0);
    assert_eq!(calm.oxygen, 1.0);
}

#[test]
fn the_classic_controls_use_the_tank_with_the_right_sticks_click() {
    let (t, r, d) = (survival(), props(), DropSurvival::default());
    for (classic, pad, uses) in [
        (false, OxygenPad { use_oxygen: true, use_oxygen_classic: false }, true),
        (false, OxygenPad { use_oxygen: false, use_oxygen_classic: true }, false),
        (true, OxygenPad { use_oxygen: true, use_oxygen_classic: false }, false),
        (true, OxygenPad { use_oxygen: false, use_oxygen_classic: true }, true),
    ] {
        let mut s = SurvivalState { low_oxygen: true, oxygen_pad: pad, ..Default::default() };
        s.init_outputs(&t, &r);
        s.update_oxygen(&t, &r, &d, RUN, 30.0, classic, 0.0, 2.0);
        assert_eq!((s.oxygen_use_requested, s.oxygen_tank < 1.0), (uses, uses), "classic {classic}, {pad:?}");
    }
}

#[test]
fn batteries_spring_and_loadout() {
    let (t, r) = (survival(), props());
    let mut s = SurvivalState::default();
    s.update_batteries(&t, &r, RUN, 2.0);
    assert!((s.head_lamp_battery - (1.0 - 2.0 / (18_000.0 * 2.9))).abs() < 1e-7);
    s.update_batteries(&t, &r, RunFlags::default(), 2.0);
    assert!((s.head_lamp_battery - (1.0 - 2.0 / (18_000.0 * 2.9))).abs() < 1e-7, "only while running");
    let (mut x, mut v) = (1.0, 0.0);
    for _ in 0..400 {
        (x, v) = spring(x, v, 0.0, 0.05, 2.0);
        assert!(x >= -1e-6);
    }
    assert!(x.abs() < 1e-6);
    let g = loadout(&[4, 3, 7], 100.0);
    assert!(g.slots[3] && g.slots[4] && g.stats[11] == 100.0 && g.stats[15] == 100.0 && !g.slots[5]);
}
