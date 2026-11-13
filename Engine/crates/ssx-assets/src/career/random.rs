use std::sync::Mutex;

static GLOBAL: Mutex<Option<Mt19937>> = Mutex::new(None);

pub const TIME_BASE_HZ: u64 = 50_000_000;

// PORT(choice): the host's wall clock in 50 MHz ticks stands in (both unknown to the player); no game value behind it:
// the console's time base counts from power-on and cannot be reproduced.
pub fn time_base() -> u32 {
    let ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    (ns * u128::from(TIME_BASE_HZ) / 1_000_000_000) as u32
}

#[derive(Clone, Debug)]
pub struct Mt19937 {
    mt: Box<[u32; 624]>,
    index: usize,
}

impl Mt19937 {
    pub fn new(seed: u32) -> Self {
        let mut mt = Box::new([0u32; 624]);
        mt[0] = seed;
        for i in 1..624 {
            mt[i] = 1_812_433_253u32.wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        Mt19937 { mt, index: 624 }
    }

    pub fn seeded(seed: u32) -> Self {
        let mut mt = Box::new([0u32; 624]);
        let mut s = seed | 1;
        for w in mt.iter_mut() {
            let m = s.wrapping_mul(69_069);
            *w = (s & 0xFFFF_0000) | (m >> 16);
            s = m.wrapping_mul(69_069).wrapping_add(69_070);
        }
        let mut g = Mt19937 { mt, index: 624 };
        g.twist();
        g
    }

    fn twist(&mut self) {
        let mt = &mut self.mt;
        for k in 0..624 {
            let y = (mt[k] & 0x8000_0000) | (mt[(k + 1) % 624] & 0x7FFF_FFFF);
            mt[k] = mt[(k + 397) % 624] ^ (y >> 1) ^ if y & 1 != 0 { 0x9908_B0DF } else { 0 };
        }
        self.index = 0;
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            self.twist();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9D2C_5680;
        y ^= (y << 15) & 0xEFC6_0000;
        y ^ (y >> 18)
    }
}

pub trait GameRandom {
    fn next_u32(&mut self) -> u32;

    fn below(&mut self, n: u32) -> u32 {
        ((u64::from(self.next_u32()) * u64::from(n)) >> 32) as u32
    }

    fn range_f(&mut self, lo: f32, hi: f32) -> f32 {
        let x = f64::from(self.next_u32() as i32) * 2.328_306_436_538_696_3e-10 + 0.5;
        (x * f64::from(hi - lo) + f64::from(lo)) as f32
    }
}

impl GameRandom for Mt19937 {
    fn next_u32(&mut self) -> u32 {
        Mt19937::next_u32(self)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GlobalRandom;

impl GameRandom for GlobalRandom {
    fn next_u32(&mut self) -> u32 {
        with(|g| g.next_u32())
    }
}

#[derive(Clone, Debug, Default)]
pub enum Source {
    #[default]
    Global,
    Own(Mt19937),
}

impl Source {
    pub fn seeded(seed: Option<u32>) -> Self {
        seed.map_or(Source::Global, |s| Source::Own(Mt19937::seeded(s)))
    }
}

impl GameRandom for Source {
    fn next_u32(&mut self) -> u32 {
        match self {
            Source::Global => GlobalRandom.next_u32(),
            Source::Own(g) => g.next_u32(),
        }
    }
}

pub fn boot(time_base: u32) -> Mt19937 {
    let mut local = Mt19937::seeded(time_base);
    let hi = local.below(0x1_0000);
    for _ in 0..=local.below(8) {
        local.next_u32();
    }
    let lo = local.below(0x1_0000);
    Mt19937::seeded(hi << 16 | lo)
}

pub fn with<R>(f: impl FnOnce(&mut Mt19937) -> R) -> R {
    let mut g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
    let g = g.get_or_insert_with(|| match std::env::var("SSX_RANDOM_SEED").ok().and_then(|s| s.parse().ok()) {
        Some(seed) => Mt19937::seeded(seed),
        None => boot(time_base()),
    });
    f(g)
}

pub fn reseed(seed: u32) {
    *GLOBAL.lock().unwrap_or_else(|e| e.into_inner()) = Some(Mt19937::seeded(seed));
}

pub fn below(n: u32) -> u32 {
    GlobalRandom.below(n)
}

pub fn range(lo: f32, hi: f32) -> f32 {
    GlobalRandom.range_f(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_twister_matches_the_reference() {
        let mut mt = Mt19937::new(5489);
        assert_eq!(mt.next_u32(), 3_499_211_612);
        assert_eq!(mt.next_u32(), 581_869_302);
        let last = (2..10_000).map(|_| mt.next_u32()).last();
        assert_eq!(last, Some(4_123_659_995));
    }

    #[test]
    fn the_games_seeding_and_ranges() {
        let (mut a, mut b) = (Mt19937::seeded(4), Mt19937::seeded(5));
        assert!((0..100).all(|_| a.next_u32() == b.next_u32()));
        let (mut a, mut b) = (Mt19937::seeded(1), Mt19937::seeded(1));
        for n in 0..50 {
            let x = a.below(n);
            assert!((x < n || n == 0) && x == b.below(n));
        }
        let mut g = Mt19937::seeded(1);
        for _ in 0..1000 {
            let x = g.range_f(0.0, 1.0);
            assert!((0.0..=1.0).contains(&x));
        }
        let (mut x, mut y) = (boot(11), boot(11));
        assert_eq!(x.next_u32(), y.next_u32());
        let (mut x, mut y) = (boot(11), boot(13));
        assert_ne!((x.next_u32(), x.next_u32()), (y.next_u32(), y.next_u32()));
    }

    #[test]
    fn a_source_draws_from_its_generator() {
        let mut own = Source::seeded(Some(7));
        let mut g = Mt19937::seeded(7);
        for n in 1..200 {
            assert_eq!(own.below(n), g.below(n));
        }
        assert!(matches!(Source::seeded(None), Source::Global));
    }
}
