use std::sync::Arc;

use crate::author::{InputDo, Logic, MenuKind, MenuOp, PanelHeight, TextFit};
use crate::avm1::Ctx;
use crate::avm1::builtins::{R, arg};
use crate::avm1::object::{ObjId, flags};
use crate::avm1::value::Value;

pub struct ViewClass {
    pub logic: Logic,
}

pub struct CellClass {
    pub def: crate::author::CellDef,
}

pub fn global_path(cx: &mut Ctx, path: &str) -> Value {
    let mut cur = Value::Object(cx.vm.global);
    for p in path.split('.') {
        cur = match cx.get_member(&cur, p) {
            Ok(v) if !v.is_nullish() => v,
            _ => return Value::Undefined,
        };
    }
    cur
}

pub(super) fn path_from(cx: &mut Ctx, base: &Value, path: &str) -> Value {
    let mut cur = base.clone();
    for p in path.split('.') {
        cur = match cx.get_member(&cur, p) {
            Ok(v) if !v.is_nullish() => v,
            _ => return Value::Undefined,
        };
    }
    cur
}

pub(super) fn call_method(cx: &mut Ctx, obj: &Value, method: &str, args: &[Value]) -> R {
    if obj.is_nullish() {
        return Ok(Value::Undefined);
    }
    let (f, home) = cx.get_method(obj, method)?;
    if !cx.is_function(&f) {
        cx.note_missing(obj, method);
        return Ok(Value::Undefined);
    }
    cx.call(&f, obj.clone(), args, home)
}

fn super_call(cx: &mut Ctx, proto_path: &str, method: &str, this: &Value, args: &[Value]) -> R {
    let proto = global_path(cx, &format!("{proto_path}.prototype"));
    if proto.is_nullish() {
        return Ok(Value::Undefined);
    }
    let (f, home) = cx.lookup_from(&proto, this, method)?;
    if !cx.is_function(&f) {
        return Ok(Value::Undefined);
    }
    cx.call(&f, this.clone(), args, home)
}

fn class_of<T: Send + Sync + 'static>(cx: &mut Ctx, this: &Value) -> Option<Arc<T>> {
    let mut cur = cx.proto_of(this);
    for _ in 0..64 {
        let id = cur.as_object()?;
        if let Some(d) = cx.vm.class_data.get(&id).cloned().and_then(|d| d.downcast::<T>().ok()) {
            return Some(d);
        }
        cur = cx.vm.heap.get(id).map(|o| o.proto.clone()).unwrap_or_default();
    }
    None
}

fn constant(cx: &mut Ctx, class: &str, member: &str) -> Value {
    let c = global_path(cx, class);
    let v = cx.get_member(&c, member).unwrap_or_default();
    if !v.is_nullish() {
        return v;
    }
    let proto = cx.get_member(&c, "prototype").unwrap_or_default();
    cx.get_member(&proto, member).unwrap_or_default()
}

fn play_sound(cx: &mut Ctx, name: &str) -> R {
    let event = constant(cx, "gameData.Events", "kFEPlaySFX");
    let id = constant(cx, "gameData.AudioActions", name);
    let dispatcher = global_path(cx, "Dispatcher");
    call_method(cx, &dispatcher, "Action", &[event, id])
}

fn base_class(cx: &mut Ctx, this: &Value) -> &'static str {
    class_of::<ViewClass>(cx, this).map_or(crate::author::Base::View.class(), |c| c.logic.base.class())
}

fn super_construct(cx: &mut Ctx, class: &str, this: Value) -> R {
    let sup = global_path(cx, class);
    if cx.is_function(&sup) {
        let home = cx.get_member(&sup, "prototype")?;
        cx.call(&sup, this, &[], home)?;
    }
    Ok(Value::Undefined)
}

pub fn view_ctor(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let base = base_class(cx, &this);
    super_construct(cx, base, this)
}

