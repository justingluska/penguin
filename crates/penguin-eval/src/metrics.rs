//! Ranking metrics, defined to match trec_eval so numbers can be checked
//! against it (`search-eval run --trec DIR` writes qrels and a run file).
//!
//! - nDCG@k: trec_eval `ndcg_cut`. Gain is the grade itself (linear), the
//!   discount is log2(rank + 1) from rank 1, and the ideal ranking is built
//!   from every judged document for the query, retrieved or not
//!   (Järvelin & Kekäläinen 2002; trec_eval m_ndcg_cut.c).
//! - RR (mean = MRR): 1 / rank of the first relevant document, 0 if none is
//!   retrieved (Voorhees, TREC-8 QA). No cutoff beyond the retrieved depth.
//! - Recall@k: relevant documents in the top k divided by min(k, R), where R
//!   is the number of relevant documents ("capped" recall, so a query with 300
//!   relevant items can still reach 1.0 at k = 10).
//!
//! "Relevant" for the binary measures means grade ≥ 2 (highly or perfectly
//! relevant); `trec_eval -l 2` gives the same RR and recall.

use std::collections::BTreeMap;

/// Minimum grade that counts as relevant for RR and recall.
pub const REL: u8 = 2;

pub fn grade(qrels: &BTreeMap<String, u8>, doc: &str) -> u8 {
    qrels.get(doc).copied().unwrap_or(0)
}

pub fn dcg(gains: impl Iterator<Item = u8>, k: usize) -> f64 {
    gains
        .take(k)
        .enumerate()
        .map(|(i, g)| g as f64 / ((i + 2) as f64).log2())
        // An empty f64 sum is -0.0; normalise so reports never show "-0.000".
        .sum::<f64>()
        + 0.0
}

pub fn ndcg_at(ranked: &[String], qrels: &BTreeMap<String, u8>, k: usize) -> f64 {
    let mut ideal: Vec<u8> = qrels.values().copied().filter(|g| *g > 0).collect();
    ideal.sort_unstable_by(|a, b| b.cmp(a));
    let idcg = dcg(ideal.into_iter(), k);
    if idcg == 0.0 {
        return 0.0;
    }
    dcg(ranked.iter().map(|d| grade(qrels, d)), k) / idcg
}

pub fn rr(ranked: &[String], qrels: &BTreeMap<String, u8>) -> f64 {
    ranked
        .iter()
        .position(|d| grade(qrels, d) >= REL)
        .map_or(0.0, |i| 1.0 / (i + 1) as f64)
}

pub fn recall_at(ranked: &[String], qrels: &BTreeMap<String, u8>, k: usize) -> f64 {
    let r = qrels.values().filter(|g| **g >= REL).count();
    if r == 0 {
        return 0.0;
    }
    let found = ranked
        .iter()
        .take(k)
        .filter(|d| grade(qrels, d) >= REL)
        .count();
    found as f64 / r.min(k) as f64
}

/// Share of the top k that has a judgment (real-mail mode: unjudged results
/// count as irrelevant, so a low value means the metrics are pessimistic
/// until those results are judged).
pub fn judged_at(ranked: &[String], judged: &BTreeMap<String, u8>, k: usize) -> f64 {
    let top: Vec<&String> = ranked.iter().take(k).collect();
    if top.is_empty() {
        return 1.0;
    }
    top.iter()
        .filter(|d| judged.contains_key(d.as_str()))
        .count() as f64
        / top.len() as f64
}

/// Nearest-rank percentile of already sorted values.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[i]
}

pub fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

