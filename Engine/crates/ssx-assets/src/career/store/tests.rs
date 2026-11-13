use super::purchase::{Catalog, CatalogEntry, OfferPurchase, PurchaseError, sell_price};
use super::*;
use crate::career::data::{Description, StoreTables, StoreTemplate};
use crate::career::profile::Profile;

fn template(key: u32, kind: i64, slot: i64, stats: Vec<i64>) -> StoreTemplate {
    StoreTemplate {
        key,
        title: String::new(),
        slot,
        kind,
        stats,
        price_scale: 1.0,
        charlock_chance: 0,
        bonus_chance: 0,
        min_level: 0,
        max_level: 100,
        min_rarity: 0,
        max_rarity: 5,
        fixed_points: -1,
        bonus_stats: Vec::new(),
        descriptions: Vec::new(),
        members: Vec::new(),
    }
}

fn data() -> StoreData {
    let mut templates = vec![
        template(10, 0, 12, vec![]),
        template(11, 1, 0, vec![0, 1, 2]),
        template(12, 2, 3, vec![11, 12]),
        template(13, 3, 8, vec![1]),
        template(14, 3, 8, vec![5]),
        template(15, 4, -1, vec![]),
    ];
    templates[0].charlock_chance = 100;
    templates[1].bonus_chance = 100;
    templates[1].bonus_stats = vec![30];
    templates[2].fixed_points = 90;
    for (g, members) in [(100, vec![10]), (101, vec![11]), (102, vec![12]), (103, vec![13, 14]), (104, vec![15])] {
        let mut t = template(g, -1, -1, vec![]);
        t.members = members;
        templates.push(t);
    }
    let tables = StoreTables {
        groups: vec![100, 101, 102, 103, 104],
        rarity_weights: vec![690, 0, 200, 90, 20],
        rarity_bands: vec![30, 0, 35, 28, 7],
        rarity_price: vec![1.12, 1.12, 1.2, 1.45, 1.85],
        stat_points: vec![(35, 50), (50, 65)],
        bonus_points: vec![(0, 0), (5, 10)],
        prices: (0..10).map(|i| (100 + 1000 * i, 1100 + 1000 * i)).collect(),
        price_spread: vec![0, 100, 100, 200, 300, 400],
        durations: vec![10, 30, 90],
        special_interval: (12, 15),
        special_min_rarity: 4,
        mystery_chance: 64,
        stat_cap: 95,
        bonus_min_rarity: 2,
        bonus_price: 1.2,
        name_variants: [1, 2, 1, 1],
        suit_match_chance: 50,
        sell_percent: 22,
        suit_score: 43,
    };
    let descriptions = vec![
        Description { key: 1, rider: 7, id: 3, slot: 12, excluded: false, icon: String::new() },
        Description { key: 2, rider: 7, id: 3, slot: 13, excluded: false, icon: String::new() },
        Description { key: 3, rider: 8, id: 3, slot: 12, excluded: false, icon: String::new() },
        Description { key: 4, rider: 0, id: 1, slot: 12, excluded: true, icon: String::new() },
        Description { key: 5, rider: 8, id: 3, slot: 13, excluded: false, icon: String::new() },
    ];
    StoreData { tables, templates: templates.into_iter().map(|t| (t.key, t)).collect(), descriptions }
}

fn ctx() -> StoreContext {
    StoreContext { rider: 7, level: 1, gear_types: 1 << 3, ..Default::default() }
}

#[test]
fn the_store_draws_its_patterns() {
    let mut r = Mt19937::new(1);
    assert!((0..1000).all(|_| (0..7).contains(&r.below_i(7))));
    let mut s = Mt19937::new(1);
    let mut t = s.clone();
    assert_eq!((s.pick(1), s.pick(0), s.range(5, 5), s.range(6, 5)), (0, 0, 5, 6));
    assert_eq!(s.next_u32(), t.next_u32(), "no draw for one, none or an empty range");
    assert!((0..1000).all(|_| (3..=9).contains(&r.range(3, 9))));
}

