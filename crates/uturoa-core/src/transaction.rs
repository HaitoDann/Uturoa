//! Transactions : la liste fermée de SPEC §8.

use crate::{Amount, CoreError, Multisig, encode};
use borsh::{BorshDeserialize, BorshSerialize};
use uturoa_crypto::{Address, Domain, Hash, KeyPair, PublicKey, Signature, hash_tagged, tagged};

/// Taille maximale d'un mémo, en octets.
pub const MAX_MEMO_BYTES: usize = 64;

/// Nombre maximal de paiements d'un paiement groupé.
pub const MAX_BATCH_PAYMENTS: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Transaction {
    pub chain_id: String,
    pub sender: Address,
    /// Doit valoir `nonce du compte + 1`.
    pub nonce: u64,
    /// Au moins `frais de base par octet × taille de la transaction signée`.
    /// 80 % fondus, 20 % au proposeur.
    pub fee: Amount,
    pub body: TxBody,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum TxBody {
    /// Envoie `amount` à `to`, avec un mémo d'au plus [`MAX_MEMO_BYTES`] octets.
    Transfer {
        to: Address,
        amount: Amount,
        memo: Option<Vec<u8>>,
    } = 0x01,
    /// Plusieurs paiements en une transaction, au plus [`MAX_BATCH_PAYMENTS`].
    BatchTransfer { payments: Vec<Payment> } = 0x02,
    /// Rend le compte multisignature, change sa règle, ou la retire (`None`).
    ConfigureMultisig { multisig: Option<Multisig> } = 0x03,
    /// Délègue `amount` au validateur `validator` (phase 2).
    Delegate {
        validator: PublicKey,
        amount: Amount,
    } = 0x04,
    /// Retire `amount` de la délégation ; restitué après le délai de SPEC §4.3.
    Undelegate {
        validator: PublicKey,
        amount: Amount,
    } = 0x05,
    /// Inscrit le compte émetteur comme validateur, avec sa clé de consensus.
    RegisterValidator {
        key: PublicKey,
        info: ValidatorInfo,
        /// Commission sur les récompenses des délégants, en points de base.
        commission_bps: u16,
    } = 0x06,
    /// Met à jour la fiche et la commission du validateur de l'émetteur.
    UpdateValidatorInfo {
        info: ValidatorInfo,
        commission_bps: u16,
    } = 0x07,
    /// Demande la réintégration d'un validateur mis à l'écart.
    Unjail = 0x08,
    /// Quitte l'ensemble des validateurs.
    Leave = 0x09,
    /// Preuve de double signature, encodée par le moteur de consensus.
    ReportEquivocation { evidence: Vec<u8> } = 0x0A,
    /// Signale que le validateur de l'émetteur est prêt pour `version`.
    SignalUpgrade { version: u32 } = 0x0B,
}

/// Un paiement d'un [`TxBody::BatchTransfer`].
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Payment {
    pub to: Address,
    pub amount: Amount,
    pub memo: Option<Vec<u8>>,
}

/// Fiche publique d'un validateur. Les longueurs maximales sont vérifiées par l'état.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ValidatorInfo {
    pub name: String,
    pub site: String,
    pub contact: String,
}

impl TxBody {
    /// Les mémos respectent-ils [`MAX_MEMO_BYTES`] ?
    pub fn memos_fit(&self) -> bool {
        let fits = |m: &Option<Vec<u8>>| m.as_ref().is_none_or(|m| m.len() <= MAX_MEMO_BYTES);
        match self {
            TxBody::Transfer { memo, .. } => fits(memo),
            TxBody::BatchTransfer { payments } => payments.iter().all(|p| fits(&p.memo)),
            _ => true,
        }
    }
}

/// Approbation d'un signataire d'un compte multisignature.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Approval {
    /// Indice dans `Multisig::signers`.
    pub signer_index: u8,
    pub signature: Signature,
}

