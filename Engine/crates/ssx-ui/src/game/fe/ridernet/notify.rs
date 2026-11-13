use std::collections::VecDeque;

use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{ActionParam, FeEnv, FrontEnd};
use crate::game::loc::Localization;

pub const DP: &str = "dp_notification_data";

pub const KIND_BADGE: u32 = 0x14;
pub const KIND_GEOTAG: u32 = 0xE;
pub const QUEUE_MAX: usize = 25;

// PORT(choice): the port's notices show as the docks' generic notification (style 0: the gamer picture, the header, the
// body; the rider's icon as the picture), non-actionable, held until the next dock asks (the results' DockPostGame, not
// the ride's dock that unloads with the switch) and then taken by request 1 as types 7/0xE/0x10/0x14/0x17 are, 6 s, the
// generic sounds 0x1E/0x1F; a design choice to review; the game has no such notice (port feature).
pub const KIND_PORT: u32 = 0x100;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notification {
    pub kind: u32,
    pub id: i32,
    pub title: String,
    pub message: String,
    pub icon: String,
}

#[derive(Clone, Debug, Default)]
pub struct Notifications {
    pub queue: VecDeque<Notification>,
    pub current: Option<Notification>,
    pub request: u32,
    pub expanded: bool,
    next_id: i32,
    held: Vec<Notification>,
}

pub fn style(kind: u32) -> i32 {
    match kind {
        10 | 0xB => 2,
        0x11 => 3,
        0x14 => 1,
        _ => 0,
    }
}

pub fn display_time(kind: u32) -> i32 {
    match kind {
        1 | 4 | 6 | 7 | 0xB | 0xC | 0xD | 0x13 | 0x15 | 0x18 => 10000,
        0x14 | 0x16 | 0x17 => 3000,
        _ => 6000,
    }
}

pub fn sounds(kind: u32) -> (i32, i32) {
    match kind {
        1 | 3 | 4 | 0x13 => (0x20, 0x21),
        8 => (0x22, 0x23),
        9..=0xB => (0x24, 0x25),
        0xE..=0x11 => (0x26, 0x27),
        0x14 => (0x1A, 0x1B),
        0x16 => (0x1C, 0x1D),
        _ => (0x1E, 0x1F),
    }
}

pub fn actionable(kind: u32) -> bool {
    matches!(kind, 2 | 3 | 4 | 6 | 8 | 9 | 10 | 0xC | 0xD | 0xF | 0x13 | 0x15)
}

impl Notifications {
    pub fn push(&mut self, mut n: Notification) {
        n.id = self.next_id;
        self.next_id += 1;
        self.queue.push_back(n);
        while self.queue.len() > QUEUE_MAX {
            self.queue.pop_front();
        }
    }

    pub fn hold(&mut self, n: Notification) {
        self.held.push(n);
        if self.held.len() > QUEUE_MAX {
            self.held.remove(0);
        }
    }

    pub fn set_request(&mut self, n: u32) {
        self.request = n;
        if n != 0 {
            for h in std::mem::take(&mut self.held) {
                self.push(h);
            }
        }
    }

    fn take(&mut self) -> Option<Notification> {
        let all = match self.request {
            1 => false,
            2 => true,
            _ => return None,
        };
        let i = self.queue.iter().position(|n| all || matches!(n.kind, 7 | 0xE | 0x10 | 0x14 | 0x17 | KIND_PORT))?;
        self.queue.remove(i)
    }
}

pub fn post_badge(fe: &mut FrontEnd, loc: &mut Localization, b: &ssx_assets::career::profile::BadgeText) {
    let n = Notification {
        kind: KIND_BADGE,
        id: 0,
        title: loc.text(&b.title),
        message: loc.text(&b.description),
        icon: b.icon.clone(),
    };
    fe.note(format!("notification: badge {} ({})", b.name, n.title));
    fe.notifications.push(n);
}

pub fn post_geotag(fe: &mut FrontEnd, loc: &mut Localization, value: i64, collected: usize, total: usize) {
    let title = loc.text("TXT_NOTIFY_GEOTAG_COLLECTED_TITLE");
    let money = loc.currency(value, true);
    let count = loc.int(collected as i64);
    let of = loc.int(total as i64);
    let message = loc.format("TXT_NOTIFY_GEOTAG_OFFLINE_COLLECTED_BODY", &[&money, &count, &of]);
    let n = Notification {
        kind: KIND_GEOTAG,
        id: 0,
        title,
        message,
        icon: "assets\\notificationIcons\\notification_geotag.dds".into(),
    };
    fe.note(format!("notification: geotag collected ({value} credits, {collected} of {total})"));
    fe.notifications.push(n);
}