fn sd_layout(cx: &mut Ctx, this: &Value, sd: &crate::author::SdDef) -> R {
    let view_util = global_path(cx, "utility.ViewUtil");
    for (target, left) in &sd.moves {
        let clip = path_from(cx, this, target);
        call_method(cx, &view_util, "RepositionIfSD", &[clip, Value::Bool(*left)])?;
    }
    if let Some(k) = sd.scale {
        // PORT(choice): a port screen wider than the 4:3 picture (the server browser, the maps: 93 … 1209 of the 1280
        // stage pixels, where the 4:3 display keeps 160 … 1120) is scaled about the stage's middle there instead of an
        // own 4:3 layout (the port's screens); the disc's screens move their edge clips by 136 (`RepositionIfSD`), and
        // the badges' panel scales `SECONDARY_MENU_SD_XSCALE`.
        let centre = (f64::from(sd.centre.0), f64::from(sd.centre.1));
        crate::game::display_class::scale_if_sd(cx, this, f64::from(k), centre)?;
    }
    Ok(Value::Undefined)
}

pub fn view_initialize(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    let Some(class) = class_of::<ViewClass>(cx, &this) else { return Ok(Value::Undefined) };
    let logic = &class.logic;
    let input_manager = arg(args, 0);
    if let Some(sd) = &logic.sd {
        sd_layout(cx, &this, sd)?;
    }
    for m in &logic.menus {
        let menu = path_from(cx, &this, &m.target);
        let (data, index, cell) = (Value::str(&m.data), Value::str(&m.index), Value::str(&m.cell));
        match m.kind {
            MenuKind::Grid { columns, rows, cell_width, cell_height, spacing_x, spacing_y, wrap } => {
                let n = |v: f32| Value::Number(f64::from(v));
                let setup = [
                    data,
                    index,
                    cell,
                    Value::Number(f64::from(columns)),
                    Value::Number(f64::from(rows)),
                    n(cell_width),
                    n(cell_height),
                    Value::Bool(false),
                ];
                call_method(cx, &menu, "Setup", &setup)?;
                call_method(cx, &menu, "SetCellSpacing", &[n(spacing_x), n(spacing_y)])?;
                call_method(cx, &menu, "AllowWrappingHorizontally", &[Value::Bool(wrap)])?;
                call_method(cx, &menu, "AllowWrappingVertically", &[Value::Bool(wrap)])?;
            }
            MenuKind::List { rows, cell_width, cell_height, spacing, panel_gaps, wrap, use_panel } => {
                let n = |v: f32| Value::Number(f64::from(v));
                let setup = [data, index, cell, Value::Number(f64::from(rows)), n(cell_width), n(cell_height)];
                call_method(cx, &menu, "Setup", &setup)?;
                if use_panel {
                    call_method(cx, &menu, "SetUsePanel", &[Value::Bool(true)])?;
                }
                call_method(cx, &menu, "SetWrapEnabled", &[Value::Bool(wrap)])?;
                call_method(cx, &menu, "SetRowSpacing", &[n(spacing)])?;
                if let Some((top, bottom)) = panel_gaps {
                    call_method(cx, &menu, "SetPanelToRowVerticalSpacing", &[n(top), n(bottom)])?;
                }
                if logic.panels.iter().any(|p| p.height == PanelHeight::FitMenu(m.target.clone())) {
                    let refreshed = cx.new_native(menu_refreshed);
                    call_method(cx, &menu, "SetMenuUpdateCallback", &[Value::Object(refreshed)])?;
                }
            }
        }
        super::scroll::hand_to_menu(cx, &this, logic, &m.target, &menu)?;
        call_method(cx, &menu, "Initialize", std::slice::from_ref(&input_manager))?;
    }
    super_call(cx, logic.base.class(), "Initialize", &this, std::slice::from_ref(&input_manager))?;
    super::scroll::set_lengths(cx, &this, logic)?;
    if let Some(p) = logic.input_priority {
        let priority = constant(cx, "ux.input.InputManager", p);
        call_method(cx, &input_manager, "SetInputPriority", &[this.clone(), priority])?;
    }
    if logic.on_enter.clear_inputs {
        call_method(cx, &this, "ClearInputs", &[])?;
    }
    for p in &logic.panels {
        let panel = path_from(cx, &this, &p.target);
        if let PanelHeight::Fixed(h) = p.height {
            call_method(cx, &panel, "SetSize", &[Value::Number(f64::from(p.width)), Value::Number(f64::from(h))])?;
        }
        if let Some(d) = p.dimmer {
            call_method(cx, &panel, "SetDimmer", &[Value::Bool(true), Value::Number(f64::from(d))])?;
        }
    }
    if let Some(c) = &logic.callouts {
        let bar = path_from(cx, &this, &c.target);
        let style = constant(cx, "components.ButtonWithLabel", "STYLE_NORMAL");
        for (icon, _) in &c.buttons {
            let icon = constant(cx, "components.ButtonCallout", icon);
            call_method(cx, &bar, "AddButton", &[icon, Value::str(""), style.clone()])?;
        }
        let view_class = global_path(cx, "ux.view.View");
        let kind = path_from(cx, &view_class, "ResourceType.LocalizedString");
        let resources = call_method(cx, &this, "GetResourceManager", &[])?;
        for (_, label) in &c.buttons {
            call_method(cx, &resources, "AcquireResource", &[kind.clone(), Value::str(label)])?;
        }
        if !bar.is_nullish() {
            cx.set_member(&bar, "_visible", Value::Bool(true))?;
        }
    }
    for t in logic.texts.iter().filter(|t| t.fit == TextFit::Shrink) {
        let target = path_from(cx, &this, &t.target);
        if !target.is_nullish() {
            cx.set_member(&target, "textAutoSize", Value::str("shrink"))?;
        }
    }
    let providers = call_method(cx, &this, "GetProviderManager", &[])?;
    for d in &logic.data {
        if logic.menus.iter().any(|m| m.data == *d || m.index == *d) {
            continue;
        }
        call_method(cx, &providers, "AddDataProvider", &[Value::str(d)])?;
    }
    for b in &logic.billboards {
        let container = path_from(cx, &this, &b.target);
        call_method(cx, &container, "AddBillboardDataProvider", &[Value::str(&b.data), Value::str(&b.symbol)])?;
    }
    if let Some(event) = logic.on_enter.event {
        let kind = constant(cx, "ssx.UIEvents", event);
        let ctor = global_path(cx, "ux.event.Event");
        if cx.is_function(&ctor) {
            let e = cx.construct(&ctor, &[kind])?;
            let statics = global_path(cx, "ssx.StaticDispatcher");
            let dispatcher = call_method(cx, &statics, "GetDispatcher", &[])?;
            call_method(cx, &dispatcher, "DispatchEvent", &[e])?;
        }
    }
    if let Some(sound) = logic.on_enter.sound {
        play_sound(cx, sound)?;
    }
    // PORT(choice): `unselectableData` is the window's last row (a step down moves the focus to the last row, then the
    // window) in the port's list; the inventory's own grid (`EnabledPagedData`, views/equipmentinventory.gfx 0x0ae1)
    // sends `{maxData: columns × rows, unselectableData: mNumUnselectableGridsTop}` for a carousel whose end columns
    // cannot be selected (`INVENTORY_GRID_NUM_UNSELECTABLE_COLUMNS_LEFT` / `_RIGHT`), which a vertical list has none
    // of.
    if let Some(p) = &logic.paging
        && let Some(MenuKind::List { rows, .. }) = logic.menus.iter().find(|m| m.target == p.menu).map(|m| m.kind)
    {
        let setup = Value::Object(cx.new_object());
        cx.set_member(&setup, "maxData", Value::Number(f64::from(rows)))?;
        cx.set_member(&setup, "unselectableData", Value::Number(f64::from(rows.saturating_sub(1))))?;
        let providers = call_method(cx, &this, "GetProviderManager", &[])?;
        call_method(cx, &providers, "Action", &[Value::str(&p.setup), setup])?;
    }
    Ok(Value::Undefined)
}

