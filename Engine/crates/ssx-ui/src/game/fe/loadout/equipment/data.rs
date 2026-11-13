use super::*;

pub(super) fn set(cx: &mut Ctx, o: &Value, k: &str, v: Value) {
    let _ = cx.set_member(o, k, v);
}

pub(super) fn common_fields(cx: &mut Ctx, o: &Value, u: &UiItem) {
    set(cx, o, "assetUrl", Value::str(&u.asset));
    set(cx, o, "secondaryAssetUrl", Value::str(&u.secondary));
    for (k, f) in [
        ("isAvailable", flag::AVAILABLE),
        ("isPurchaseable", flag::PURCHASEABLE),
        ("isLevelPurchaseable", flag::LEVEL_PURCHASEABLE),
        ("isSellable", flag::SELLABLE),
        ("showStats", flag::SHOW_STATS),
        ("isRecommended", flag::RECOMMENDED),
        ("isNew", flag::NEW),
        ("isEquipped", flag::EQUIPPED),
        ("isMystery", flag::MYSTERY),
    ] {
        set(cx, o, k, Value::Bool(u.has(f)));
    }
}

pub(super) fn stat_fields(cx: &mut Ctx, loc: &mut Localization, o: &Value, u: &UiItem) {
    for (k, v) in stat_columns(loc, u) {
        let v = v.to_value(cx);
        set(cx, o, k, v);
    }
}

fn entry_fields(cx: &mut Ctx, o: &Value, u: &UiItem, i: usize, events: [&str; 3]) {
    if u.has(flag::EMPTY) {
        set(cx, o, "state", Value::Number(2.0));
        set(cx, o, "strItem", Value::str(&u.name));
    } else {
        set(cx, o, "state", Value::Number(0.0));
    }
    let i = i as f64;
    for (k, v) in [
        ("itemId", Value::Number(i)),
        ("bFocusable", Value::Bool(true)),
        ("bEnabled", Value::Bool(true)),
        ("eventAdvance", Value::str(events[0])),
        ("eventAdvanceParam", Value::Number(i)),
        ("eventHighlight", Value::str(events[1])),
        ("eventHighlightParam", Value::Number(i)),
        ("eventBack", Value::str(events[2])),
    ] {
        set(cx, o, k, v);
    }
}

impl Equipment {
    fn items_entries(&mut self) {
        let live = self.inv.as_ref().map_or(&[][..], |i| &i.owned_list[..]);
        if self.category == 0 {
            self.list = live.to_vec();
            let at = self.list.iter().position(|u| u.key == self.last_rider).unwrap_or(0);
            self.rider_index = i32::try_from(at).unwrap_or(0);
            self.rider_count = i32::try_from(self.list.len()).unwrap_or(i32::MAX);
        } else {
            self.list = live.iter().take(7).cloned().collect();
        }
    }

    fn store_entries(&mut self) {
        self.store = self.inv.as_ref().map_or_else(Vec::new, |i| i.store_list.iter().take(4).cloned().collect());
    }

    fn inventory_entries(&mut self) {
        let live = self.inv.as_ref().map_or(&[][..], |i| &i.inventory_list[..]);
        let mut window = Vec::new();
        for i in 0..self.inv_max.max(0) {
            let at = usize::try_from(self.inv_first.wrapping_add(i)).ok();
            let Some(u) = at.and_then(|a| live.get(a)) else { break };
            window.push(u.clone());
        }
        self.inventory = window;
    }

    pub fn rebuild(&mut self, dp: &str) {
        let inventory = self.state == state::INVENTORY;
        match dp {
            "dp_playable_characters" | "dp_equipment_items" if !inventory => self.items_entries(),
            "dp_store_items" if !inventory => self.store_entries(),
            "dp_inventory_items" if inventory => self.inventory_entries(),
            _ => {}
        }
    }
}

pub(super) fn publish_list(fe: &mut FrontEnd, dp: &str) {
    fe.equipment.rebuild(dp);
    fe.publish.push(dp.into());
}

