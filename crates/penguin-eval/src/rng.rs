//! splitmix64: tiny, fast and deterministic, so a seed always yields the
//! same mailbox on every machine.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// An independent stream for one generator family, so adding messages to
    /// one family doesn't reshuffle every other family.
    pub fn fork(seed: u64, family: &str) -> Rng {
        let h = family.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
        });
        Rng(seed ^ h)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo + 1) as u64) as i64
    }

    pub fn f(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.f() < p
    }

    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }

    /// Index drawn from a Zipf-like distribution (weight 1/(i+1)), so a few
    /// correspondents and senders dominate, as in a real mailbox.
    pub fn zipf(&mut self, n: usize) -> usize {
        let total: f64 = (1..=n).map(|i| 1.0 / i as f64).sum();
        let mut x = self.f() * total;
        for i in 0..n {
            x -= 1.0 / (i + 1) as f64;
            if x <= 0.0 {
                return i;
            }
        }
        n - 1
    }

    /// Random digits of length `n` (no leading zero).
    pub fn digits(&mut self, n: usize) -> String {
        let mut s = String::with_capacity(n);
        s.push((b'1' + self.below(9) as u8) as char);
        for _ in 1..n {
            s.push((b'0' + self.below(10) as u8) as char);
        }
        s
    }

    /// Upper-case letters and digits without look-alikes (booking codes).
    pub fn code(&mut self, n: usize) -> String {
        const A: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        (0..n).map(|_| A[self.below(A.len())] as char).collect()
    }
}
