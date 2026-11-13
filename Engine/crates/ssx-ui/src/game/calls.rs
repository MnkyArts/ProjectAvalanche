use super::*;

impl GameUi {
    pub fn fe_call(&mut self, f: impl FnOnce(&mut fe::FrontEnd, &mut fe::FeEnv)) {
        let Some(front) = self.fe.as_mut() else { return };
        for (key, text) in std::mem::take(&mut front.pending_strings) {
            if let Err(e) = self.loc.add_port_string(&key, &text) {
                front.note(format!("content: the string {key:?} is not shown: {e:#}"));
            }
        }
        let mut nav_events = Vec::new();
        let mut env = fe::FeEnv { values: &mut self.values, loc: &mut self.loc, nav_events: &mut nav_events };
        f(front, &mut env);
        self.game_actions(&mut nav_events);
        for e in nav_events {
            self.nav.raise(&e);
        }
    }

    pub(super) fn take_calls(&mut self, cx: &mut Ctx) {
        for (object, method, args) in cx.vm.natives.take_queue() {
            let strings: Vec<String> =
                args.iter().map(|v| cx.to_string(v).map(|s| s.to_string()).unwrap_or_default()).collect();
            let s = |i: usize| -> String { strings.get(i).cloned().unwrap_or_default() };
            match (object, method) {
                ("LocalDataStore", "BindData") => {
                    let (path, name) = (s(0), s(1));
                    let Some(binding) = self.store.binding(&name) else {
                        self.stats.binds.insert(name, String::new());
                        continue;
                    };
                    self.stats.binds.insert(name, binding.clone());
                    if binding == fe::boot::ESRB_BINDING {
                        self.stats.taken.insert(binding, "bootflow".into());
                        self.fe_call(|front, _| fe::boot::bind(front, true));
                        continue;
                    }
                    let clip = Clip::bind(cx, &path);
                    let mut nav_events = Vec::new();
                    let mut env = HudEnv {
                        values: &mut self.values,
                        loc: &mut self.loc,
                        event_type: self.event_type,
                        sounds: &mut self.sounds,
                        nav_events: &mut nav_events,
                        gear: self.gear,
                    };
                    if self.hud.bind(cx, &mut env, &binding, clip) {
                        self.stats.taken.insert(binding.clone(), "hud".into());
                    }
                }
                ("LocalDataStore", "UnbindData") => {
                    if let Some(binding) = self.store.binding_any(&s(1)) {
                        if binding == fe::boot::ESRB_BINDING {
                            self.fe_call(|front, _| fe::boot::bind(front, false));
                        } else {
                            self.hud.unbind(&binding);
                        }
                    }
                }
                ("LocalDataStore", "Subscribe") => {
                    let (path, name) = (s(0), s(1));
                    let ok = self.store.subscribe(&path, &name);
                    self.stats.subscribes.insert(name, ok);
                }
                ("LocalDataStore", "Unsubscribe") => self.store.unsubscribe(&s(0), &s(1)),
                ("ResourceHandler", "AcquireResource") => {
                    let kind = args.get(1).map_or(0.0, |v| cx.to_number(v)) as i32;
                    self.resources.push(ResourceRequest { path: s(0), kind, tag: s(2) });
                }
                ("InputProvider", "Subscribe") => self.input.subscribe(&s(0)),
                ("InputProvider", "Unsubscribe") => self.input.unsubscribe(&s(0)),
                ("Dispatcher", "DispatchEvent") => {
                    let ev = args.first().map_or(0.0, |v| cx.to_number(v)) as i64;
                    let data = s(1);
                    if ev == i64::from(navev::VIEW_READY) {
                        self.handler = Some(data.clone());
                    }
                    if ev == i64::from(navev::POST_ON_ENTER_VIEW) {
                        self.loading_views.retain(|v| *v != data);
                        self.view_acknowledged(true, &data);
                        self.relayout_answer(true, &data);
                    }
                    if ev == i64::from(navev::POST_ON_EXIT_VIEW) {
                        self.view_acknowledged(false, &data);
                        self.relayout_answer(false, &data);
                    }
                    // TODO(guess): the game's consumer of the queued framework events (0x82B41858, callback 0x82B417D8) is not traced; a movie's `kNavChangeView` with a nav event's name is raised on the nav; settle: that consumer.
                    if ev == i64::from(navev::CHANGE_VIEW) && self.fe.is_some() {
                        self.nav.raise(&data);
                    }
                    self.stats.dispatched.push(format!("{ev:#x} {data}"));
                }
                ("Dispatcher", "DataAction") if self.fe.is_some() && self.store.data_action(&s(0), &s(1)).is_some() => {
                    let (provider, dp, action) = self.store.data_action(&s(0), &s(1)).unwrap_or_default();
                    let param = fe::ActionParam::of(cx, args.get(2));
                    self.stats.actions.push(format!("DataAction {} {} → {action} ({provider})", s(0), s(1)));
                    self.fe_call(|fe, env| fe.data_action(env, &provider, &dp, &action, &param));
                }
                ("Dispatcher", "Action") if self.fe.is_some() && self.store.action_provider(&s(0)).is_some() => {
                    let (provider, action) = self.store.action_provider(&s(0)).unwrap_or_default();
                    let param = fe::ActionParam::of(cx, args.get(1));
                    self.stats.actions.push(format!("Action {} → {action} ({provider})", s(0)));
                    if action == "A_UtilityGetCurrencyCountAnimData" {
                        self.currency_count(cx, args.get(1).cloned().unwrap_or_default());
                    } else if provider == "nisprompt" && action == "A_GameChangeGear" {
                        let active = self.hud.nis_prompt.is_active();
                        self.fe_call(|fe, env| {
                            fe.note(format!("action {action} ({provider})"));
                            hud::nis::change_gear(fe, env, active);
                        });
                    } else {
                        self.fe_call(|fe, env| fe.action(env, &provider, &action, &param));
                    }
                }
                ("Dispatcher", "DataAction") if self.fe.is_some() && s(0) == fe::popup::DP_POPUP => {
                    let action = s(1);
                    self.stats.actions.push(format!("DataAction std_popup {action}"));
                    if action == "A_AudioPlayPopupSFX" {
                        let id = fe::ActionParam::of(cx, args.get(2)).number as i32;
                        self.fe_call(|fe, _| fe.play_sound(id));
                    } else {
                        let top = self.fe.as_ref().and_then(|f| f.popups.stack.last().and_then(|n| f.popups.def(n)));
                        let index = (top.is_some_and(|d| d.custom) && self.values.int(fe::saveload::TYPE) == 1)
                            .then(|| fe::ActionParam::of(cx, args.get(2)).fields.get("index").copied())
                            .flatten();
                        self.nav.raise(&action);
                        if let Some(index) = index {
                            self.fe_call(|fe, env| fe::saveload::answer(fe, env, index as i32));
                        }
                    }
                }
                ("Dispatcher", "Action") | ("Dispatcher", "DataAction") => {
                    let name = s(0);
                    let action = self.store.action(&name).map(str::to_string);
                    self.stats.actions.push(format!("{method} {name} → {action:?}"));
                    if action.as_deref() == Some("A_UtilityGetCurrencyCountAnimData") {
                        self.currency_count(cx, args.get(1).cloned().unwrap_or_default());
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn hud_constructed(&mut self) {
        self.values.set_bool(fe::pause::PAUSE_IS_EXITING_BE, false);
        let drop = self.fe.as_ref().map_or(self.nav.drop_key, |f| f.current_drop);
        if drop == tutorial::TUTORIAL_HUD_DROP {
            self.nav.raise("tutorialhud_enable");
        }
    }

    pub(super) fn hud_destroyed(&mut self) {
        use values::id;
        self.values.set_bool(id::HUDMESSAGE_IS_ENABLED, false);
        self.values.set_bool(id::READY, false);
        self.nav.raise("nistitle_disable");
        self.values.set_str(id::NIS_TITLE_MESSAGE, "");
        for i in [id::NIS_TITLE_ACTIVE, id::NIS_TITLE_ENABLED, id::NIS_TITLE_INITIALIZED] {
            self.values.set_bool(i, false);
        }
        self.nav.raise("tutorialhud_disable");
        self.nav.raise("hudmeters_disable");
        self.values.set_bool(id::METERS_ACTIVE, false);
    }

    pub(super) fn hint_panel(&mut self, construct: bool) {
        let mut nav_events = Vec::new();
        let mut env = HudEnv {
            values: &mut self.values,
            loc: &mut self.loc,
            event_type: self.event_type,
            sounds: &mut self.sounds,
            nav_events: &mut nav_events,
            gear: self.gear,
        };
        if construct {
            self.hud.hint = hud::hint::HintPanel::construct(&mut env);
        } else {
            self.hud.hint.destroy(&mut env);
            self.hud.hint = Default::default();
        }
    }

    pub(super) fn game_actions(&mut self, nav_events: &mut Vec<String>) {
        let Some(fe) = self.fe.as_mut() else { return };
        for (name, param) in std::mem::take(&mut fe.game_actions) {
            let registered = CPP_ACTIONS
                .iter()
                .find(|(p, a)| *a == name && self.store.active.iter().any(|x| x == p))
                .map(|(p, a)| (p.to_string(), a.to_string()));
            let Some((provider, action)) = self.store.action_provider(&name).or(registered) else {
                fe.note(format!("game action {name}: no active provider has it"));
                continue;
            };
            self.stats.actions.push(format!("game action {name} → {action} ({provider})"));
            let mut fenv = fe::FeEnv { values: &mut self.values, loc: &mut self.loc, nav_events };
            fe.action(&mut fenv, &provider, &action, &param);
        }
    }

    pub(super) fn currency_count(&mut self, cx: &mut Ctx, requests: Value) {
        let data = utility::currency_count_anim_data(cx, &mut self.loc, &requests);
        for sub in self.store.subscribers.iter().filter(|s| s.dp == "dp_utility_currency_count_anim_data") {
            invoke_path(cx, &format!("{}.Publish", sub.path), &[Value::str(&sub.name), data.clone()]);
            self.stats.published.push(format!("{} → {}", sub.name, sub.path));
            self.stats.publishes += 1;
        }
    }

    fn resource_text(&mut self, tag: &str) -> String {
        #[cfg(feature = "net")]
        if tag == "TXT_LE_RESULTS_ADVANCE" && self.fe.as_ref().is_some_and(|f| f.ava.session.rounds.is_some()) {
            return self.loc.text("TXT_AVA_RESULTS_PLAYERS");
        }
        self.loc.text(tag)
    }

    pub(super) fn answer_resources(&mut self, cx: &mut Ctx) {
        for r in std::mem::take(&mut self.resources) {
            if r.kind != 0 {
                // TODO(guess): resource types other than 0 (strings) are not traced (none on the HUD); settle: the resource handler's other types (0x82B3ED08).
                self.stats.resources.insert(r.tag, format!("type {} not answered", r.kind));
                continue;
            }
            let text = self.resource_text(&r.tag);
            self.stats.resources.insert(r.tag.clone(), text.clone());
            let obj = Value::Object(cx.new_object());
            let _ = cx.set_member(&obj, "locString", Value::str(&text));
            invoke_path(cx, &format!("{}.ResourceAcquired", r.path), &[Value::str(&r.tag), obj]);
        }
    }
}
