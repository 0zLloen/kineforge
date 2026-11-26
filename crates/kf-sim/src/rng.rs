//! Seeded random numbers (STACK_DECISION.md §1: own generator, value-stable
//! across targets and releases). The generators are the published reference
//! algorithms; the samplers are fixed here and pinned by golden values, so a
//! seed always yields the same stimuli.

/// SplitMix64 (Vigna, <https://prng.di.unimi.it/splitmix64.c>): only used to
/// expand a 64-bit seed into a xoshiro state (header `seeding: "splitmix64"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// xoshiro256++ 1.0 (Blackman & Vigna,
/// <https://prng.di.unimi.it/xoshiro256plusplus.c>); header `algorithm:
/// "xoshiro256++"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Xoshiro256pp {
    pub s: [u64; 4],
}

impl Xoshiro256pp {
    /// State filled with four successive SplitMix64 outputs of `seed`
    /// (never all zero).
    pub fn from_seed(seed: u64) -> Xoshiro256pp {
        let mut sm = SplitMix64(seed);
        Xoshiro256pp {
            s: [sm.next_u64(), sm.next_u64(), sm.next_u64(), sm.next_u64()],
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.s;
        let result = s[0].wrapping_add(s[3]).rotate_left(23).wrapping_add(s[0]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Equivalent to 2¹²⁸ calls of [`next_u64`](Self::next_u64): the start of
    /// the next non-overlapping subsequence.
    pub fn jump(&mut self) {
        const JUMP: [u64; 4] = [
            0x180e_c6d3_3cfd_0aba,
            0xd5a6_1266_f0c9_392c,
            0xa958_2618_e03f_c9aa,
            0x39ab_dc45_29b1_661c,
        ];
        let mut acc = [0u64; 4];
        for word in JUMP {
            for b in 0..64 {
                if word & (1 << b) != 0 {
                    for (a, s) in acc.iter_mut().zip(self.s) {
                        *a ^= s;
                    }
                }
                self.next_u64();
            }
        }
        self.s = acc;
    }

    /// Uniform in [0, 1): the top 53 bits × 2⁻⁵³ (exact).
    pub fn unit_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// `a + (b − a) × unit_f64()`, for finite `a ≤ b`; `a` when they are equal.
    pub fn range_f64(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit_f64()
    }

    /// Uniform integer in [0, n) for n > 0, unbiased (Lemire's
    /// multiply-shift with rejection; usually one draw).
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0)");
        let mut m = u128::from(self.next_u64()) * u128::from(n);
        if (m as u64) < n {
            let t = n.wrapping_neg() % n;
            while (m as u64) < t {
                m = u128::from(self.next_u64()) * u128::from(n);
            }
        }
        (m >> 64) as u64
    }

    /// Fisher–Yates: for i from len − 1 down to 1, swap i with `below(i + 1)`.
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            v.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference vectors from the published C sources (next/jump copied
    // verbatim, state explicit), compiled with `cc -O2 -std=c11`:
    // https://prng.di.unimi.it/xoshiro256plusplus.c and splitmix64.c,
    // fetched 2026-11-26. The [1, 2, 3, 4] row also matches the values
    // published in rand_xoshiro's tests (41943041, 58720359, …).
    #[test]
    fn reference_vectors() {
        let take =
            |mut f: Box<dyn FnMut() -> u64>, n: usize| (0..n).map(|_| f()).collect::<Vec<_>>();
        let mut sm = SplitMix64(0);
        assert_eq!(
            take(Box::new(move || sm.next_u64()), 5),
            [
                0xe220a8397b1dcdaf,
                0x6e789e6aa1b965f4,
                0x06c45d188009454f,
                0xf88bb8a8724c81ec,
                0x1b39896a51a8749b
            ]
        );
        let mut sm = SplitMix64(0x5eed0001);
        assert_eq!(
            take(Box::new(move || sm.next_u64()), 5),
            [
                0x03317bb4875fb038,
                0xdc1fe0f7d1dc60cc,
                0x753b132de64bc477,
                0x2e63f48cf0091f85,
                0x50874239b8d4ed79
            ]
        );
        let mut x = Xoshiro256pp { s: [1, 2, 3, 4] };
        assert_eq!(
            take(Box::new(move || x.next_u64()), 10),
            [
                0x0000000002800001,
                0x0000000003800067,
                0x000cc00003800067,
                0x000cc201994400b2,
                0x8012a2019ac433cd,
                0x8a69978acdee33ba,
                0xc271134733154abd,
                0xac2ba09179169e97,
                0xdbf3190a8f073fd8,
                0x9105f14ab2229220
            ]
        );
        let seeded = |seed: u64, jumps: usize, n: usize| {
            let mut x = Xoshiro256pp::from_seed(seed);
            for _ in 0..jumps {
                x.jump();
            }
            take(Box::new(move || x.next_u64()), n)
        };
        assert_eq!(
            seeded(0, 0, 6),
            [
                0x53175d61490b23df,
                0x61da6f3dc380d507,
                0x5c0fdf91ec9a7bfc,
                0x02eebf8c3bbe5e1a,
                0x7eca04ebaf4a5eea,
                0x0543c37757f08d9a
            ]
        );
        assert_eq!(
            seeded(0x5eed0001, 0, 6),
            [
                0x23ed301c65f87af0,
                0x00e1fce97c5b6ac8,
                0x4bc7a2e19d3f2445,
                0x0996fd5ed3eae378,
                0xa8516ba67bfd1c3b,
                0x202ba120027cd3bc
            ]
        );
        assert_eq!(
            seeded(0x5eed0001, 1, 6),
            [
                0xf822d96f73e58241,
                0x09cb4de2f6aadc39,
                0xd867d77c82445334,
                0x03d88728bb0d72a0,
                0x8a653579078d06d0,
                0x4578553f2ad2a19e
            ]
        );
        assert_eq!(
            seeded(0x5eed0001, 2, 6),
            [
                0x085ff828e252201f,
                0x975166e507283d51,
                0x9c014e6d70868d7d,
                0x608df547175857eb,
                0xd383e9624d315f23,
                0x0c955141c1a0ead6
            ]
        );
        assert_eq!(
            seeded(u64::MAX, 0, 4),
            [
                0x56ccf8ce948e27b2,
                0xe68588432e5a5b90,
                0xe3e9b5a48119ca8b,
                0x460f19495532ae73
            ]
        );
    }

    #[test]
    fn samplers() {
        let mut x = Xoshiro256pp::from_seed(0x5eed0001);
        // unit_f64 is the top 53 bits of next_u64.
        let mut y = x;
        let u = x.unit_f64();
        assert_eq!(u, (y.next_u64() >> 11) as f64 / 9007199254740992.0);
        assert!((0.0..1.0).contains(&u));
        // Extremes of the mapping.
        assert_eq!(Xoshiro256pp { s: [0; 4] }.unit_f64(), 0.0);
        assert_eq!(x.range_f64(2.5, 2.5), 2.5);
        // below: in range, every value reachable, and n = 1 is always 0.
        let mut seen = [0u32; 7];
        for _ in 0..7000 {
            seen[x.below(7) as usize] += 1;
        }
        assert!(seen.iter().all(|&c| c > 800), "{seen:?}");
        assert_eq!(x.below(1), 0);
        assert!(x.below(u64::MAX) < u64::MAX);
        // shuffle is a permutation.
        let mut v: Vec<u32> = (0..24).collect();
        x.shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..24).collect::<Vec<_>>());
        assert_ne!(v, sorted);
    }

