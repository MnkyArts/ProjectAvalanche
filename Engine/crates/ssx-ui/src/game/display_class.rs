use std::collections::HashMap;

use crate::avm1::builtins::{R, arg};
use crate::avm1::value::Value;
use crate::avm1::{Ctx, Fault, Vm};
use crate::display::{InstanceId, Player};
use crate::library::Library;

use super::GameUi;

pub const SD_PROPORTIONS_UPPER_BOUND: f64 = 1.55;
pub const WIDESCREEN_TO_SD_TRANSLATION: f64 = 136.0;
pub const MAX_SD_MOVES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayClass {
    pub widescreen: bool,
}

impl DisplayClass {
    pub const WIDESCREEN: DisplayClass = DisplayClass { widescreen: true };

    pub fn of_window(width: u32, height: u32) -> DisplayClass {
        // PORT(choice): a PC window has no widescreen flag, so its shape is classed by the movies' own test
        // (`ViewUtil.IsWidescreen`: displayWidth / displayHeight > 1.55, classloader.gfx 0x0f9f): 16:10 (1.6) is
        // widescreen, 3:2 (1.5) and 4:3 are not; the console told the movies its video mode's widescreen flag
        // (`XGetVideoMode` at 0x82FB57A0: 1280 × 720 or 960 × 720).
        let widescreen = height == 0 || f64::from(width) / f64::from(height) > SD_PROPORTIONS_UPPER_BOUND;
        DisplayClass { widescreen }
    }

    pub fn size(self) -> (u32, u32) {
        if self.widescreen { (1280, 720) } else { (960, 720) }
    }
}

fn view_util(cx: &mut Ctx) -> Value {
    let global = Value::Object(cx.vm.global);
    let utility = cx.get_member(&global, "utility").unwrap_or_default();
    if utility.is_nullish() {
        return Value::Undefined;
    }
    cx.get_member(&utility, "ViewUtil").unwrap_or_default()
}

fn number(cx: &mut Ctx, obj: &Value, name: &str) -> Result<f64, crate::avm1::Fault> {
    let v = cx.get_member(obj, name)?;
    Ok(cx.to_number(&v))
}

fn is_widescreen(cx: &mut Ctx) -> Result<bool, Fault> {
    let vu = view_util(cx);
    let (f, home) = cx.get_method(&vu, "IsWidescreen")?;
    let wide = cx.call(&f, vu, &[], home)?;
    Ok(cx.to_bool(&wide))
}

fn keep<V>(kept: &mut HashMap<InstanceId, V>, player: &Player, id: InstanceId, v: V) {
    if kept.len() >= MAX_SD_MOVES && !kept.contains_key(&id) {
        kept.retain(|c, _| player.get(*c).is_some());
    }
    if kept.len() < MAX_SD_MOVES || kept.contains_key(&id) {
        kept.insert(id, v);
    }
}

pub fn install(cx: &mut Ctx) -> bool {
    let vu = view_util(cx);
    if vu.as_object().is_none() {
        return false;
    }
    let f = Value::Object(cx.new_native(reposition_if_sd));
    cx.set_member(&vu, "RepositionIfSD", f).is_ok()
}

fn reposition_if_sd(cx: &mut Ctx, _this: Value, a: &[Value]) -> R {
    let clip = arg(a, 0);
    let vu = view_util(cx);
    let wide = is_widescreen(cx)?;
    let mut left = arg(a, 1);
    if left.is_nullish() {
        let x = number(cx, &clip, "_x")?;
        let width = number(cx, &clip, "_width")?;
        let stage = number(cx, &vu, "STAGE_WIDTH")?;
        left = Value::Bool(x < stage - x - width);
    }
    let anchored_left = cx.to_bool(&left);
    if !wide {
        let x = number(cx, &clip, "_x")?;
        let t = number(cx, &vu, "WIDESCREEN_TO_SD_TRANSLATION")?;
        cx.set_member(&clip, "_x", Value::Number(if anchored_left { x + t } else { x - t }))?;
    }
    if let Some(id) = cx.display_of(&clip) {
        keep(&mut cx.vm.natives.sd_moves, cx.player, id, anchored_left);
        // PORT(choice): for windows wider than 16:9 — the HUD's `RepositionIfSD` clips (its side panels, under an
        // `ssx_hud*` clip) move out by the window's extra stage width towards their side; the console's displays are
        // 16:9 or 4:3.
        if in_hud(cx.player, id) {
            let extra = cx.player.hud_extra;
            if extra > 0.0 {
                let x = number(cx, &clip, "_x")?;
                cx.set_member(&clip, "_x", Value::Number(if anchored_left { x - extra } else { x + extra }))?;
            }
            keep(&mut cx.vm.natives.hud_wide, cx.player, id, (anchored_left, extra));
        }
    }
    Ok(Value::Undefined)
}

