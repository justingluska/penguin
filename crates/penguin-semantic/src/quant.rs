//! Vector quantization and the distance kernels over it.
//!
//! - **int8, per-vector scale** (`I8Vec`): each unit vector is stored as
//!   `round(x / s)` with `s = max|x| / 127`, so one float per row keeps the
//!   full range of that row. Cosine ≈ `dot_i8(a, b) · s_a · s_b`. 4× smaller
//!   than f32; the ranking error is measured in `tests/quality.rs` and the
//!   bench (docs/SEMANTIC.md).
//! - **binary** (`bits`): the sign of each dimension, 1 bit per dimension,
//!   compared with Hamming distance. 32× smaller than f32; only good enough
//!   to pick candidates, which are then rescored with int8.
//!
//! The kernels are plain loops written so LLVM vectorizes them: on
//! aarch64-apple-darwin (baseline CPU `apple-m1`, which has the `dotprod`
//! extension) the int8 dot product becomes `sdot`, on x86-64 `pmaddwd`
//! (SSE2) or AVX2 when enabled; the Hamming loop becomes `cnt` (NEON) or
//! `popcnt`.

/// Quantize a (unit) vector to int8 with a per-vector scale. Returns the
/// codes and the scale `s` such that `x ≈ code · s`.
pub fn quantize_i8(v: &[f32], out: &mut Vec<i8>) -> f32 {
    let max = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    out.clear();
    if max == 0.0 || !max.is_finite() {
        out.resize(v.len(), 0);
        return 0.0;
    }
    let inv = 127.0 / max;
    out.extend(v.iter().map(|x| (x * inv).round().clamp(-127.0, 127.0) as i8));
    max / 127.0
}

/// Undo [`quantize_i8`] (approximately).
pub fn dequantize_i8(codes: &[i8], scale: f32) -> Vec<f32> {
    codes.iter().map(|&c| c as f32 * scale).collect()
}

/// Integer dot product of two int8 vectors of equal length.
#[inline]
pub fn dot_i8(a: &[i8], b: &[i8]) -> i32 {
    debug_assert_eq!(a.len(), b.len());
    // Blocks of 32 with i32 lanes: the shape LLVM turns into sdot / pmaddwd.
    let mut acc = [0i32; 32];
    let chunks = a.len() / 32;
    for c in 0..chunks {
        let (x, y) = (&a[c * 32..c * 32 + 32], &b[c * 32..c * 32 + 32]);
        for i in 0..32 {
            acc[i] += x[i] as i32 * y[i] as i32;
        }
    }
    let mut sum: i32 = acc.iter().sum();
    for i in chunks * 32..a.len() {
        sum += a[i] as i32 * b[i] as i32;
    }
    sum
}

/// int4 codes (−7…7) derived from int8 codes of the same vector, packed two
/// per byte (even dimension in the low nibble). Returns the scale such that
/// `x ≈ nibble · scale`, given the int8 scale `s8`.
pub fn pack_i4_from_i8(codes: &[i8], s8: f32, out: &mut Vec<u8>) -> f32 {
    let max = codes.iter().map(|c| c.unsigned_abs()).max().unwrap_or(0);
    out.clear();
    out.resize(codes.len().div_ceil(2), 0);
    if max == 0 {
        return 0.0;
    }
    let k = 7.0 / max as f32;
    for (i, &c) in codes.iter().enumerate() {
        let n = (c as f32 * k).round().clamp(-7.0, 7.0) as i8;
        let nib = (n as u8) & 0x0f;
        out[i / 2] |= if i % 2 == 0 { nib } else { nib << 4 };
    }
    s8 * max as f32 / 7.0
}

/// Dot product of int8 query codes with packed int4 codes.
#[inline]
pub fn dot_i8_i4(q: &[i8], packed: &[u8]) -> i32 {
    debug_assert_eq!(packed.len(), q.len().div_ceil(2));
    let mut acc = [0i32; 16];
    let pairs = q.len() / 2;
    let blocks = pairs / 16;
    for b in 0..blocks {
        let p = &packed[b * 16..b * 16 + 16];
        let x = &q[b * 32..b * 32 + 32];
        for j in 0..16 {
            // Sign-extend each nibble with arithmetic shifts.
            let lo = ((p[j] << 4) as i8 >> 4) as i32;
            let hi = (p[j] as i8 >> 4) as i32;
            acc[j] += lo * x[2 * j] as i32 + hi * x[2 * j + 1] as i32;
        }
    }
    let mut sum: i32 = acc.iter().sum();
    for i in blocks * 32..q.len() {
        let byte = packed[i / 2];
        let n = if i % 2 == 0 { (byte << 4) as i8 >> 4 } else { byte as i8 >> 4 };
        sum += n as i32 * q[i] as i32;
    }
    sum
}

/// Words of 64 bits needed for `dims` sign bits.
pub fn bit_words(dims: usize) -> usize {
    dims.div_ceil(64)
}

