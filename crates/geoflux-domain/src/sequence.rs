//! The order towers are walked in: natural order of their numbers, so `9_0` comes before `10_0`
//! and `76_0` before `76_1`.

use std::cmp::Ordering;

/// Compares two labels the way people read them: runs of digits by value, other text without
/// regard to case. Ties fall back to the plain text, so the order is total and stable.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.trim(), b.trim());
    loop {
        match (x.chars().next(), y.chars().next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let (nx, rx) = split_digits(x);
                let (ny, ry) = split_digits(y);
                let ord = cmp_digits(nx, ny);
                if ord != Ordering::Equal {
                    return ord;
                }
                (x, y) = (rx, ry);
            }
            (Some(c), Some(d)) => {
                let ord = c.to_lowercase().cmp(d.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                (x, y) = (&x[c.len_utf8()..], &y[d.len_utf8()..]);
            }
        }
    }
}

/// The leading run of ASCII digits and the rest.
fn split_digits(s: &str) -> (&str, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s.split_at(end)
}

/// Compares digit runs by value without parsing, so any length works: leading zeros dropped,
/// then the longer number is larger, then digit by digit.
fn cmp_digits(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// The rows of `labels` (row, label) in natural order of label; equal labels keep row order.
pub fn order<'a>(labels: impl IntoIterator<Item = (usize, &'a str)>) -> Vec<usize> {
    let mut v: Vec<(usize, &str)> = labels.into_iter().collect();
    v.sort_by(|(ra, a), (rb, b)| natural_cmp(a, b).then(ra.cmp(rb)));
    v.into_iter().map(|(r, _)| r).collect()
}

/// The entry before or after `current` in `sequence`, stopping at the ends. When `current` is
/// not in it, the first entry (or the last, going back).
pub fn step(sequence: &[usize], current: Option<usize>, forward: bool) -> Option<usize> {
    let at = current.and_then(|c| sequence.iter().position(|&r| r == c));
    match (at, forward) {
        (None, true) => sequence.first().copied(),
        (None, false) => sequence.last().copied(),
        (Some(i), true) => sequence.get(i + 1).or(sequence.get(i)).copied(),
        (Some(i), false) => sequence.get(i.saturating_sub(1)).copied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_sort_by_value() {
        let mut v = vec!["10_0", "9_0", "76_1", "76_0", "T-100", "T-20", "a2", "A10", "007"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["007", "9_0", "10_0", "76_0", "76_1", "a2", "A10", "T-20", "T-100"]);
    }

    #[test]
    fn huge_and_odd_labels_never_panic() {
        let long = "9".repeat(500);
        assert_eq!(natural_cmp(&long, "1"), Ordering::Greater);
        assert_eq!(natural_cmp("", ""), Ordering::Equal);
        assert_eq!(natural_cmp("", "1"), Ordering::Less);
        assert_eq!(natural_cmp("ä1", "Ä2"), Ordering::Less);
        assert_ne!(natural_cmp("01", "1"), Ordering::Equal, "ties fall back to the text");
    }

    #[test]
    fn order_and_step() {
        let rows = order([(0, "77_0"), (1, "76_0"), (2, "78_0"), (3, "76_0")]);
        assert_eq!(rows, [1, 3, 0, 2]);
        assert_eq!(step(&rows, Some(3), true), Some(0));
        assert_eq!(step(&rows, Some(2), true), Some(2), "stops at the end");
        assert_eq!(step(&rows, Some(1), false), Some(1), "stops at the start");
        assert_eq!(step(&rows, None, true), Some(1));
        assert_eq!(step(&rows, Some(99), false), Some(2));
        assert_eq!(step(&[], None, true), None);
    }
}
