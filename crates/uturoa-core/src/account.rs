//! Comptes et multisignatures (SPEC §8).

use crate::Amount;
use borsh::{BorshDeserialize, BorshSerialize};
use uturoa_crypto::PublicKey;

/// Nombre maximal de signataires d'un compte multisignature.
pub const MAX_MULTISIG_SIGNERS: usize = 10;

#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Account {
    pub balance: Amount,
    pub nonce: u64,
    /// Connue après la première transaction signée du compte.
    pub key: Option<PublicKey>,
    /// Si présente, seules les approbations des signataires autorisent le compte.
    pub multisig: Option<Multisig>,
}

/// Règle `threshold` sur `signers.len()`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Multisig {
    /// Clés distinctes, au plus [`MAX_MULTISIG_SIGNERS`].
    pub signers: Vec<PublicKey>,
    /// Entre 1 et `signers.len()`.
    pub threshold: u8,
}

impl Multisig {
    /// Règle bien formée : 1 à 10 clés distinctes, seuil entre 1 et le nombre de clés.
    pub fn is_valid(&self) -> bool {
        let n = self.signers.len();
        let distinct = self.signers.iter().enumerate().all(|(i, k)| {
            self.signers
                .iter()
                .skip(i.saturating_add(1))
                .all(|o| o != k)
        });
        (1..=MAX_MULTISIG_SIGNERS).contains(&n)
            && self.threshold >= 1
            && usize::from(self.threshold) <= n
            && distinct
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uturoa_crypto::KeyPair;

    fn keys(n: usize) -> Vec<PublicKey> {
        (0..n).map(|_| KeyPair::generate().public_key()).collect()
    }

    #[test]
    fn multisig_rules() {
        assert!(
            Multisig {
                signers: keys(3),
                threshold: 2
            }
            .is_valid()
        );
        assert!(
            Multisig {
                signers: keys(10),
                threshold: 10
            }
            .is_valid()
        );
        assert!(
            !Multisig {
                signers: keys(11),
                threshold: 2
            }
            .is_valid()
        );
        assert!(
            !Multisig {
                signers: keys(3),
                threshold: 0
            }
            .is_valid()
        );
        assert!(
            !Multisig {
                signers: keys(3),
                threshold: 4
            }
            .is_valid()
        );
        assert!(
            !Multisig {
                signers: vec![],
                threshold: 1
            }
            .is_valid()
        );
        let k = keys(1);
        assert!(
            !Multisig {
                signers: vec![k[0].clone(), k[0].clone()],
                threshold: 1
            }
            .is_valid()
        );
    }
}
