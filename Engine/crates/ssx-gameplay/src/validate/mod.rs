mod limits;
mod reports;
mod track;

#[cfg(test)]
mod tests;

pub use limits::*;
pub use track::{Arrival, RiderValidation};

pub use ssx_net_proto::Check;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Warn,
    Invalidate,
    Kick,
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "warn" => Some(Self::Warn),
            "invalidate" => Some(Self::Invalidate),
            "kick" => Some(Self::Kick),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Invalidate => "invalidate",
            Self::Kick => "kick",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub actions: [Action; 6],
}

impl Default for Policy {
    // PORT(choice): the defaults: invalidate for every check; tuning: the operators' experience with kicks against
    // invalidations on public servers; the game has no validation.
    fn default() -> Self {
        Self { actions: [Action::Invalidate; 6] }
    }
}

impl Policy {
    pub fn of(&self, check: Check) -> Action {
        self.actions[check.index()]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub check: Check,
    pub execution: f64,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Measure {
    Fall,
    Horizontal,
    Step,
    StepHorizontal,
    Landing,
    Execution,
    SplitOrder,
    SplitHeight,
    Finish,
    ClockStart,
    ClockReady,
    ClockBehind,
    Tricks,
    StartExecution,
    StartPlace,
    StartFirst,
    Gates,
    TimeLimit,
}

impl Measure {
    pub const ALL: [Measure; 18] = [
        Measure::Fall,
        Measure::Horizontal,
        Measure::Step,
        Measure::StepHorizontal,
        Measure::Landing,
        Measure::Execution,
        Measure::SplitOrder,
        Measure::SplitHeight,
        Measure::Finish,
        Measure::ClockStart,
        Measure::ClockReady,
        Measure::ClockBehind,
        Measure::Tricks,
        Measure::StartExecution,
        Measure::StartPlace,
        Measure::StartFirst,
        Measure::Gates,
        Measure::TimeLimit,
    ];

    pub fn check(self) -> Check {
        match self {
            Self::Fall | Self::Horizontal => Check::Speed,
            Self::Step
            | Self::StepHorizontal
            | Self::Landing
            | Self::Execution
            | Self::StartPlace
            | Self::StartFirst => Check::Step,
            Self::SplitOrder | Self::SplitHeight => Check::Splits,
            Self::Finish | Self::Gates => Check::Finish,
            Self::ClockStart | Self::ClockReady | Self::ClockBehind | Self::StartExecution | Self::TimeLimit => {
                Check::Clock
            }
            Self::Tricks => Check::Tricks,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Fall => "speed: fall",
            Self::Horizontal => "speed: horizontal",
            Self::Step => "step",
            Self::StepHorizontal => "step: horizontal",
            Self::Landing => "step: a warp's landing",
            Self::Execution => "step: an execution ahead of the server's time",
            Self::SplitOrder => "splits: order",
            Self::SplitHeight => "splits: height",
            Self::Finish => "finish",
            Self::ClockStart => "clock: the start",
            Self::ClockReady => "clock: against Ready",
            Self::ClockBehind => "clock: behind",
            Self::Tricks => "tricks",
            Self::StartExecution => "clock: the hand-over's execution",
            Self::StartPlace => "step: the hand-over's place",
            Self::StartFirst => "step: the start's first executions",
            Self::Gates => "finish: the map's gates",
            Self::TimeLimit => "clock: the map's time limit",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeasureCount {
    pub made: u64,
    pub found: u64,
    pub worst: Option<(f64, f64)>,
    pub top: [Option<f64>; 3],
}

impl MeasureCount {
    fn note(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        let mut v = value;
        for slot in &mut self.top {
            match *slot {
                None => {
                    *slot = Some(v);
                    return;
                }
                Some(s) if v > s => {
                    *slot = Some(v);
                    v = s;
                }
                Some(_) => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    pub measures: [MeasureCount; Measure::ALL.len()],
}

impl Counts {
    pub(crate) fn made(&mut self, m: Measure, value: f64, limit: f64) {
        let c = &mut self.measures[m.index()];
        c.made += 1;
        if value > limit || value.is_nan() {
            c.found += 1;
        }
        if value.is_finite() && c.worst.is_none_or(|(v, l)| value - limit > v - l) {
            c.worst = Some((value, limit));
        }
        c.note(value - limit);
    }

    pub fn add(&mut self, other: &Counts) {
        for (a, b) in self.measures.iter_mut().zip(other.measures) {
            a.made += b.made;
            a.found += b.found;
            if let Some((v, l)) = b.worst
                && a.worst.is_none_or(|(x, y)| v - l > x - y)
            {
                a.worst = Some((v, l));
            }
            for v in b.top.into_iter().flatten() {
                a.note(v);
            }
        }
    }

    pub fn of(&self, m: Measure) -> MeasureCount {
        self.measures[m.index()]
    }

    pub fn check(&self, check: Check) -> (u64, u64) {
        Measure::ALL
            .iter()
            .filter(|m| m.check() == check)
            .map(|&m| self.of(m))
            .fold((0, 0), |(a, b), c| (a + c.made, b + c.found))
    }
}