/// Sign bits of `v` (bit set = positive), packed little-endian into u64s.
pub fn binarize(v: &[f32], out: &mut Vec<u64>) {
    out.clear();
    out.resize(bit_words(v.len()), 0);
    for (i, x) in v.iter().enumerate() {
        if *x > 0.0 {
            out[i / 64] |= 1 << (i % 64);
        }
    }
}

/// Same as [`binarize`] for int8 codes.
pub fn binarize_i8(v: &[i8], out: &mut Vec<u64>) {
    out.clear();
    out.resize(bit_words(v.len()), 0);
    for (i, x) in v.iter().enumerate() {
        if *x > 0 {
            out[i / 64] |= 1 << (i % 64);
        }
    }
}

/// Number of differing bits.
#[inline]
pub fn hamming(a: &[u64], b: &[u64]) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(seed: u64, dims: usize) -> Vec<f32> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut v: Vec<f32> = (0..dims)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s % 20_001) as f32 / 10_000.0 - 1.0
            })
            .collect();
        crate::normalize(&mut v);
        v
    }

    #[test]
    fn int8_roundtrip_keeps_cosine_close() {
        let mut qa = Vec::new();
        let mut qb = Vec::new();
        for seed in 0..50 {
            let a = unit(seed, 384);
            let b = unit(seed + 1000, 384);
            let sa = quantize_i8(&a, &mut qa);
            let sb = quantize_i8(&b, &mut qb);
            let exact = crate::dot(&a, &b);
            let approx = dot_i8(&qa, &qb) as f32 * sa * sb;
            assert!((exact - approx).abs() < 0.01, "{exact} vs {approx}");
            let self_sim = dot_i8(&qa, &qa) as f32 * sa * sa;
            assert!((self_sim - 1.0).abs() < 0.01, "{self_sim}");
        }
    }

    #[test]
    fn dot_i8_matches_naive_for_odd_lengths() {
        for len in [0, 1, 31, 32, 33, 100, 257, 768] {
            let a: Vec<i8> = (0..len).map(|i| ((i * 37) % 255) as i16 as i8).collect();
            let b: Vec<i8> = (0..len).map(|i| ((i * 91 + 7) % 255) as i16 as i8).collect();
            let naive: i32 = a.iter().zip(&b).map(|(x, y)| *x as i32 * *y as i32).sum();
            assert_eq!(dot_i8(&a, &b), naive, "len {len}");
        }
    }

    #[test]
    fn int4_packing_and_dot() {
        for len in [1usize, 2, 31, 32, 33, 64, 256, 257] {
            let q: Vec<i8> = (0..len).map(|i| ((i * 53 % 255) as i16 - 127) as i8).collect();
            let codes: Vec<i8> = (0..len).map(|i| ((i * 29 % 255) as i16 - 127) as i8).collect();
            let mut packed = Vec::new();
            let s = pack_i4_from_i8(&codes, 0.01, &mut packed);
            let max = codes.iter().map(|c| c.unsigned_abs()).max().unwrap() as f32;
            let nibbles: Vec<i32> = codes.iter().map(|&c| (c as f32 * 7.0 / max).round() as i32).collect();
            let naive: i32 = nibbles.iter().zip(&q).map(|(n, x)| n * *x as i32).sum();
            assert_eq!(dot_i8_i4(&q, &packed), naive, "len {len}");
            assert!((s - 0.01 * max / 7.0).abs() < 1e-6);
        }
        // int4 cosine tracks the real one closely enough to pick candidates.
        let a = unit(1, 256);
        let b = unit(2, 256);
        let (mut qa, mut qb, mut pb) = (Vec::new(), Vec::new(), Vec::new());
        let sa = quantize_i8(&a, &mut qa);
        let sb = quantize_i8(&b, &mut qb);
        let s4 = pack_i4_from_i8(&qb, sb, &mut pb);
        let approx = dot_i8_i4(&qa, &pb) as f32 * sa * s4;
        assert!((approx - crate::dot(&a, &b)).abs() < 0.05, "{approx}");
    }

    #[test]
    fn zero_vector_quantizes_to_zero() {
        let mut q = Vec::new();
        assert_eq!(quantize_i8(&[0.0; 8], &mut q), 0.0);
        assert_eq!(q, vec![0; 8]);
    }

    #[test]
    fn binary_codes_and_hamming() {
        let mut a = Vec::new();
        let mut b = Vec::new();
        binarize(&[1.0, -1.0, 0.5, 0.0], &mut a);
        assert_eq!(a, vec![0b0101]);
        binarize(&[-1.0, -1.0, 0.5, 0.2], &mut b);
        assert_eq!(hamming(&a, &b), 2);
        let v: Vec<f32> = (0..130).map(|i| if i % 3 == 0 { 1.0 } else { -1.0 }).collect();
        binarize(&v, &mut a);
        assert_eq!(a.len(), 3);
        let mut q = Vec::new();
        quantize_i8(&v, &mut q);
        binarize_i8(&q, &mut b);
        assert_eq!(a, b);
        assert_eq!(hamming(&a, &b), 0);
    }
}
