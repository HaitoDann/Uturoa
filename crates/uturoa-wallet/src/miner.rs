//! Mineur intégré : frappe RandomX sur plusieurs cœurs.
//!
//! Un fil rafraîchit le travail (`/mint/work`) toutes les 2 s ; chaque fil de
//! calcul a sa propre machine RandomX et essaie des nonces distincts ; les
//! parts trouvées sont envoyées par lots.

use crate::node::{Node, hash_from_hex};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use uturoa_core::{Address, Hash, MintShare};
use uturoa_pow::RandomXHasher;
use uturoa_state::{PowHasher, meets_target};

#[derive(Clone, Debug)]
struct Work {
    chain_id: String,
    anchor_height: u64,
    anchor_hash: Hash,
    seed: Hash,
    difficulty: u64,
}

#[derive(Default)]
struct Counters {
    hashes: AtomicU64,
    found: AtomicU64,
    accepted: AtomicU64,
}

/// État affiché par la page.
#[derive(Clone, Debug, Serialize)]
pub struct MinerStatus {
    pub running: bool,
    pub threads: usize,
    pub beneficiary: Option<String>,
    pub hashes_per_second: f64,
    pub found: u64,
    pub accepted: u64,
    pub difficulty: Option<u64>,
    pub last_error: Option<String>,
}

pub struct Miner {
    stop: Arc<AtomicBool>,
    counters: Arc<Counters>,
    work: Arc<Mutex<Option<Work>>>,
    last_error: Arc<Mutex<Option<String>>>,
    handles: Vec<JoinHandle<()>>,
    threads: usize,
    beneficiary: Option<Address>,
    started: Option<Instant>,
}

impl Default for Miner {
    fn default() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            counters: Arc::new(Counters::default()),
            work: Arc::new(Mutex::new(None)),
            last_error: Arc::new(Mutex::new(None)),
            handles: Vec::new(),
            threads: 0,
            beneficiary: None,
            started: None,
        }
    }
}

fn parse_work(v: &serde_json::Value) -> Option<Work> {
    Some(Work {
        chain_id: v.get("chain_id")?.as_str()?.to_owned(),
        anchor_height: v.get("anchor_height")?.as_u64()?,
        anchor_hash: hash_from_hex(v.get("anchor_hash")?.as_str()?).ok()?,
        seed: hash_from_hex(v.get("seed")?.as_str()?).ok()?,
        difficulty: v.get("difficulty")?.as_u64()?,
    })
}

impl Miner {
    pub fn is_running(&self) -> bool {
        !self.stop.load(Ordering::Relaxed)
    }

    pub fn start(&mut self, node: Node, beneficiary: Address, threads: usize) {
        self.stop_and_wait();
        let threads = threads.clamp(1, 64);
        self.stop = Arc::new(AtomicBool::new(false));
        self.counters = Arc::new(Counters::default());
        self.work = Arc::new(Mutex::new(None));
        self.threads = threads;
        self.beneficiary = Some(beneficiary);
        self.started = Some(Instant::now());
        let found: Arc<Mutex<Vec<MintShare>>> = Arc::new(Mutex::new(Vec::new()));

        // Travail et envoi des parts.
        {
            let (stop, work, found, counters, err, node) = (
                self.stop.clone(),
                self.work.clone(),
                found.clone(),
                self.counters.clone(),
                self.last_error.clone(),
                node,
            );
            self.handles.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match node.mint_work() {
                        Ok(v) => {
                            if let (Some(w), Ok(mut slot)) = (parse_work(&v), work.lock()) {
                                *slot = Some(w);
                            }
                            if let Ok(mut e) = err.lock() {
                                *e = None;
                            }
                        }
                        Err(e) => {
                            if let Ok(mut slot) = err.lock() {
                                *slot = Some(format!("{e:#}"));
                            }
                        }
                    }
                    let batch: Vec<MintShare> = found
                        .lock()
                        .map(|mut f| std::mem::take(&mut *f))
                        .unwrap_or_default();
                    if !batch.is_empty() {
                        if let Ok(n) = node.submit_shares(&batch) {
                            counters.accepted.fetch_add(n, Ordering::Relaxed);
                        }
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            }));
        }

        // Fils de calcul.
        for t in 0..threads {
            let (stop, work, found, counters) = (
                self.stop.clone(),
                self.work.clone(),
                found.clone(),
                self.counters.clone(),
            );
            let start_nonce = u64::try_from(t).unwrap_or(0) << 48;
            self.handles.push(std::thread::spawn(move || {
                let pow = RandomXHasher::new();
                let mut nonce = start_nonce;
                while !stop.load(Ordering::Relaxed) {
                    let Some(w) = work.lock().ok().and_then(|g| g.clone()) else {
                        std::thread::sleep(Duration::from_millis(200));
                        continue;
                    };
                    let share = MintShare {
                        chain_id: w.chain_id.clone(),
                        beneficiary,
                        anchor_height: w.anchor_height,
                        anchor_hash: w.anchor_hash,
                        nonce,
                    };
                    nonce = nonce.wrapping_add(1);
                    let h = pow.hash(&w.seed, &share.pow_input());
                    counters.hashes.fetch_add(1, Ordering::Relaxed);
                    if meets_target(&h, w.difficulty) {
                        counters.found.fetch_add(1, Ordering::Relaxed);
                        if let Ok(mut f) = found.lock() {
                            // Pas plus de 100 parts par envoi (un bloc n'en prend pas plus).
                            if f.len() < 100 {
                                f.push(share);
                            }
                        }
                    }
                }
            }));
        }
    }

    pub fn stop_and_wait(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
        self.started = None;
    }

    /// Arrête sans attendre la fin des calculs en cours.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.handles.clear();
        self.started = None;
    }

    pub fn status(&self) -> MinerStatus {
        let elapsed = self.started.map_or(0.0, |s| s.elapsed().as_secs_f64());
        let hashes = self.counters.hashes.load(Ordering::Relaxed);
        #[allow(clippy::cast_precision_loss)]
        let rate = if elapsed > 0.0 {
            hashes as f64 / elapsed
        } else {
            0.0
        };
        MinerStatus {
            running: self.is_running(),
            threads: self.threads,
            beneficiary: self.beneficiary.map(|a| a.to_string()),
            hashes_per_second: rate,
            found: self.counters.found.load(Ordering::Relaxed),
            accepted: self.counters.accepted.load(Ordering::Relaxed),
            difficulty: self
                .work
                .lock()
                .ok()
                .and_then(|w| w.as_ref().map(|w| w.difficulty)),
            last_error: self.last_error.lock().ok().and_then(|e| e.clone()),
        }
    }
}