#[test]
fn items_follow_their_template_and_tables() {
    let (d, c) = (data(), ctx());
    let mut s = Store::new(3);
    for k in 0..KINDS {
        for _ in 0..50 {
            s.generate(&d, &c, k, &Request::NORMAL, 0).unwrap();
        }
    }
    for i in &s.stock[0] {
        assert_eq!((i.level, i.kind, i.slot), (0, 0, 12));
        assert!((4100..=5100).contains(&i.cost), "{}", i.cost);
        assert_eq!(i.descriptions, [1, 2]);
    }
    for i in &s.stock[2] {
        assert_eq!(i.stats[0].1 as i64 + i.stats[1].1 as i64, 90);
        assert_eq!((i.stats[2], i.slot), ((-1, 0), 3));
    }
    for i in &s.stock[1] {
        let (lo, hi) = (50, 65);
        let sum: i64 = i.stats[..3].iter().map(|s| i64::from(s.1)).sum();
        assert!(sum >= lo * 3 / 3 && sum <= hi, "{sum}");
        assert!(i.cost % 100 == 0 && i.cost >= 100);
        if i.stats[5].0 != -1 {
            assert!(i.rarity >= 2 && (5..=10).contains(&i64::from(i.stats[5].1)));
        }
        assert!(i.name_format[1] < 2 && i.name_format[0] == 0);
        assert_eq!(i.expires_ms, 90 * 60_000);
    }
    assert!(s.stock[3].iter().all(|i| i.template == 13));
    let worn = StoreContext { equipped_gear: 1 << 1, ..c.clone() };
    let mut s2 = Store::new(4);
    for _ in 0..40 {
        s2.generate(&d, &worn, 3, &Request::NORMAL, 0);
    }
    let picked: Vec<u32> = s2.stock[3].iter().map(|i| i.template).collect();
    assert!(picked.contains(&14) && picked.contains(&13));
    assert!(s.stock[4].iter().all(|i| i.kind == 4));
    assert_eq!(s.stock[0][0].id, FIRST_STOCK_ID);
}

#[test]
fn the_rarity_floor_and_special_offers() {
    let (d, c) = (data(), ctx());
    let mut s = Store::new(9);
    let req = Request { special: true, min_rarity: 4, ..Request::NORMAL };
    for _ in 0..20 {
        s.generate(&d, &c, 1, &req, 1000);
    }
    for i in &s.stock[1] {
        assert_eq!(i.rarity, 4);
        assert!(i.special && [10, 30, 90].iter().any(|m| i.expires_ms == 1000 + m * 60_000));
    }
}

#[test]
fn refresh_fills_four_per_kind_and_purges() {
    let (d, c) = (data(), ctx());
    let mut s = Store::new(1);
    s.update(&d, &c, false, 0);
    let shown: Vec<usize> = (0..KINDS).map(|k| s.offers(&d, &c, k).len()).collect();
    assert_eq!(shown, vec![4, 4, 4, 4, 0]);
    assert_eq!(s.stock[0].len(), 4);
    s.update(&d, &c, false, 1);
    assert_eq!((s.stock[0].len(), s.stock[1].len()), (8, 4));
    assert_eq!(s.offers(&d, &c, KIND_SUIT as usize).len(), 4);
    let online = StoreContext { geotags_online: true, ..c.clone() };
    s.update(&d, &online, false, FIRST_SPECIAL_MS + 1);
    assert_eq!(s.stock.iter().flatten().filter(|i| i.special).count(), 1);
    assert!(s.special_due_ms >= FIRST_SPECIAL_MS + 1 + 12 * 60_000);
    s.purge(true, FIRST_SPECIAL_MS + 2);
    assert_eq!(s.stock.iter().flatten().count(), 1);
    assert_eq!(s.stock.iter().flatten().next().unwrap().id, FIRST_STOCK_ID);
    s.purge(false, 200 * 60_000);
    assert_eq!(s.stock.iter().flatten().count(), 0);
    let mut e = Store::new(1);
    e.refresh(&d, &StoreContext::default(), 0);
    assert!(e.stock.iter().all(Vec::is_empty));
}

#[test]
fn items_of_another_rider_convert_by_their_look() {
    let d = data();
    let mut i = StoreItem { charlock: 7, slot: 12, descriptions: [1, 2], ..Default::default() };
    assert!(fits_rider(&d, &mut i, 8));
    assert_eq!((i.charlock, i.descriptions), (8, [3, 5]));
    let mut k = StoreItem { charlock: 7, slot: 12, descriptions: [1, 99], ..Default::default() };
    assert!(fits_rider(&d, &mut k, 8));
    assert_eq!((k.charlock, k.descriptions), (8, [3, 0]));
    let mut f = StoreItem { charlock: 7, slot: 12, descriptions: [4, 2], ..Default::default() };
    assert!(fits_rider(&d, &mut f, 8));
    assert_eq!((f.charlock, f.descriptions), (8, [4, 5]));
    let mut j = StoreItem { charlock: 8, slot: 12, descriptions: [3, 5], ..Default::default() };
    assert!(!fits_rider(&d, &mut j, 9));
    assert_eq!((j.charlock, j.descriptions), (8, [3, 5]));
    assert!(fits_rider(&d, &mut j, 0));
}

