use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatchResult {
    Match,
    Pending,
    #[default]
    Fail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchType {
    Enter,
    Hold,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Y,
    X,
    B,
    A,
    Lb,
    Rb,
    Lt,
    Rt,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Test {
    Stick { min_angle: f32, max_angle: f32, min_mag: f32, max_mag: f32 },
    Button(Button),
}

impl Test {
    pub(super) fn holds(self, p: &PadSample) -> bool {
        match self {
            Test::Stick { min_angle, max_angle, min_mag, max_mag } => {
                let a = p.angle;
                let in_angle = if min_angle < max_angle {
                    min_angle <= a && a <= max_angle
                } else {
                    a <= max_angle || a >= min_angle
                };
                let mag = p.mag.min(1.0);
                in_angle && min_mag <= mag && mag <= max_mag
            }
            Test::Button(b) => p.button(b),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matcher {
    pub test: Test,
    pub kind: MatchType,
    pub negate: bool,
}

impl Matcher {
    pub fn eval(&self, prev: &PadSample, cur: &PadSample) -> MatchResult {
        let (p, c) = (self.test.holds(prev), self.test.holds(cur));
        let hit = match self.kind {
            MatchType::Enter => !p && c,
            MatchType::Hold => c,
            MatchType::Exit => p && !c,
        };
        if hit != self.negate { MatchResult::Match } else { MatchResult::Fail }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub matchers: &'static [Matcher],
    pub min_ticks: f32,
    pub max_ticks: f32,
}

impl Step {
    pub(super) fn eval(&self, prev: &PadSample, cur: &PadSample) -> MatchResult {
        let mut r = MatchResult::Match;
        for m in self.matchers {
            match m.eval(prev, cur) {
                MatchResult::Fail => return MatchResult::Fail,
                MatchResult::Pending => r = MatchResult::Pending,
                MatchResult::Match => {}
            }
        }
        r
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GestureState {
    pub step: usize,
    pub step_timer: f32,
    pub match_timer: f32,
    pub last: MatchResult,
}

pub fn gesture_match(steps: &[Step], s: &mut GestureState, prev: &PadSample, cur: &PadSample, dt: f32) -> MatchResult {
    let n = steps.len();
    if n == 0 {
        return MatchResult::Fail;
    }
    if s.step >= n {
        if s.step_timer < steps[n - 1].max_ticks && s.last == MatchResult::Match {
            s.step = n - 1;
        } else {
            *s = GestureState::default();
        }
    }
    let mut k = s.step;
    loop {
        let st = &steps[k];
        s.step = k + 1;
        let mut r = if s.step_timer <= st.max_ticks { st.eval(prev, cur) } else { MatchResult::Fail };
        if r == MatchResult::Fail {
            let was_zero = s.match_timer == 0.0;
            s.match_timer = 0.0;
            if s.step_timer > st.max_ticks - st.min_ticks || s.step_timer >= st.max_ticks || !was_zero || k == 0 {
                s.last = MatchResult::Fail;
                return MatchResult::Fail;
            }
            r = MatchResult::Pending;
        } else {
            s.match_timer += dt;
        }
        if r == MatchResult::Pending || s.match_timer < st.min_ticks {
            s.step = k;
            s.step_timer += dt;
            s.last = MatchResult::Pending;
            return MatchResult::Pending;
        }
        let (step_timer, match_timer) = (s.step_timer, s.match_timer);
        (s.step_timer, s.match_timer) = (0.0, 0.0);
        k += 1;
        if k < n {
            continue;
        }
        (s.step, s.step_timer, s.match_timer, s.last) = (n, step_timer + dt, match_timer, MatchResult::Match);
        return MatchResult::Match;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    Matcher(Matcher),
    Steps(&'static [Step]),
    Shoulders([bool; 4]),
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enable {
    InAir,
    UpStart,
    LeftStart,
    RightStart,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gates {
    pub in_air: bool,
    pub up_start: bool,
    pub left_start: bool,
    pub right_start: bool,
}

impl Gates {
    pub(super) fn get(&self, e: Enable) -> bool {
        match e {
            Enable::InAir => self.in_air,
            Enable::UpStart => self.up_start,
            Enable::LeftStart => self.left_start,
            Enable::RightStart => self.right_start,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrabMapping {
    pub gesture: Gesture,
    pub enabled: Enable,
}

impl GrabMapping {
    pub(super) fn update(&self, s: &mut GestureState, gates: &Gates, prev: &PadSample, cur: &PadSample) -> bool {
        if gates.get(self.enabled) {
            let r = match self.gesture {
                Gesture::Matcher(m) => m.eval(prev, cur),
                Gesture::Steps(steps) => gesture_match(steps, s, prev, cur, 1.0),
                Gesture::Shoulders(set) => {
                    if [cur.lb, cur.rb, cur.lt, cur.rt] == set {
                        MatchResult::Match
                    } else {
                        MatchResult::Fail
                    }
                }
                Gesture::Never => MatchResult::Fail,
            };
            match r {
                MatchResult::Match => return true,
                MatchResult::Pending => return false,
                MatchResult::Fail => {}
            }
        }
        *s = GestureState::default();
        false
    }
}

pub(super) const OPEN: f32 = 1_073_741_824.0;

pub(super) const fn sector(min_angle: f32, max_angle: f32) -> Test {
    Test::Stick { min_angle, max_angle, min_mag: 0.6, max_mag: 1.0 }
}
pub(super) const RS_UP: Test = sector(FRAC_PI_4, 2.356_194_5);
pub(super) const RS_LEFT: Test = sector(2.356_194_5, 3.926_991);
pub(super) const RS_DOWN: Test = sector(3.926_991, 5.497_787);
pub(super) const RS_RIGHT: Test = sector(5.497_787, FRAC_PI_4);
pub(super) const BTN_Y: Test = Test::Button(Button::Y);
pub(super) const BTN_X: Test = Test::Button(Button::X);
pub(super) const BTN_B: Test = Test::Button(Button::B);
pub(super) const BTN_A: Test = Test::Button(Button::A);

pub(super) const fn on_enter(test: Test) -> Matcher {
    Matcher { test, kind: MatchType::Enter, negate: false }
}
pub(super) const fn held(test: Test) -> Matcher {
    Matcher { test, kind: MatchType::Hold, negate: false }
}
pub(super) const fn on_exit(test: Test) -> Matcher {
    Matcher { test, kind: MatchType::Exit, negate: false }
}
pub(super) const RS_INVALID: Matcher = Matcher {
    test: Test::Stick { min_angle: 2.356_194_5, max_angle: 2.356_194_5, min_mag: 0.0, max_mag: 0.6 },
    kind: MatchType::Hold,
    negate: true,
};

macro_rules! step {
    ($min:expr, $max:expr; $($m:expr),+) => {
        Step { matchers: &[$($m),+], min_ticks: $min, max_ticks: $max }
    };
}
macro_rules! double {
    ($first:expr, $a:expr, $second:expr, $b:expr) => {
        [step!(0.0, 0.0; $first), step!(0.0, 10.0; on_exit($a)), step!(0.0, 10.0; $second), step!(0.0, OPEN; held($b))]
    };
}

pub(super) const RS_G_UP: [Step; 1] = [step!(8.0, OPEN; held(RS_UP))];
pub(super) const RS_G_UP_LEFT: [Step; 3] = [
    step!(0.0, 10.0; on_exit(RS_UP)),
    step!(0.0, 0.0; on_enter(RS_LEFT), RS_INVALID),
    step!(0.0, OPEN; held(RS_LEFT), RS_INVALID),
];
pub(super) const RS_G_UP_RIGHT: [Step; 3] = [
    step!(0.0, 10.0; on_exit(RS_UP)),
    step!(0.0, 0.0; on_enter(RS_RIGHT), RS_INVALID),
    step!(0.0, OPEN; held(RS_RIGHT)),
];
pub(super) const RS_G_UP_DOWN: [Step; 4] = double!(on_enter(RS_UP), RS_UP, on_enter(RS_DOWN), RS_DOWN);
pub(super) const RS_G_LEFT_RIGHT: [Step; 4] = double!(on_enter(RS_LEFT), RS_LEFT, on_enter(RS_RIGHT), RS_RIGHT);
pub(super) const RS_G_LEFT_UP: [Step; 4] = double!(on_enter(RS_LEFT), RS_LEFT, on_enter(RS_UP), RS_UP);
pub(super) const RS_G_LEFT_DOWN: [Step; 4] = double!(on_enter(RS_LEFT), RS_LEFT, on_enter(RS_DOWN), RS_DOWN);
pub(super) const RS_G_RIGHT: [Step; 1] = [step!(0.0, 200.0; held(RS_RIGHT))];
pub(super) const RS_G_RIGHT_LEFT: [Step; 4] = double!(on_enter(RS_RIGHT), RS_RIGHT, on_enter(RS_LEFT), RS_LEFT);
pub(super) const RS_G_RIGHT_UP: [Step; 4] = double!(on_enter(RS_RIGHT), RS_RIGHT, on_enter(RS_UP), RS_UP);
pub(super) const RS_G_RIGHT_DOWN: [Step; 4] = double!(on_enter(RS_RIGHT), RS_RIGHT, on_enter(RS_DOWN), RS_DOWN);

pub(super) const BTN_G_UP: [Step; 1] = [step!(0.0, OPEN; held(BTN_Y))];
pub(super) const BTN_G_UP_LEFT: [Step; 4] = double!(held(BTN_Y), BTN_Y, held(BTN_X), BTN_X);
pub(super) const BTN_G_UP_RIGHT: [Step; 4] = double!(held(BTN_Y), BTN_Y, held(BTN_B), BTN_B);
pub(super) const BTN_G_UP_DOWN: [Step; 4] = double!(held(BTN_Y), BTN_Y, held(BTN_A), BTN_A);
pub(super) const BTN_G_LEFT: [Step; 1] = [step!(0.0, OPEN; held(BTN_X))];
pub(super) const BTN_G_LEFT_RIGHT: [Step; 4] = double!(on_enter(BTN_X), BTN_X, held(BTN_B), BTN_B);
pub(super) const BTN_G_LEFT_UP: [Step; 4] = double!(held(BTN_X), BTN_X, held(BTN_Y), BTN_Y);
pub(super) const BTN_G_LEFT_DOWN: [Step; 4] = double!(held(BTN_X), BTN_X, held(BTN_A), BTN_A);
pub(super) const BTN_G_RIGHT: [Step; 1] = [step!(0.0, OPEN; held(BTN_B))];
pub(super) const BTN_G_RIGHT_LEFT: [Step; 4] = double!(on_enter(BTN_B), BTN_B, held(BTN_X), BTN_X);
pub(super) const BTN_G_RIGHT_UP: [Step; 4] = double!(held(BTN_B), BTN_B, held(BTN_Y), BTN_Y);
pub(super) const BTN_G_RIGHT_DOWN: [Step; 4] = double!(held(BTN_B), BTN_B, held(BTN_A), BTN_A);
pub(super) const BTN_G_DOWN: [Step; 1] = [step!(0.0, OPEN; held(BTN_A))];

pub(super) const fn map(steps: &'static [Step], enabled: Enable) -> GrabMapping {
    GrabMapping { gesture: Gesture::Steps(steps), enabled }
}
pub(super) const fn map_matcher(m: Matcher) -> GrabMapping {
    GrabMapping { gesture: Gesture::Matcher(m), enabled: Enable::InAir }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableEntry {
    pub button: GrabMapping,
    pub stick: GrabMapping,
    pub len: i32,
    pub regular: TrickType,
    pub goofy: TrickType,
}

pub(super) const fn e(
    button: GrabMapping,
    stick: GrabMapping,
    len: i32,
    regular: TrickType,
    goofy: TrickType,
) -> TableEntry {
    TableEntry { button, stick, len, regular, goofy }
}

pub const AIR_TRICKS: [TableEntry; 13] = {
    use Enable::{InAir, LeftStart, RightStart, UpStart};
    use TrickType::{FlyingSquirrel, Indy, Melon, Mute, Nose, Rocket, Seatbelt, Stalefish, Stiffy, Tail};
    use TrickType::{Truckdriver, Wheelman};
    [
        e(map(&BTN_G_UP, InAir), map(&RS_G_UP, InAir), 1, Rocket, Rocket),
        e(map(&BTN_G_UP_LEFT, UpStart), map(&RS_G_UP_LEFT, InAir), 2, FlyingSquirrel, Stiffy),
        e(map(&BTN_G_UP_RIGHT, UpStart), map(&RS_G_UP_RIGHT, InAir), 2, Stiffy, FlyingSquirrel),
        e(map(&BTN_G_UP_DOWN, UpStart), map(&RS_G_UP_DOWN, InAir), 2, Truckdriver, Truckdriver),
        e(map(&BTN_G_LEFT, InAir), map_matcher(held(RS_LEFT)), 1, Melon, Indy),
        e(map(&BTN_G_LEFT_RIGHT, LeftStart), map(&RS_G_LEFT_RIGHT, InAir), 2, Mute, Stalefish),
        e(map(&BTN_G_LEFT_UP, LeftStart), map(&RS_G_LEFT_UP, InAir), 2, Nose, Wheelman),
        e(map(&BTN_G_LEFT_DOWN, LeftStart), map(&RS_G_LEFT_DOWN, InAir), 2, Seatbelt, Tail),
        e(map(&BTN_G_RIGHT, InAir), map(&RS_G_RIGHT, InAir), 1, Indy, Melon),
        e(map(&BTN_G_RIGHT_LEFT, RightStart), map(&RS_G_RIGHT_LEFT, InAir), 2, Stalefish, Mute),
        e(map(&BTN_G_RIGHT_UP, RightStart), map(&RS_G_RIGHT_UP, InAir), 2, Wheelman, Nose),
        e(map(&BTN_G_RIGHT_DOWN, RightStart), map(&RS_G_RIGHT_DOWN, InAir), 2, Tail, Seatbelt),
        e(map(&BTN_G_DOWN, InAir), map_matcher(held(RS_DOWN)), 2, Truckdriver, Truckdriver),
    ]
};

pub(super) const fn classic_button(lb: bool, rb: bool, lt: bool, rt: bool) -> GrabMapping {
    GrabMapping { gesture: Gesture::Shoulders([lb, rb, lt, rt]), enabled: Enable::InAir }
}
pub(super) const fn classic_none() -> GrabMapping {
    GrabMapping { gesture: Gesture::Never, enabled: Enable::InAir }
}

pub const AIR_TRICKS_CLASSIC: [TableEntry; 15] = {
    use TrickType::{FlyingSquirrel, Indy, Melon, Mute, Nose, Rocket, Seatbelt, Stalefish, Stiffy, Tail};
    use TrickType::{Truckdriver, Wheelman};
    [
        e(classic_button(true, false, false, false), classic_none(), 0, Nose, Wheelman),
        e(classic_button(false, true, false, false), classic_none(), 0, Wheelman, Nose),
        e(classic_button(false, false, true, false), classic_none(), 0, Melon, Indy),
        e(classic_button(false, false, false, true), classic_none(), 0, Indy, Melon),
        e(classic_button(true, true, false, false), classic_none(), 1, Truckdriver, Truckdriver),
        e(classic_button(true, false, true, false), classic_none(), 1, Mute, Stalefish),
        e(classic_button(true, false, false, true), classic_none(), 1, FlyingSquirrel, Stiffy),
        e(classic_button(false, true, false, true), classic_none(), 1, Stalefish, Mute),
        e(classic_button(false, true, true, false), classic_none(), 1, Stiffy, FlyingSquirrel),
        e(classic_button(false, false, true, true), classic_none(), 1, Rocket, Rocket),
        e(classic_button(true, true, true, false), classic_none(), 2, Seatbelt, Tail),
        e(classic_button(true, true, false, true), classic_none(), 2, Tail, Seatbelt),
        e(classic_button(true, false, true, true), classic_none(), 2, Stiffy, FlyingSquirrel),
        e(classic_button(false, true, true, true), classic_none(), 2, FlyingSquirrel, Stiffy),
        e(classic_button(true, true, true, true), classic_none(), 2, Truckdriver, Truckdriver),
    ]
};

pub(super) fn air_table(classic: bool) -> &'static [TableEntry] {
    if classic { &AIR_TRICKS_CLASSIC } else { &AIR_TRICKS }
}

// PORT(choice): an added trick's input is the air doubles' shape (a press, its release within 10 ticks, the second
// press within 10, then held) with one direction twice; the disc has no air same-direction double (TrickSceneOp's
// ButtonGrabUpUp/LeftLeft/RightRight fields name BLN_Pad_Grab_Button_UpUp…, whose mappings take the
// Pad_Grab_Button_UpDown/LeftDown/RightDown gestures), its only same-button doubles are the grind double taps
// (Pad_Grind_Button_UpUp: press, release within 4, held within 4); for added content: nothing on this disc to match.
pub(super) const BTN_G_UP_UP: [Step; 4] = double!(held(BTN_Y), BTN_Y, held(BTN_Y), BTN_Y);
pub(super) const BTN_G_LEFT_LEFT: [Step; 4] = double!(held(BTN_X), BTN_X, held(BTN_X), BTN_X);
pub(super) const BTN_G_RIGHT_RIGHT: [Step; 4] = double!(held(BTN_B), BTN_B, held(BTN_B), BTN_B);
pub(super) const RS_G_UP_UP: [Step; 4] = double!(on_enter(RS_UP), RS_UP, on_enter(RS_UP), RS_UP);
pub(super) const RS_G_LEFT_LEFT: [Step; 4] = double!(on_enter(RS_LEFT), RS_LEFT, on_enter(RS_LEFT), RS_LEFT);
pub(super) const RS_G_RIGHT_RIGHT: [Step; 4] = double!(on_enter(RS_RIGHT), RS_RIGHT, on_enter(RS_RIGHT), RS_RIGHT);

pub const ADDED_INPUTS: [(GrabMapping, GrabMapping); 3] = [
    (map(&BTN_G_UP_UP, Enable::UpStart), map(&RS_G_UP_UP, Enable::InAir)),
    (map(&BTN_G_LEFT_LEFT, Enable::LeftStart), map(&RS_G_LEFT_LEFT, Enable::InAir)),
    (map(&BTN_G_RIGHT_RIGHT, Enable::RightStart), map(&RS_G_RIGHT_RIGHT, Enable::InAir)),
];
pub const ADDED_LEN: i32 = 2;