pub fn post_best_not_kept(
    fe: &mut FrontEnd,
    loc: &mut Localization,
    why: ssx_assets::career::profile::BestNotKept,
    icon: &str,
) {
    use ssx_assets::career::profile::BestNotKept;
    let body = match why {
        BestNotKept::ModStateFull => "TXT_AVA_BEST_NOT_KEPT_FULL",
        BestNotKept::NoEventId => "TXT_AVA_BEST_NOT_KEPT_NO_ID",
    };
    let n = Notification {
        kind: KIND_PORT,
        id: 0,
        title: loc.text("TXT_AVA_BEST_NOT_KEPT_TITLE"),
        message: loc.text(body),
        icon: icon.to_string(),
    };
    fe.note(format!("notification: best not kept ({why:?}), held for the next dock that asks"));
    fe.notifications.hold(n);
}

pub const KIND_HINT: u32 = 0x18;

pub fn post_hint(fe: &mut FrontEnd, loc: &mut Localization, r: &crate::game::fe::hint::Record) {
    let page = r.pages.first().cloned().unwrap_or_default();
    let n = Notification {
        kind: KIND_HINT,
        id: 0,
        title: loc.text(&r.title),
        message: loc.text(&page.body),
        icon: page.images[0].clone(),
    };
    fe.note(format!("notification: tutorial hint {:#010x} ({})", r.key, n.title));
    fe.notifications.push(n);
}

pub fn update(fe: &mut FrontEnd, env: &mut FeEnv) {
    if env.values.int(crate::game::values::id::FLOW_RECEIVE_NOTIFICATIONS) != 1 || fe.notifications.current.is_some() {
        return;
    }
    if let Some(n) = fe.notifications.take() {
        fe.note(format!("notification {} published (type {:#x})", n.id, n.kind));
        fe.notifications.current = Some(n);
        fe.publish.push(DP.into());
    }
}

pub fn action(fe: &mut FrontEnd, action: &str, param: &ActionParam) {
    match action {
        "A_NotificationExecute" | "A_NotificationRelease" => fe.notifications.current = None,
        "A_NotificationSetRequestType" => {
            let n = param.number as u32;
            if n <= 2 {
                fe.notifications.set_request(n);
            }
        }
        "A_NotificationSetExpanded" => fe.notifications.expanded = param.number != 0.0,
        _ => {}
    }
}

pub fn build(fe: &FrontEnd, cx: &mut Ctx) -> Value {
    let o = Value::Object(cx.new_object());
    let n = fe.notifications.current.clone().unwrap_or_default();
    let (open, close) = sounds(n.kind);
    for (k, v) in [
        ("messagetitle", Value::str(&n.title)),
        ("message", Value::str(&n.message)),
        ("iconurl", Value::str(&n.icon)),
        ("moneyLabel", Value::str("")),
        ("moneyLocalized", Value::str("")),
        ("bracketLocalized", Value::str("")),
        ("numGeotags", Value::Number(0.0)),
        ("bracket", Value::Number(0.0)),
        ("isActionable", Value::Bool(actionable(n.kind))),
        ("openSound", Value::Number(f64::from(open))),
        ("closeSound", Value::Number(f64::from(close))),
        ("style", Value::Number(f64::from(style(n.kind)))),
        ("displayTime", Value::Number(f64::from(display_time(n.kind)))),
    ] {
        let _ = cx.set_member(&o, k, v);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_drops_its_oldest_and_a_request_takes_its_kind() {
        let mut q = Notifications::default();
        for i in 0..30 {
            q.push(Notification { kind: if i == 29 { KIND_BADGE } else { 1 }, ..Default::default() });
        }
        assert_eq!(q.queue.len(), QUEUE_MAX);
        assert_eq!(q.queue.front().map(|n| n.id), Some(5));
        assert!(q.take().is_none(), "no request");
        q.request = 1;
        assert_eq!(q.take().map(|n| n.kind), Some(KIND_BADGE));
        assert!(q.take().is_none(), "type 1 waits for request 2");
        q.request = 2;
        assert_eq!(q.take().map(|n| n.id), Some(5));
        assert_eq!((style(KIND_BADGE), display_time(KIND_BADGE), sounds(KIND_BADGE)), (1, 3000, (26, 27)));
        assert_eq!(
            (style(KIND_GEOTAG), display_time(KIND_GEOTAG), sounds(KIND_GEOTAG), actionable(KIND_GEOTAG)),
            (0, 6000, (0x26, 0x27), false)
        );
        q.request = 1;
        q.hold(Notification { kind: KIND_PORT, ..Default::default() });
        assert!(q.take().is_none(), "held from the dock that asked before it");
        q.set_request(0);
        assert!(q.take().is_none(), "a provider's reset does not ask");
        q.set_request(1);
        assert_eq!(q.take().map(|n| n.kind), Some(KIND_PORT));
        q.queue.clear();
        for i in 0..30 {
            q.hold(Notification { kind: KIND_PORT, message: i.to_string(), ..Default::default() });
        }
        q.set_request(2);
        assert_eq!(q.queue.len(), QUEUE_MAX);
        assert_eq!(q.queue.front().map(|n| n.message.as_str()), Some("5"));
        assert_eq!(
            (style(KIND_PORT), display_time(KIND_PORT), sounds(KIND_PORT), actionable(KIND_PORT)),
            (0, 6000, (0x1E, 0x1F), false)
        );
    }
}
