//! Stockage redb : blocs validés, leurs certificats, et instantanés d'état.

use anyhow::{Context, Result};
use redb::{Database, ReadableTable, TableDefinition};
use std::path::Path;
use uturoa_consensus::SignGuard;
use uturoa_core::{Block, CommitCertificate, decode, encode};
use uturoa_state::State;

const BLOCKS: TableDefinition<u64, &[u8]> = TableDefinition::new("blocks");
const COMMITS: TableDefinition<u64, &[u8]> = TableDefinition::new("commits");
const SNAPSHOTS: TableDefinition<u64, &[u8]> = TableDefinition::new("snapshots");
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const GUARD: &str = "guard";
const LOCK: &str = "lock";

pub struct Store {
    db: Database,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
        let db = Database::create(dir.join("chain.redb")).context("ouverture de la base")?;
        let tx = db.begin_write()?;
        tx.open_table(BLOCKS)?;
        tx.open_table(COMMITS)?;
        tx.open_table(SNAPSHOTS)?;
        tx.open_table(META)?;
        tx.commit()?;
        Ok(Self { db })
    }

    /// Enregistre un bloc validé, son certificat et, s'il est fourni, l'état
    /// après ce bloc, en une seule transaction.
    pub fn put(
        &self,
        block: &Block,
        commit: &CommitCertificate,
        snapshot: Option<&State>,
    ) -> Result<()> {
        let h = block.header.height;
        let tx = self.db.begin_write()?;
        {
            tx.open_table(BLOCKS)?.insert(h, encode(block).as_slice())?;
            tx.open_table(COMMITS)?
                .insert(h, encode(commit).as_slice())?;
            if let Some(state) = snapshot {
                let mut t = tx.open_table(SNAPSHOTS)?;
                t.insert(h, encode(state).as_slice())?;
                // On ne garde que les deux derniers instantanés.
                let old: Vec<u64> = t
                    .range(..h)?
                    .rev()
                    .skip(1)
                    .filter_map(|e| e.ok().map(|(k, _)| k.value()))
                    .collect();
                for k in old {
                    t.remove(k)?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn block(&self, height: u64) -> Result<Option<Block>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(BLOCKS)?;
        t.get(height)?
            .map(|v| decode(v.value()).context("bloc illisible"))
            .transpose()
    }

    pub fn commit(&self, height: u64) -> Result<Option<CommitCertificate>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(COMMITS)?;
        t.get(height)?
            .map(|v| decode(v.value()).context("certificat illisible"))
            .transpose()
    }

    /// Hauteur du dernier bloc enregistré (0 si aucun).
    pub fn tip(&self) -> Result<u64> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(BLOCKS)?;
        Ok(t.last()?.map_or(0, |(k, _)| k.value()))
    }

    /// Dernier instantané d'état, encodé, avec sa hauteur.
    pub fn latest_snapshot_raw(&self) -> Result<Option<(u64, Vec<u8>)>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(SNAPSHOTS)?;
        Ok(t.last()?.map(|(k, v)| (k.value(), v.value().to_vec())))
    }

    /// Installe un état reçu d'un pair, avec son bloc et son certificat.
    pub fn install_snapshot(
        &self,
        block: &Block,
        commit: &CommitCertificate,
        state: &State,
    ) -> Result<()> {
        self.put(block, commit, Some(state))
    }

    /// Dernier instantané d'état.
    pub fn latest_snapshot(&self) -> Result<Option<State>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(SNAPSHOTS)?;
        t.last()?
            .map(|(_, v)| decode(v.value()).context("instantané illisible"))
            .transpose()
    }

    fn meta<T: borsh::BorshDeserialize>(&self, key: &str) -> Result<Option<T>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(META)?;
        t.get(key)?
            .map(|v| decode(v.value()).context("métadonnée illisible"))
            .transpose()
    }

    fn put_meta<T: borsh::BorshSerialize>(&self, key: &str, value: &T) -> Result<()> {
        let tx = self.db.begin_write()?;
        tx.open_table(META)?.insert(key, encode(value).as_slice())?;
        tx.commit()?;
        Ok(())
    }

    /// Garde anti-double signature. À enregistrer **avant** d'envoyer la
    /// signature qu'elle autorise.
    pub fn load_guard(&self) -> Result<SignGuard> {
        Ok(self.meta(GUARD)?.unwrap_or_default())
    }

    pub fn save_guard(&self, guard: &SignGuard) -> Result<()> {
        self.put_meta(GUARD, guard)
    }

    /// Verrou de consensus `(hauteur, round, bloc)` du dernier precommit non
    /// nul, rétabli après un redémarrage. À enregistrer avant le precommit.
    pub fn load_lock(&self) -> Result<Option<(u64, u32, Block)>> {
        self.meta(LOCK)
    }

    pub fn save_lock(&self, height: u64, round: u32, block: &Block) -> Result<()> {
        self.put_meta(LOCK, &(height, round, block))
    }
}
