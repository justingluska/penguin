//! IMAP sequence sets (`1:5,7,9:*`), for UID ranges both ways.

/// Compress UIDs into a sequence set (`1:3,7`). Empty input → empty string.
pub fn format(uids: &[u32]) -> String {
    let mut sorted: Vec<u32> = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = String::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == end {
            out.push_str(&start.to_string());
        } else {
            out.push_str(&format!("{start}:{end}"));
        }
        i += 1;
    }
    out
}

/// Expand a sequence set into UIDs. `*` means `star` (the largest UID the
/// caller knows); ranges are capped at `limit` UIDs in total so a hostile
/// `1:4294967295` can't exhaust memory. None if malformed.
pub fn parse(set: &str, star: u32, limit: usize) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for part in set.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let num = |s: &str| -> Option<u32> {
            if s == "*" {
                Some(star)
            } else {
                s.parse().ok()
            }
        };
        match part.split_once(':') {
            Some((a, b)) => {
                let (a, b) = (num(a)?, num(b)?);
                let (lo, hi) = (a.min(b), a.max(b));
                if (hi - lo) as usize >= limit.saturating_sub(out.len()) {
                    return None;
                }
                out.extend(lo..=hi);
            }
            None => out.push(num(part)?),
        }
        if out.len() > limit {
            return None;
        }
    }
    Some(out)
}

/// Split UIDs (any order) into chunks of at most `n`, highest first.
pub fn chunks_desc(uids: &[u32], n: usize) -> Vec<Vec<u32>> {
    let mut sorted: Vec<u32> = uids.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    sorted.dedup();
    sorted.chunks(n.max(1)).map(<[u32]>::to_vec).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(format(&[7, 1, 2, 3, 9, 10]), "1:3,7,9:10");
        assert_eq!(format(&[]), "");
        assert_eq!(format(&[5]), "5");
        assert_eq!(
            parse("1:3,7,9:10", 0, 100).unwrap(),
            vec![1, 2, 3, 7, 9, 10]
        );
        assert_eq!(parse("5:*", 7, 100).unwrap(), vec![5, 6, 7]);
        assert_eq!(parse("3:1", 0, 100).unwrap(), vec![1, 2, 3]);
        assert!(parse("1:4294967295", 0, 1000).is_none());
        assert!(parse("x", 0, 10).is_none());
    }

    #[test]
    fn chunks_go_newest_first() {
        assert_eq!(
            chunks_desc(&[1, 5, 3, 4, 2], 2),
            vec![vec![5, 4], vec![3, 2], vec![1]]
        );
    }
}
