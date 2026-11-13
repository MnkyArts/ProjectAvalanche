use bevy_ecs::component::Component;
use serde::de::{Error as _, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ssx_net_proto::join::MAX_NAME_BYTES;
use ssx_net_proto::name::player_name;
use ssx_net_proto::{Appearance, InstanceSpec, InstanceState, PlayerStatus};

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RiderNumber(pub u16);

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InInstance(pub u16);

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlayerKeyId(pub [u8; 8]);

#[derive(Component, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlayerName(String);

impl PlayerName {
    pub fn new(name: &str) -> Result<Self, String> {
        match player_name(name) {
            Ok(n) if n == name => Ok(Self(n)),
            Ok(_) => Err("the name has spaces the name rule removes".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn unchecked(name: String) -> Self {
        Self(name)
    }
}

impl Serialize for PlayerName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PlayerName {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Name;
        impl Visitor<'_> for Name {
            type Value = PlayerName;
            fn expecting(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
                write!(f, "a player name of at most {MAX_NAME_BYTES} bytes by the name rule")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<PlayerName, E> {
                if v.len() > MAX_NAME_BYTES {
                    return Err(E::invalid_length(v.len(), &self));
                }
                PlayerName::new(v).map_err(E::custom)
            }
        }
        d.deserialize_str(Name)
    }
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayerAppearance(pub Appearance);

// PORT(choice): 9 999 ms, four digits of the players screen's ping column (netcode ends a link after 10 s without a
// packet, the connect token's timeout); tuning: the column's width and the round trips of phase 4's public servers; the
// original has no real-time multiplayer.
pub const MAX_PING_MS: u16 = 9_999;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayerPing(u16);

impl PlayerPing {
    pub fn from_millis(ms: u128) -> Self {
        Self(u16::try_from(ms).map_or(MAX_PING_MS, |ms| ms.min(MAX_PING_MS)))
    }

    pub fn millis(self) -> u16 {
        self.0
    }
}

impl Serialize for PlayerPing {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u16(self.0)
    }
}

impl<'de> Deserialize<'de> for PlayerPing {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let ms = u16::deserialize(d)?;
        if ms > MAX_PING_MS {
            return Err(D::Error::custom(format!("a ping of {ms} ms (at most {MAX_PING_MS})")));
        }
        Ok(Self(ms))
    }
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayerState(pub PlayerStatus);

impl Serialize for PlayerState {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(self.0 as u8)
    }
}

impl<'de> Deserialize<'de> for PlayerState {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let b = u8::deserialize(d)?;
        PlayerStatus::from_byte(b).map(Self).ok_or_else(|| D::Error::custom(format!("an unknown player status {b}")))
    }
}

#[derive(Component, Clone, Debug, PartialEq, Eq, Hash)]
pub struct InstanceInfo {
    pub id: u16,
    pub spec: InstanceSpec,
    pub state: InstanceState,
    pub round: u16,
}

pub(crate) fn bytes_of<'de, D: Deserializer<'de>, T>(
    d: D,
    max: usize,
    what: &'static str,
    read: fn(&[u8]) -> Result<T, String>,
) -> Result<T, D::Error> {
    struct Bytes<T> {
        max: usize,
        what: &'static str,
        read: fn(&[u8]) -> Result<T, String>,
    }
    impl<T> Visitor<'_> for Bytes<T> {
        type Value = T;
        fn expecting(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
            write!(f, "{} in at most {} bytes", self.what, self.max)
        }
        fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<T, E> {
            if v.len() > self.max {
                return Err(E::invalid_length(v.len(), &self));
            }
            (self.read)(v).map_err(E::custom)
        }
    }
    d.deserialize_bytes(Bytes { max, what, read }).map_err(D::Error::custom)
}

impl Serialize for PlayerAppearance {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0.encode())
    }
}

impl<'de> Deserialize<'de> for PlayerAppearance {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        bytes_of(d, Appearance::SIZE, "an appearance", |b| {
            Appearance::decode(b).map(PlayerAppearance).map_err(|e| e.to_string())
        })
    }
}

const INSTANCE_HEAD: usize = 5;

impl Serialize for InstanceInfo {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut b = Vec::with_capacity(INSTANCE_HEAD + ssx_net_proto::instance::MAX_SPEC_SIZE);
        b.extend_from_slice(&self.id.to_be_bytes());
        b.push(self.state as u8);
        b.extend_from_slice(&self.round.to_be_bytes());
        b.extend_from_slice(&self.spec.encode());
        s.serialize_bytes(&b)
    }
}

impl<'de> Deserialize<'de> for InstanceInfo {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        bytes_of(d, INSTANCE_HEAD + ssx_net_proto::instance::MAX_SPEC_SIZE, "an instance", |b| {
            let Some((&[i0, i1, state, r0, r1], spec)) = b.split_first_chunk::<INSTANCE_HEAD>() else {
                return Err(format!("an instance of {} bytes", b.len()));
            };
            let state = InstanceState::from_byte(state).ok_or("an unknown instance state")?;
            let spec = InstanceSpec::decode(spec).map_err(|e| e.to_string())?;
            if spec.mode == ssx_net_proto::Mode::Freeride && (state != InstanceState::Running || [r0, r1] != [0, 0]) {
                return Err(format!("a freeride instance {} in round {}", state.name(), u16::from_be_bytes([r0, r1])));
            }
            Ok(InstanceInfo { id: u16::from_be_bytes([i0, i1]), spec, state, round: u16::from_be_bytes([r0, r1]) })
        })
    }
}

#[cfg(test)]
mod tests;