/// Autorisation d'une transaction.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum Authorization {
    /// Compte ordinaire. La clé est jointe tant que l'état ne la connaît pas.
    Single {
        public_key: Option<PublicKey>,
        signature: Signature,
    } = 0x01,
    /// Compte multisignature : approbations à indices strictement croissants.
    Multi { approvals: Vec<Approval> } = 0x02,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedTransaction {
    pub tx: Transaction,
    pub auth: Authorization,
}

impl Transaction {
    /// Message signé par un compte ordinaire : `"utu/tx" ‖ borsh(tx)`.
    pub fn signing_message(&self) -> Vec<u8> {
        tagged(Domain::Tx, &encode(self))
    }

    /// Message approuvé par un signataire multisignature : `"utu/tx/approval" ‖ borsh(tx)`.
    pub fn approval_message(&self) -> Vec<u8> {
        tagged(Domain::TxApproval, &encode(self))
    }

    /// Signe pour un compte ordinaire. `include_key` joint la clé (première dépense).
    pub fn sign(self, keypair: &KeyPair, include_key: bool) -> SignedTransaction {
        let signature = keypair.sign(&self.signing_message());
        SignedTransaction {
            tx: self,
            auth: Authorization::Single {
                public_key: include_key.then(|| keypair.public_key()),
                signature,
            },
        }
    }

    /// Approbation du signataire `index` d'un compte multisignature.
    pub fn approve(&self, index: u8, keypair: &KeyPair) -> Approval {
        Approval {
            signer_index: index,
            signature: keypair.sign(&self.approval_message()),
        }
    }
}

impl SignedTransaction {
    /// `H("utu/txid" ‖ borsh(signed_tx))`.
    pub fn id(&self) -> Hash {
        hash_tagged(Domain::TxId, &encode(self))
    }

    /// Vérifie une autorisation `Single` avec la clé du compte.
    pub fn verify_single(&self, key: &PublicKey) -> Result<(), CoreError> {
        match &self.auth {
            Authorization::Single { signature, .. } => key
                .verify(&self.tx.signing_message(), signature)
                .map_err(|_| CoreError::InvalidSignature),
            Authorization::Multi { .. } => Err(CoreError::InvalidSignature),
        }
    }

