//! Parts de frappe de la phase 1 (SPEC §5).
//!
//! Une part n'est pas une transaction : elle n'a ni émetteur, ni nonce, ni
//! frais, pour qu'un mineur sans aucune pièce puisse frapper. Le bénéficiaire
//! fait partie des données hachées par RandomX : copier une part en changeant
//! le bénéficiaire la rend invalide. La vérification RandomX et la difficulté
//! relèvent de l'état (lot 3).

use crate::encode;
use borsh::{BorshDeserialize, BorshSerialize};
use uturoa_crypto::{Address, Domain, Hash, hash_tagged, tagged};

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct MintShare {
    pub chain_id: String,
    /// Adresse créditée.
    pub beneficiary: Address,
    /// Hauteur du bloc d'ancrage, récent : interdit le calcul à l'avance.
    pub anchor_height: u64,
    /// Empreinte du bloc d'ancrage.
    pub anchor_hash: Hash,
    pub nonce: u64,
}

impl MintShare {
    /// Données passées à RandomX : `"utu/mint" ‖ borsh(part)`.
    pub fn pow_input(&self) -> Vec<u8> {
        tagged(Domain::MintShare, &encode(self))
    }

    /// Identifiant de la part, pour ne l'accepter qu'une fois.
    pub fn id(&self) -> Hash {
        hash_tagged(Domain::MintShare, &encode(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;

    fn share() -> MintShare {
        MintShare {
            chain_id: "utu-test".into(),
            beneficiary: Address::from_bytes([3; 20]),
            anchor_height: 10,
            anchor_hash: [4; 32],
            nonce: 7,
        }
    }

    #[test]
    fn roundtrip_and_binding() {
        let s = share();
        assert_eq!(decode::<MintShare>(&encode(&s)).unwrap(), s);
        let mut other = s.clone();
        other.beneficiary = Address::from_bytes([9; 20]);
        assert_ne!(other.pow_input(), s.pow_input());
        assert_ne!(other.id(), s.id());
    }
}