fn in_hud(player: &Player, id: InstanceId) -> bool {
    let mut cur = Some(id);
    let mut steps = 0;
    while let Some(c) = cur {
        let Some(i) = player.get(c) else { return false };
        if i.name.starts_with("ssx_hud") {
            return true;
        }
        cur = i.parent;
        steps += 1;
        if steps > 64 {
            return false;
        }
    }
    false
}

pub fn hud_extra_for(width: u32, height: u32) -> f64 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let visible = 720.0 * f64::from(width) / f64::from(height);
    ((visible - 1280.0) / 2.0).max(0.0).floor()
}

pub fn scale_if_sd(cx: &mut Ctx, view: &Value, k: f64, centre: (f64, f64)) -> Result<(), Fault> {
    if !is_widescreen(cx)? {
        scale_about(cx, view, k, centre)?;
    }
    if let Some(id) = cx.display_of(view) {
        keep(&mut cx.vm.natives.sd_scales, cx.player, id, [k, centre.0, centre.1]);
    }
    Ok(())
}

fn scale_about(cx: &mut Ctx, clip: &Value, k: f64, centre: (f64, f64)) -> Result<(), Fault> {
    for (prop, c) in [("_x", Some(centre.0)), ("_y", Some(centre.1)), ("_xscale", None), ("_yscale", None)] {
        let v = number(cx, clip, prop)?;
        let to = match c {
            Some(c) => c + (v - c) * k,
            None => v * k,
        };
        cx.set_member(clip, prop, Value::Number(to))?;
    }
    Ok(())
}

fn move_kept(cx: &mut Ctx, widescreen: bool) -> usize {
    let vu = view_util(cx);
    let t = number(cx, &vu, "WIDESCREEN_TO_SD_TRANSLATION").unwrap_or(f64::NAN);
    let mut moves = std::mem::take(&mut cx.vm.natives.sd_moves);
    moves.retain(|c, _| cx.player.get(*c).is_some());
    let mut n = 0;
    for (&id, &anchored_left) in &moves {
        let clip = Value::Object(cx.object_for(id));
        let Ok(x) = number(cx, &clip, "_x") else { continue };
        let inward = if anchored_left { t } else { -t };
        let to = if widescreen { x - inward } else { x + inward };
        if cx.set_member(&clip, "_x", Value::Number(to)).is_ok() {
            n += 1;
        }
    }
    cx.vm.natives.sd_moves = moves;
    let mut scales = std::mem::take(&mut cx.vm.natives.sd_scales);
    scales.retain(|c, _| cx.player.get(*c).is_some());
    for (&id, &[k, x, y]) in &scales {
        let view = Value::Object(cx.object_for(id));
        if scale_about(cx, &view, if widescreen { 1.0 / k } else { k }, (x, y)).is_ok() {
            n += 1;
        }
    }
    cx.vm.natives.sd_scales = scales;
    n
}

impl GameUi {
    pub fn set_hud_extra(&mut self, vm: &mut Vm, lib: &mut Library, player: &mut Player, extra: f64) {
        if (extra - player.hud_extra).abs() < 0.5 {
            return;
        }
        player.hud_extra = extra;
        let mut cx = vm.ctx(lib, player);
        let mut kept = std::mem::take(&mut cx.vm.natives.hud_wide);
        kept.retain(|c, _| cx.player.get(*c).is_some());
        for (&id, (left, applied)) in kept.iter_mut() {
            let clip = Value::Object(cx.object_for(id));
            let Ok(x) = number(&mut cx, &clip, "_x") else { continue };
            let d = extra - *applied;
            if cx.set_member(&clip, "_x", Value::Number(if *left { x - d } else { x + d })).is_ok() {
                *applied = extra;
            }
        }
        cx.vm.natives.hud_wide = kept;
    }

