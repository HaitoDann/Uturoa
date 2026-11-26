//! La chaîne locale : construction, vérification et validation définitive des
//! blocs, au-dessus de l'état et du stockage.

use crate::genesis_file::GenesisFile;
use crate::store::Store;
use anyhow::{Context, Result, anyhow, bail};
use std::path::Path;
use uturoa_consensus::{check_header_link, proposer_index};
use uturoa_core::{
    Address, Block, BlockHeader, CommitCertificate, Hash, MintShare, PublicKey, SignedTransaction,
    encode,
};
use uturoa_crypto::{Domain, hash_tagged};
use uturoa_state::{BlockInput, PowHasher, State};

/// Un instantané d'état est enregistré tous les `SNAPSHOT_INTERVAL` blocs.
pub const SNAPSHOT_INTERVAL: u64 = 1_000;

/// Empreinte d'un ensemble de validateurs, engagée dans l'en-tête.
pub fn validators_hash(keys: &[PublicKey]) -> Hash {
    hash_tagged(Domain::Validators, &encode(&keys.to_vec()))
}

/// Empreinte du bloc entier (en-tête et contenu).
fn full_hash(block: &Block) -> Hash {
    hash_tagged(Domain::Block, &encode(block))
}

pub struct Chain {
    state: State,
    store: Store,
    genesis_hash: Hash,
    genesis_time_ms: u64,
    last_header: Option<BlockHeader>,
    pow: Box<dyn PowHasher + Send>,
    snapshot_interval: u64,
    /// Dernier bloc vérifié (empreinte du bloc entier) et l'état qui en
    /// résulte : un bloc est vérifié à sa proposition puis validé, sans être
    /// réappliqué.
    verified: std::sync::Mutex<Option<(Hash, State)>>,
}

impl Chain {
    /// Ouvre la chaîne : dernier instantané (ou genèse), puis rejoue les blocs
    /// enregistrés après lui.
    pub fn open(dir: &Path, genesis: &GenesisFile, pow: Box<dyn PowHasher + Send>) -> Result<Self> {
        let store = Store::open(dir)?;
        let genesis_state = genesis.state()?;
        let state = match store.latest_snapshot()? {
            Some(s) if s.chain_id == genesis_state.chain_id => s,
            Some(_) => bail!("la base appartient à une autre chaîne"),
            None => genesis_state,
        };
        let mut chain = Self {
            state,
            store,
            genesis_hash: genesis.hash()?,
            genesis_time_ms: genesis.genesis_time_ms,
            last_header: None,
            pow,
            snapshot_interval: SNAPSHOT_INTERVAL,
            verified: std::sync::Mutex::new(None),
        };
        if chain.state.height > 0 {
            chain.last_header = chain.store.block(chain.state.height)?.map(|b| b.header);
        }
        let tip = chain.store.tip()?;
        for h in chain.state.height.saturating_add(1)..=tip {
            let block = chain
                .store
                .block(h)?
                .ok_or_else(|| anyhow!("bloc {h} manquant dans la base"))?;
            let next = chain
                .verify(&block, u64::MAX)
                .with_context(|| format!("rejeu du bloc {h}"))?;
            chain.state = next;
            chain.last_header = Some(block.header);
        }
        Ok(chain)
    }

    /// Intervalle entre deux instantanés (tests et réglages).
    pub fn set_snapshot_interval(&mut self, blocks: u64) {
        self.snapshot_interval = blocks.max(1);
    }

    pub fn pow(&self) -> &dyn PowHasher {
        self.pow.as_ref()
    }

