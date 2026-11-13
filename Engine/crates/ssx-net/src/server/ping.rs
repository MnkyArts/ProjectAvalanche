use core::time::Duration;

use bevy_ecs::prelude::*;
use bevy_time::{Real, Time};
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{Link, PingManager};

use super::guard::{Kicked, Leaving};
use super::instance::PlayerEntity;
use crate::roster::PlayerPing;

// PORT(choice): every second, a players screen's refresh (lightyear's ping manager sends a ping every 100 ms and
// smooths the round trips it measures); tuning: the players screen's use (phase 3's standings may want it less often);
// the original has no real-time multiplayer.
pub const PING_PERIOD: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct LastPublish(Option<Duration>);

type Measured<'a> = (&'a Link, &'a PingManager, &'a PlayerEntity);
type Placed = (With<ClientOf>, Without<Kicked>, Without<Leaving>);

pub(super) fn publish_pings(
    links: Query<Measured, Placed>,
    pings: Query<&PlayerPing>,
    time: Res<Time<Real>>,
    mut last: Local<LastPublish>,
    mut commands: Commands,
) {
    let now = time.elapsed();
    if last.0.is_some_and(|t| now.saturating_sub(t) < PING_PERIOD) {
        return;
    }
    last.0 = Some(now);
    for (link, manager, player) in &links {
        if manager.pongs_recv == 0 {
            continue;
        }
        let ping = PlayerPing::from_millis(link.stats.rtt.as_millis());
        if pings.get(player.0).is_ok_and(|p| *p == ping) {
            continue;
        }
        if let Ok(mut e) = commands.get_entity(player.0) {
            e.insert(ping);
        }
    }
}