pub fn menu_refreshed(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let view = cx.get_member(&this, "_parent")?;
    let Some(class) = class_of::<ViewClass>(cx, &view) else { return Ok(Value::Undefined) };
    for m in &class.logic.menus {
        let MenuKind::List { rows, cell_height, spacing, panel_gaps, .. } = m.kind else { continue };
        let menu = path_from(cx, &view, &m.target);
        if !matches!((&menu, &this), (Value::Object(a), Value::Object(b)) if a == b) {
            continue;
        }
        let items = call_method(cx, &menu, "GetNumMenuItems", &[])?;
        let shown = cx.to_number(&items).clamp(0.0, f64::from(rows)).floor();
        let (top, bottom) = panel_gaps.unwrap_or((0.0, 0.0));
        let height =
            shown * f64::from(cell_height) + (shown - 1.0) * f64::from(spacing) + f64::from(top) + f64::from(bottom);
        for p in class.logic.panels.iter().filter(|p| p.height == PanelHeight::FitMenu(m.target.clone())) {
            let panel = path_from(cx, &view, &p.target);
            call_method(cx, &panel, "SetSize", &[Value::Number(f64::from(p.width)), Value::Number(height)])?;
        }
    }
    Ok(Value::Undefined)
}

pub fn view_publish(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    let Some(class) = class_of::<ViewClass>(cx, &this) else { return Ok(Value::Undefined) };
    let name = cx.to_string(&arg(args, 0))?;
    let value = arg(args, 1);
    for t in class.logic.texts.iter().filter(|t| *t.data == *name) {
        let field = cx.get_member(&value, &t.field)?;
        let text = if field.is_undefined() { Value::str("") } else { Value::String(cx.to_string(&field)?) };
        let target = path_from(cx, &this, &t.target);
        if target.is_nullish() {
            continue;
        }
        match &t.fit {
            TextFit::List { items, more, separator } => {
                let head = cx.to_string(&text)?.to_string();
                fit_list(cx, &target, &value, &head, items, more, separator)?;
            }
            _ => {
                cx.set_member(&target, if t.html { "htmlText" } else { "text" }, text)?;
            }
        }
    }
    for v in class.logic.visibles.iter().filter(|v| *v.data == *name) {
        let field = cx.get_member(&value, &v.field)?;
        let shown = cx.to_bool(&field);
        let target = path_from(cx, &this, &v.target);
        if !target.is_nullish() {
            cx.set_member(&target, "_visible", Value::Bool(shown))?;
        }
    }
    for i in class.logic.images.iter().filter(|i| *i.data == *name) {
        let path = cx.get_member(&value, &i.field)?;
        let path = if path.is_nullish() { String::new() } else { cx.to_string(&path)?.to_string() };
        let loader = path_from(cx, &this, &i.target);
        if loader.is_nullish() {
            continue;
        }
        cx.set_member(&loader, "_visible", Value::Bool(!path.is_empty()))?;
        let shown = cx.get_member(&loader, IMAGE_PATH)?;
        let same = !shown.is_undefined() && *cx.to_string(&shown)? == *path;
        if !path.is_empty() && !same {
            call_method(cx, &loader, "LoadImage", &[Value::str(&path)])?;
        }
        cx.set_member(&loader, IMAGE_PATH, Value::str(&path))?;
    }
    super::scroll::publish_window(cx, &this, &class.logic, &name, &value)?;
    for d in class.logic.drop_panels.iter().filter(|d| *d.data == *name) {
        let panel = path_from(cx, &this, &d.target);
        if panel.is_nullish() {
            continue;
        }
        if value.is_nullish() {
            cx.set_member(&panel, "_visible", Value::Bool(false))?;
            continue;
        }
        let square = constant(cx, "gameData.FEGameData", "PanelType_Square");
        cx.set_member(&value, "ePanelType", square)?;
        cx.set_member(&value, "strIcon", Value::Null)?;
        call_method(cx, &panel, "SetPositionLocked", &[Value::Bool(true)])?;
        let at = [value.clone(), Value::Number(0.0), Value::Number(0.0), Value::Null];
        call_method(cx, &panel, "UpdateWithIndicatorDataObject", &at)?;
        cx.set_member(&panel, "_visible", Value::Bool(true))?;
    }
    Ok(Value::Undefined)
}

