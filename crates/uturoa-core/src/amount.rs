//! Unités monétaires et constantes d'émission (SPEC §3, §5, §6).

/// Montant en unités : 1 pièce = 10⁸ unités.
pub type Amount = u64;

/// Nombre d'unités dans une pièce (8 décimales).
pub const UNITS_PER_COIN: Amount = 100_000_000;

/// Plafond absolu : 10⁹ pièces, soit 10¹⁷ unités.
pub const MAX_SUPPLY: Amount = 1_000_000_000 * UNITS_PER_COIN;

/// Durée de la phase de frappe, en blocs (6 mois de blocs de 2 s).
pub const MINT_PHASE_BLOCKS: u64 = 7_884_000;

/// Quantité visée par la phase de frappe : 150 000 000 pièces.
pub const MINT_PHASE_TOTAL: Amount = 150_000_000 * UNITS_PER_COIN;

/// Unités frappées par bloc pendant la phase 1 (division entière).
pub const MINT_PER_BLOCK: Amount = MINT_PHASE_TOTAL / MINT_PHASE_BLOCKS;

/// Décalage de la forge : `forge = réservoir >> FORGE_SHIFT`.
pub const FORGE_SHIFT: u32 = 29;

// Vérifications à la compilation.
const _: () = assert!(MAX_SUPPLY < u64::MAX / 100);
const _: () = assert!(MINT_PER_BLOCK == 1_902_587_519);
const _: () =
    assert!(MINT_PER_BLOCK as u128 * MINT_PHASE_BLOCKS as u128 <= MINT_PHASE_TOTAL as u128);

/// Forge d'un bloc, `in_circulation` étant l'offre avant le bloc.
///
/// Toujours strictement inférieure au réservoir tant qu'il n'est pas vide :
/// le plafond ne peut pas être dépassé.
pub fn forge_amount(in_circulation: Amount) -> Amount {
    MAX_SUPPLY.saturating_sub(in_circulation) >> FORGE_SHIFT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supply_is_ten_to_the_seventeen() {
        assert_eq!(MAX_SUPPLY, 10u64.pow(17));
    }

    #[test]
    fn mint_phase_stays_under_target() {
        let minted = MINT_PER_BLOCK * MINT_PHASE_BLOCKS;
        assert!(minted <= MINT_PHASE_TOTAL);
        assert!(MINT_PHASE_TOTAL - minted < MINT_PHASE_BLOCKS);
    }

    #[test]
    fn forge_never_exceeds_cap() {
        assert_eq!(forge_amount(MAX_SUPPLY), 0);
        assert_eq!(forge_amount(MAX_SUPPLY + 5), 0);
        assert_eq!(forge_amount(0), MAX_SUPPLY >> 29);
        let after_mint = MINT_PER_BLOCK * MINT_PHASE_BLOCKS;
        // Environ 1,58 pièce par bloc à la bascule.
        assert_eq!(forge_amount(after_mint), (MAX_SUPPLY - after_mint) >> 29);
        let mut supply = MAX_SUPPLY - 1_000;
        for _ in 0..10 {
            supply += forge_amount(supply);
            assert!(supply <= MAX_SUPPLY);
        }
    }
}
