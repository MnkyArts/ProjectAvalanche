use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use ssx_net_proto::keys::random_bytes;
use ssx_net_proto::master::{CHALLENGE_LIFETIME, MasterChallenge, OneTimeSecret};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::address::canonical;

const TAG_DOMAIN: &[u8] = b"SSXA master challenge v1\0";
const SECRET_DOMAIN: &[u8] = b"SSXA master one-time key v1\0";

// PORT(choice): 65 536 (the global heartbeat rate, 500 a second, over a challenge's 30 s is 15 000); tuning: a measured
// heartbeat load of a public master; the original has no master server.
pub const MAX_USED: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChallengeRefusal {
    #[error("the challenge is not this master's, not this start's or not for this address")]
    Foreign,
    #[error("the challenge expired (a heartbeat posts it within 30 s)")]
    Expired,
    #[error("the challenge was used before")]
    Used,
    #[error("the master remembers too many challenges now; try again")]
    Busy,
}

pub struct Challenges {
    key: Zeroizing<[u8; 64]>,
    started: Instant,
    used: Mutex<HashMap<[u8; 16], u64>>,
}

fn ip_bytes(ip: IpAddr) -> [u8; 16] {
    match canonical(ip) {
        IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
        IpAddr::V6(v6) => v6.octets(),
    }
}

impl Default for Challenges {
    fn default() -> Self {
        Self { key: Zeroizing::new(random_bytes()), started: Instant::now(), used: Mutex::new(HashMap::new()) }
    }
}

impl Challenges {
    fn mac(&self, domain: &[u8], ip: IpAddr, issued_ms: u64, nonce: &[u8; 16], extra: &[u8]) -> Zeroizing<[u8; 32]> {
        let mut mac = <Hmac<Sha256> as KeyInit>::new(&(*self.key).into());
        mac.update(domain);
        mac.update(&ip_bytes(ip));
        mac.update(&issued_ms.to_be_bytes());
        mac.update(nonce);
        mac.update(extra);
        Zeroizing::new(mac.finalize().into_bytes().into())
    }

    fn secret(&self, ip: IpAddr, issued_ms: u64, nonce: &[u8; 16]) -> OneTimeSecret {
        OneTimeSecret::from_bytes(*self.mac(SECRET_DOMAIN, ip, issued_ms, nonce, &[]))
    }

    fn now_ms(&self, now: Instant) -> u64 {
        u64::try_from(now.saturating_duration_since(self.started).as_millis()).unwrap_or(u64::MAX)
    }

    pub fn issue(&self, peer: IpAddr, now: Instant) -> MasterChallenge {
        let issued_ms = self.now_ms(now);
        let nonce: [u8; 16] = random_bytes();
        let ephemeral = self.secret(peer, issued_ms, &nonce).public();
        let full = self.mac(TAG_DOMAIN, peer, issued_ms, &nonce, &ephemeral);
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&full[..16]);
        MasterChallenge { issued_ms, nonce, ephemeral, tag }
    }

    pub fn check(&self, c: &MasterChallenge, peer: IpAddr, now: Instant) -> Result<OneTimeSecret, ChallengeRefusal> {
        let full = self.mac(TAG_DOMAIN, peer, c.issued_ms, &c.nonce, &c.ephemeral);
        if !bool::from(full[..16].ct_eq(&c.tag)) {
            return Err(ChallengeRefusal::Foreign);
        }
        let now_ms = self.now_ms(now);
        let lifetime = CHALLENGE_LIFETIME.as_millis() as u64;
        if c.issued_ms > now_ms || now_ms - c.issued_ms >= lifetime {
            return Err(ChallengeRefusal::Expired);
        }
        Ok(self.secret(peer, c.issued_ms, &c.nonce))
    }

    pub fn redeem(&self, c: &MasterChallenge, now: Instant) -> Result<(), ChallengeRefusal> {
        let now_ms = self.now_ms(now);
        let lifetime = CHALLENGE_LIFETIME.as_millis() as u64;
        let Ok(mut used) = self.used.lock() else { return Err(ChallengeRefusal::Busy) };
        if used.contains_key(&c.nonce) {
            return Err(ChallengeRefusal::Used);
        }
        if used.len() >= MAX_USED {
            used.retain(|_, issued| now_ms.saturating_sub(*issued) < lifetime);
            if used.len() >= MAX_USED {
                return Err(ChallengeRefusal::Busy);
            }
        }
        used.insert(c.nonce, c.issued_ms);
        Ok(())
    }

    pub fn remembered(&self) -> usize {
        self.used.lock().map_or(0, |u| u.len())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_challenge_is_for_its_address_its_time_and_once() {
        let c = Challenges::default();
        let t0 = Instant::now();
        let a: IpAddr = "203.0.113.5".parse().unwrap();
        let ch = c.issue(a, t0);
        let s = c.check(&ch, a, t0 + Duration::from_secs(1)).expect("good");
        assert_eq!(s.public(), ch.ephemeral, "the derived secret is the challenge's key");
        let mapped: IpAddr = "::ffff:203.0.113.5".parse().unwrap();
        assert!(c.check(&ch, mapped, t0).is_ok(), "an IPv4-mapped address is the address");
        assert_eq!(c.check(&ch, "203.0.113.6".parse().unwrap(), t0).err(), Some(ChallengeRefusal::Foreign));
        assert_eq!(c.check(&ch, a, t0 + CHALLENGE_LIFETIME).err(), Some(ChallengeRefusal::Expired));
        let mut other_key = ch;
        other_key.ephemeral[0] ^= 1;
        assert_eq!(c.check(&other_key, a, t0).err(), Some(ChallengeRefusal::Foreign), "the key is under the tag");
        assert_eq!(Challenges::default().check(&ch, a, t0).err(), Some(ChallengeRefusal::Foreign), "another start");
        assert_eq!(c.redeem(&ch, t0), Ok(()));
        assert_eq!(c.redeem(&ch, t0), Err(ChallengeRefusal::Used));
        assert_eq!(c.remembered(), 1);
    }
}
