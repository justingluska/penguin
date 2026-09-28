//! The curated label colors Penguin offers. Gmail accepts label colors only
//! from a fixed set of hex values (users.labels `color.backgroundColor` /
//! `color.textColor`; anything else is a 400). Each entry pairs a palette
//! background with a palette text color that reads on it. The UI mirror is
//! `apps/desktop/src/lib/labelColors.ts` (keep the backgrounds in lockstep).

/// One offered label color: Gmail background, its text color, friendly name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelColor {
    pub background: &'static str,
    pub text: &'static str,
    pub name: &'static str,
}

const fn c(background: &'static str, text: &'static str, name: &'static str) -> LabelColor {
    LabelColor {
        background,
        text,
        name,
    }
}

pub const LABEL_COLORS: &[LabelColor] = &[
    c("#fb4c2f", "#ffffff", "Red"),
    c("#ffad47", "#ffffff", "Orange"),
    c("#fad165", "#000000", "Yellow"),
    c("#16a766", "#ffffff", "Green"),
    c("#43d692", "#094228", "Mint"),
    c("#4a86e8", "#ffffff", "Blue"),
    c("#c9daf8", "#1c4587", "Light blue"),
    c("#285bac", "#ffffff", "Navy"),
    c("#a479e2", "#ffffff", "Purple"),
    c("#f691b3", "#662e37", "Pink"),
    c("#822111", "#ffffff", "Maroon"),
    c("#999999", "#ffffff", "Gray"),
];

/// The palette entry for a background hex (case-insensitive), if offered.
pub fn by_background(background: &str) -> Option<&'static LabelColor> {
    let bg = background.trim();
    LABEL_COLORS
        .iter()
        .find(|c| c.background.eq_ignore_ascii_case(bg))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gmail's documented allowed values for both backgroundColor and
    /// textColor (users.labels reference, `Color`).
    const GMAIL_ALLOWED: &str = "#000000 #434343 #666666 #999999 #cccccc #efefef #f3f3f3 #ffffff \
        #fb4c2f #ffad47 #fad165 #16a766 #43d692 #4a86e8 #a479e2 #f691b3 #f6c5be #ffe6c7 #fef1d1 \
        #b9e4d0 #c6f3de #c9daf8 #e4d7f5 #fcdee8 #efa093 #ffd6a2 #fce8b3 #89d3b2 #a0eac9 #a4c2f4 \
        #d0bcf1 #fbc8d9 #e66550 #ffbc6b #fcda83 #44b984 #68dfa9 #6d9eeb #b694e8 #f7a7c0 #cc3a21 \
        #eaa041 #f2c960 #149e60 #3dc789 #3c78d8 #8e63ce #e07798 #ac2b16 #cf8933 #d5ae49 #0b804b \
        #2a9c68 #285bac #653e9b #b65775 #822111 #a46a21 #aa8831 #076239 #1a764d #1c4587 #41236d \
        #83334c #464646 #e7e7e7 #0d3472 #b6cff5 #0d3b44 #98d7e4 #3d188e #e3d7ff #711a36 #fbd3e0 \
        #8a1c0a #f2b2a8 #7a2e0b #ffc8af #7a4706 #ffdeb5 #594c05 #fbe983 #684e07 #fdedc1 #0b4f30 \
        #b3efd3 #04502e #a2dcc1 #c2c2c2 #4986e7 #2da2bb #b99aff #994a64 #f691b2 #ff7537 #ffad46 \
        #662e37 #ebdbde #cca6ac #094228 #42d692 #16a765";

    #[test]
    fn every_offered_color_is_in_gmails_allowed_set() {
        let allowed: Vec<&str> = GMAIL_ALLOWED.split_whitespace().collect();
        for c in LABEL_COLORS {
            assert!(
                allowed.contains(&c.background),
                "{} not allowed",
                c.background
            );
            assert!(allowed.contains(&c.text), "{} not allowed", c.text);
        }
        let mut bgs: Vec<_> = LABEL_COLORS.iter().map(|c| c.background).collect();
        bgs.sort();
        bgs.dedup();
        assert_eq!(bgs.len(), LABEL_COLORS.len(), "duplicate background");
    }

    #[test]
    fn lookup_is_case_insensitive_and_rejects_others() {
        assert_eq!(by_background("#FB4C2F").unwrap().text, "#ffffff");
        assert!(by_background("#123456").is_none());
        // Allowed by Gmail but not offered.
        assert!(by_background("#434343").is_none());
    }
}
