//! Portefeuille de bureau Uturoa.
//!
//! `uturoa-wallet` sert une page sur `127.0.0.1` et l'ouvre dans le
//! navigateur. Les clés ne quittent jamais ce programme : la page ne voit que
//! des adresses, des montants et des aperçus.

pub mod amount;
pub mod app;
pub mod miner;
pub mod node;
pub mod vault;
