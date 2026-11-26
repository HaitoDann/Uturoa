//! Frais dynamiques et partage melt / proposeur (SPEC §7).

use uturoa_core::{Amount, MELT_PERCENT, MIN_BASE_FEE_PER_BYTE};

/// Variation maximale du frais de base par bloc : 1/8, soit 12,5 %.
const MAX_CHANGE_DENOMINATOR: u128 = 8;

/// Frais de base du bloc suivant.
///
/// La cible est la moitié de `max_block_bytes`. Au-dessus, le frais monte
/// (d'au moins une unité) ; en dessous, il baisse ; jamais sous le plancher.
pub fn next_base_fee(base: Amount, used_bytes: u64, max_block_bytes: u64) -> Amount {
    let target = u128::from(max_block_bytes / 2).max(1);
    let used = u128::from(used_bytes);
    let base_wide = u128::from(base);
    let next = if used > target {
        let delta = scaled(base_wide, used.saturating_sub(target), target);
        base_wide.saturating_add(delta.max(1))
    } else {
        let delta = scaled(base_wide, target.saturating_sub(used), target);
        base_wide.saturating_sub(delta)
    };
    Amount::try_from(next)
        .unwrap_or(Amount::MAX)
        .max(MIN_BASE_FEE_PER_BYTE)
}

/// `base × écart / cible / 8`, sans division par zéro (cible ≥ 1).
fn scaled(base: u128, gap: u128, target: u128) -> u128 {
    base.saturating_mul(gap)
        .checked_div(target.saturating_mul(MAX_CHANGE_DENOMINATOR))
        .unwrap_or(0)
}

/// Partage d'un frais : `(fondu, proposeur)`. L'arrondi va au proposeur.
pub fn split_fee(fee: Amount) -> (Amount, Amount) {
    let melt = u128::from(fee)
        .saturating_mul(u128::from(MELT_PERCENT))
        .checked_div(100)
        .unwrap_or(0);
    let melt = Amount::try_from(melt).unwrap_or(0);
    (melt, fee.saturating_sub(melt))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: u64 = 1_000_000;

    #[test]
    fn base_fee_moves_toward_target() {
        let base = 10_000;
        assert_eq!(next_base_fee(base, MAX / 2, MAX), base);
        assert_eq!(next_base_fee(base, MAX, MAX), base + base / 8);
        assert_eq!(next_base_fee(base, 0, MAX), base - base / 8);
        let up = next_base_fee(base, MAX * 3 / 4, MAX);
        assert!(up > base && up < base + base / 8);
    }

    #[test]
    fn base_fee_respects_floor_and_always_rises_when_full() {
        assert_eq!(
            next_base_fee(MIN_BASE_FEE_PER_BYTE, 0, MAX),
            MIN_BASE_FEE_PER_BYTE
        );
        assert_eq!(next_base_fee(1, 0, MAX), MIN_BASE_FEE_PER_BYTE);
        assert!(next_base_fee(MIN_BASE_FEE_PER_BYTE, MAX, MAX) > MIN_BASE_FEE_PER_BYTE);
        assert_eq!(next_base_fee(Amount::MAX, MAX, MAX), Amount::MAX);
    }

    #[test]
    fn fee_split_is_eighty_twenty() {
        assert_eq!(split_fee(100), (80, 20));
        assert_eq!(split_fee(7), (5, 2));
        assert_eq!(split_fee(0), (0, 0));
        let (m, p) = split_fee(Amount::MAX);
        assert_eq!(m.checked_add(p), Some(Amount::MAX));
    }
}