pub(super) fn items_array(fe: &FrontEnd, cx: &mut Ctx, loc: &mut Localization) -> Value {
    let riders = fe.equipment.category == 0;
    let highlight = if riders { "A_EquipmentRiderHighlighted" } else { "A_EquipmentItemHighlighted" };
    let mut out = Vec::new();
    for (i, u) in fe.equipment.list.iter().enumerate() {
        let o = Value::Object(cx.new_object());
        common_fields(cx, &o, u);
        if riders {
            set(cx, &o, "strItem", Value::str(&u.name));
            set(cx, &o, "level", Value::Number(u.level as f64));
            set(cx, &o, "levelProgress", Value::Number(u.level_progress as f64));
        }
        entry_fields(cx, &o, u, i, ["A_EquipmentItemSelected", highlight, "A_EquipmentBackCategory"]);
        out.push(o);
    }
    if !riders {
        let o = Value::Object(cx.new_object());
        let more = loc.text("TXT_INV_VIEWMORE");
        for (k, v) in [
            ("state", Value::Number(1.0)),
            ("strItem", Value::str(&more)),
            ("isAvailable", Value::Bool(true)),
            ("isPurchaseable", Value::Bool(false)),
            ("isAffordable", Value::Bool(true)),
            ("isNew", Value::Bool(false)),
            ("bFocusable", Value::Bool(true)),
            ("bEnabled", Value::Bool(true)),
            ("eventAdvance", Value::str("A_EquipmentViewMoreSelected")),
            ("eventHighlight", Value::str("A_EquipmentViewMoreHighlighted")),
            ("eventBack", Value::str("A_EquipmentBackCategory")),
        ] {
            set(cx, &o, k, v);
        }
        out.push(o);
    }
    Value::Object(cx.new_array(out))
}

pub(super) fn store_array(cx: &mut Ctx, loc: &mut Localization, list: &[UiItem]) -> Value {
    let mut out = Vec::new();
    for (i, u) in list.iter().enumerate() {
        let o = Value::Object(cx.new_object());
        common_fields(cx, &o, u);
        let price = loc.currency(u.cost, true);
        set(cx, &o, "price", Value::str(&price));
        set(cx, &o, "state", Value::Number(0.0));
        let n = i as f64;
        for (k, v) in [
            ("itemId", Value::Number(n)),
            ("bFocusable", Value::Bool(true)),
            ("bEnabled", Value::Bool(true)),
            ("eventAdvance", Value::str("A_EquipmentStoreItemSelected")),
            ("eventAdvanceParam", Value::Number(n)),
            ("eventHighlight", Value::str("A_EquipmentStoreItemHighlighted")),
            ("eventHighlightParam", Value::Number(n)),
            ("eventBack", Value::str("A_EquipmentBackCategory")),
        ] {
            set(cx, &o, k, v);
        }
        out.push(o);
    }
    Value::Object(cx.new_array(out))
}

pub(super) fn inventory_array(cx: &mut Ctx, list: &[UiItem]) -> Value {
    let mut out = Vec::new();
    for (i, u) in list.iter().enumerate() {
        let o = Value::Object(cx.new_object());
        common_fields(cx, &o, u);
        let events = ["A_EquipmentInventoryItemSelected", "A_EquipmentInventoryItemHighlighted"];
        entry_fields(cx, &o, u, i, [events[0], events[1], "A_EquipmentInventoryItemBack"]);
        out.push(o);
    }
    Value::Object(cx.new_array(out))
}