    pub fn set_display(&mut self, vm: &mut Vm, lib: &mut Library, player: &mut Player, class: DisplayClass) {
        let changed = class != self.display;
        self.display = class;
        let (w, h) = class.size();
        super::set_display(&mut self.values, w as i32, h as i32);
        let mut cx = vm.ctx(lib, player);
        crate::avm1::natives::set_game_globals(&mut cx, w, h, class.widescreen);
        let vu = view_util(&mut cx);
        let cached = cx.get_member(&vu, "mIsWidescreen").unwrap_or_default();
        if !cached.is_nullish()
            && let Err(e) = cx.set_member(&vu, "mIsWidescreen", Value::Bool(class.widescreen))
        {
            self.display_note(format!("display: ViewUtil.mIsWidescreen not set: {e:?}"));
        }
        if !changed {
            return;
        }
        // PORT(choice): a window crossing the 1.55 bound gets the new display at once in `_global`, the UI values
        // `FE_Display*` and `ViewUtil.mIsWidescreen` (the per-update readers), the kept `RepositionIfSD` clips (205
        // calls in 54 movies) moved by 136, the hint panel's places and the globe's peak shots for it, and every loaded
        // view unloaded and loaded again at the front end's next rest (35 `IsWidescreen` reads in 27 movies, most at a
        // view's initialisation); the port's window; the console's display never changed while the game ran
        // (`XGetVideoMode` once, 0x82FB57A0).
        let moved = move_kept(&mut cx, class.widescreen);
        self.hud.hint.set_wide(&mut cx, class.widescreen);
        if let Some(fe) = self.fe.as_mut() {
            fe.set_widescreen(class.widescreen);
        }
        let laid_out = !self.store.loaded_view_models.is_empty();
        if !laid_out {
            self.views_display = class;
        }
        self.relayout_due = laid_out && class != self.views_display;
        self.display_note(format!(
            "display: {w} × {h} ({}), {moved} clips moved{}",
            if class.widescreen { "widescreen" } else { "4:3" },
            if self.relayout_due { ", the views to be loaded again" } else { "" }
        ));
    }
}

