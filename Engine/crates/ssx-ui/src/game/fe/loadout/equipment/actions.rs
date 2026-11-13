use super::*;

pub(super) fn fctiwz(v: f64) -> i32 {
    if v.is_nan() { i32::MIN } else { v as i32 }
}

impl Equipment {
    pub fn inventory_setup(&mut self, max: f64, unselectable: f64) {
        (self.inv_max, self.inv_unselectable) = (fctiwz(max), fctiwz(unselectable));
        (self.inv_first, self.inv_focus, self.inv_step) = (0, 0, 0);
    }

    pub fn inventory_step(&mut self, step: f64) {
        let d = fctiwz(step);
        if d < 1 {
            if self.inv_first > 0 {
                self.inv_first = self.inv_first.wrapping_add(d);
            } else {
                self.inv_focus = self.inv_focus.wrapping_add(d);
            }
        } else if self.inv_unselectable <= self.inv_focus {
            self.inv_first = self.inv_first.wrapping_add(d);
        } else {
            self.inv_focus = self.inv_focus.wrapping_add(d);
        }
        self.inv_index = self.inv_focus;
        self.inv_step = d.wrapping_abs();
    }

    pub fn inventory_sold(&mut self) {
        let shown = i32::try_from(self.inventory.len()).unwrap_or(i32::MAX);
        if shown > self.inv_focus.wrapping_add(25) {
            return;
        }
        if self.inv_first >= 1 {
            self.inv_first = self.inv_first.wrapping_sub(self.inv_step);
        } else if self.inv_focus > 0 {
            self.inv_focus = self.inv_focus.wrapping_sub(self.inv_step);
            self.inv_index = self.inv_focus;
        }
    }
}

fn entry(list: &[UiItem], index: usize) -> Option<(u32, u8)> {
    list.get(index).map(|u| (u.key, u.src))
}

fn fill(fe: &mut FrontEnd, env: &mut FeEnv, item: Option<UiItem>) {
    if let Some(u) = item {
        fill_entry(&mut fe.equipment.gear_info, env.loc, env.values, &u);
    }
}

fn live(fe: &FrontEnd, list: List, index: usize) -> Option<UiItem> {
    let inv = fe.equipment.inv.as_ref()?;
    match list {
        List::Items => inv.owned_list.get(index),
        List::Store => inv.store_list.get(index),
        List::Inventory => inv.inventory_list.get(index),
    }
    .cloned()
}

pub fn action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str, param: &ActionParam) {
    let index = usize::try_from(fctiwz(param.number)).unwrap_or(usize::MAX);
    match action {
        "A_GameAdvanceFlow" => advance(fe, env),
        "A_EquipmentBackCategory" => match fe.equipment.state {
            state::RIDER => {
                fe.note("equipment: back (FE manager +0x88(4), save)");
                save_state_reset(env);
                crate::game::fe::globe::back_from_equipment(fe, env);
            }
            state::GEAR => {
                let (k, slot) = (kind_of(fe.equipment.category), usize::try_from(fe.equipment.slot).unwrap_or(0));
                let mut inv = inv(fe);
                inv.close_category(&fe.career, &fe.profile, k, slot);
                fe.equipment.inv = Some(inv);
                set_category(fe, env, fe.equipment.category);
                fe.equipment.state = state::READY;
                fe.equipment.score = env.values.int(0x6C);
                fe.publish.extend(
                    [
                        "dp_screen_state",
                        "dp_selected_rider",
                        "dp_equipment_categories",
                        "dp_equipment_categories_index",
                        "dp_equipment_score",
                    ]
                    .map(String::from),
                );
            }
            _ => {}
        },
        "A_EquipmentCategoryHighlight" if matches!(fe.equipment.state, state::READY | state::READY_CATEGORY) => {
            fe.equipment.category = fctiwz(param.field("category")).wrapping_add(1);
            fe.equipment.slot = fctiwz(param.field("slot"));
            publish_list(fe, "dp_equipment_items");
            publish_list(fe, "dp_store_items");
            let k = usize::try_from(kind_of(fe.equipment.category)).unwrap_or(0);
            let slot = usize::try_from(fe.equipment.slot).unwrap_or(0);
            let gear = fe.equipment.inv.as_ref().and_then(|i| {
                let n = |&n: &i64| usize::try_from(n).unwrap_or(0);
                let before = i.category_slots.iter().take(k).map(n).fold(0usize, usize::saturating_add);
                i.selected_gear.get(before.saturating_add(slot).saturating_add(1)).cloned()
            });
            fill(fe, env, gear);
            fe.publish.push("dp_selected_gear_info".into());
        }
        "A_EquipmentCategorySelect" => {
            fe.equipment.state = state::GEAR;
            let (k, slot) = (kind_of(fe.equipment.category), usize::try_from(fe.equipment.slot).unwrap_or(0));
            let mut inv = inv(fe);
            inv.open_category(&fe.career, &fe.profile, k, slot);
            let survival = inv.survival;
            fe.equipment.inv = Some(inv);
            set_category(fe, env, fe.equipment.category);
            if let Some(inv) = fe.equipment.inv.as_mut() {
                inv.survival = survival;
            }
            env.values.set_int(0x6C, survival);
            fe.publish.push("dp_screen_state".into());
        }
        "A_EquipmentBackReady" => {
            if fe.equipment.rider_count == 1 {
                fe.note("equipment: back (one rider: save and leave)");
                save_state_reset(env);
                crate::game::fe::globe::back_from_equipment(fe, env);
                return;
            }
            fe.equipment.slot = 0;
            fe.equipment.category = 0;
            set_category(fe, env, 0);
            publish_list(fe, "dp_playable_characters");
            fe.publish.push("dp_playable_characters_index".into());
            fe.equipment.state = state::RIDER;
            fe.publish.push("dp_screen_state".into());
        }
        "A_EquipmentItemSelected" => {
            let Some(item) = fe.equipment.list.get(index).cloned() else { return };
            if fe.equipment.category != 0 {
                if item.has(flag::AVAILABLE) {
                    select(fe, env, List::Items, index);
                }
                return;
            }
            fe.equipment.rider_index = i32::try_from(index).unwrap_or(0);
            fe.equipment.last_rider = item.key;
            env.values.set_int(0x65, item.key as i32);
            if !item.has(flag::AVAILABLE) {
                fe.equipment.pending = Some((List::Items, index));
                purchase(fe, env, List::Items, index);
                return;
            }
            select(fe, env, List::Items, index);
        }
        _ => highlight_action(fe, env, action, param, index),
    }
}

