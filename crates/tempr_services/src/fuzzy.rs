//! Small in-house fuzzy matcher for the command palette (docs/11-gpui.md
//! → Palette). Case-insensitive subsequence match with a score that favours
//! consecutive runs and word starts; no external dependency.

/// A successful match: higher `score` is better; `indices` are the byte
/// offsets of the matched characters in the candidate (for highlighting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    pub indices: Vec<usize>,
}

const MATCH: i32 = 1;
const CONSECUTIVE_BONUS: i32 = 10;
const WORD_START_BONUS: i32 = 6;
const EXACT_CASE_BONUS: i32 = 1;
const GAP_PENALTY: i32 = 1;

/// Match `query` against `candidate`. An empty query matches everything with
/// score 0. Greedy left-to-right; good enough for a few hundred commands.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    if query.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            indices: Vec::new(),
        });
    }
    let mut indices = Vec::with_capacity(query.chars().count());
    let mut score = 0i32;
    let mut cand = candidate.char_indices().peekable();
    let mut prev_matched_at: Option<usize> = None;
    let mut prev_char: Option<char> = None;

    for q in query.chars() {
        let mut gap = 0i32;
        let mut found = None;
        for (i, c) in cand.by_ref() {
            if c.eq_ignore_ascii_case(&q) || c.to_lowercase().eq(q.to_lowercase()) {
                found = Some((i, c));
                break;
            }
            gap += 1;
            prev_char = Some(c);
        }
        let (i, c) = found?;
        score += MATCH;
        if c == q {
            score += EXACT_CASE_BONUS;
        }
        let consecutive = prev_matched_at.is_some_and(|p| {
            // previous match immediately precedes this char
            candidate[p..i].chars().count() == 1
        });
        if consecutive {
            score += CONSECUTIVE_BONUS;
        }
        let word_start = match prev_char {
            None => i == 0 || prev_matched_at.is_none(),
            Some(pc) => !pc.is_alphanumeric() || (pc.is_lowercase() && c.is_uppercase()),
        };
        if word_start && !consecutive {
            score += WORD_START_BONUS;
        }
        score -= gap * GAP_PENALTY;
        indices.push(i);
        prev_matched_at = Some(i);
        prev_char = Some(c);
    }
    Some(FuzzyMatch { score, indices })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(q: &str, c: &str) -> i32 {
        fuzzy_match(q, c).map(|m| m.score).unwrap_or(i32::MIN)
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy_match("", "anything").unwrap().score, 0);
    }

    #[test]
    fn subsequence_required() {
        assert!(fuzzy_match("rq", "Run Query").is_some());
        assert!(fuzzy_match("yq", "Run Query").is_none());
        assert!(fuzzy_match("xyz", "Run Query").is_none());
    }

    #[test]
    fn indices_point_at_matched_chars() {
        let m = fuzzy_match("rq", "Run Query").unwrap();
        assert_eq!(m.indices, vec![0, 4]);
        let m = fuzzy_match("Ü", "grüße").unwrap();
        assert_eq!(m.indices, vec![2]);
    }

    #[test]
    fn scoring_prefers_word_starts_and_runs() {
        assert!(score("rq", "Run Query") > score("rq", "for query"));
        assert!(score("run", "Run Query") > score("run", "r u n"));
        assert!(
            score("Run", "Run Query") > score("run", "Run Query"),
            "exact case bonus"
        );
        assert!(
            score("cancel", "Cancel Query") > score("cancel", "Toggle Command Palette: cancel")
        );
    }
}
