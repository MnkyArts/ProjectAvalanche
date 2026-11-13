use core::time::Duration;
use std::collections::VecDeque;

use bevy_ecs::prelude::*;
use bevy_time::{Real, Time};
use lightyear::prelude::{Disconnect, MessageReceiver, MessageSender};
use ssx_net_proto::{ChatRefusal, ClientChat, RiderId, ServerChat};

use super::{ClientRoster, Connection, RemotePoses};
use crate::protocol::{ChatChannel, Wire};

// PORT(choice): 64 lines, ten times the chat layer's rows (a game takes them every frame; a test client may not), a
// bound on what a server can make a client hold; the original has no chat.
pub const KEPT_LINES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatFrom {
    Rider(RiderId),
    Server,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatBody {
    Text(String),
    Quick(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatLine {
    pub from: ChatFrom,
    pub body: ChatBody,
    pub at: Duration,
}

#[derive(Resource, Default)]
pub struct ChatInbox {
    pub lines: VecDeque<ChatLine>,
    pub refusals: VecDeque<(ChatRefusal, Duration)>,
    pub received: u64,
    pub unknown_riders: u64,
    pub overflowed: u64,
}

impl ChatInbox {
    fn keep(&mut self, line: ChatLine) {
        if self.lines.len() == KEPT_LINES {
            self.lines.pop_front();
            self.overflowed += 1;
        }
        self.lines.push_back(line);
    }

    fn refused(&mut self, r: ChatRefusal, at: Duration) {
        if self.refusals.len() == KEPT_LINES {
            self.refusals.pop_front();
        }
        self.refusals.push_back((r, at));
    }
}

#[derive(Resource, Default)]
pub struct ChatSend(pub Vec<ClientChat>);

type ChatReceivers<'a> =
    (Entity, Option<&'a mut MessageReceiver<Wire<ServerChat>>>, Option<&'a mut MessageReceiver<Wire<ClientChat>>>);

pub(super) fn receive_chat(
    mut links: Query<ChatReceivers, Connection>,
    (mut inbox, roster, mut poses): (ResMut<ChatInbox>, Res<ClientRoster>, ResMut<RemotePoses>),
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let now = time.elapsed();
    for (entity, chat, wrong) in &mut links {
        if let Some(mut wrong) = wrong
            && wrong.receive().count() > 0
        {
            poses.refused.get_or_insert_with(|| "the server sent a client-to-server chat line".into());
            commands.trigger(Disconnect { entity });
            continue;
        }
        let Some(mut chat) = chat else { continue };
        for wire in chat.receive() {
            inbox.received += 1;
            let message = match ServerChat::decode(wire.bytes()) {
                Ok(m) => m,
                Err(e) => {
                    poses.refused.get_or_insert_with(|| format!("the server sent a malformed chat message: {e}"));
                    commands.trigger(Disconnect { entity });
                    break;
                }
            };
            let (from, body) = match message {
                ServerChat::Line { rider, text } => (ChatFrom::Rider(RiderId(rider)), ChatBody::Text(text)),
                ServerChat::Quick { rider, index } => (ChatFrom::Rider(RiderId(rider)), ChatBody::Quick(index)),
                ServerChat::Server(text) => (ChatFrom::Server, ChatBody::Text(text)),
                ServerChat::Refused(r) => {
                    inbox.refused(r, now);
                    continue;
                }
            };
            if let ChatFrom::Rider(r) = from
                && !roster.players.contains_key(&r)
            {
                inbox.unknown_riders += 1;
                continue;
            }
            inbox.keep(ChatLine { from, body, at: now });
        }
    }
}

pub(super) fn send_chat(mut out: ResMut<ChatSend>, mut links: Query<&mut MessageSender<Wire<ClientChat>>, Connection>) {
    for line in out.0.drain(..) {
        let wire = Wire::client_chat(&line);
        for mut sender in &mut links {
            sender.send::<ChatChannel>(wire.clone());
        }
    }
}
