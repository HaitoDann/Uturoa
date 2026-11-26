//! Stockage redb : blocs validés, leurs certificats, et instantanés d'état.

use anyhow::{Context, Result};
use redb::{Database, ReadableTable, TableDefinition};
use std::path::Path;
use uturoa_core::{Block, CommitCertificate, decode, encode};
use uturoa_state::State;

const BLOCKS: TableDefinition<u64, &[u8]> = TableDefinition::new("blocks");
const COMMITS: TableDefinition<u64, &[u8]> = TableDefinition::new("commits");
const SNAPSHOTS: TableDefinition<u64, &[u8]> = TableDefinition::new("snapshots");

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

    /// Dernier instantané d'état.
    pub fn latest_snapshot(&self) -> Result<Option<State>> {
        let tx = self.db.begin_read()?;
        let t = tx.open_table(SNAPSHOTS)?;
        t.last()?
            .map(|(_, v)| decode(v.value()).context("instantané illisible"))
            .transpose()
    }
}