#[test]
fn buying_and_selling() {
    let (d, c) = (data(), ctx());
    let mut s = Store::new(5);
    s.update(&d, &c, false, 0);
    let mut p = Profile::new();
    p.add_credits(1_000_000, false);
    let offer = s.offers(&d, &c, 2)[0].clone();
    let before = p.credits();
    let id = p.buy_offer(&mut s, &d, &c, OfferPurchase { kind: 2, id: offer.id, clock_s: 1234, now_ms: 0 }).unwrap();
    assert_eq!((id, p.credits()), (0, before - offer.cost));
    assert_eq!(p.generated[&0].cost, offer.cost);
    assert!(p.gear_types() & (1 << 3) != 0);
    assert!(s.find(2, offer.id).is_none() && s.offers(&d, &c, 2).len() == 4);
    let next = s.offers(&d, &c, 2)[0].clone();
    assert_eq!(p.buy_offer(&mut s, &d, &c, OfferPurchase { kind: 2, id: next.id, clock_s: 0, now_ms: 0 }).unwrap(), 1);
    let credits = p.credits();
    assert_eq!(p.sell_generated(&d, 0), Some(sell_price(offer.cost, 22)));
    assert_eq!(p.credits(), credits + offer.cost * 22 / 100);
    let mut poor = Profile::new();
    let dear = s.offers(&d, &c, 1).into_iter().find(|o| o.cost > 4500);
    if let Some(o) = dear {
        assert!(matches!(
            poor.buy_offer(&mut s, &d, &c, OfferPurchase { kind: 1, id: o.id, clock_s: 0, now_ms: 0 }),
            Err(PurchaseError::NotEnoughCredits { .. })
        ));
    }
    let mut cat = Catalog::default();
    cat.entries.insert(55, CatalogEntry { kind: 5, slot: -1, cost: 10_000, locked: true });
    assert_eq!(poor.buy_fixed(&cat, 55), Err(PurchaseError::NotEnoughCredits { cost: 10_000, credits: 4500 }));
    poor.add_credits(6000, false);
    assert_eq!(poor.buy_fixed(&cat, 55), Ok(10_000));
    assert!(poor.owned_items.contains(&55) && poor.riders.contains_key(&55) && poor.credits() == 500);
    assert_eq!(poor.buy_fixed(&cat, 55), Err(PurchaseError::AlreadyOwned));
}

#[test]
fn the_next_equip_id_wraps_as_a_word() {
    use crate::career::profile::GeneratedItem;
    let mut p = Profile::new();
    assert_eq!(p.next_equip_id(), 0);
    p.generated.insert(41, GeneratedItem::default());
    assert_eq!(p.next_equip_id(), 42);
    p.generated.insert(u32::MAX, GeneratedItem::default());
    assert_eq!(p.next_equip_id(), 0);
}

#[test]
fn fresh_suits_are_offered_and_each_refresh_generates_more() {
    let d = data();
    let c = StoreContext { level: 3, ..ctx() };
    let suit = KIND_SUIT as usize;
    let mut s = Store::new(3);
    s.update(&d, &c, false, 0);
    let first: Vec<u32> = s.offers(&d, &c, suit).iter().map(|o| o.id).collect();
    assert_eq!(first.len(), 4);
    assert_eq!(s.stock[suit].len(), 4);
    s.update(&d, &c, false, 1);
    let second: Vec<u32> = s.offers(&d, &c, suit).iter().map(|o| o.id).collect();
    assert_eq!(second.len(), 4);
    assert_eq!(s.stock[suit].len(), 8);
    assert!(second.iter().all(|id| *id >= FIRST_STOCK_ID + 4));
}

#[test]
fn suits_are_offered_at_every_rider_level() {
    let d = data();
    let c = StoreContext { level: 3, ..ctx() };
    let mut s = Store::new(3);
    s.refresh(&d, &c, 0);
    assert_eq!(s.stock[KIND_SUIT as usize].len(), 4);
    assert!(s.stock[KIND_SUIT as usize].iter().all(|i| i.level == 0), "a suit's level is 0 (0x82523FB4)");
    assert_eq!(s.offers(&d, &c, KIND_SUIT as usize).len(), 4, "the page lists the level-0 suits at level 3");
    s.refresh(&d, &c, 1);
    assert_eq!(s.stock[KIND_SUIT as usize].len(), 8);
    assert_eq!(s.offers(&d, &c, KIND_SUIT as usize).len(), 8);
}

#[test]
fn the_shown_special_and_its_clock() {
    let d = data();
    let c = ctx();
    let mut s = Store::new(5);
    let special = Request { special: true, ..Request::NORMAL };
    let id = s.generate(&d, &c, 2, &special, 1000).expect("a special");
    let due = s.find(2, id).map(|i| i.expires_ms).expect("in the stock");
    assert_eq!(s.shown_special_left(1000), None, "nothing shown (+0x160 / +0x164 −1)");
    s.show_special(2, id);
    assert_eq!(s.shown_special_left(1000), Some(due - 1000));
    assert_eq!(s.shown_special_left(due + 5), Some(0), "0x825201A0 writes 0 once the time is past");
    s.show_special(2, 999_999);
    assert_eq!(s.shown_special_left(1000), None, "FUN_8251FFF0 looks up stock ids from 1 000 000 only");
    s.show_special(2, id);
    s.clear_shown_special();
    assert_eq!(s.shown_special_left(1000), None);
    let normal = s.generate(&d, &c, 2, &Request::NORMAL, 1000).expect("an offer");
    s.drop_expired_specials(2, due);
    assert!(s.find(2, id).is_some(), "not before its end");
    s.drop_expired_specials(2, due + 1);
    assert!(s.find(2, id).is_none());
    assert!(s.find(2, normal).is_some());
}
