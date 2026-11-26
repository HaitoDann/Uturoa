//! Nœud Uturoa.
//!
//! - [`genesis_file`] : fichier de genèse en JSON ;
//! - [`store`] : blocs, certificats et instantanés d'état (redb) ;
//! - [`chain`] : construction, vérification et validation définitive des blocs ;
//! - [`mempool`], [`network`], [`node`] : file d'attente, réseau libp2p, consensus ;
//! - [`service`] : démarrage d'un nœud complet.

pub mod chain;
pub mod genesis_file;
pub mod mempool;
pub mod network;
pub mod node;
pub mod service;
pub mod store;