pub(super) fn offer_clock(ms: i64) -> String {
    let ms = ms.max(0) as u64;
    let (h, m, s) = (ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60);
    if h != 0 { format!("{h:02}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
}

pub fn build(
    fe: &mut FrontEnd,
    cx: &mut Ctx,
    _values: &mut UiValues,
    loc: &mut Localization,
    dp: &str,
) -> Option<Value> {
    fe.equipment.rebuild(dp);
    let eq = &fe.equipment;
    match dp {
        "dp_screen_state" => Some(Value::Number(f64::from(eq.state))),
        "dp_online_status" => Some(Value::Number(f64::from(eq.online))),
        "dp_playable_characters_index"
        | "dp_equipment_items_index"
        | "dp_store_items_index"
        | "dp_inventory_items_index" => Some(Value::Number(match eq.state {
            state::RIDER => f64::from(eq.rider_index),
            state::INVENTORY => f64::from(eq.inv_index),
            _ => 0.0,
        })),
        "dp_equipment_categories_index" => Some(Value::Number(f64::from(eq.category.wrapping_sub(1)))),
        "dp_playable_characters" | "dp_equipment_items" => Some(items_array(fe, cx, loc)),
        "dp_store_items" => Some(store_array(cx, loc, &eq.store)),
        "dp_inventory_items" => Some(inventory_array(cx, &eq.inventory)),
        "dp_selected_rider" => {
            let o = Value::Object(cx.new_object());
            if let Some(u) = eq.inv.as_ref().and_then(|i| i.selected_gear.first()) {
                set(cx, &o, "strItem", Value::str(&u.name));
                set(cx, &o, "assetUrl", Value::str(&u.asset));
                set(cx, &o, "secondaryAssetUrl", Value::str(&u.secondary));
                set(cx, &o, "level", Value::Number(u.level as f64));
                set(cx, &o, "levelProgress", Value::Number(u.level_progress as f64));
            }
            set(cx, &o, "isAvailable", Value::Bool(true));
            Some(o)
        }
        "dp_equipment_categories" => Some(categories(fe, cx, loc)),
        "dp_equipment_score" => {
            let o = Value::Object(cx.new_object());
            let label = loc.text("TXT_EQUIP_SCORE_LABEL");
            let s = eq.score;
            let text = if s < 0 { loc.text("TXT_N_A") } else { loc.percent(i64::from(s)) };
            set(cx, &o, "scoreLabel", Value::str(&label));
            set(cx, &o, "scoreValue", Value::Number(f64::from(s)));
            set(cx, &o, "scoreStr", Value::str(&text));
            set(cx, &o, "scoreAvailable", Value::Bool(s >= 0));
            set(cx, &o, "scoreWarning", Value::Bool((0..30).contains(&s)));
            Some(o)
        }
        "dp_limited_time_offer" => {
            let special = eq.store.iter().any(|u| u.has(flag::SPECIAL));
            Some(Value::str(&if special { offer_clock(i64::from(eq.offer_ms)) } else { String::new() }))
        }
        "dp_equipment_purchase_result" => Some(Value::Bool(eq.purchase_result)),
        "dp_selected_gear_info" => Some(eq.gear_info.to_value(cx)),
        "dp_selected_rider_info" => Some(rider_info(fe, cx, loc)),
        _ => None,
    }
}

fn categories(fe: &FrontEnd, cx: &mut Ctx, loc: &mut Localization) -> Value {
    let eq = &fe.equipment;
    let mut out = Vec::new();
    let Some(inv) = eq.inv.as_ref() else { return Value::Object(cx.new_array(out)) };
    let mut gear = inv.selected_gear.iter().skip(1);
    for (k, &n) in inv.category_slots.iter().enumerate() {
        if n <= 0 {
            continue;
        }
        let cat = k as i32 + 1;
        let o = Value::Object(cx.new_object());
        set(cx, &o, "isAvailable", Value::Bool(cat != 5 || eq.online != 0));
        set(cx, &o, "numSlots", Value::Number(n as f64));
        if cat == eq.category {
            set(cx, &o, "selectedSlot", Value::Number(f64::from(eq.slot)));
        }
        let name = loc.text(&format!("TXT_EQUIP_CAT{cat}_NAME"));
        let desc = loc.text(&format!("TXT_EQUIP_CAT{cat}_DESC"));
        set(cx, &o, "strItem", Value::str(&name));
        set(cx, &o, "description", Value::str(&desc));
        let mut list = Vec::new();
        for _ in 0..n {
            let Some(u) = gear.next() else { break };
            let g = Value::Object(cx.new_object());
            common_fields(cx, &g, u);
            stat_fields(cx, loc, &g, u);
            list.push(g);
        }
        let a = Value::Object(cx.new_array(list));
        set(cx, &o, "gearList", a);
        out.push(o);
    }
    Value::Object(cx.new_array(out))
}

fn rider_info(fe: &FrontEnd, cx: &mut Ctx, loc: &mut Localization) -> Value {
    let o = Value::Object(cx.new_object());
    if let Some(inv) = fe.equipment.inv.as_ref() {
        let b = &inv.bio;
        set(cx, &o, "gearAsset", Value::str(&b.gear_asset));
        set(cx, &o, "gearName", Value::str(&b.gear_name));
        let title = loc.text("TXT_SPECIAL_GEAR_TITLE");
        set(cx, &o, "gearTitle", Value::str(&title));
        let perks = loc.text("TXT_CHAR_PERKS_TITLE");
        set(cx, &o, "perkTitle", Value::str(&perks));
        let names: Vec<Value> = b.perks.iter().filter(|p| p.1 > 0).map(|p| Value::str(&p.0)).collect();
        let vals: Vec<Value> = b.perks.iter().filter(|p| p.1 > 0).map(|p| Value::Number(p.1 as f64)).collect();
        let n = Value::Object(cx.new_array(names));
        let v = Value::Object(cx.new_array(vals));
        set(cx, &o, "perkNames", n);
        set(cx, &o, "perkValues", v);
    }
    o
}