    /// `below` rejects about half the draws for n = 2⁶³ + 1 (t = 2⁶³ − 1):
    /// pinned values and the number of draws consumed cover the rejection
    /// loop, so a biased shortcut shows up.
    #[test]
    fn below_rejection_is_pinned() {
        let mut x = Xoshiro256pp::from_seed(0x5eed0001);
        let n = (1u64 << 63) + 1;
        let v: Vec<u64> = (0..8).map(|_| x.below(n)).collect();
        let mut reference = Xoshiro256pp::from_seed(0x5eed0001);
        let mut rejected = 0;
        let mut expect = Vec::new();
        while expect.len() < 8 {
            let m = u128::from(reference.next_u64()) * u128::from(n);
            if (m as u64) < n.wrapping_neg() % n {
                rejected += 1;
            } else {
                expect.push((m >> 64) as u64);
            }
        }
        assert_eq!(v, expect);
        assert!(rejected > 0, "the seed must exercise a rejection");
        assert_eq!(x.next_u64(), reference.next_u64(), "same number of draws");
    }

    /// Pinned sampler outputs: any change to a sampler changes every
    /// stimulus sequence, so it must show up here.
    #[test]
    fn sampler_golden_values() {
        let mut x = Xoshiro256pp::from_seed(0x5eed0001);
        let u: Vec<u64> = (0..4).map(|_| x.unit_f64().to_bits()).collect();
        let r: Vec<u64> = (0..4).map(|_| x.range_f64(-3.0, 5.0).to_bits()).collect();
        let b: Vec<u64> = [1, 2, 3, 7, 1000, u64::MAX].map(|n| x.below(n)).to_vec();
        let mut p: Vec<u8> = (0..16).collect();
        x.shuffle(&mut p);
        assert_eq!(
            (&u[..], &r[..], &b[..], &p[..], x.next_u64()),
            (
                &[
                    0x3fc1f6980e32fc3c,
                    0x3f6c3f9d2f8b6d00,
                    0x3fd2f1e8b8674fc8,
                    0x3fa32dfabda7d5c0
                ][..],
                &[
                    0x4002145ae99eff46,
                    0xbfffea2f6ffec198,
                    0x400be9158a46dbe4,
                    0xc007711ecdf49ee6
                ][..],
                &[0, 1, 1, 5, 430, 8027112489945272907][..],
                &[13, 12, 11, 6, 8, 1, 2, 5, 7, 14, 0, 10, 4, 9, 15, 3][..],
                0xa6157a04616d6560
            ),
            "sampler outputs changed: every seeded stimulus would change"
        );
    }
}
