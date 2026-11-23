use rand_chacha::ChaCha8Rng;
use rand_core::{Rng, SeedableRng};

fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

/// Chains SplitMix64 finalizers over seed, tick, entity and salt.
/// See the module tests for fixed vectors.
pub fn stateless_u64(seed: u64, tick: u64, entity: u64, salt: u64) -> u64 {
    let seed = mix(seed.wrapping_add(0x9e3779b97f4a7c15));
    mix(mix(mix(seed ^ tick) ^ entity) ^ salt)
}

/// Maps a uniform word into `0..n` by multiply-shift. Zero maps to zero.
pub fn below(r: u64, n: u64) -> u64 {
    ((u128::from(r) * u128::from(n)) >> 64) as u64
}

pub struct SystemRng(ChaCha8Rng);

impl SystemRng {
    pub fn new(seed: u64, system_id: u64) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        rng.set_stream(system_id);
        Self(rng)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }

    pub fn word_pos(&self) -> u128 {
        self.0.get_word_pos()
    }
}
