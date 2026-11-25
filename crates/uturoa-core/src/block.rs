//! Blocs (SPEC §4).

use crate::{CommitCertificate, MintShare, SignedTransaction, encode};
use borsh::{BorshDeserialize, BorshSerialize};
use uturoa_crypto::{Domain, Hash, hash_tagged, merkle_root};

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlockHeader {
    pub protocol_version: u32,
    pub chain_id: String,
    pub height: u64,
    /// Millisecondes Unix.
    pub timestamp: u64,
    pub prev_hash: Hash,
    /// Merkle des `tx_id` puis des identifiants de parts, dans l'ordre du bloc.
    pub tx_root: Hash,
    /// Empreinte de `last_commit` (zéros au premier bloc).
    pub last_commit_hash: Hash,
    /// État après application du bloc.
    pub state_root: Hash,
    /// Ensemble de validateurs du bloc suivant.
    pub validators_hash: Hash,
    /// Indice du proposeur dans l'ensemble actif.
    pub proposer: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Block {
    pub header: BlockHeader,
    /// Certificat du bloc précédent, avec toutes les signatures reçues.
    /// Absent au premier bloc après la genèse.
    pub last_commit: Option<CommitCertificate>,
    pub txs: Vec<SignedTransaction>,
    /// Parts de frappe, acceptées seulement pendant la phase 1.
    pub mint_shares: Vec<MintShare>,
}

impl BlockHeader {
    /// `H("utu/block" ‖ borsh(header))`.
    pub fn hash(&self) -> Hash {
        hash_tagged(Domain::Block, &encode(self))
    }
}

impl Block {
    pub fn hash(&self) -> Hash {
        self.header.hash()
    }

    /// Racine des transactions et des parts, à comparer à `header.tx_root`.
    pub fn compute_tx_root(&self) -> Hash {
        let ids: Vec<Hash> = self
            .txs
            .iter()
            .map(SignedTransaction::id)
            .chain(self.mint_shares.iter().map(MintShare::id))
            .collect();
        merkle_root(&ids)
    }

    /// Empreinte de `last_commit`, à comparer à `header.last_commit_hash`.
    pub fn compute_last_commit_hash(&self) -> Hash {
        match &self.last_commit {
            Some(c) => hash_tagged(Domain::Commit, &encode(c)),
            None => [0; 32],
        }
    }

    /// Taille encodée, à comparer à `max_block_bytes`.
    pub fn encoded_len(&self) -> usize {
        encode(self).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Transaction, TxBody, decode};
    use uturoa_crypto::{Address, KeyPair};

    fn block() -> Block {
        let kp = KeyPair::generate();
        let tx = Transaction {
            chain_id: "utu-test".into(),
            sender: Address::from_public_key(&kp.public_key()),
            nonce: 1,
            fee: 1,
            body: TxBody::Leave,
        }
        .sign(&kp, true);
        let share = MintShare {
            chain_id: "utu-test".into(),
            beneficiary: Address::from_bytes([5; 20]),
            anchor_height: 1,
            anchor_hash: [6; 32],
            nonce: 0,
        };
        let mut b = Block {
            header: BlockHeader {
                protocol_version: 1,
                chain_id: "utu-test".into(),
                height: 2,
                timestamp: 1_700_000_000_000,
                prev_hash: [0; 32],
                tx_root: [0; 32],
                last_commit_hash: [0; 32],
                state_root: [1; 32],
                validators_hash: [2; 32],
                proposer: 0,
            },
            last_commit: Some(CommitCertificate {
                height: 1,
                round: 0,
                block_hash: [7; 32],
                signatures: vec![],
            }),
            txs: vec![tx],
            mint_shares: vec![share],
        };
        b.header.tx_root = b.compute_tx_root();
        b.header.last_commit_hash = b.compute_last_commit_hash();
        b
    }

    #[test]
    fn roundtrip_and_hash() {
        let b = block();
        let bytes = encode(&b);
        assert_eq!(decode::<Block>(&bytes).unwrap(), b);
        assert_eq!(b.encoded_len(), bytes.len());
        let mut other = b.header.clone();
        other.timestamp += 1;
        assert_ne!(other.hash(), b.hash());
    }

    #[test]
    fn roots_cover_txs_shares_and_commit() {
        let b = block();
        let ids = [b.txs[0].id(), b.mint_shares[0].id()];
        assert_eq!(b.compute_tx_root(), merkle_root(&ids));
        let mut no_share = b.clone();
        no_share.mint_shares.clear();
        assert_ne!(no_share.compute_tx_root(), b.header.tx_root);
        let mut no_commit = b.clone();
        no_commit.last_commit = None;
        assert_eq!(no_commit.compute_last_commit_hash(), [0; 32]);
        assert_ne!(
            no_commit.compute_last_commit_hash(),
            b.header.last_commit_hash
        );
    }
}
