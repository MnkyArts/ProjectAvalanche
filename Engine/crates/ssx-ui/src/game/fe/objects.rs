use super::*;

impl FrontEnd {
    pub fn results_campaign(&self, loc: &mut Localization) -> Option<(bool, String, String)> {
        if self.fe_mode != 1 {
            return None;
        }
        let e = self.career.event(self.current_drop);
        let title = e.map(|e| loc.text(&e.results_title)).unwrap_or_default();
        let body = e.map(|e| loc.text(&e.results_body)).unwrap_or_default();
        Some((self.last_success.unwrap_or(false), title, body))
    }

    pub fn build(&mut self, cx: &mut Ctx, values: &mut UiValues, loc: &mut Localization, dp: &str) -> Option<Value> {
        match dp {
            "dp_boot_logo_screen_data" => Some(boot::build(self, cx)),
            boot::ESRB_READY => boot::esrb_ready(self),
            "dp_debug_menu" | "dp_debug_menu_index" if self.post_race.active => {
                postrace::build(self, cx, values, loc, dp)
            }
            "dp_post_race_menu_status" | "dp_post_race_quit" => postrace::build(self, cx, values, loc, dp),
            d if d.starts_with("dp_settings_") => settings::build(self, cx, values, loc, dp),
            "dp_unlocks_data" => Some(unlocks::build(self, cx)),
            "dp_debug_menu" | "dp_debug_menu_index" | "dp_sponsored_events_data" => menu::build(self, cx, dp),
            "dp_user_data" | "dp_drop_data" => user::build(self, cx, values, loc, dp),
            user::DP_DIMMER => Some(user::dimmer_object(cx, self.dimmer.action, &self.dimmer.state)),
            popup::DP_POPUP => {
                let objective = self.career.event(self.current_drop).map(|e| (e.hud_title.clone(), e.hud_body.clone()));
                Some(self.popups.build(cx, values, loc, objective))
            }
            "dp_pause_menu_data" | "dp_pause_menu_index" => pause::build(self, cx, loc, dp),
            "dp_race_results_upload" => {
                let o = Value::Object(cx.new_object());
                let _ = cx.set_member(&o, "isComplete", Value::Bool(values.bool(0x1B2)));
                let _ = cx.set_member(&o, "uploadDescription", Value::str("  "));
                Some(o)
            }
            "dp_loadingscreen_data" => Some(loading::build(self, cx, loc)),
            "dp_globe_data" | "dp_drop_overlay_data" | "dp_profile_callout" => globe::build(self, cx, values, loc, dp),
            notify::DP => self.notifications.current.is_some().then(|| notify::build(self, cx)),
            tutorial_pause::DP_POPUP | tutorial_pause::DP_MENU | tutorial_pause::DP_INDEX
                if self.tutorial_pause.active =>
            {
                tutorial_pause::build(self, cx, loc, dp)
            }
            badges::DP_CATEGORIES
            | badges::DP_COLLECTION
            | badges::DP_INDEX
            | badges::DP_DETAILS
            | badges::DP_RECENT
                if self.badges.active =>
            {
                badges::build(self, cx, loc, dp)
            }
            credits::DP => self.credits_text.as_deref().map(|t| credits::build(t, cx, loc)),
            ava::keyboard::DP | ava::keyboard::DP_KEYS | ava::keyboard::DP_KEYS_INDEX if self.ava.keyboard.active => {
                ava::keyboard::build(self, cx, loc, dp)
            }
            ava::hub::DP_MENU | ava::hub::DP_INDEX if self.ava.hub.active => ava::hub::build(self, cx, dp),
            ava::credits::DP => Some(ava::credits::build(self, cx, loc)),
            d if d.starts_with("dp_ava_maps") && self.ava.maps.active => ava::maps::build(self, cx, loc, d),
            d if d.starts_with("dp_ava_opt") => ava::options::build(self, cx, loc, d),
            #[cfg(feature = "net")]
            d if d.starts_with("dp_ava_browser") && self.ava.browser.active => ava::browser::build(self, cx, loc, d),
            #[cfg(feature = "net")]
            d if d.starts_with("dp_ava_bf_") => ava::browser::filters::build(self, cx, loc, d),
            #[cfg(feature = "net")]
            d if d.starts_with("dp_ava_players") && self.ava.session.list.active => {
                ava::session::players::build(self, cx, loc, d)
            }
            #[cfg(feature = "net")]
            d if d.starts_with("dp_ava_chat") || d.starts_with("dp_ava_quickchat") => {
                ava::session::chat::build(self, cx, loc, d)
            }
            music::DP_MENU
            | music::DP_MENU_INDEX
            | music::DP_CATEGORIES
            | music::DP_SORT_TYPES
            | music::DP_CATEGORY_MUSIC
            | music::DP_SSX_MUSIC
            | music::DP_SCREEN => music::build(self, cx, loc, dp),
            "dp_online_pass_enabled" | "dp_online_pass_data" | "dp_welcome_screen_data" => {
                user::build_offline(self, cx, loc, dp)
            }
            "dp_friends_online_status" => Some(Value::Bool(values.int(0x22F) != 0)),
            "dp_friends_locked_status" | "dp_profile_save_complete" => Some(Value::Bool(false)),
            "dp_friends_items" => Some(Value::Object(cx.new_array(Vec::new()))),
            _ => equipment::build(self, cx, values, loc, dp),
        }
    }
}