const MAX_LIST_ITEMS: u32 = 1024;
const GUTTER_PX: f64 = 2.0;

fn fit_list(cx: &mut Ctx, target: &Value, data: &Value, head: &str, items: &str, more: &str, separator: &str) -> R {
    let list = cx.get_member(data, items)?;
    let n = if list.is_nullish() { Value::Undefined } else { cx.get_member(&list, "length")? };
    let n = cx.to_number(&n);
    let n = if n.is_finite() { n.clamp(0.0, f64::from(MAX_LIST_ITEMS)) as u32 } else { 0 };
    let mut names = Vec::with_capacity(n as usize);
    for i in 0..n {
        let v = cx.get_member(&list, &i.to_string())?;
        names.push(cx.to_string(&v)?.to_string());
    }
    let more = cx.get_member(data, more)?;
    let more = if more.is_nullish() { String::new() } else { cx.to_string(&more)?.to_string() };
    let text = |k: usize| {
        let mut t = format!("{head}{}", names[..k].join(separator));
        if k < names.len() {
            t.push_str(&more.replace("%1s", &(names.len() - k).to_string()));
        }
        t
    };
    let fits = |cx: &mut Ctx, k: usize| -> Result<bool, crate::avm1::Fault> {
        cx.set_member(target, "text", Value::str(&text(k)))?;
        let height = cx.get_member(target, "_height")?;
        let laid = cx.get_member(target, "textHeight")?;
        Ok(cx.to_number(&laid) <= cx.to_number(&height) - 2.0 * GUTTER_PX)
    };
    if fits(cx, names.len())? {
        return Ok(Value::Undefined);
    }
    let (mut lo, mut hi) = (0, names.len());
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if fits(cx, mid)? { lo = mid } else { hi = mid }
    }
    cx.set_member(target, "text", Value::str(&text(lo)))?;
    Ok(Value::Undefined)
}

