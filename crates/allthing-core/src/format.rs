// SPDX-License-Identifier: AGPL-3.0-or-later
//! Number and address formatting for display.

/// Compact counts: 999, 9,999, 12.3k, 123k, 1.2m.
pub fn compact_number(n: u64) -> String {
    if n < 10_000 {
        full_number(n)
    } else if n < 1_000_000 {
        let k = n as f64 / 1000.0;
        trim_zero(if n < 100_000 { format!("{k:.1}") } else { format!("{k:.0}") }) + "k"
    } else {
        trim_zero(format!("{:.1}", n as f64 / 1_000_000.0)) + "m"
    }
}

/// Counts where room is short, as the Farcaster app shows them: 999, 1.2k,
/// 12.3k, 123k, 1.2m (a reply's row, under the post).
pub fn short_number(n: u64) -> String {
    if (1_000..10_000).contains(&n) { trim_zero(format!("{:.1}", n as f64 / 1000.0)) + "k" } else { compact_number(n) }
}

fn trim_zero(value: String) -> String {
    value.strip_suffix(".0").map(str::to_owned).unwrap_or(value)
}

/// 1221547 -> "1,221,547".
pub fn full_number(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "1 reply", "2,048 replies".
pub fn plural(n: u64, singular: &str, plural: &str) -> String {
    format!("{} {}", full_number(n), if n == 1 { singular } else { plural })
}

/// "0x187c…8899" / "ExAq…j9Lx".
pub fn short_address(address: &str) -> String {
    let chars: Vec<char> = address.chars().collect();
    let head = if address.starts_with("0x") { 6 } else { 4 };
    if chars.len() <= head + 6 {
        return address.to_owned();
    }
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - 4..].iter().collect();
    format!("{start}…{end}")
}

/// A chart's value axis for counts up to `max`: a round step (1, 2 or 5
/// times a power of ten) and the first multiple of it at or above `max`, in
/// four steps at most. 7,200 → (2,000, 8,000).
pub fn axis(max: u64) -> (u64, u64) {
    let mut unit: u64 = 1;
    loop {
        for step in [unit, unit.saturating_mul(2), unit.saturating_mul(5)] {
            let steps = max.div_ceil(step).max(1);
            if steps <= 4 {
                return (step, steps.saturating_mul(step));
            }
        }
        unit = unit.saturating_mul(10);
    }
}

/// An axis label: 0, 500, 1.5k, 8k, 2m.
pub fn tick(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => trim_zero(format!("{:.1}", n as f64 / 1_000.0)) + "k",
        _ => trim_zero(format!("{:.1}", n as f64 / 1_000_000.0)) + "m",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_take_round_steps() {
        assert_eq!(axis(7_200), (2_000, 8_000));
        assert_eq!(axis(6_174), (2_000, 8_000));
        assert_eq!(axis(1_350), (500, 1_500));
        assert_eq!(axis(3_047), (1_000, 4_000));
        assert_eq!(axis(41), (20, 60));
        assert_eq!(axis(4), (1, 4));
        assert_eq!(axis(0), (1, 1));
        assert_eq!(axis(u64::MAX).1, u64::MAX);
        let labels: Vec<String> = [0, 500, 1_500, 8_000, 12_000, 2_000_000].into_iter().map(tick).collect();
        assert_eq!(labels, ["0", "500", "1.5k", "8k", "12k", "2m"]);
    }

    #[test]
    fn compact_and_full_numbers() {
        assert_eq!(compact_number(0), "0");
        assert_eq!(compact_number(9_999), "9,999");
        assert_eq!(compact_number(12_345), "12.3k");
        assert_eq!(compact_number(20_000), "20k");
        assert_eq!(compact_number(123_456), "123k");
        assert_eq!(compact_number(1_221_548), "1.2m");
        assert_eq!(compact_number(3_000_000), "3m");
        assert_eq!(full_number(1_221_548), "1,221,548");
        assert_eq!(full_number(999), "999");
        assert_eq!(plural(1, "reply", "replies"), "1 reply");
        assert_eq!(plural(2048, "reply", "replies"), "2,048 replies");
    }

    #[test]
    fn short_numbers() {
        assert_eq!(short_number(999), "999");
        assert_eq!(short_number(1_000), "1k");
        assert_eq!(short_number(1_234), "1.2k");
        assert_eq!(short_number(9_960), "10k");
        assert_eq!(short_number(12_345), "12.3k");
        assert_eq!(short_number(1_221_548), "1.2m");
    }

    #[test]
    fn short_addresses() {
        assert_eq!(short_address("0x187c7b0393ebe86378128f2653d0930e33218899"), "0x187c…8899");
        assert_eq!(short_address("ExAqci8uUVKtqHqFW58fmwgMMY9PATfRGGyv6837j9Lx"), "ExAq…j9Lx");
        assert_eq!(short_address("0xabc"), "0xabc");
    }
}
