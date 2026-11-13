use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use ssx_net::address::AddressKey;
use ssx_net_proto::join::CHALLENGE_SIZE;
use ssx_net_proto::keys::{ServerKey, random_bytes};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

// PORT(choice): 30 s, three HTTP timeouts of the client's join (the info, the challenge, the join); tuning: the join
// times of phase 2's clients over real links; the original has no dedicated server.
pub const CHALLENGE_LIFETIME: Duration = Duration::from_secs(30);

// PORT(choice): 8 192, far above the joins the per-address limits admit in a challenge's lifetime from a few hundred
// addresses; tuning: the join rate of phase 4's public servers; the original has no dedicated server.
pub const MAX_USED_CHALLENGES: usize = 8192;

const DOMAIN: &[u8] = b"SSXA challenge v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChallengeRefusal {
    Foreign,
    Expired,
    Used,
    Full,
}

pub struct Challenges {
    key: Zeroizing<[u8; 64]>,
    server: ServerKey,
    start: Instant,
    used: Mutex<HashMap<[u8; 16], u64>>,
}

fn address_bytes(key: AddressKey) -> [u8; 9] {
    let mut out = [0u8; 9];
    match key {
        AddressKey::V4(v4) => {
            out[0] = 4;
            out[1..5].copy_from_slice(&v4);
        }
        AddressKey::V6Prefix(prefix) => {
            out[0] = 6;
            out[1..].copy_from_slice(&prefix);
        }
    }
    out
}

impl Challenges {
    pub fn new(server: ServerKey) -> Self {
        Self { key: Zeroizing::new(random_bytes()), server, start: Instant::now(), used: Mutex::new(HashMap::new()) }
    }

    fn tag(&self, address: AddressKey, issued_ms: u64, nonce: &[u8; 16]) -> [u8; 16] {
        let mut mac = <Hmac<Sha256> as KeyInit>::new(&(*self.key).into());
        mac.update(DOMAIN);
        mac.update(self.server.as_bytes());
        mac.update(&address_bytes(address));
        mac.update(&issued_ms.to_be_bytes());
        mac.update(nonce);
        let full = mac.finalize().into_bytes();
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&full[..16]);
        tag
    }

    fn now_ms(&self, now: Instant) -> u64 {
        u64::try_from(now.saturating_duration_since(self.start).as_millis()).unwrap_or(u64::MAX)
    }

    pub fn issue(&self, address: AddressKey, now: Instant) -> [u8; CHALLENGE_SIZE] {
        let issued_ms = self.now_ms(now);
        let nonce: [u8; 16] = random_bytes();
        let mut out = [0u8; CHALLENGE_SIZE];
        out[..8].copy_from_slice(&issued_ms.to_be_bytes());
        out[8..24].copy_from_slice(&nonce);
        out[24..].copy_from_slice(&self.tag(address, issued_ms, &nonce));
        out
    }

    pub fn redeem(
        &self,
        challenge: &[u8; CHALLENGE_SIZE],
        address: AddressKey,
        now: Instant,
    ) -> Result<(), ChallengeRefusal> {
        let mut issued = [0u8; 8];
        issued.copy_from_slice(&challenge[..8]);
        let issued_ms = u64::from_be_bytes(issued);
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(&challenge[8..24]);
        if !bool::from(self.tag(address, issued_ms, &nonce).ct_eq(&challenge[24..])) {
            return Err(ChallengeRefusal::Foreign);
        }
        let now_ms = self.now_ms(now);
        let lifetime = u64::try_from(CHALLENGE_LIFETIME.as_millis()).unwrap_or(u64::MAX);
        let expires_ms = issued_ms.saturating_add(lifetime);
        if issued_ms > now_ms || expires_ms <= now_ms {
            return Err(ChallengeRefusal::Expired);
        }
        let mut used = self.used.lock().map_err(|_| ChallengeRefusal::Full)?;
        if used.contains_key(&nonce) {
            return Err(ChallengeRefusal::Used);
        }
        if used.len() >= MAX_USED_CHALLENGES {
            used.retain(|_, expires| *expires > now_ms);
            if used.len() >= MAX_USED_CHALLENGES {
                return Err(ChallengeRefusal::Full);
            }
        }
        used.insert(nonce, expires_ms);
        Ok(())
    }

    pub fn remembered(&self) -> usize {
        self.used.lock().map_or(0, |u| u.len())
    }
}

#[cfg(test)]
mod tests {
    use ssx_net_proto::keys::ServerSecret;

    use super::*;

    #[test]
    fn once_in_time_from_its_address() {
        let c = Challenges::new(ServerSecret::generate().public());
        let (here, there) = (AddressKey::V4([192, 0, 2, 1]), AddressKey::V4([192, 0, 2, 2]));
        let t0 = c.start + Duration::from_secs(1);
        let ch = c.issue(here, t0);
        assert_eq!(c.redeem(&ch, there, t0), Err(ChallengeRefusal::Foreign), "another address");
        assert_eq!(c.redeem(&ch, here, t0 + CHALLENGE_LIFETIME), Err(ChallengeRefusal::Expired));
        assert_eq!(c.redeem(&ch, here, t0), Ok(()));
        assert_eq!(c.redeem(&ch, here, t0), Err(ChallengeRefusal::Used), "replayed");
        for i in 0..CHALLENGE_SIZE {
            let mut bad = c.issue(here, t0);
            bad[i] ^= 1;
            assert_ne!(c.redeem(&bad, here, t0), Ok(()), "byte {i}");
        }
        let other = Challenges::new(ServerSecret::generate().public());
        assert_eq!(other.redeem(&c.issue(here, t0), here, t0), Err(ChallengeRefusal::Foreign), "another server");
    }
}
