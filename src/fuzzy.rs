//! Fuzzy matching for the window search.

/// How well `query` matches `text`, or `None` when its letters do not appear
/// in order. Case-insensitive; runs of consecutive letters and letters that
/// start a word score higher. An empty query matches everything equally.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut total = 0;
    let mut at = 0;
    let mut prev: Option<usize> = None;
    for q in query.chars().flat_map(char::to_lowercase) {
        // Prefer the nearest word start, else the next occurrence.
        let found = (at..text.len()).find(|&i| text[i] == q && word_start(&text, i)).or_else(|| (at..text.len()).find(|&i| text[i] == q))?;
        total += 1;
        if word_start(&text, found) {
            total += 8;
        }
        if prev.is_some_and(|p| p + 1 == found) {
            total += 5;
        }
        if prev.is_none() {
            // Matches near the start win.
            total -= (found as i32).min(10);
        }
        prev = Some(found);
        at = found + 1;
    }
    Some(total)
}

fn word_start(text: &[char], i: usize) -> bool {
    i == 0 || !text[i - 1].is_alphanumeric()
}

/// Indices of `items` that match, best first (ties keep their order).
pub fn rank<'a>(query: &str, items: impl Iterator<Item = &'a str>) -> Vec<usize> {
    let mut hits: Vec<(usize, i32)> =
        items.enumerate().filter_map(|(i, t)| score(query, t).map(|s| (i, s))).collect();
    hits.sort_by_key(|&(_, s)| std::cmp::Reverse(s));
    hits.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_subsequences_case_insensitively() {
        assert!(score("srv", "2 Server").is_some());
        assert!(score("SRV", "server").is_some());
        assert!(score("vrs", "server").is_none());
        assert_eq!(score("", "anything"), Some(0));
        assert!(score("x", "").is_none());
    }

    #[test]
    fn ranks_word_starts_and_runs_first() {
        let names = ["1 my-server", "2 server", "3 observer", "4 shell"];
        let order = rank("serv", names.iter().copied());
        assert_eq!(order[0], 1, "{order:?}");
        assert!(order.contains(&0) && order.contains(&2));
        assert!(!order.contains(&3));
    }

    #[test]
    fn ties_keep_window_order() {
        let order = rank("", ["a", "b", "c"].into_iter());
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn numbers_match_the_label() {
        assert_eq!(rank("3", ["1 a", "2 b", "3 c"].into_iter()), vec![2]);
    }
}