pub fn view_resource_acquired(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    let Some(class) = class_of::<ViewClass>(cx, &this) else { return Ok(Value::Undefined) };
    let Some(c) = &class.logic.callouts else { return Ok(Value::Undefined) };
    let tag = cx.to_string(&arg(args, 0))?;
    let text = cx.get_member(&arg(args, 1), "locString")?;
    let bar = path_from(cx, &this, &c.target);
    for (i, (_, label)) in c.buttons.iter().enumerate() {
        if **label == *tag {
            call_method(cx, &bar, "SetButtonLabelByIndex", &[Value::Number(i as f64), text.clone()])?;
        }
    }
    Ok(Value::Undefined)
}

fn key_code(cx: &mut Ctx, key: &str) -> R {
    let util = global_path(cx, "utility.ViewUtil");
    match key {
        "ADVANCE" => call_method(cx, &util, "GetAdvanceInputCode", &[]),
        "BACK" => call_method(cx, &util, "GetBackInputCode", &[]),
        k => Ok(constant(cx, "ux.input.InputCode", k)),
    }
}

pub fn view_handle_input(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    let Some(class) = class_of::<ViewClass>(cx, &this) else { return Ok(Value::Bool(false)) };
    let logic = &class.logic;
    let until = cx.get_member(&this, INPUT_LOCKED_UNTIL)?;
    if !until.is_undefined() && cx.vm.timer_ms() < cx.to_number(&until) {
        return Ok(Value::Bool(false));
    }
    let input = arg(args, 0);
    let code = cx.get_member(&input, "KeyCode")?;
    let code = cx.to_number(&code);
    for i in &logic.inputs {
        let want = key_code(cx, i.key)?;
        if want.is_undefined() || cx.to_number(&want) != code {
            continue;
        }
        match &i.what {
            InputDo::Take => {}
            InputDo::Menu { target, op, blocked_sound: Some(sound), .. } => {
                let menu = path_from(cx, &this, target);
                let (can, go) = match op {
                    MenuOp::Up => ("CanHighlightPreviousIndex", "Previous"),
                    _ => ("CanHighlightNextIndex", "Next"),
                };
                let possible = call_method(cx, &menu, can, &[])?;
                if cx.to_bool(&possible) {
                    call_method(cx, &menu, go, &[])?;
                } else {
                    play_sound(cx, sound)?;
                }
            }
            InputDo::Menu { target, op, blocked_sound: None, sound } => {
                if let Some(s) = sound {
                    play_sound(cx, s)?;
                }
                let menu = path_from(cx, &this, target);
                let grid = logic.menus.iter().any(|m| m.target == *target && matches!(m.kind, MenuKind::Grid { .. }));
                let method = match (op, grid) {
                    (MenuOp::Up, true) => "MoveHighlightUp",
                    (MenuOp::Down, true) => "MoveHighlightDown",
                    (MenuOp::Left, true) => "MoveHighlightLeft",
                    (MenuOp::Right, true) => "MoveHighlightRight",
                    (MenuOp::Up | MenuOp::Left, false) => "Previous",
                    (MenuOp::Down | MenuOp::Right, false) => "Next",
                    (MenuOp::Advance, _) => "OnAdvance",
                    (MenuOp::Back, _) => "OnBack",
                    (MenuOp::PageUp, _) => "PreviousPage",
                    (MenuOp::PageDown, _) => "NextPage",
                };
                call_method(cx, &menu, method, &[])?;
            }
            InputDo::Action { name, sound, param } => {
                if let Some(s) = sound {
                    play_sound(cx, s)?;
                }
                let providers = call_method(cx, &this, "GetProviderManager", &[])?;
                let param = param.map_or(Value::Null, Value::Number);
                call_method(cx, &providers, "Action", &[Value::str(name), param])?;
            }
        }
        if let Some(ms) = i.lock_ms {
            let until = Value::Number(cx.vm.timer_ms() + ms);
            let Value::Object(id) = this.clone() else { return Ok(Value::Bool(true)) };
            hidden(cx, id, INPUT_LOCKED_UNTIL, until);
        }
        return Ok(Value::Bool(true));
    }
    Ok(Value::Bool(logic.consume_unmapped))
}