/// Two-sided paired randomization (permutation) test on the mean of
/// per-query differences, as recommended by Smucker, Allan & Carterette
/// (CIKM 2007): randomly swap each query's pair of scores `b` times and count
/// how often the absolute mean difference is at least the observed one.
pub fn randomization_test(a: &[f64], b: &[f64], samples: usize, seed: u64) -> f64 {
    assert_eq!(a.len(), b.len());
    let d: Vec<f64> = a.iter().zip(b).map(|(x, y)| y - x).collect();
    if d.is_empty() {
        return 1.0;
    }
    let observed = mean(&d).abs();
    if observed == 0.0 {
        return 1.0;
    }
    let mut rng = crate::rng::Rng::new(seed);
    let mut hits = 0usize;
    for _ in 0..samples {
        let mut s = 0.0;
        let mut bits = 0u64;
        for (i, x) in d.iter().enumerate() {
            if i % 64 == 0 {
                bits = rng.next_u64();
            }
            s += if bits >> (i % 64) & 1 == 1 { *x } else { -*x };
        }
        if (s / d.len() as f64).abs() >= observed - 1e-12 {
            hits += 1;
        }
    }
    // Count the observed labelling itself (Phipson & Smyth: never p = 0).
    (hits + 1) as f64 / (samples + 1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pairs: &[(&str, u8)]) -> BTreeMap<String, u8> {
        pairs.iter().map(|(d, g)| (d.to_string(), *g)).collect()
    }
    fn r(ds: &[&str]) -> Vec<String> {
        ds.iter().map(|d| d.to_string()).collect()
    }

    #[test]
    fn ndcg_matches_hand_computation() {
        // Judged: a=3, b=2, c=1, d=0 (not retrieved: e=2).
        let qrels = q(&[("a", 3), ("b", 2), ("c", 1), ("d", 0), ("e", 2)]);
        let ranked = r(&["c", "a", "x", "b"]);
        // DCG@10 = 1/log2(2) + 3/log2(3) + 0 + 2/log2(5)
        let dcg = 1.0 + 3.0 / 3f64.log2() + 2.0 / 5f64.log2();
        // Ideal: 3,2,2,1
        let idcg = 3.0 + 2.0 / 3f64.log2() + 2.0 / 2.0 + 1.0 / 5f64.log2();
        assert!((ndcg_at(&ranked, &qrels, 10) - dcg / idcg).abs() < 1e-12);
        // Perfect ranking scores 1.
        assert!((ndcg_at(&r(&["a", "b", "e", "c"]), &qrels, 10) - 1.0).abs() < 1e-12);
        // Cutoff applies to both DCG and the ideal.
        assert!((ndcg_at(&r(&["a"]), &qrels, 1) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn rr_and_capped_recall_use_grade_two() {
        let qrels = q(&[("a", 1), ("b", 2), ("c", 3)]);
        assert_eq!(rr(&r(&["a", "x", "b"]), &qrels), 1.0 / 3.0);
        assert_eq!(rr(&r(&["a"]), &qrels), 0.0);
        assert_eq!(recall_at(&r(&["b", "x"]), &qrels, 10), 0.5);
        assert_eq!(recall_at(&r(&["c"]), &qrels, 1), 1.0); // capped at min(k, R)
    }

    #[test]
    fn randomization_test_separates_real_and_null_differences() {
        let a: Vec<f64> = (0..40).map(|i| (i % 5) as f64 / 10.0).collect();
        let better: Vec<f64> = a.iter().map(|x| x + 0.2).collect();
        assert!(randomization_test(&a, &better, 20_000, 7) < 0.001);
        let same = a.clone();
        assert_eq!(randomization_test(&a, &same, 1000, 7), 1.0);
        let noisy: Vec<f64> = a
            .iter()
            .enumerate()
            .map(|(i, x)| if i % 2 == 0 { x + 0.05 } else { x - 0.05 })
            .collect();
        assert!(randomization_test(&a, &noisy, 20_000, 7) > 0.5);
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        assert_eq!(percentile(&v, 0.5), 50.0);
        assert_eq!(percentile(&v, 0.95), 95.0);
        assert_eq!(percentile(&[3.0], 0.95), 3.0);
    }
}