fn highlight_action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str, param: &ActionParam, index: usize) {
    match action {
        "A_EquipmentItemHighlighted" => {
            let Some(e) = entry(&fe.equipment.list, index) else { return };
            preview(fe, env, Some(e));
            fe.equipment.score = env.values.int(0x6C);
            let item = live(fe, List::Items, index);
            fill(fe, env, item);
            fe.publish.extend(["dp_equipment_score", "dp_selected_gear_info", "dp_selected_rider"].map(String::from));
        }
        "A_EquipmentRiderHighlighted" if fe.equipment.category == 0 => {
            let Some(e) = entry(&fe.equipment.list, index) else { return };
            preview(fe, env, Some(e));
            let item = live(fe, List::Items, index);
            fill(fe, env, item);
            fe.publish.extend(["dp_selected_gear_info", "dp_selected_rider_info"].map(String::from));
        }
        "A_EquipmentOptimizeGear" if matches!(fe.equipment.state, state::READY | state::READY_CATEGORY) => {
            let mut inv = inv(fe);
            let drop = drop_info(fe);
            inv.optimize(&fe.career, &fe.profile, env.loc, drop.as_ref());
            inv.save_loadout(&fe.career, &mut fe.profile);
            env.values.set_int(0x6C, inv.survival);
            env.values.set_int(0x64, inv.inventory_size);
            fe.equipment.inv = Some(inv);
            fe.equipment.score = env.values.int(0x6C);
            fe.publish.extend(["dp_equipment_score", "dp_equipment_categories"].map(String::from));
        }
        "A_EquipmentStoreItemSelected" => {
            fe.equipment.pending = Some((List::Store, index));
            purchase(fe, env, List::Store, index);
        }
        "A_EquipmentStoreItemHighlighted" => {
            let Some(e) = entry(&fe.equipment.store, index) else { return };
            preview(fe, env, Some(e));
            fe.equipment.score = env.values.int(0x6C);
            let item = live(fe, List::Store, index);
            fill(fe, env, item);
            fe.publish.extend(["dp_equipment_score", "dp_selected_gear_info", "dp_selected_rider"].map(String::from));
        }
        "A_EquipmentRefreshStore" => {
            publish_list(fe, "dp_equipment_items");
            publish_list(fe, "dp_store_items");
        }
        "A_EquipmentViewMoreSelected" => {
            fe.equipment.state = state::INVENTORY;
            save_state(fe, env);
            fe.go_to_screen(env, 0x31);
        }
        "A_EquipmentViewMoreHighlighted" if fe.equipment.state == state::GEAR => {
            preview(fe, env, None);
            fe.equipment.score = env.values.int(0x6C);
            fill_view_more(&mut fe.equipment.gear_info, env.loc, env.values, fe.equipment.category);
            fe.publish.extend(["dp_equipment_score", "dp_selected_gear_info"].map(String::from));
        }
        "A_EquipmentInventorySetup" => {
            fe.equipment.inventory_setup(param.field("maxData"), param.field("unselectableData"));
        }
        "A_EquipmentInventoryRequestData" => {
            fe.equipment.inventory_step(param.number);
            publish_list(fe, "dp_inventory_items");
            fe.publish.push("dp_inventory_items_index".into());
        }
        "A_EquipmentInventoryItemSelected" => select(fe, env, List::Inventory, index),
        "A_EquipmentInventoryItemHighlighted" => {
            let Some(e) = entry(&fe.equipment.inventory, index) else { return };
            preview(fe, env, Some(e));
            let at = usize::try_from(fe.equipment.inv_first).ok().and_then(|f| f.checked_add(index));
            let item = at.and_then(|a| live(fe, List::Inventory, a));
            fill(fe, env, item);
            fe.publish.push("dp_selected_gear_info".into());
        }
        "A_EquipmentInventoryItemBack" => {
            fe.equipment.state = state::GEAR;
            save_state(fe, env);
            fe.go_to_screen(env, screens::id::EQUIPMENT);
        }
        "A_EquipmentSellGear" => {
            fe.equipment.pending = Some((List::Inventory, index));
            env.nav_events.push("equip_sell_confirm_popup".into());
        }
        "A_EquipmentPurchaseRiderLevel" => purchase_rider_level(fe, env, i64::from(fctiwz(param.number))),
        "A_EquipmentShowHelp" => crate::game::fe::hint::gear_help(fe, env, param.number, true),
        _ => {}
    }
}