const INPUT_LOCKED_UNTIL: &str = "__avaInputLockedUntil";

pub fn view_terminate(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let base = base_class(cx, &this);
    super_call(cx, base, "Terminate", &this, &[])
}

pub fn cell_ctor(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    super_construct(cx, "ux.view.component.Cell", this.clone())?;
    if matches!(cell_def(cx, &this), Some(crate::author::CellDef::Own { .. })) {
        cell_method(cx, this, "Initialize", &[])?;
    }
    Ok(Value::Undefined)
}

const PENDING: &str = "__avaPending";
const ENABLED: &str = "__avaEnabled";
const IMAGE_PATH: &str = "__avaImage_";
// PORT(choice): an own cell (the port's) dims as the disc's small text cell (100 / 50 %); the disc's `MainMenuCell`
// uses 100 / 10, `ListMenuCellLarge` 100 / 10.
const ALPHA_ENABLED: f64 = 100.0;
const ALPHA_DISABLED: f64 = 50.0;

fn child_call(cx: &mut Ctx, this: &Value, child: &str, method: &str, args: &[Value]) -> R {
    reach_child(cx, this, child, method, args, false)
}

fn title_call(cx: &mut Ctx, this: &Value, child: &str, method: &str, args: &[Value]) -> R {
    reach_child(cx, this, child, method, args, true)
}

fn reach_child(cx: &mut Ctx, this: &Value, child: &str, method: &str, args: &[Value], title: bool) -> R {
    let inner = cx.get_member(this, child)?;
    let (f, home) =
        if inner.is_nullish() { (Value::Undefined, Value::Undefined) } else { cx.get_method(&inner, method)? };
    let pending = cx.get_member(this, PENDING)?;
    if cx.is_function(&f) && pending.is_undefined() {
        return if title { title_frames(cx, &inner, method, args) } else { cx.call(&f, inner, args, home) };
    }
    let call = Value::Object(cx.new_object());
    cx.set_member(&call, "c", Value::str(child))?;
    cx.set_member(&call, "m", Value::str(method))?;
    if title {
        cx.set_member(&call, "t", Value::Bool(true))?;
    }
    let list = Value::Object(cx.new_array(args.to_vec()));
    cx.set_member(&call, "a", list)?;
    let queue = if pending.is_undefined() {
        let q = Value::Object(cx.new_array(Vec::new()));
        cx.set_member(this, PENDING, q.clone())?;
        let replay = cx.new_native(cell_replay);
        cx.set_member(this, "onEnterFrame", Value::Object(replay))?;
        q
    } else {
        pending
    };
    call_method(cx, &queue, "push", &[call])?;
    Ok(Value::Undefined)
}

pub fn cell_replay(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    let queue = cx.get_member(&this, PENDING)?;
    let n = cx.get_member(&queue, "length")?;
    let n = cx.to_number(&n).clamp(0.0, 4096.0) as u32;
    let mut i = 0;
    while i < n {
        let call = cx.get_member(&queue, &i.to_string())?;
        let child = cx.get_member(&call, "c")?;
        let child = cx.to_string(&child)?;
        let method = cx.get_member(&call, "m")?;
        let method = cx.to_string(&method)?;
        let inner = cx.get_member(&this, &child)?;
        let f = if inner.is_nullish() { Value::Undefined } else { cx.get_member(&inner, &method)? };
        if !cx.is_function(&f) {
            break;
        }
        let list = cx.get_member(&call, "a")?;
        let k = cx.get_member(&list, "length")?;
        let k = cx.to_number(&k).clamp(0.0, 16.0) as u32;
        let mut args = Vec::new();
        for j in 0..k {
            args.push(cx.get_member(&list, &j.to_string())?);
        }
        let title = cx.get_member(&call, "t")?;
        if matches!(title, Value::Bool(true)) {
            title_frames(cx, &inner, &method, &args)?;
        } else {
            call_method(cx, &inner, &method, &args)?;
        }
        i += 1;
    }
    if i < n {
        call_method(cx, &queue, "splice", &[Value::Number(0.0), Value::Number(f64::from(i))])?;
        return Ok(Value::Undefined);
    }
    cx.delete_member(&this, PENDING);
    cx.delete_member(&this, "onEnterFrame");
    cx.set_member(&this, "onEnterFrame", Value::Null)?;
    let highlight = cx.get_member(&this, "shouldHighlight")?;
    if matches!(highlight, Value::Bool(true)) {
        cx.set_member(&this, "shouldHighlight", Value::Undefined)?;
        call_method(cx, &this, "Highlight", &[])?;
    }
    Ok(Value::Undefined)
}

