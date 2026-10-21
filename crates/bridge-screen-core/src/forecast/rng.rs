//! A small seeded random number generator (SplitMix64) and the seed hash (FNV-1a), so a
//! forecast is the same for the same inputs on every machine and every run.

/// SplitMix64: fast, well mixed, and enough for a Monte Carlo of a thousand runs.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n` (n > 0), without modulo bias worth noticing for small `n`.
    pub fn below(&mut self, n: usize) -> usize {
        let n = n as u64;
        // Multiply-shift: the high 64 bits of a 128-bit product are uniform in 0..n.
        let wide = u128::from(self.next_u64()) * u128::from(n);
        usize::try_from(wide >> 64).unwrap_or(0)
    }
}

/// FNV-1a over the parts, each followed by a separator byte.
pub fn seed(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for part in parts {
        for b in part.iter().chain(std::iter::once(&0xFF)) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::{Rng, seed};

    #[test]
    fn same_seed_same_numbers() {
        let a: Vec<u64> = (0..5)
            .scan(Rng::new(7), |r, _| Some(r.next_u64()))
            .collect();
        let b: Vec<u64> = (0..5)
            .scan(Rng::new(7), |r, _| Some(r.next_u64()))
            .collect();
        assert_eq!(a, b);
        assert_ne!(a[0], Rng::new(8).next_u64());
        assert_eq!(seed(&[b"repo", b"12"]), seed(&[b"repo", b"12"]));
        assert_ne!(seed(&[b"repo", b"12"]), seed(&[b"repo1", b"2"]));
    }

    #[test]
    fn below_covers_the_range_evenly() {
        let mut r = Rng::new(1);
        let mut counts = [0_u32; 4];
        for _ in 0..4000 {
            counts[r.below(4)] += 1;
        }
        assert!(
            counts.iter().all(|&c| (850..1150).contains(&c)),
            "{counts:?}"
        );
    }
}
