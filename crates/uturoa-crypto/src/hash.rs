//! Hachage BLAKE3 et séparation de domaines.

/// Condensat BLAKE3-256.
pub type Hash = [u8; 32];

/// Étiquettes de domaine du protocole.
///
/// Chaque hachage et chaque message signé est préfixé par l'une d'elles, pour
/// qu'une valeur produite dans un contexte ne soit jamais valide dans un autre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Address,
    State,
    Tx,
    TxId,
    Block,
    Vote,
    Proposal,
    MerkleLeaf,
    MerkleNode,
    MerkleEmpty,
    SmtLeaf,
    SmtNode,
    Validators,
    TxApproval,
    TableKey,
    MintShare,
    Commit,
}

impl Domain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Domain::Address => "utu/address",
            Domain::State => "utu/state",
            Domain::Tx => "utu/tx",
            Domain::TxId => "utu/txid",
            Domain::Block => "utu/block",
            Domain::Vote => "utu/vote",
            Domain::Proposal => "utu/proposal",
            Domain::MerkleLeaf => "utu/merkle/leaf",
            Domain::MerkleNode => "utu/merkle/node",
            Domain::MerkleEmpty => "utu/merkle/empty",
            Domain::SmtLeaf => "utu/smt/leaf",
            Domain::SmtNode => "utu/smt/node",
            Domain::Validators => "utu/validators",
            Domain::TxApproval => "utu/tx/approval",
            Domain::TableKey => "utu/table/key",
            Domain::MintShare => "utu/mint",
            Domain::Commit => "utu/commit",
        }
    }
}

/// Préfixe `longueur(étiquette) ‖ étiquette` sans allocation.
///
/// La longueur sur un octet rend l'encodage injectif : `utu/tx` ne peut pas être
/// confondu avec le début de `utu/txid`.
fn prefix(domain: Domain) -> ([u8; 1], &'static [u8]) {
    let tag = domain.as_str().as_bytes();
    // Toutes les étiquettes font moins de 256 octets (vérifié par les tests).
    ([tag.len() as u8], tag)
}

/// Message préfixé par son domaine, tel qu'il est signé : `len ‖ étiquette ‖ données`.
pub fn tagged(domain: Domain, data: &[u8]) -> Vec<u8> {
    let (len, tag) = prefix(domain);
    let mut out = Vec::with_capacity(
        len.len()
            .saturating_add(tag.len())
            .saturating_add(data.len()),
    );
    out.extend_from_slice(&len);
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    out
}

/// `H(len ‖ étiquette ‖ données)` avec BLAKE3-256.
pub fn hash_tagged(domain: Domain, data: &[u8]) -> Hash {
    hash_tagged_parts(domain, &[data])
}

/// Comme [`hash_tagged`], sur la concaténation de plusieurs morceaux.
pub fn hash_tagged_parts(domain: Domain, parts: &[&[u8]]) -> Hash {
    let (len, tag) = prefix(domain);
    let mut hasher = blake3::Hasher::new();
    hasher.update(&len);
    hasher.update(tag);
    for part in parts {
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Domain; 17] = [
        Domain::Address,
        Domain::State,
        Domain::Tx,
        Domain::TxId,
        Domain::Block,
        Domain::Vote,
        Domain::Proposal,
        Domain::MerkleLeaf,
        Domain::MerkleNode,
        Domain::MerkleEmpty,
        Domain::SmtLeaf,
        Domain::SmtNode,
        Domain::Validators,
        Domain::TxApproval,
        Domain::TableKey,
        Domain::MintShare,
        Domain::Commit,
    ];

    /// Vecteur officiel : BLAKE3("abc"). Fige l'algorithme.
    #[test]
    fn blake3_reference_vector() {
        assert_eq!(
            hex::encode(blake3::hash(b"abc").as_bytes()),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
    }

    #[test]
    fn tags_are_short_and_distinct() {
        for (i, a) in ALL.iter().enumerate() {
            assert!(a.as_str().len() < 256);
            for b in &ALL[i + 1..] {
                assert_ne!(a.as_str(), b.as_str());
            }
        }
    }

    #[test]
    fn tagged_layout() {
        let m = tagged(Domain::Tx, b"abc");
        assert_eq!(m, b"\x06utu/txabc");
    }

    /// Sans préfixe de longueur, `utu/tx` ‖ "id…" et `utu/txid` ‖ "…" collisionneraient.
    #[test]
    fn prefix_domains_do_not_collide() {
        assert_ne!(
            hash_tagged(Domain::Tx, b"idX"),
            hash_tagged(Domain::TxId, b"X")
        );
    }

    #[test]
    fn same_data_different_domains() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(hash_tagged(*a, b"x"), hash_tagged(*b, b"x"));
            }
        }
    }

    #[test]
    fn parts_equal_concatenation() {
        assert_eq!(
            hash_tagged_parts(Domain::State, &[b"ab", b"c"]),
            hash_tagged(Domain::State, b"abc")
        );
        assert_eq!(
            hash_tagged(Domain::State, b"abc"),
            *blake3::hash(&tagged(Domain::State, b"abc")).as_bytes()
        );
    }
}
