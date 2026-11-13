use super::view::{call_method, hidden, path_from};
use crate::author::{Logic, MenuKind};
use crate::avm1::builtins::R;
use crate::avm1::value::Value;
use crate::avm1::{Ctx, Fault};

const SHOWN_TOTAL: &str = "__avaScrollTotal";
const SHOWN_FIRST: &str = "__avaScrollFirst";
const MAX_ITEMS: f64 = 1_000_000.0;

pub fn hand_to_menu(cx: &mut Ctx, view: &Value, logic: &Logic, target: &str, menu: &Value) -> R {
    for b in logic.scrollbars.iter().filter(|b| b.menu == target && b.window.is_none()) {
        let bar = path_from(cx, view, &b.target);
        if !bar.is_nullish() {
            call_method(cx, menu, "SetScrollbar", &[bar])?;
        }
    }
    Ok(Value::Undefined)
}

pub fn set_lengths(cx: &mut Ctx, view: &Value, logic: &Logic) -> R {
    for b in &logic.scrollbars {
        let bar = path_from(cx, view, &b.target);
        call_method(cx, &bar, "SetLength", &[Value::Number(f64::from(b.length))])?;
    }
    Ok(Value::Undefined)
}

fn count(cx: &mut Ctx, window: &Value, field: &str) -> Result<f64, Fault> {
    let v = cx.get_member(window, field)?;
    let n = cx.to_number(&v);
    Ok(if n.is_finite() { n.clamp(0.0, MAX_ITEMS).floor() } else { 0.0 })
}

pub fn publish_window(cx: &mut Ctx, view: &Value, logic: &Logic, name: &str, window: &Value) -> R {
    for b in logic.scrollbars.iter().filter(|b| b.window.as_deref() == Some(name)) {
        let rows = match logic.menus.iter().find(|m| m.target == b.menu).map(|m| m.kind) {
            Some(MenuKind::List { rows, .. }) => f64::from(rows),
            _ => continue,
        };
        let bar = path_from(cx, view, &b.target);
        let Value::Object(id) = bar.clone() else { continue };
        if window.is_nullish() {
            continue;
        }
        let (total, first) = (count(cx, window, "total")?, count(cx, window, "first")?);
        let shown_total = cx.get_member(&bar, SHOWN_TOTAL)?;
        let shown_first = cx.get_member(&bar, SHOWN_FIRST)?;
        if shown_total.is_undefined() || cx.to_number(&shown_total) != total {
            call_method(cx, &bar, "SetupScrollbar", &[Value::Number(total), Value::Number(rows)])?;
            call_method(cx, &bar, "SetPosition", &[Value::Number(first)])?;
        } else {
            let step = first - cx.to_number(&shown_first);
            if step == 1.0 {
                call_method(cx, &bar, "Next", &[])?;
            } else if step == -1.0 {
                call_method(cx, &bar, "Previous", &[])?;
            } else if step != 0.0 {
                // PORT(choice): a window that jumps (a wrap from the last row to the first, a page key, the list made
                // again) is placed by `SetPosition` without the arrow's pulse; no paged list of the disc jumps: the
                // disc's one paged list (`activityfeedtracklisting`, 0x17bc…0x19c7) only ever steps one row (`Next` /
                // `Previous` with the pulse) or starts again at 0 with a new total.
                call_method(cx, &bar, "SetPosition", &[Value::Number(first)])?;
            }
        }
        hidden(cx, id, SHOWN_TOTAL, Value::Number(total));
        hidden(cx, id, SHOWN_FIRST, Value::Number(first));
    }
    Ok(Value::Undefined)
}
