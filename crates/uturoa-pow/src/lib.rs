//! RandomX pour la frappe (SPEC §5).
//!
//! [`RandomXHasher`] implémente [`PowHasher`] en mode léger (256 Mo de
//! mémoire) : c'est ce qu'utilisent les validateurs pour vérifier les parts.
//! Il est partageable entre fils d'exécution.
//! La clé RandomX change tous les `SEED_PERIOD` blocs ; la machine virtuelle
//! est reconstruite à chaque changement de clé (environ une demi-seconde).

use randomx_rs::{RandomXCache, RandomXFlag, RandomXVM};
use std::sync::{Mutex, mpsc};
use uturoa_core::{Hash, MintShare};
use uturoa_state::{PowHasher, meets_target};

/// Empreinte renvoyée si RandomX échoue : elle ne satisfait aucune
/// difficulté supérieure à 1, donc la part est refusée plutôt qu'acceptée à tort.
const FAILURE: Hash = [0xFF; 32];

struct Job {
    seed: Hash,
    input: Vec<u8>,
    reply: mpsc::Sender<Option<Hash>>,
}

/// RandomX en mode léger. La machine virtuelle RandomX ne peut pas changer de
/// fil d'exécution : elle vit dans un fil dédié, qui reçoit les calculs par
/// message. `RandomXHasher` peut donc être partagé entre fils.
pub struct RandomXHasher {
    jobs: Mutex<mpsc::Sender<Job>>,
}

impl Default for RandomXHasher {
    fn default() -> Self {
        Self::new()
    }
}

fn compute(
    flags: RandomXFlag,
    vm: &mut Option<(Hash, RandomXVM)>,
    seed: &Hash,
    input: &[u8],
) -> Option<Hash> {
    if vm.as_ref().is_none_or(|(s, _)| s != seed) {
        let cache = RandomXCache::new(flags, seed).ok()?;
        *vm = Some((*seed, RandomXVM::new(flags, Some(cache), None).ok()?));
    }
    let (_, machine) = vm.as_ref()?;
    machine.calculate_hash(input).ok()?.try_into().ok()
}

impl RandomXHasher {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            let flags = RandomXFlag::get_recommended_flags();
            let mut vm: Option<(Hash, RandomXVM)> = None;
            // Le fil s'arrête quand le dernier émetteur disparaît.
            while let Ok(job) = rx.recv() {
                let out = compute(flags, &mut vm, &job.seed, &job.input);
                let _ = job.reply.send(out);
            }
        });
        Self {
            jobs: Mutex::new(tx),
        }
    }
}

impl PowHasher for RandomXHasher {
    fn hash(&self, seed: &Hash, input: &[u8]) -> Hash {
        let (reply, answer) = mpsc::channel();
        let job = Job {
            seed: *seed,
            input: input.to_vec(),
            reply,
        };
        let sent = self.jobs.lock().ok().is_some_and(|tx| tx.send(job).is_ok());
        if !sent {
            return FAILURE;
        }
        answer.recv().ok().flatten().unwrap_or(FAILURE)
    }
}

/// Mémoïse les empreintes d'un autre calcul de preuve de travail : une part
/// vérifiée à son arrivée ne coûte plus rien quand elle repasse dans un bloc.
pub struct CachedPow<P> {
    inner: P,
    cache: Mutex<std::collections::HashMap<[u8; 32], Hash>>,
    capacity: usize,
}

impl<P: PowHasher> CachedPow<P> {
    pub fn new(inner: P, capacity: usize) -> Self {
        Self {
            inner,
            cache: Mutex::new(std::collections::HashMap::new()),
            capacity,
        }
    }
}

impl<P: PowHasher> PowHasher for CachedPow<P> {
    fn hash(&self, seed: &Hash, input: &[u8]) -> Hash {
        let mut key_input = Vec::with_capacity(input.len().saturating_add(32));
        key_input.extend_from_slice(seed);
        key_input.extend_from_slice(input);
        let key = *blake3::hash(&key_input).as_bytes();
        if let Some(h) = self.cache.lock().ok().and_then(|c| c.get(&key).copied()) {
            return h;
        }
        let h = self.inner.hash(seed, input);
        if let Ok(mut c) = self.cache.lock() {
            if c.len() >= self.capacity {
                c.clear();
            }
            c.insert(key, h);
        }
        h
    }
}

/// Cherche, en au plus `max_tries` essais, un nonce qui rend la part valide
/// pour `difficulty`, en partant du nonce de `template`.
pub fn mine(
    hasher: &dyn PowHasher,
    seed: &Hash,
    template: &MintShare,
    difficulty: u64,
    max_tries: u64,
) -> Option<MintShare> {
    let mut share = template.clone();
    for _ in 0..max_tries {
        if meets_target(&hasher.hash(seed, &share.pow_input()), difficulty) {
            return Some(share);
        }
        share.nonce = share.nonce.wrapping_add(1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use uturoa_core::Address;

    fn template() -> MintShare {
        MintShare {
            chain_id: "utu-test".into(),
            beneficiary: Address::from_bytes([1; 20]),
            anchor_height: 1,
            anchor_hash: [2; 32],
            nonce: 0,
        }
    }

    #[test]
    fn randomx_is_deterministic_and_seed_dependent() {
        let h = RandomXHasher::new();
        let input = template().pow_input();
        let a = h.hash(&[1; 32], &input);
        assert_ne!(a, FAILURE);
        assert_eq!(h.hash(&[1; 32], &input), a);
        assert_ne!(h.hash(&[2; 32], &input), a);
        // Retour à la première clé : même résultat.
        assert_eq!(h.hash(&[1; 32], &input), a);
        // Une seconde instance donne la même empreinte.
        assert_eq!(RandomXHasher::new().hash(&[1; 32], &input), a);
    }

    #[test]
    fn cache_returns_the_same_hashes() {
        let cached = CachedPow::new(RandomXHasher::new(), 10);
        let input = template().pow_input();
        let direct = RandomXHasher::new().hash(&[3; 32], &input);
        assert_eq!(cached.hash(&[3; 32], &input), direct);
        assert_eq!(cached.hash(&[3; 32], &input), direct);
        assert_ne!(cached.hash(&[4; 32], &input), direct);
    }

    #[test]
    fn mining_finds_a_valid_share() {
        let h = RandomXHasher::new();
        let seed = [7; 32];
        let share = mine(&h, &seed, &template(), 4, 400).unwrap();
        assert!(meets_target(&h.hash(&seed, &share.pow_input()), 4));
        assert!(mine(&h, &seed, &template(), u64::MAX, 3).is_none());
    }
}