    /// Horodatage du dernier bloc validé (celui de la genèse au départ).
    pub fn parent_time_ms(&self) -> u64 {
        self.last_header
            .as_ref()
            .map_or(self.genesis_time_ms, |h| h.timestamp)
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn height(&self) -> u64 {
        self.state.height
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Empreinte du dernier bloc validé (celle de la genèse au départ).
    pub fn tip_hash(&self) -> Hash {
        self.last_header
            .as_ref()
            .map_or(self.genesis_hash, BlockHeader::hash)
    }

    /// Validateurs du prochain bloc, dans l'ordre du consensus.
    pub fn next_validators(&self) -> Vec<PublicKey> {
        self.state.next_validators()
    }

    /// Certificat du dernier bloc validé, à inclure dans le suivant.
    pub fn last_commit(&self) -> Result<Option<CommitCertificate>> {
        if self.state.height == 0 {
            return Ok(None);
        }
        self.store.commit(self.state.height)
    }

    fn proposer_address(&self, index: u16) -> Address {
        self.next_validators()
            .get(usize::from(index))
            .and_then(|k| self.state.staking().operator_of(k))
            .unwrap_or(Address::from_bytes([0; 20]))
    }

    /// Construit le prochain bloc pour le round `round`.
    pub fn build_block(
        &self,
        round: u32,
        txs: Vec<SignedTransaction>,
        shares: Vec<MintShare>,
        timestamp_ms: u64,
    ) -> Result<Block> {
        let height = self.state.height.saturating_add(1);
        let n = self.next_validators().len();
        let proposer = if n == 0 {
            0
        } else {
            proposer_index(height, round, n).ok_or_else(|| anyhow!("pas de proposeur"))?
        };
        let mut block = Block {
            header: BlockHeader {
                protocol_version: self.state.protocol_version,
                chain_id: self.state.chain_id.clone(),
                height,
                timestamp: timestamp_ms,
                prev_hash: self.tip_hash(),
                tx_root: [0; 32],
                last_commit_hash: [0; 32],
                state_root: [0; 32],
                validators_hash: [0; 32],
                proposer,
            },
            last_commit: self.last_commit()?,
            txs,
            mint_shares: shares,
        };
        block.header.tx_root = block.compute_tx_root();
        block.header.last_commit_hash = block.compute_last_commit_hash();
        let next = self.execute(&block)?;
        block.header.state_root = next.state_root();
        block.header.validators_hash = validators_hash(&next.next_validators());
        if let Ok(mut v) = self.verified.lock() {
            *v = Some((full_hash(&block), next));
        }
        Ok(block)
    }

    /// Applique le contenu du bloc à une copie de l'état.
    fn execute(&self, block: &Block) -> Result<State> {
        let mut next = self.state.clone();
        let proposer = self.proposer_address(block.header.proposer);
        // La taille encodée ne dépend pas des valeurs des racines (32 octets
        // chacune) : le proposeur et les vérificateurs obtiennent la même.
        let bytes = u64::try_from(block.encoded_len()).unwrap_or(u64::MAX);
        next.apply_block(
            &BlockInput {
                height: block.header.height,
                parent_hash: block.header.prev_hash,
                txs: &block.txs,
                shares: &block.mint_shares,
                proposer: &proposer,
                last_commit: block.last_commit.as_ref(),
                block_bytes: bytes,
            },
            self.pow.as_ref(),
        )
        .map_err(|e| anyhow!("bloc {} refusé : {e}", block.header.height))?;
        Ok(next)
    }

    /// Vérifie entièrement un bloc candidat et renvoie l'état qui en résulte.
    pub fn verify(&self, block: &Block, now_ms: u64) -> Result<State> {
        // Clé du cache : le bloc **complet**, pas seulement l'en-tête, pour
        // qu'un contenu modifié sous le même en-tête soit revérifié.
        let hash = full_hash(block);
        let cached = self.verified.lock().ok().and_then(|v| {
            v.as_ref()
                .filter(|(h, s)| *h == hash && s.height == block.header.height)
                .map(|(_, s)| s.clone())
        });
        if let Some(state) = cached {
            // Seul l'horodatage dépend de l'heure de la vérification.
            if let Some(parent) = &self.last_header {
                check_header_link(parent, &block.header, now_ms)
                    .map_err(|e| anyhow!("chaînage de l'en-tête : {e:?}"))?;
            }
            return Ok(state);
        }
        let state = self.verify_uncached(block, now_ms)?;
        if let Ok(mut v) = self.verified.lock() {
            *v = Some((hash, state.clone()));
        }
        Ok(state)
    }

    fn verify_uncached(&self, block: &Block, now_ms: u64) -> Result<State> {
        let h = &block.header;
        if h.chain_id != self.state.chain_id {
            bail!("autre chaîne");
        }
        if h.protocol_version != self.state.protocol_version {
            bail!("version de protocole inattendue");
        }
        match &self.last_header {
            Some(parent) => check_header_link(parent, h, now_ms)
                .map_err(|e| anyhow!("chaînage de l'en-tête : {e:?}"))?,
            None => {
                if h.height != 1 || h.prev_hash != self.genesis_hash {
                    bail!("le premier bloc doit suivre la genèse");
                }
                if h.timestamp <= self.genesis_time_ms {
                    bail!("horodatage antérieur à la genèse");
                }
            }
        }
        let n = self.next_validators().len();
        if n > 0 && usize::from(h.proposer) >= n {
            bail!("indice de proposeur hors de l'ensemble");
        }
        let params = &self.state.params;
        if u64::try_from(block.txs.len()).unwrap_or(u64::MAX) > params.max_block_txs {
            bail!("trop de transactions");
        }
        if u64::try_from(block.encoded_len()).unwrap_or(u64::MAX) > params.max_block_bytes {
            bail!("bloc trop gros");
        }
        if block.compute_tx_root() != h.tx_root {
            bail!("racine des transactions incorrecte");
        }
        if block.compute_last_commit_hash() != h.last_commit_hash {
            bail!("empreinte du certificat précédent incorrecte");
        }
        let next = self.execute(block)?;
        if next.state_root() != h.state_root {
            bail!("racine d'état incorrecte");
        }
        if validators_hash(&next.next_validators()) != h.validators_hash {
            bail!("ensemble de validateurs suivant incorrect");
        }
        Ok(next)
    }

    /// Dernier instantané enregistré : bloc, certificat et état encodé.
    pub fn snapshot(&self) -> Result<Option<(Block, CommitCertificate, Vec<u8>)>> {
        let Some((height, bytes)) = self.store.latest_snapshot_raw()? else {
            return Ok(None);
        };
        match (self.store.block(height)?, self.store.commit(height)?) {
            (Some(b), Some(c)) => Ok(Some((b, c, bytes))),
            _ => Ok(None),
        }
    }

    /// Installe un instantané reçu d'un pair. Il n'est accepté que si son
    /// certificat est signé par l'ensemble de validateurs **connu** à cette
    /// hauteur (celui de l'amorçage tant qu'il n'a pas changé), ou si
    /// l'empreinte du bloc a été fournie par l'opérateur (`trusted`).
    pub fn install_snapshot(
        &mut self,
        block: Block,
        commit: CommitCertificate,
        state_bytes: &[u8],
        trusted: Option<Hash>,
    ) -> Result<()> {
        let h = block.header.height;
        if h <= self.state.height {
            bail!("instantané plus ancien que notre chaîne");
        }
        if commit.height != h || commit.block_hash != block.hash() {
            bail!("certificat d'un autre bloc");
        }
        let state: State = uturoa_core::decode_canonical(state_bytes)
            .map_err(|_| anyhow!("état illisible ou non canonique"))?;
        if state.height != h || state.chain_id != self.state.chain_id {
            bail!("état d'une autre hauteur ou d'une autre chaîne");
        }
        if state.state_root() != block.header.state_root {
            bail!("l'état ne correspond pas à la racine du bloc");
        }
        let vouched = trusted == Some(block.hash());
        if !vouched {
            // L'ensemble qui a signé le bloc `h` est celui que l'état d'avant
            // annonçait ; on le lit dans l'historique de l'état reçu et on
            // exige qu'il soit identique à notre ensemble actuel.
            let signers = state.staking().set_at(h);
            if signers != self.next_validators() {
                bail!("ensemble de validateurs inconnu : fournir --trusted-block");
            }
            commit
                .verify(&self.state.chain_id, &signers)
                .map_err(|e| anyhow!("certificat invalide : {e}"))?;
        }
        self.store.install_snapshot(&block, &commit, &state)?;
        self.state = state;
        self.last_header = Some(block.header);
        Ok(())
    }

    /// Valide définitivement un bloc, avec le certificat qui le prouve.
    pub fn commit_block(&mut self, block: Block, commit: CommitCertificate) -> Result<()> {
        let validators = self.next_validators();
        if commit.height != block.header.height || commit.block_hash != block.hash() {
            bail!("certificat d'un autre bloc");
        }
        if !validators.is_empty() {
            commit
                .verify(&self.state.chain_id, &validators)
                .map_err(|e| anyhow!("certificat invalide : {e}"))?;
        }
        let next = self.verify(&block, u64::MAX)?;
        let snapshot =
            (next.height.checked_rem(self.snapshot_interval) == Some(0)).then_some(&next);
        self.store.put(&block, &commit, snapshot)?;
        self.state = next;
        self.last_header = Some(block.header);
        Ok(())
    }
}
