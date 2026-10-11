//! Fuzzy matching (the same `nucleo` engine used by the Helix editor).

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

pub struct Scored {
    pub index: usize,
    pub score: u32,
    /// Char positions in the haystack that matched (for highlighting).
    pub positions: Vec<usize>,
}

/// Rank `items` against `query`. An empty query keeps the original order.
pub fn rank<T: AsRef<str>>(query: &str, items: &[T], paths: bool) -> Vec<Scored> {
    if query.trim().is_empty() {
        return (0..items.len())
            .map(|index| Scored {
                index,
                score: 0,
                positions: Vec::new(),
            })
            .collect();
    }
    let config = if paths {
        Config::DEFAULT.match_paths()
    } else {
        Config::DEFAULT
    };
    let mut matcher = Matcher::new(config);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut idx = Vec::new();
    let mut out: Vec<Scored> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            idx.clear();
            let hay = Utf32Str::new(item.as_ref(), &mut buf);
            let score = pattern.indices(hay, &mut matcher, &mut idx)?;
            let mut positions: Vec<usize> = idx.iter().map(|i| *i as usize).collect();
            positions.sort_unstable();
            positions.dedup();
            Some(Scored {
                index,
                score,
                positions,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.score.cmp(&a.score).then(
            items[a.index]
                .as_ref()
                .len()
                .cmp(&items[b.index].as_ref().len()),
        )
    });
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn ranks_best_match_first() {
        let items = ["src/main.rs", "src/editor.rs", "README.md"];
        let r = super::rank("edrs", &items, true);
        assert_eq!(items[r[0].index], "src/editor.rs");
        assert!(!r[0].positions.is_empty());
    }
}
