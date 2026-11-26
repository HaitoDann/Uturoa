//! Unités monétaires et constantes d'émission (SPEC §3, §5, §6).

/// Montant en unités : 1 pièce = 10⁹ unités.
pub type Amount = u64;

/// Nombre d'unités dans une pièce (9 décimales).
pub const UNITS_PER_COIN: Amount = 1_000_000_000;

/// Plafond absolu : 10⁹ pièces, soit 10¹⁸ unités.
pub const MAX_SUPPLY: Amount = 1_000_000_000 * UNITS_PER_COIN;

/// Durée visée d'un bloc, en millisecondes (SPEC §4).
///
/// Toute l'émission est comptée en blocs : changer la durée d'un bloc exige
/// une version de protocole qui ajuste en même temps [`MINT_PHASE_BLOCKS`],
/// [`MINT_PER_BLOCK`] et [`FORGE_DIVISOR`], sinon le calendrier d'émission
/// change.
pub const TARGET_BLOCK_MS: u64 = 12_000;

/// Blocs par an à la durée visée (365 jours).
pub const BLOCKS_PER_YEAR: u64 = 365 * 24 * 3_600 * 1_000 / TARGET_BLOCK_MS;

/// Durée de la phase de frappe, en blocs : 6 mois.
pub const MINT_PHASE_BLOCKS: u64 = BLOCKS_PER_YEAR / 2;

/// Quantité visée par la phase de frappe : 150 000 000 pièces.
pub const MINT_PHASE_TOTAL: Amount = 150_000_000 * UNITS_PER_COIN;

/// Unités frappées par bloc pendant la phase 1 (division entière).
pub const MINT_PER_BLOCK: Amount = MINT_PHASE_TOTAL / MINT_PHASE_BLOCKS;

/// Réserve minimale d'un compte : 0,01 pièce (SPEC §3).
pub const MIN_RESERVE: Amount = UNITS_PER_COIN / 100;

/// Plancher du frais de base, par octet de transaction : un envoi simple
/// (environ 200 octets) coûte alors environ 0,0001 pièce (SPEC §7).
pub const MIN_BASE_FEE_PER_BYTE: Amount = 500;

/// Part des frais fondue, en pourcentage ; le reste va au proposeur (SPEC §7).
pub const MELT_PERCENT: Amount = 80;

/// Diviseur de la forge : `forge = réservoir / FORGE_DIVISOR`.
///
/// Vaut 2²⁹ / 6 : le même rythme annuel qu'un décalage de 29 bits avec des
/// blocs de 2 s, soit environ 16 % d'inflation la première année après la
/// bascule.
pub const FORGE_DIVISOR: Amount = 89_478_485;

// Vérifications à la compilation.
const _: () = assert!(MAX_SUPPLY < u64::MAX / 18);
const _: () = assert!(BLOCKS_PER_YEAR == 2_628_000);
const _: () = assert!(MINT_PHASE_BLOCKS == 1_314_000);
const _: () = assert!(MINT_PER_BLOCK == 114_155_251_141);
const _: () =
    assert!(MINT_PER_BLOCK as u128 * MINT_PHASE_BLOCKS as u128 <= MINT_PHASE_TOTAL as u128);

/// Forge d'un bloc, `in_circulation` étant l'offre avant le bloc.
///
/// Toujours strictement inférieure au réservoir tant qu'il n'est pas vide :
/// le plafond ne peut pas être dépassé.
pub fn forge_amount(in_circulation: Amount) -> Amount {
    MAX_SUPPLY.saturating_sub(in_circulation) / FORGE_DIVISOR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supply_is_ten_to_the_eighteen() {
        assert_eq!(MAX_SUPPLY, 10u64.pow(18));
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
        assert_eq!(forge_amount(0), MAX_SUPPLY / FORGE_DIVISOR);
        let after_mint = MINT_PER_BLOCK * MINT_PHASE_BLOCKS;
        // Environ 9,5 pièces par bloc à la bascule.
        assert_eq!(
            forge_amount(after_mint),
            (MAX_SUPPLY - after_mint) / FORGE_DIVISOR
        );
        assert_eq!(forge_amount(after_mint), 9_499_490_296);
        let mut supply = MAX_SUPPLY - 1_000;
        for _ in 0..10 {
            supply += forge_amount(supply);
            assert!(supply <= MAX_SUPPLY);
        }
    }
}