impl GameUi {
    pub(super) fn display_note(&mut self, line: String) {
        if let Some(fe) = self.fe.as_mut() {
            fe.note(&line);
        }
        self.stats.actions.push(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avm1::natives::set_game_globals;
    use crate::display::hit::tests::{at, hit_movie};
    use crate::display::tests::empty_lib;

    fn is_widescreen_global(cx: &mut Ctx, _: Value, _: &[Value]) -> R {
        let global = Value::Object(cx.vm.global);
        cx.get_member(&global, "isWidescreen")
    }

    fn world(widescreen: bool) -> (Vm, Library, Player) {
        let mut lib = empty_lib();
        let m = hit_movie(
            &mut lib,
            vec![at(1, 20, "a", 0, 0, 1.0), at(2, 20, "b", 1200, 0, 1.0), at(3, 20, "v", 100, 50, 1.0)],
        );
        let (mut vm, mut player) = (Vm::new(), Player::new());
        vm.load_level(&mut lib, &mut player, m, 0);
        {
            let mut cx = vm.ctx(&mut lib, &mut player);
            let global = Value::Object(cx.vm.global);
            let (utility, vu) = (Value::Object(cx.new_object()), Value::Object(cx.new_object()));
            cx.set_member(&global, "utility", utility.clone()).unwrap();
            cx.set_member(&utility, "ViewUtil", vu.clone()).unwrap();
            cx.set_member(&vu, "STAGE_WIDTH", Value::Number(1280.0)).unwrap();
            cx.set_member(&vu, "WIDESCREEN_TO_SD_TRANSLATION", Value::Number(WIDESCREEN_TO_SD_TRANSLATION)).unwrap();
            let f = Value::Object(cx.new_native(is_widescreen_global));
            cx.set_member(&vu, "IsWidescreen", f).unwrap();
            assert!(install(&mut cx));
            let (w, h) = DisplayClass { widescreen }.size();
            set_game_globals(&mut cx, w, h, widescreen);
        }
        (vm, lib, player)
    }

    fn clip(cx: &mut Ctx, name: &str) -> Value {
        let id = cx.player.child_by_name(cx.player.levels[&0], name, true).expect(name);
        Value::Object(cx.object_for(id))
    }

    fn place(cx: &mut Ctx, name: &str) -> (f64, f64, f64) {
        let c = clip(cx, name);
        (number(cx, &c, "_x").unwrap(), number(cx, &c, "_y").unwrap(), number(cx, &c, "_xscale").unwrap())
    }

    #[test]
    fn the_sd_moves_are_kept_for_a_crossing() {
        let (mut vm, mut lib, mut player) = world(false);
        let mut cx = vm.ctx(&mut lib, &mut player);
        let vu = view_util(&mut cx);
        let (a, b, v) = (clip(&mut cx, "a"), clip(&mut cx, "b"), clip(&mut cx, "v"));
        let (f, home) = cx.get_method(&vu, "RepositionIfSD").unwrap();
        cx.call(&f, vu.clone(), &[a], home.clone()).unwrap();
        cx.call(&f, vu.clone(), &[b, Value::Bool(false)], home).unwrap();
        scale_if_sd(&mut cx, &v, 0.5, (640.0, 360.0)).unwrap();
        let sd = [(136.0, 0.0, 100.0), (1064.0, 0.0, 100.0), (370.0, 205.0, 50.0)];
        let now = |cx: &mut Ctx| [place(cx, "a"), place(cx, "b"), place(cx, "v")];
        assert_eq!(now(&mut cx), sd);
        set_game_globals(&mut cx, 1280, 720, true);
        assert_eq!(move_kept(&mut cx, true), 3);
        assert_eq!(now(&mut cx), [(0.0, 0.0, 100.0), (1200.0, 0.0, 100.0), (100.0, 50.0, 100.0)]);
        set_game_globals(&mut cx, 960, 720, false);
        assert_eq!(move_kept(&mut cx, false), 3);
        assert_eq!(now(&mut cx), sd);

        let (mut vm, mut lib, mut player) = world(true);
        let mut cx = vm.ctx(&mut lib, &mut player);
        let vu = view_util(&mut cx);
        let (a, v) = (clip(&mut cx, "a"), clip(&mut cx, "v"));
        let (f, home) = cx.get_method(&vu, "RepositionIfSD").unwrap();
        cx.call(&f, vu.clone(), &[a, Value::Bool(true)], home).unwrap();
        scale_if_sd(&mut cx, &v, 0.5, (640.0, 360.0)).unwrap();
        assert_eq!((place(&mut cx, "a"), place(&mut cx, "v")), ((0.0, 0.0, 100.0), (100.0, 50.0, 100.0)));
        set_game_globals(&mut cx, 960, 720, false);
        assert_eq!(move_kept(&mut cx, false), 2);
        assert_eq!((place(&mut cx, "a"), place(&mut cx, "v")), ((136.0, 0.0, 100.0), (370.0, 205.0, 50.0)));
    }

    #[test]
    fn the_window_is_classed_by_the_movies_bound() {
        let wide = |w, h| DisplayClass::of_window(w, h).widescreen;
        for (w, h) in [(1920, 1080), (2560, 1080), (3440, 1440), (5120, 1440), (1280, 800), (1600, 900)] {
            assert!(wide(w, h), "{w} x {h}");
        }
        for (w, h) in [(1024, 768), (1600, 1200), (1500, 1000), (1550, 1000), (900, 1600)] {
            assert!(!wide(w, h), "{w} x {h}");
        }
        assert!(wide(0, 0));
        assert_eq!(DisplayClass::of_window(1024, 768).size(), (960, 720));
        assert_eq!(DisplayClass::of_window(1920, 1080).size(), (1280, 720));
    }
}