    /// Vérifie une autorisation `Multi` contre la règle du compte : au moins
    /// `threshold` signataires distincts, indices strictement croissants.
    pub fn verify_multisig(&self, multisig: &Multisig) -> Result<(), CoreError> {
        let bad = CoreError::InvalidApprovals;
        let Authorization::Multi { approvals } = &self.auth else {
            return Err(bad("approbations attendues"));
        };
        if approvals.len() < usize::from(multisig.threshold) {
            return Err(bad("seuil non atteint"));
        }
        if approvals.len() > multisig.signers.len() {
            return Err(bad("trop d'approbations"));
        }
        let message = self.tx.approval_message();
        let mut previous: Option<u8> = None;
        for approval in approvals {
            if previous.is_some_and(|p| approval.signer_index <= p) {
                return Err(bad("indices non strictement croissants"));
            }
            previous = Some(approval.signer_index);
            let key = multisig
                .signers
                .get(usize::from(approval.signer_index))
                .ok_or(bad("indice de signataire inconnu"))?;
            key.verify(&message, &approval.signature)
                .map_err(|_| bad("signature invalide"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;

    fn transfer(kp: &KeyPair) -> Transaction {
        Transaction {
            chain_id: "utu-test".into(),
            sender: Address::from_public_key(&kp.public_key()),
            nonce: 1,
            fee: 10_000,
            body: TxBody::Transfer {
                to: Address::from_bytes([1; 20]),
                amount: 5,
                memo: Some(b"FAC-1".to_vec()),
            },
        }
    }

    #[test]
    fn body_codes_match_spec() {
        let k = KeyPair::generate().public_key();
        let codes = [
            TxBody::Transfer {
                to: Address::from_bytes([1; 20]),
                amount: 1,
                memo: None,
            },
            TxBody::BatchTransfer { payments: vec![] },
            TxBody::ConfigureMultisig { multisig: None },
            TxBody::Delegate {
                validator: k.clone(),
                amount: 1,
            },
            TxBody::Undelegate {
                validator: k.clone(),
                amount: 1,
            },
            TxBody::RegisterValidator {
                key: k,
                info: ValidatorInfo::default(),
                commission_bps: 0,
            },
            TxBody::UpdateValidatorInfo {
                info: ValidatorInfo::default(),
                commission_bps: 0,
            },
            TxBody::Unjail,
            TxBody::Leave,
            TxBody::ReportEquivocation { evidence: vec![] },
            TxBody::SignalUpgrade { version: 2 },
        ]
        .map(|b| encode(&b)[0]);
        assert_eq!(codes, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
        assert!(decode::<TxBody>(&[0x00]).is_err());
        assert!(decode::<TxBody>(&[0x0C]).is_err());
    }

    #[test]
    fn sign_verify_roundtrip() {
        let kp = KeyPair::generate();
        let stx = transfer(&kp).sign(&kp, true);
        assert_eq!(stx.verify_single(&kp.public_key()), Ok(()));
        let bytes = encode(&stx);
        assert_eq!(decode::<SignedTransaction>(&bytes).unwrap(), stx);
        let mut extra = bytes;
        extra.push(0);
        assert!(decode::<SignedTransaction>(&extra).is_err());
    }

    #[test]
    fn any_change_invalidates_signature() {
        let kp = KeyPair::generate();
        let stx = transfer(&kp).sign(&kp, false);
        for mutate in [
            |t: &mut Transaction| t.chain_id = "autre-chaine".into(),
            |t: &mut Transaction| t.nonce = 2,
            |t: &mut Transaction| t.fee = 0,
            |t: &mut Transaction| t.body = TxBody::Leave,
        ] {
            let mut forged = stx.clone();
            mutate(&mut forged.tx);
            assert_eq!(
                forged.verify_single(&kp.public_key()),
                Err(CoreError::InvalidSignature)
            );
        }
    }

    #[test]
    fn multisig_two_of_three() {
        let kps: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
        let ms = Multisig {
            signers: kps.iter().map(KeyPair::public_key).collect(),
            threshold: 2,
        };
        let tx = transfer(&kps[0]);
        let ok = SignedTransaction {
            auth: Authorization::Multi {
                approvals: vec![tx.approve(0, &kps[0]), tx.approve(2, &kps[2])],
            },
            tx: tx.clone(),
        };
        assert_eq!(ok.verify_multisig(&ms), Ok(()));
        let one = SignedTransaction {
            auth: Authorization::Multi {
                approvals: vec![tx.approve(1, &kps[1])],
            },
            tx: tx.clone(),
        };
        assert!(one.verify_multisig(&ms).is_err());
        let dup = SignedTransaction {
            auth: Authorization::Multi {
                approvals: vec![tx.approve(1, &kps[1]), tx.approve(1, &kps[1])],
            },
            tx: tx.clone(),
        };
        assert!(dup.verify_multisig(&ms).is_err());
        let wrong_key = SignedTransaction {
            auth: Authorization::Multi {
                approvals: vec![tx.approve(0, &kps[1]), tx.approve(2, &kps[2])],
            },
            tx: tx.clone(),
        };
        assert!(wrong_key.verify_multisig(&ms).is_err());
        // Une signature ordinaire n'est pas une approbation, et inversement.
        assert!(
            tx.clone()
                .sign(&kps[0], false)
                .verify_multisig(&ms)
                .is_err()
        );
        assert!(ok.verify_single(&kps[0].public_key()).is_err());
    }

    #[test]
    fn memo_limit() {
        let mut body = TxBody::Transfer {
            to: Address::from_bytes([1; 20]),
            amount: 1,
            memo: Some(vec![0; 64]),
        };
        assert!(body.memos_fit());
        body = TxBody::Transfer {
            to: Address::from_bytes([1; 20]),
            amount: 1,
            memo: Some(vec![0; 65]),
        };
        assert!(!body.memos_fit());
    }
}
