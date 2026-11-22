use super::*;

impl GameUi {
    pub fn run_nav(&mut self, vm: &mut Vm, lib: &mut Library, player: &mut Player) {
        if self.nav_wait.is_some() || self.relayout.is_some() {
            return;
        }
        while let Some(action) = self.nav.next_action() {
            match self.nav_action(vm, lib, player, &action) {
                None => self.nav.complete(),
                Some(wait) => {
                    self.nav_wait = Some(wait);
                    break;
                }
            }
        }
    }

    // TODO(guess): the game calls the completion inside `DispatchEvent` (0x82B46760 → 0x82F9B5A8, which takes it out of
    // the map +8 / +0x28 by the view's name and calls its vt+0xC: the Lua continuation 0x825A2768, so the nav's next
    // actions run at once, inside the movie's frame); the port's VM queues its native calls and `take_calls` answers
    // them at the next FE update, so the nav's next actions run one FE update later, after that update's providers;
    // settle: a native the VM answers inside its frame (a synchronous call hook), then this called from there.
    pub(super) fn view_acknowledged(&mut self, entered: bool, view: &str) {
        let wait = match (&self.nav_wait, entered) {
            (Some(NavWait::Enter(v)), true) if v == view => self.nav_wait.take(),
            (Some(NavWait::Exit { view: v, .. }), false) if v == view => self.nav_wait.take(),
            _ => return,
        };
        if let Some(NavWait::Exit { view_model: Some(vm_name), .. }) = wait {
            self.release_view_model(&vm_name);
        }
        self.nav.complete();
    }

    pub(super) fn release_view_model(&mut self, vm_name: &str) {
        let providers = self.store.view_models.get(vm_name).map(|v| v.providers.clone()).unwrap_or_default();
        self.store.deactivate(vm_name);
        let released: Vec<String> = providers.into_iter().filter(|p| !self.store.active.contains(p)).collect();
        for p in &released {
            if p == "hint" {
                self.hint_panel(false);
            }
            if p == "hud" {
                self.hud_destroyed();
            }
            if p == "nisprompt" {
                hud::nis::NisPrompt::destroyed(&mut self.values);
            }
            self.hud.release(p);
        }
        let vm_name = vm_name.to_string();
        self.fe_call(|fe, env| {
            for p in &released {
                fe.deactivate(env, p, &vm_name);
            }
        });
    }

    pub(super) fn nav_action(
        &mut self,
        vm: &mut Vm,
        lib: &mut Library,
        player: &mut Player,
        a: &nav::Action,
    ) -> Option<NavWait> {
        let wait = self.nav_action_inner(vm, lib, player, a);
        self.stats.actions.push(a.to_string());
        wait
    }

    pub(super) fn nav_action_inner(
        &mut self,
        vm: &mut Vm,
        lib: &mut Library,
        player: &mut Player,
        a: &nav::Action,
    ) -> Option<NavWait> {
        match a.name.as_str() {
            "loadView" => {
                let vm_name = a.args.first()?;
                if self.store.loaded_view_models.iter().any(|v| v == vm_name) {
                    return None;
                }
                let constructed: Vec<String> = match self.store.view_models.get(vm_name) {
                    Some(v) => v.providers.iter().filter(|p| !self.store.active.contains(p)).cloned().collect(),
                    None => Vec::new(),
                };
                let Some(view) = self.store.activate(vm_name).map(|v| v.view.clone()) else {
                    self.stats.actions.push(format!("loadView({vm_name}): no such view model"));
                    return None;
                };
                let providers = self.store.view_models.get(vm_name).map(|v| v.providers.clone()).unwrap_or_default();
                if providers.iter().any(|p| p == "hint") {
                    self.hint_panel(true);
                }
                if constructed.iter().any(|p| p == "hud") {
                    self.hud_constructed();
                }
                let name = vm_name.clone();
                self.fe_call(|fe, env| {
                    for p in &constructed {
                        fe.construct(p);
                    }
                    for p in &providers {
                        fe.activate(env, p, &name);
                    }
                });
                self.pending_views.push(view.clone());
                return Some(NavWait::Enter(view));
            }
            "unloadView" => {
                let vm_name = a.args.first()?;
                if !self.store.loaded_view_models.iter().any(|v| v == vm_name) {
                    return None;
                }
                let view = self.store.view_models.get(vm_name).map(|v| v.view.clone())?;
                if let Some(i) = self.pending_views.iter().position(|v| *v == view) {
                    self.pending_views.remove(i);
                    self.release_view_model(vm_name);
                    return None;
                }
                if let Some(i) = self.due_views.iter().position(|v| *v == view) {
                    self.due_views.remove(i);
                    self.release_view_model(vm_name);
                    return None;
                }
                vm.game_event(lib, player, navev::UNLOAD_VIEW, Value::str(&view));
                return Some(NavWait::Exit { view, view_model: Some(vm_name.clone()) });
            }
            "pushPopup" | "popPopup" | "popAllPopups" if self.fe.is_some() => {
                let name = a.args.first().cloned().unwrap_or_default();
                let mode = a.args.get(1).cloned().unwrap_or_default();
                let front = self.fe.as_mut()?;
                let was_empty = front.popups.stack.is_empty();
                let (load, unload) = match a.name.as_str() {
                    "pushPopup" => (front.popups.push(&name, &mode), false),
                    "popPopup" if was_empty => {
                        // PORT(choice): the port goes on (unreached by the game's flows); `popPopup` on an empty stack
                        // (0x82F968C0) neither pops nor continues, so the game's nav would wait for good.
                        front.note(format!("popPopup {name} on an empty stack (the game's handler never completes)"));
                        (false, false)
                    }
                    "popPopup" => (false, front.popups.pop(&name)),
                    _ => {
                        front.popups.stack.clear();
                        (false, !was_empty)
                    }
                };
                front.note(format!("{} {name} → stack {:?}", a.name, front.popups.stack));
                front.publish.push(fe::popup::DP_POPUP.into());
                if load {
                    vm.game_event(lib, player, navev::LOAD_VIEW, Value::str(fe::popup::VIEW));
                    self.loading_views.push(fe::popup::VIEW.into());
                    return Some(NavWait::Enter(fe::popup::VIEW.into()));
                }
                if unload {
                    vm.game_event(lib, player, navev::UNLOAD_VIEW, Value::str(fe::popup::VIEW));
                    return Some(NavWait::Exit { view: fe::popup::VIEW.into(), view_model: None });
                }
            }
            "showHUD" | "hideHUD" => {
                let nis = self.values.bool(values::id::NIS_IS_ACTIVE);
                hud::set_hud_shown(&mut self.values, a.name == "showHUD", nis, self.nav.drop_key, self.free_ride);
            }
            name @ ("set_dimmer_state" | "push_dimmer_state" | "pop_dimmer_state") if self.fe.is_none() => {
                self.dimmer.nav(name, a.args.first().map_or("", String::as_str));
            }
            name => {
                let args = a.args.clone();
                let name = name.to_string();
                self.fe_call(|fe, env| {
                    fe.nav_action(env, &name, &args);
                });
            }
        }
        None
    }
}

