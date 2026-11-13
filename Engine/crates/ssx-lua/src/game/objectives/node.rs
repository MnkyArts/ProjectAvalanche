use super::{Cmp, Ctx, Effect, FloatBinding, GsValue, Name, RacerQuery, StatKey, StatValue};

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    And { a: Box<Node>, b: Option<Box<Node>>, a_held: bool },
    Or { a: Box<Node>, b: Option<Box<Node>>, a_held: bool, b_held: bool },
    Not(Option<Box<Node>>),
    Wait(Wait),
    Do(Do),
    Restart,
    Stats(StatsBlock),
    Nop,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatsBlock {
    pub badge: Name,
    pub waits: Vec<Wait>,
    pub closed: bool,
    pub swallowed: u32,
    pub reported: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Wait {
    Time { duration: f32, elapsed: f32 },
    GameState { entity: Name, id: u16, cmp: Cmp, value: GsValue },
    NamedFlag(Name),
    EventType(i32),
    ObjectiveComplete(Name),
    PostedEvent { event: Name, entity: Option<Name> },
    RemainingDistance { entity: Name, distance: f32 },
    TriggerByName(Name),
    TriggerByGroup(u16),
    Equipped { item: i32, equipment_type: i32, held: bool },
    WorldEntityValue { a: FloatBinding, cmp: Cmp, b: FloatBinding },
    Night,
    TutorialMessageReady,
    TutorialTricks { entity: Name },
    ObjectiveMessage(ObjectiveMessageWait),
    BadgeSet { badge: Name, kind: i32, value: bool },
    Stat(StatWait),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectiveMessageWait {
    pub entity: Name,
    pub message: Option<super::DropMessage>,
    pub shown: bool,
    pub time: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatWait {
    pub key: StatKey,
    pub cmp: Cmp,
    pub value: f64,
    pub kind: u8,
    pub progress: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    Effect(Effect),
    NamedFlag { name: Name, value: bool, temporary: bool },
    SurviveFog { source: Name, target: Name, blend: f32, fraction: f32, target_on: bool, begun: bool },
}

impl Node {
    pub fn complete(&self) -> bool {
        match self {
            Node::And { b, .. } | Node::Or { b, .. } => b.as_ref().is_some_and(|n| n.complete()),
            Node::Not(c) => c.as_ref().is_some_and(|n| n.complete()),
            Node::Stats(s) => s.closed,
            _ => true,
        }
    }

    pub fn set_operand(&mut self, n: Node) {
        match self {
            Node::And { b, .. } | Node::Or { b, .. } => match b {
                Some(b) => b.set_operand(n),
                None => *b = Some(Box::new(n)),
            },
            Node::Not(c) => *c = Some(Box::new(n)),
            Node::Stats(s) => s.swallowed += 1,
            _ => {}
        }
    }

    pub fn begin(&mut self, cx: &mut Ctx) {
        match self {
            Node::And { a, b, .. } | Node::Or { a, b, .. } => {
                a.begin(cx);
                if let Some(b) = b {
                    b.begin(cx);
                }
            }
            Node::Not(Some(c)) => c.begin(cx),
            Node::Wait(w) => w.begin(cx),
            Node::Do(Do::SurviveFog { source, begun, .. }) => {
                *begun = true;
                cx.effect(Effect::SurviveFogBegin { source: source.clone() });
            }
            _ => {}
        }
    }

    pub fn test(&mut self, cx: &mut Ctx, dt: f32) -> bool {
        match self {
            Node::And { a, b: Some(b), a_held } => {
                if !a.test(cx, dt) {
                    if *a_held {
                        b.reset(cx, true, false);
                        *a_held = false;
                    }
                    return false;
                }
                *a_held = true;
                if b.test(cx, dt) {
                    return true;
                }
                a.reset(cx, true, false);
                false
            }
            Node::Or { a, b: Some(b), a_held, b_held } => {
                *a_held = a.test(cx, dt);
                *b_held = b.test(cx, dt);
                *a_held || *b_held
            }
            Node::Not(Some(c)) => {
                let r = c.test(cx, dt);
                c.reset(cx, true, false);
                !r
            }
            Node::And { b: None, .. } | Node::Or { b: None, .. } | Node::Not(None) => false,
            Node::Wait(w) => w.test(cx, dt),
            Node::Do(d) => d.test(cx),
            Node::Restart | Node::Nop => true,
            Node::Stats(s) => s.test(cx, dt),
        }
    }

    pub fn execute(&mut self, cx: &mut Ctx) {
        match self {
            Node::And { a, b, .. } => {
                a.execute(cx);
                if let Some(b) = b {
                    b.execute(cx);
                }
            }
            Node::Or { a, b, a_held, b_held } => {
                if *a_held {
                    a.execute(cx);
                }
                if *b_held && let Some(b) = b {
                    b.execute(cx);
                }
            }
            Node::Wait(Wait::ObjectiveMessage(m)) if m.shown => cx.effect(Effect::ObjectiveMessageHide),
            Node::Do(Do::Effect(e)) => {
                if let Effect::PostEvent { name, entity } = &*e {
                    cx.events.post(name.clone(), entity.clone());
                }
                cx.effect(e.clone());
            }
            Node::Do(Do::NamedFlag { name, value, temporary }) => {
                let name = name.clone();
                cx.flag(&name, *value, *temporary);
            }
            Node::Restart => cx.restart = true,
            _ => {}
        }
    }

    pub fn reset(&mut self, cx: &mut Ctx, reached: bool, passed: bool) {
        match self {
            Node::And { a, b, a_held } => {
                a.reset(cx, reached, passed);
                if let Some(b) = b {
                    b.reset(cx, reached && *a_held, passed);
                }
            }
            Node::Or { a, b, a_held, b_held } => {
                a.reset(cx, reached, passed && *a_held);
                *a_held = false;
                if let Some(b) = b {
                    b.reset(cx, reached, passed && *b_held);
                }
                *b_held = false;
            }
            Node::Not(Some(c)) => c.reset(cx, false, false),
            Node::Wait(w) => w.reset(cx),
            Node::Do(d) => d.reset(cx, reached, passed),
            _ => {}
        }
    }
}

const ID_IS_NIS_READY_LOOP: u16 = 0x75;

impl Wait {
    fn begin(&mut self, cx: &mut Ctx) {
        match self {
            Wait::Equipped { item, equipment_type, held } => {
                let (i, t) = (*item, *equipment_type);
                *held = cx.ask("equipped", |w| w.equipped(i, t)).unwrap_or(false);
            }
            Wait::ObjectiveMessage(m) => {
                let msg = cx.ask("drop message", |w| w.drop_message());
                m.message = msg.filter(|d| d.title.is_some() && d.body.is_some()).or(Some(Default::default()));
            }
            Wait::Stat(s) => s.kind = stat_kind(cx, &s.key),
            _ => {}
        }
    }

    fn test(&mut self, cx: &mut Ctx, dt: f32) -> bool {
        match self {
            Wait::Time { duration, elapsed } => {
                let r = *elapsed > *duration;
                *elapsed += dt;
                r
            }
            Wait::GameState { entity, id, cmp, value } => {
                cx.game_state(entity, *id).is_some_and(|v| cmp.value(v, *value))
            }
            Wait::NamedFlag(n) => cx.flags.check(n),
            Wait::EventType(t) => cx.ask("event type", |w| w.event_type()) == Some(*t),
            Wait::ObjectiveComplete(n) => (cx.complete)(n.hash) == Some(true),
            Wait::PostedEvent { event, entity } => cx.events.posted(event, entity.as_ref()),
            Wait::RemainingDistance { entity, distance } => cx
                .ask(&format!("{entity} remaining distance"), |w| w.racer(entity, RacerQuery::RemainingDistance))
                .is_some_and(|r| r < *distance),
            Wait::TriggerByName(n) => cx.ask(&format!("touching {n}"), |w| w.touching_name(n)).unwrap_or(false),
            Wait::TriggerByGroup(m) => {
                let m = *m;
                cx.ask(&format!("touching group mask {m:#x}"), |w| w.touching_group(m)).unwrap_or(false)
            }
            Wait::Equipped { held, .. } => *held,
            Wait::WorldEntityValue { a, cmp, b } => match (b.eval(cx), a.eval(cx)) {
                (Some(y), Some(x)) => cmp.f32(x, y),
                _ => false,
            },
            Wait::Night => cx.ask("night", |w| w.night()).unwrap_or(false),
            Wait::TutorialMessageReady => {
                cx.ask("tutorial message ready", |w| w.tutorial_message_ready()).unwrap_or(false)
            }
            Wait::TutorialTricks { entity } => {
                cx.ask("tutorial tricks", |w| w.tutorial_tricks(entity)).unwrap_or(false)
            }
            Wait::ObjectiveMessage(m) => m.test(cx, dt),
            Wait::BadgeSet { badge, kind, value } => {
                let k = *kind;
                cx.ask(&format!("badge {badge}"), |w| w.badge_set(badge, k)) == Some(*value)
            }
            Wait::Stat(s) => s.test(cx),
        }
    }

    fn reset(&mut self, cx: &mut Ctx) {
        match self {
            Wait::Time { elapsed, .. } => *elapsed = 0.0,
            Wait::Equipped { held, .. } => *held = false,
            Wait::TutorialTricks { .. } => cx.effect(Effect::TutorialTrickClear),
            Wait::ObjectiveMessage(m) => m.message = None,
            _ => {}
        }
    }

    fn progress(&self) -> f32 {
        match self {
            Wait::Stat(s) => s.progress,
            _ => 0.0,
        }
    }
}

impl ObjectiveMessageWait {
    fn test(&mut self, cx: &mut Ctx, dt: f32) -> bool {
        let ready_loop =
            |cx: &mut Ctx, e: &Name| matches!(cx.game_state(e, ID_IS_NIS_READY_LOOP), Some(super::GsValue::Bool(true)));
        let mut show = false;
        if !self.shown {
            let Some(m) = self.message.clone() else { return true };
            if m.title.is_none() || m.body.is_none() {
                return true;
            }
            if cx.ask("free ride", |w| w.free_ride()) == Some(true) {
                return true;
            }
            if cx.ask("UI 0x124", |w| w.ui_flag(0x124)) != Some(true) {
                return false;
            }
            if ready_loop(cx, &self.entity) {
                show = true;
            } else if m.kind == 1 && cx.ask("UI 0x121", |w| w.ui_flag(0x121)) == Some(true) {
                self.time = 4.0;
                show = true;
            }
        } else {
            if self.time <= 0.0 {
                return !ready_loop(cx, &self.entity);
            }
            self.time -= dt;
            if self.time <= 0.0 {
                return true;
            }
        }
        if show && let Some(m) = &self.message {
            let (title, body) = (m.title.clone().unwrap_or_default(), m.body.clone().unwrap_or_default());
            cx.effect(Effect::ObjectiveMessage { title, body, seconds: self.time, parameter: m.parameter });
            self.shown = true;
        }
        false
    }
}

fn stat_kind(cx: &mut Ctx, key: &StatKey) -> u8 {
    let what = match &key.location {
        Some(l) => format!("stat {} @ {l}", key.name),
        None => format!("stat {}", key.name),
    };
    match cx.ask(&what, |w| w.stat(key)) {
        Some(StatValue::Int(_)) => 0,
        Some(StatValue::Float(_)) => 1,
        None => 2,
    }
}

impl StatWait {
    fn test(&mut self, cx: &mut Ctx) -> bool {
        if self.kind == 2 {
            self.kind = stat_kind(cx, &self.key);
        }
        let (held, current) = match (self.kind, cx.world.stat(&self.key)) {
            (0, Some(StatValue::Int(v))) => (self.cmp.u32(v, (self.value as i64) as u32), f64::from(v)),
            (1, Some(StatValue::Float(v))) => (self.cmp.f32(v, self.value as f32), f64::from(v)),
            _ => {
                self.progress = 0.0;
                return false;
            }
        };
        self.progress = stat_progress(f64::from(self.value as f32), current, self.cmp) as f32;
        held
    }
}

pub fn stat_progress(reference: f64, current: f64, cmp: Cmp) -> f64 {
    let near = |a: f64, b: f64| ((b - a) as f32).abs() < 1.525_878_9e-5;
    let ratio = |r: f64| {
        let r = r as f32 as f64;
        let p = if -r < 0.0 { r } else { 0.0 };
        if ((1.0 - p) as f32) < 0.0 { 1.0 } else { p }
    };
    match cmp {
        Cmp::LessThan => f64::from(u8::from(reference > current)),
        Cmp::LessThanOrEqualTo => f64::from(u8::from(reference >= current)),
        Cmp::EqualTo => f64::from(u8::from(near(reference, current))),
        Cmp::NotEqualTo => f64::from(u8::from(!near(reference, current))),
        Cmp::GreaterThanOrEqualTo if current < reference => ratio(current / reference),
        Cmp::GreaterThan if current <= reference => {
            ratio(current / ((reference * 0.009_999_999_776_482_582 + reference) as f32 as f64))
        }
        _ => 1.0,
    }
}

impl StatsBlock {
    fn test(&mut self, cx: &mut Ctx, dt: f32) -> bool {
        let mut all = true;
        let mut sum = 0.0f32;
        for w in &mut self.waits {
            let r = w.test(cx, dt);
            all = all && r;
            sum += w.progress();
        }
        if !self.waits.is_empty() {
            let mean = sum / self.waits.len() as f32;
            if mean > 0.0 && (mean - self.reported).abs() >= 1.525_878_9e-5 {
                cx.effect(Effect::BadgeProgress { badge: self.badge.clone(), progress: mean });
                self.reported = mean;
            }
        }
        all
    }
}

impl Do {
    fn test(&mut self, cx: &mut Ctx) -> bool {
        let Do::SurviveFog { target, blend, fraction, target_on, .. } = self else { return true };
        let player = Name::new("Player1");
        let loops = cx.ask("Player1 loops", |w| w.racer(&player, RacerQuery::Loops));
        let progress = cx.ask("Player1 progress", |w| w.racer(&player, RacerQuery::Progress));
        if let (Some(loops), Some(progress)) = (loops, progress) {
            let threshold = f64::from(1.0 - *fraction).powf(f64::from(loops as i32 as f32)) as f32;
            let want = progress.partial_cmp(&threshold) != Some(std::cmp::Ordering::Less);
            if want != *target_on {
                cx.effect(Effect::SurviveFogTarget { target: target.clone(), on: want, blend: *blend });
                *target_on = want;
            }
        }
        false
    }

    fn reset(&mut self, cx: &mut Ctx, reached: bool, passed: bool) {
        let undo = match self {
            Do::Effect(Effect::PostEffectCue { name, .. }) if passed => {
                Some(Effect::PostEffectCue { name: name.clone(), cue: 0 })
            }
            Do::Effect(Effect::Visibility { set, .. }) if passed => {
                Some(Effect::Visibility { set: set.clone(), state: "ALL".into() })
            }
            Do::Effect(Effect::PostFxStart { name, .. }) if passed => {
                Some(Effect::PostFxStop { name: name.clone(), blend: 0.0 })
            }
            Do::Effect(Effect::LightingOverrideFromParam { target, .. }) if passed => {
                Some(Effect::LightingOverrideRemove(target.clone()))
            }
            Do::Effect(Effect::SetStatsWriteable(_)) if passed => Some(Effect::SetStatsWriteable(true)),
            Do::Effect(Effect::DisableSplitsCallout(_)) => Some(Effect::DisableSplitsCallout(false)),
            Do::SurviveFog { begun, .. } if reached && *begun => Some(Effect::SurviveFogEnd),
            _ => None,
        };
        if let Some(e) = undo {
            cx.effect(e);
        }
    }
}
