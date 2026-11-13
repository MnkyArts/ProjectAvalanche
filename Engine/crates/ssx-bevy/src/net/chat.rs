use bevy::prelude::*;
use ssx_core::text::shown;
use ssx_net::client::{ChatBody, ChatFrom};
use ssx_net::{ChatInbox, ChatSend, ClientRoster, ServerSaid};
use ssx_net_proto::{ChatRefusal, ClientChat};
use ssx_ui::game::fe::ava::session::{Body, ChatIn, ChatOut, Sender, SessionPlayer};

use super::session::NetSession;
use crate::shell::game_mode::GameUiRes;
use crate::shell::live::{LiveLevel, LiveSession};

// PORT(choice): 32 lines a frame (the layer shows 6 and keeps 32), a bound on what a server can make a frame do; the
// original has no chat.
const LINES_PER_FRAME: usize = 32;

#[derive(Default)]
pub struct ChatCounts {
    shown: u64,
    skipped: u64,
    logged: (u64, u64, u64, u64, u64),
    at: Option<std::time::Instant>,
    mods_said: u64,
}

pub fn sync(
    session: Res<NetSession>,
    live: Res<LiveSession>,
    (roster, said): (Res<ClientRoster>, Res<ServerSaid>),
    (mut inbox, mut send): (ResMut<ChatInbox>, ResMut<ChatSend>),
    mut ui: ResMut<GameUiRes>,
    ride: Option<ResMut<crate::ride::Ride>>,
    (mut counts, content): (Local<ChatCounts>, Option<Res<crate::shell::session_content::SessionContent>>),
) {
    if let Some(mut ride) = ride {
        let held = ui.chat_held();
        if ride.chat_held != held {
            ride.hold_for_chat(held);
        }
    }
    let on_hud = ui.fe.as_ref().is_some_and(|fe| fe.screen == ssx_ui::game::fe::screens::id::HUD);
    let level_shown = session.connected() && live.level == LiveLevel::Up && !live.ending() && on_hud;
    ui.session_level(level_shown);
    if !session.connected() {
        return;
    }
    let own = roster.own.map(|(r, _)| r);
    let instance = roster.own.map(|(_, i)| i);
    let round = roster.instance.as_ref().map(|i| i.round);
    let rows = roster.standings.as_ref().filter(|(r, _)| Some(*r) == round).map(|(_, rows)| rows.as_slice());
    let standing = |rider: u16| {
        let rows = rows?;
        let k = rows.iter().position(|row| row.rider == rider)?;
        Some((u16::try_from(k + 1).unwrap_or(u16::MAX), rows[k]))
    };
    let players: Vec<SessionPlayer> = roster
        .players
        .iter()
        .filter(|(_, p)| Some(p.instance) == instance)
        .map(|(r, p)| SessionPlayer {
            rider: r.0,
            name: p.name.clone(),
            key_id: p.key_id,
            character: p.appearance.map(|a| a.character),
            status: p.status,
            ping_ms: p.ping_ms,
            own: Some(*r) == own,
            place: standing(r.0).map(|(k, _)| k),
            standing: standing(r.0).map(|(_, row)| row),
        })
        .collect();
    ui.session_players(players);
    let skip = inbox.lines.len().saturating_sub(LINES_PER_FRAME);
    counts.skipped += skip as u64;
    counts.shown += (inbox.lines.len() - skip) as u64;
    let now = (inbox.received, inbox.unknown_riders, inbox.overflowed, counts.shown, counts.skipped);
    if now != counts.logged && counts.at.is_none_or(|t| t.elapsed().as_secs_f32() >= 10.0) {
        info!(
            "network: chat: {} lines received ({} of riders not on the roster, {} dropped as the oldest kept), {} \
             handed to the HUD, {} past {LINES_PER_FRAME} a frame",
            now.0, now.1, now.2, now.3, now.4
        );
        (counts.logged, counts.at) = (now, Some(std::time::Instant::now()));
    }
    let mut items: Vec<ChatIn> = inbox
        .lines
        .drain(..)
        .skip(skip)
        .filter_map(|l| {
            let from = match l.from {
                ChatFrom::Server => Sender::Server,
                ChatFrom::Rider(r) => {
                    let p = roster.players.get(&r)?;
                    Sender::Player { name: p.name.clone(), key_id: p.key_id }
                }
            };
            let body = match l.body {
                ChatBody::Text(t) => Body::Text(t),
                ChatBody::Quick(i) => Body::Quick(i),
            };
            Some(ChatIn::Line { from, body })
        })
        .collect();
    items.extend(inbox.refusals.drain(..).map(|(r, _)| match r {
        ChatRefusal::Flood { wait_ms } => ChatIn::Flood { wait_ms },
        ChatRefusal::Disabled => ChatIn::Disabled,
        ChatRefusal::Muted { until, reason } => ChatIn::Muted {
            until: match until {
                ssx_net_proto::operator::Expiry::Never => None,
                at => Some(at.describe()),
            },
            reason,
        },
    }));
    if let Some(content) = content
        && level_shown
        && content.slot.serial() != counts.mods_said
    {
        let (serial, off) = content.slot.switched_off();
        counts.mods_said = serial;
        if !off.is_empty() {
            let whole = off.iter().all(|(_, ids)| ids.is_empty());
            info!(
                "network: the server's policy switches off {}: {}",
                if whole { "the client mods" } else { "the simulation changes of the client mods" },
                off.iter().map(|(n, ids)| format!("{} ({} ids)", shown(n), ids.len())).collect::<Vec<_>>().join(", ")
            );
            items.push(ChatIn::ModsOff { names: off.into_iter().map(|(n, _)| n).collect(), whole });
        }
    }
    ui.session_chat(items, said.chat);
    for line in ui.take_chat_out() {
        let wire = match line {
            ChatOut::Text(t) => ClientChat::text(&t),
            ChatOut::Quick(i) => ClientChat::quick(i),
        };
        match wire {
            Ok(m) => send.0.push(m),
            Err(e) => error!("network: a chat line the wire refuses: {e}"),
        }
    }
}

pub fn keys(live: Res<LiveSession>, keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>, mut ui: ResMut<GameUiRes>) {
    if live.level != LiveLevel::Up || live.ending() || ui.text_focused() || ui.chat_held() {
        return;
    }
    let paused = ui.fe.as_ref().is_some_and(|fe| fe.screen != ssx_ui::game::fe::screens::id::HUD);
    if paused {
        return;
    }
    if keys.just_pressed(KeyCode::KeyT) {
        let opened = ui.chat_open(false);
        info!("network: T: the chat's keyboard {}", if opened { "opens" } else { "is not opened" });
    } else if pads.iter().any(|p| p.just_pressed(GamepadButton::LeftThumb)) {
        let opened = ui.chat_open(true);
        info!("network: the left stick's click: the quick chat {}", if opened { "opens" } else { "is not opened" });
    }
}
