//! Nœud Uturoa.
//!
//! - [`genesis_file`] : fichier de genèse en JSON ;
//! - [`store`] : blocs, certificats et instantanés d'état (redb) ;
//! - [`chain`] : construction, vérification et validation définitive des blocs.

pub mod chain;
pub mod genesis_file;
pub mod store;
