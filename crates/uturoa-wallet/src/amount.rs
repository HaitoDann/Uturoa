//! Montants : 1 pièce = 10⁹ unités.

use anyhow::{Result, anyhow, bail};
use uturoa_core::{Amount, UNITS_PER_COIN};

/// « 1,5 » ou « 1.5 » pièce en unités.
pub fn parse_coins(s: &str) -> Result<Amount> {
    let s = s.trim().replace(',', ".").replace(' ', "");
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    if frac.len() > 9 || (int.is_empty() && frac.is_empty()) {
        bail!("montant invalide (9 décimales au plus)");
    }
    if !int.chars().chain(frac.chars()).all(|c| c.is_ascii_digit()) {
        bail!("montant invalide");
    }
    let int: Amount = if int.is_empty() { 0 } else { int.parse()? };
    let frac: Amount = format!("{frac:0<9}").parse()?;
    int.checked_mul(UNITS_PER_COIN)
        .and_then(|v| v.checked_add(frac))
        .ok_or_else(|| anyhow!("montant trop grand"))
}

/// Unités en pièces, à la française, sans zéros inutiles : `1 234,5`.
pub fn format_coins(units: Amount) -> String {
    let int = units.checked_div(UNITS_PER_COIN).unwrap_or(0);
    let frac = units.checked_rem(UNITS_PER_COIN).unwrap_or(0);
    let digits = int.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        let left = digits.len().saturating_sub(i);
        if i > 0 && left.is_multiple_of(3) {
            grouped.push('\u{202F}');
        }
        grouped.push(c);
    }
    if frac == 0 {
        return grouped;
    }
    let frac = format!("{frac:09}");
    format!("{grouped},{}", frac.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        assert_eq!(parse_coins("1,5").unwrap(), 1_500_000_000);
        assert_eq!(parse_coins("0.000000001").unwrap(), 1);
        assert!(parse_coins("1,0000000001").is_err());
        assert!(parse_coins("-1").is_err());
        assert!(parse_coins("").is_err());
        assert_eq!(format_coins(1_500_000_000), "1,5");
        assert_eq!(format_coins(1_234 * UNITS_PER_COIN), "1\u{202F}234");
        assert_eq!(format_coins(1), "0,000000001");
    }
}