fn title_frames(cx: &mut Ctx, inner: &Value, method: &str, args: &[Value]) -> R {
    const CLASS: &str = "components.MainMenuCell";
    let timeline = cx.get_member(inner, "mcCell")?;
    let mut frame = String::new();
    let highlighted = match method {
        "Highlight" => true,
        "Unhighlight" => false,
        "Update" => {
            let data = arg(args, 0);
            let title = cx.get_member(&data, "strItem")?;
            for field in ["mcTitle.txt", "mcTitleGlow.txt"] {
                let text = path_from(cx, &timeline, field);
                if !text.is_nullish() {
                    cx.set_member(&text, "text", title.clone())?;
                }
            }
            let animated = cx.get_member(inner, "mIsAnimatedIn")?;
            if !cx.to_bool(&animated) {
                cx.set_member(inner, "mIsAnimatedIn", Value::Bool(true))?;
                let off = constant(cx, CLASS, "FRAME_OFF");
                frame.push_str(&cx.to_string(&off)?);
            }
            let h = cx.get_member(inner, "mIsHighlighted")?;
            cx.to_bool(&h)
        }
        _ => return Ok(Value::Undefined),
    };
    if method != "Update" {
        cx.set_member(inner, "mIsHighlighted", Value::Bool(highlighted))?;
    }
    let set = constant(cx, CLASS, if highlighted { "FRAME_HIGHLIGHT" } else { "FRAME_UNHIGHLIGHT" });
    frame.push_str(&cx.to_string(&set)?);
    call_method(cx, &timeline, "gotoAndPlay", &[Value::str(&frame)])?;
    if method == "Update" {
        let data = arg(args, 0);
        let focus = cx.get_member(&data, "bFocusable")?;
        let focus = cx.to_bool(&focus);
        call_method(cx, inner, "CanFocus", &[Value::Bool(focus)])?;
        let on = cx.get_member(&data, "bEnabled")?;
        let on = cx.to_bool(&on);
        call_method(cx, inner, if on { "Enable" } else { "Disable" }, &[])?;
    }
    Ok(Value::Undefined)
}

fn cell_def(cx: &mut Ctx, this: &Value) -> Option<crate::author::CellDef> {
    class_of::<CellClass>(cx, this).map(|c| c.def.clone())
}

fn set_enabled(cx: &mut Ctx, this: &Value, on: bool) -> R {
    cx.set_member(this, ENABLED, Value::Bool(on))?;
    cx.set_member(this, "_alpha", Value::Number(if on { ALPHA_ENABLED } else { ALPHA_DISABLED }))?;
    Ok(Value::Undefined)
}

fn forward(cx: &mut Ctx, this: &Value, child: &str, method: &str, args: &[Value]) -> R {
    if method == "IsEnabled" && !cx.get_member(this, PENDING)?.is_undefined() {
        return Ok(Value::Undefined);
    }
    child_call(cx, this, child, method, args)
}

