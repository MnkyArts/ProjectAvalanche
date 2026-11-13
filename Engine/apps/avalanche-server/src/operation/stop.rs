use std::collections::BTreeSet;
use std::time::Duration;

use bevy::prelude::*;
use ssx_gameplay::round::StopPlan;
use ssx_net::lightyear::prelude::LocalTimeline;
use ssx_net::server::{InstanceOutbox, Instances, KickReason, Kicked, Leaving, PlacedIn, PlayerSession, leave};
use ssx_net_proto::{ServerChat, TICK_HZ};

use crate::systems::{Bridge, Lifecycle};

// PORT(choice): 5 s, the time to read the results table; tuning: the results screen's use in a stopping server; the
// original has no dedicated server (the game's own results screen waits for a button).
pub const RESULTS_SHOWN: Duration = Duration::from_secs(5);
const MARGIN: Duration = Duration::from_secs(2);

#[derive(Resource, Clone, Copy, Debug)]
pub struct StopConfig {
    pub round_end_secs: u32,
}

#[derive(Event, Clone, Copy, Debug)]
pub struct ShutdownBegun {
    pub round_end_secs: u32,
}

pub struct Ending {
    by: Duration,
    told: BTreeSet<u16>,
}

type Rider<'a> = (Entity, Option<&'a PlacedIn>);
type Present = (With<PlayerSession>, Without<Kicked>, Without<Leaving>);
type Clocks<'w> = (Option<ResMut<'w, Instances>>, Res<'w, LocalTimeline>, Res<'w, StopConfig>, Res<'w, Time<Real>>);

pub fn end_rounds(
    mut life: ResMut<Lifecycle>,
    (instances, timeline, config, time): Clocks,
    bridge: Res<Bridge>,
    riders: Query<Rider, Present>,
    (mut outbox, mut ending): (ResMut<InstanceOutbox>, Local<Option<Ending>>),
    mut commands: Commands,
    heard: Option<Res<crate::scripts::ShutdownHeard>>,
) {
    if !matches!(*life, Lifecycle::Ending { .. }) {
        return;
    }
    let (now, tick) = (time.elapsed(), timeline.tick().0);
    let shown = u32::try_from(RESULTS_SHOWN.as_secs()).unwrap_or(u32::MAX).saturating_mul(TICK_HZ);
    let mut instances = instances;
    let state = ending.get_or_insert_with(|| {
        commands.trigger(ShutdownBegun { round_end_secs: config.round_end_secs });
        let within = config.round_end_secs.saturating_mul(TICK_HZ);
        let mut ends = 0;
        for s in instances.iter_mut().flat_map(|i| i.list.iter_mut()) {
            let text = match s.instance.stop(tick, within) {
                StopPlan::RoundEnds { .. } => {
                    ends += 1;
                    format!("The server stops: this round ends in {} s.", config.round_end_secs)
                }
                StopPlan::Idle => "The server stops.".to_string(),
            };
            if let Ok(line) = ServerChat::server(&text) {
                outbox.0.push((s.instance.id(), line));
            }
        }
        info!("stopping: {ends} rounds under way end within {} s", config.round_end_secs);
        let by = now + Duration::from_secs(u64::from(config.round_end_secs)) + RESULTS_SHOWN + MARGIN;
        Ending { by, told: BTreeSet::new() }
    });
    let done: Vec<u16> = instances
        .iter()
        .flat_map(|i| i.list.iter())
        .filter(|s| !state.told.contains(&s.instance.id()) && s.instance.stop_done(tick, shown))
        .map(|s| s.instance.id())
        .collect();
    let all_instances = instances.as_ref().map_or(0, |i| i.list.len());
    let overdue = now >= state.by || bridge.shared.stop_now.load(std::sync::atomic::Ordering::Acquire);
    if !overdue && heard.is_some_and(|h| !h.answered) {
        return;
    }
    let mut told = 0usize;
    for (link, placed) in &riders {
        let go = overdue || placed.is_none_or(|p| done.contains(&p.0) || state.told.contains(&p.0));
        if go {
            leave(&mut commands, link, KickReason::ServerStopping, "the server stops", now);
            told += 1;
        }
    }
    if told > 0 {
        info!("stopping: {told} riders told{}", if overdue { " (the bound passed)" } else { "" });
    }
    state.told.extend(done);
    if overdue || state.told.len() >= all_instances {
        *life = Lifecycle::Stopping { since: now, stopped_at: None };
    }
}