impl GameUi {
    pub fn hud_view_files(&self, vm: &mut Vm, lib: &mut Library, player: &mut Player) -> Vec<String> {
        let Some(app) = self.handler.as_deref().and_then(|h| h.strip_suffix(".mFrameworkEventHandler")) else {
            return Vec::new();
        };
        let views = self.store.view_models.values().filter(|m| is_hud_view_model(&m.name)).map(|m| m.view.as_str());
        view_map_files(vm, lib, player, app, views)
    }
}

pub(crate) fn is_hud_view_model(name: &str) -> bool {
    name == "hud" || name.starts_with("hud_")
}

pub(crate) fn view_map_files<'a>(
    vm: &mut Vm,
    lib: &mut Library,
    player: &mut Player,
    app: &str,
    views: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let Some(map) = vm.peek_path(lib, player, &format!("{app}.mNavigator.mViewMapReference.mMap")) else {
        return Vec::new();
    };
    let cx = vm.ctx(lib, player);
    views
        .into_iter()
        .filter_map(|view| match cx.peek_member(&cx.peek_member(&map, view)?, "mViewPath")? {
            Value::String(path) => Some(path.to_string()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::tests::{empty_lib, movie};

    #[test]
    fn the_hud_view_models_are_hud_and_its_containers() {
        for name in ["hud", "hud_hudmeters", "hud_boostnotification", "hud_tutorialhud"] {
            assert!(is_hud_view_model(name), "{name}");
        }
        for name in ["pausemenu", "mainmenu", "huddle", "ava_hub", "eventtimer"] {
            assert!(!is_hud_view_model(name), "{name}");
        }
    }

    fn marks(cx: &mut crate::avm1::Ctx, _: Value, _: &[Value]) -> Result<Value, crate::avm1::Fault> {
        let global = Value::Object(cx.vm.global);
        cx.set_member(&global, "ran", Value::Bool(true))?;
        Ok(Value::str("views/hud/answered.swf"))
    }

    #[test]
    fn the_view_map_names_each_views_file_as_data() {
        let mut lib = empty_lib();
        let m = movie(&mut lib, vec![vec![]]);
        let (mut vm, mut player) = (Vm::new(), Player::new());
        vm.load_level(&mut lib, &mut player, m, 0);
        {
            let mut cx = vm.ctx(&mut lib, &mut player);
            let root = cx.player.levels[&0];
            let mut at = Value::Object(cx.object_for(root));
            for name in ["app", "mNavigator", "mViewMapReference", "mMap"] {
                let o = Value::Object(cx.new_object());
                cx.set_member(&at, name, o.clone()).unwrap();
                at = o;
            }
            for (view, path) in [
                ("hud", Value::str("views/hud.swf")),
                ("hudmeters", Value::str("views/hud/hudmeterscontainer.swf")),
                ("odd", Value::Number(3.0)),
            ] {
                let o = Value::Object(cx.new_object());
                cx.set_member(&o, "mViewPath", path).unwrap();
                cx.set_member(&at, view, o).unwrap();
            }
            let marks = Value::Object(cx.new_native(marks));
            let lazy = Value::Object(cx.new_object());
            let (add, home) = cx.get_method(&lazy, "addProperty").unwrap();
            let added = cx.call(&add, lazy.clone(), &[Value::str("mViewPath"), marks.clone(), Value::Null], home);
            assert!(matches!(added, Ok(Value::Bool(true))));
            cx.set_member(&at, "lazy", lazy).unwrap();
            cx.set_member(&at, "__resolve", marks).unwrap();
        }
        let views = ["hudmeters", "missing", "odd", "lazy", "hud"];
        let files = view_map_files(&mut vm, &mut lib, &mut player, "_level0.app", views);
        assert_eq!(files, ["views/hud/hudmeterscontainer.swf", "views/hud.swf"]);
        assert!(vm.get_path(&mut lib, &mut player, "_global.ran").is_none(), "no getter and no __resolve ran");
        assert!(view_map_files(&mut vm, &mut lib, &mut player, "_level0.other", ["hud"]).is_empty());
    }
}
