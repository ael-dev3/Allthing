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

/// A Neynar score in millionths as "0.87".
pub fn score(micros: u32) -> String {
    format!("{:.2}", f64::from(micros.min(1_000_000)) / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn addresses_and_scores() {
        assert_eq!(short_address("0x187c7b0393ebe86378128f2653d0930e33218899"), "0x187c…8899");
        assert_eq!(short_address("ExAqci8uUVKtqHqFW58fmwgMMY9PATfRGGyv6837j9Lx"), "ExAq…j9Lx");
        assert_eq!(short_address("0xabc"), "0xabc");
        assert_eq!(score(1_000_000), "1.00");
        assert_eq!(score(873_400), "0.87");
        assert_eq!(score(0), "0.00");
    }
}