fn cell_method(cx: &mut Ctx, this: Value, method: &str, args: &[Value]) -> R {
    use crate::author::CellDef;
    match cell_def(cx, &this) {
        None => Ok(Value::Undefined),
        Some(CellDef::Forward(child)) => forward(cx, &this, &child, method, args),
        Some(CellDef::ForwardTitle(child)) => match method {
            "Update" | "Highlight" | "Unhighlight" => title_call(cx, &this, &child, method, args),
            _ => forward(cx, &this, &child, method, args),
        },
        Some(CellDef::Own { texts, images, shrink, highlight, width, height }) => match method {
            "Initialize" => {
                for text in &shrink {
                    let field = cx.get_member(&this, text)?;
                    if !field.is_nullish() {
                        cx.set_member(&field, "textAutoSize", Value::str("shrink"))?;
                    }
                }
                let (w, h) = (Value::Number(f64::from(width)), Value::Number(f64::from(height)));
                child_call(cx, &this, &highlight, "SetWidth", &[w])?;
                child_call(cx, &this, &highlight, "SetHeight", &[h])?;
                child_call(cx, &this, &highlight, "SetHighlightImmediate", &[Value::Bool(false)])
            }
            "Update" => {
                let data = arg(args, 0);
                for (text, name) in &texts {
                    let item = cx.get_member(&data, name)?;
                    let item = if item.is_undefined() { Value::str("") } else { Value::String(cx.to_string(&item)?) };
                    let field = cx.get_member(&this, text)?;
                    if !field.is_nullish() {
                        cx.set_member(&field, "text", item)?;
                    }
                }
                for (loader, name) in &images {
                    let path = cx.get_member(&data, name)?;
                    let path = if path.is_nullish() { String::new() } else { cx.to_string(&path)?.to_string() };
                    let clip = cx.get_member(&this, loader)?;
                    if clip.is_nullish() {
                        continue;
                    }
                    cx.set_member(&clip, "_visible", Value::Bool(!path.is_empty()))?;
                    let key = format!("{IMAGE_PATH}{loader}");
                    let shown = cx.get_member(&this, &key)?;
                    let same = !shown.is_undefined() && *cx.to_string(&shown)? == *path;
                    if !path.is_empty() && !same {
                        child_call(cx, &this, loader, "LoadImage", &[Value::str(&path)])?;
                    }
                    cx.set_member(&this, &key, Value::str(&path))?;
                }
                let on = cx.get_member(&data, "bEnabled")?;
                let on = cx.to_bool(&on);
                set_enabled(cx, &this, on)
            }
            "Highlight" => child_call(cx, &this, &highlight, "SetHighlight", &[Value::Bool(true)]),
            "Unhighlight" => child_call(cx, &this, &highlight, "SetHighlight", &[Value::Bool(false)]),
            "Enable" => set_enabled(cx, &this, true),
            "Disable" => set_enabled(cx, &this, false),
            "IsEnabled" => {
                let v = cx.get_member(&this, ENABLED)?;
                Ok(Value::Bool(!matches!(v, Value::Bool(false))))
            }
            "Attached" => {
                cx.set_member(&this, "mCellIndex", arg(args, 0))?;
                cx.set_member(&this, "mSelectedCB", arg(args, 1))?;
                Ok(Value::Undefined)
            }
            _ => Ok(Value::Undefined),
        },
    }
}

macro_rules! cell_methods {
    ($($f:ident => $m:literal),* $(,)?) => {
        $(pub fn $f(cx: &mut Ctx, this: Value, args: &[Value]) -> R { cell_method(cx, this, $m, args) })*
        pub const CELL_METHODS: &[(&str, crate::avm1::object::NativeFn)] = &[$(($m, $f)),*];
    };
}

cell_methods! {
    cell_initialize => "Initialize",
    cell_attached => "Attached",
    cell_terminate => "Terminate",
    cell_update => "Update",
    cell_enable => "Enable",
    cell_disable => "Disable",
    cell_can_focus => "CanFocus",
    cell_highlight => "Highlight",
    cell_unhighlight => "Unhighlight",
    cell_is_enabled => "IsEnabled",
}

pub const VIEW_METHODS: &[(&str, crate::avm1::object::NativeFn)] = &[
    ("Initialize", view_initialize),
    ("Publish", view_publish),
    ("ResourceAcquired", view_resource_acquired),
    ("HandleInput", view_handle_input),
    ("Terminate", view_terminate),
];

pub fn native(cx: &mut Ctx, f: crate::avm1::object::NativeFn) -> ObjId {
    cx.new_native(f)
}

pub fn hidden(cx: &mut Ctx, obj: ObjId, name: &str, v: Value) {
    cx.define(obj, name, v, flags::DONT_ENUM | flags::DONT_DELETE);
}
