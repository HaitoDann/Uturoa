//! Boucle principale : branche le moteur de consensus sur la chaîne, le
//! réseau et l'horloge.
//!
//! Sans clé de validateur, ou avec une clé absente de l'ensemble actif, le
//! nœud suit la chaîne en observateur.

use crate::chain::Chain;
use crate::mempool::Mempool;
use crate::network::{NetEvent, NetMessage, SyncRequest, SyncResponse};
use anyhow::Result;
use libp2p::PeerId;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};
use uturoa_consensus::guard::Step;
use uturoa_consensus::{Action, BlockValidator, Engine, Evidence, SignGuard, SignedVote, Timeout};
use uturoa_core::{Address, Block, CommitCertificate, Hash, Transaction, TxBody, VoteKind, encode};
use uturoa_crypto::KeyPair;

pub struct Shared {
    pub chain: Chain,
    pub mempool: Mempool,
    pub guard: SignGuard,
    pub outbound: Option<mpsc::UnboundedSender<NetMessage>>,
    pub requests: Option<mpsc::UnboundedSender<(PeerId, SyncRequest)>>,
    pub peers: usize,
    /// Empreinte de bloc fournie par l'opérateur pour accepter un instantané.
    pub trusted_block: Option<Hash>,
    /// Blocs validés depuis le démarrage, et parmi eux ceux du premier round.
    pub blocks: u64,
    pub blocks_first_round: u64,
    pub started_ms: u64,
}

impl Shared {
    pub fn new(chain: Chain) -> Result<Self> {
        let guard = chain.store().load_guard()?;
        Ok(Self {
            chain,
            mempool: Mempool::default(),
            guard,
            outbound: None,
            requests: None,
            peers: 0,
            trusted_block: None,
            blocks: 0,
            blocks_first_round: 0,
            started_ms: now_ms(),
        })
    }

    pub fn broadcast(&self, msg: NetMessage) {
        if let Some(out) = &self.outbound {
            let _ = out.send(msg);
        }
    }

    pub fn request(&self, peer: PeerId, request: SyncRequest) {
        if let Some(out) = &self.requests {
            let _ = out.send((peer, request));
        }
    }

    /// Valide définitivement un bloc et met à jour le mempool et la garde.
    fn commit(&mut self, block: Block, cert: CommitCertificate) -> Result<()> {
        let round = cert.round;
        let height = block.header.height;
        self.chain.commit_block(block, cert)?;
        let tip = self.chain.tip_hash();
        self.mempool.prune(self.chain.state(), &tip);
        self.guard.prune_below(height.saturating_add(1));
        self.chain.store().save_guard(&self.guard)?;
        self.blocks = self.blocks.saturating_add(1);
        if round == 0 {
            self.blocks_first_round = self.blocks_first_round.saturating_add(1);
        }
        info!(height, round, "bloc validé");
        Ok(())
    }
}

pub type SharedRef = Arc<Mutex<Shared>>;

/// Verrouille l'état partagé, même après la panique d'un autre fil : chaque
/// écriture disque est atomique, les données restent cohérentes.
pub fn lock(shared: &SharedRef) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

struct Checker<'a>(&'a Chain);

impl BlockValidator for Checker<'_> {
    fn is_valid(&mut self, block: &Block) -> bool {
        self.0.verify(block, now_ms()).is_ok()
    }
}

/// Messages de consensus d'une hauteur future gardés en attente, au plus.
const MAX_BUFFERED: usize = 10_000;
/// Nos messages de la hauteur en cours sont retransmis à cet intervalle :
/// un message diffusé avant la connexion des pairs serait sinon perdu.
const REBROADCAST: Duration = Duration::from_secs(1);
const MAX_OWN: usize = 8;

fn message_height(msg: &NetMessage) -> Option<u64> {
    match msg {
        NetMessage::Proposal(p) => Some(p.proposal.height),
        NetMessage::Vote(v) => Some(v.vote.height),
        _ => None,
    }
}

struct Runner {
    shared: SharedRef,
    key: Option<KeyPair>,
    engine: Engine,
    me: Option<u16>,
    queue: VecDeque<Action>,
    future: Vec<NetMessage>,
    own: VecDeque<NetMessage>,
    build: Option<(u32, u64)>,
    start_at: Option<u64>,
    timeouts: mpsc::UnboundedSender<Timeout>,
    snapshot_requested: bool,
}

impl Runner {
    fn new_height(&mut self) {
        let s = lock(&self.shared);
        let height = s.chain.height().saturating_add(1);
        let validators = s.chain.next_validators();
        self.me = self
            .key
            .as_ref()
            .and_then(|k| validators.iter().position(|v| *v == k.public_key()))
            .and_then(|i| u16::try_from(i).ok());
        let params = s.chain.state().params.clone();
        self.engine = Engine::new(
            s.chain.state().chain_id.clone(),
            height,
            validators,
            self.me,
            params.clone(),
        );
        if let Some(last) = s.guard.last_round(height) {
            let lock_state = match s.chain.store().load_lock() {
                Ok(Some((h, round, block))) if h == height => Some((round, block)),
                _ => None,
            };
            info!(
                height,
                round = last.saturating_add(1),
                "reprise du consensus"
            );
            self.engine.restore(lock_state, last.saturating_add(1));
        }
        self.build = None;
        self.own.clear();
        self.queue.clear();
        self.start_at = Some(
            s.chain
                .parent_time_ms()
                .saturating_add(params.min_block_interval_ms),
        );
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.future)
            .into_iter()
            .filter(|m| message_height(m).is_some_and(|h| h >= height))
            .partition(|m| message_height(m) == Some(height));
        self.future = later;
        for msg in now {
            let actions = match msg {
                NetMessage::Proposal(p) => self.engine.on_proposal(p, &mut Checker(&s.chain)),
                NetMessage::Vote(v) => self.engine.on_vote(v, &mut Checker(&s.chain)),
                _ => Vec::new(),
            };
            self.queue.extend(actions);
        }
    }

    fn remember(&mut self, msg: NetMessage) {
        if self.own.len() >= MAX_OWN {
            self.own.pop_front();
        }
        self.own.push_back(msg);
    }

    fn drain(&mut self) -> Result<()> {
        while let Some(action) = self.queue.pop_front() {
            match action {
                Action::BuildBlock { round, .. } => {
                    let s = lock(&self.shared);
                    let at = s
                        .chain
                        .parent_time_ms()
                        .saturating_add(s.chain.state().params.min_block_interval_ms);
                    self.build = Some((round, at));
                }
                Action::Propose(proposal) => {
                    let (Some(key), Some(_)) = (self.key.clone(), self.me) else {
                        continue;
                    };
                    let mut guard = lock(&self.shared);
                    let s = &mut *guard;
                    let content = Some(proposal.block.hash());
                    if !s
                        .guard
                        .allow(proposal.height, proposal.round, Step::Proposal, content)
                    {
                        continue;
                    }
                    s.chain.store().save_guard(&s.guard)?;
                    let signed = proposal.sign(&key);
                    let msg = NetMessage::Proposal(signed.clone());
                    s.broadcast(msg.clone());
                    let actions = self.engine.on_proposal(signed, &mut Checker(&s.chain));
                    drop(guard);
                    self.remember(msg);
                    self.queue.extend(actions);
                }
                Action::Vote(vote) => {
                    let (Some(key), Some(me)) = (self.key.clone(), self.me) else {
                        continue;
                    };
                    let mut guard = lock(&self.shared);
                    let s = &mut *guard;
                    if !s
                        .guard
                        .allow(vote.height, vote.round, vote.kind.into(), vote.block_hash)
                    {
                        continue;
                    }
                    s.chain.store().save_guard(&s.guard)?;
                    if vote.kind == VoteKind::Precommit {
                        if let Some(block) = vote.block_hash.and_then(|h| self.engine.block(&h)) {
                            s.chain.store().save_lock(vote.height, vote.round, block)?;
                        }
                    }
                    let signed = SignedVote::new(vote, me, &key);
                    let msg = NetMessage::Vote(signed.clone());
                    s.broadcast(msg.clone());
                    let actions = self.engine.on_vote(signed, &mut Checker(&s.chain));
                    drop(guard);
                    self.remember(msg);
                    self.queue.extend(actions);
                }
                Action::Equivocation(evidence) => {
                    self.report(&evidence);
                }
                Action::ScheduleTimeout { timeout, after_ms } => {
                    let tx = self.timeouts.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(after_ms)).await;
                        let _ = tx.send(timeout);
                    });
                }
                Action::Decide { block, certificate } => {
                    lock(&self.shared).commit(block, certificate)?;
                    self.new_height();
                }
            }
        }
        Ok(())
    }

    /// Double signature détectée : alerte, diffusion de la preuve et, si ce
    /// nœud a une clé, soumission de la sanction (`ReportEquivocation`).
    fn report(&self, evidence: &Evidence) {
        let mut guard = lock(&self.shared);
        let s = &mut *guard;
        error!(
            height = evidence.height(),
            round = evidence.round(),
            step = evidence.step(),
            "ÉQUIVOQUE : un validateur a signé deux messages contradictoires"
        );
        s.broadcast(NetMessage::Evidence(Box::new(evidence.clone())));
        let Some(key) = &self.key else {
            return;
        };
        let state = s.chain.state();
        let sender = Address::from_public_key(&key.public_key());
        let account = state.account(&sender);
        let nonce = account.map_or(0, |a| a.nonce).saturating_add(1);
        let include_key = account.is_none_or(|a| a.key.is_none());
        let mut tx = Transaction {
            chain_id: state.chain_id.clone(),
            sender,
            nonce,
            fee: 0,
            body: TxBody::ReportEquivocation {
                evidence: encode(evidence),
            },
        };
        tx.fee = state
            .required_fee(&tx.clone().sign(key, include_key))
            .saturating_mul(2);
        let signed = tx.sign(key, include_key);
        match s.mempool.add(state, signed.clone()) {
            Ok(()) => {
                s.broadcast(NetMessage::Txs(vec![signed]));
                info!("preuve d'équivoque soumise");
            }
            Err(e) => warn!("preuve d'équivoque non soumise : {e}"),
        }
    }

    fn build_now(&mut self, round: u32) {
        let block = {
            let s = lock(&self.shared);
            let timestamp = now_ms().max(s.chain.parent_time_ms().saturating_add(1));
            let params = &s.chain.state().params;
            let max_txs = usize::try_from(params.max_block_txs).unwrap_or(usize::MAX);
            let max_shares = usize::try_from(params.max_block_shares).unwrap_or(usize::MAX);
            let tip = s.chain.tip_hash();
            let txs = s.mempool.select_txs(s.chain.state(), max_txs);
            let shares = s.mempool.select_shares(s.chain.state(), &tip, max_shares);
            match s.chain.build_block(round, txs, shares, timestamp) {
                Ok(b) => b,
                Err(e) => {
                    // Repli : bloc vide, toujours constructible.
                    warn!("construction du bloc : {e:#} ; bloc vide");
                    match s
                        .chain
                        .build_block(round, Vec::new(), Vec::new(), timestamp)
                    {
                        Ok(b) => b,
                        Err(e) => {
                            error!("bloc vide impossible : {e:#}");
                            return;
                        }
                    }
                }
            }
        };
        self.queue.extend(self.engine.on_block_built(round, block));
    }

    fn on_net(&mut self, event: NetEvent) {
        match event {
            NetEvent::Message(msg) => match *msg {
                NetMessage::Txs(txs) => {
                    let mut guard = lock(&self.shared);
                    let s = &mut *guard;
                    for tx in txs {
                        let _ = s.mempool.add(s.chain.state(), tx);
                    }
                }
                NetMessage::Shares(shares) => {
                    let mut guard = lock(&self.shared);
                    let s = &mut *guard;
                    let tip = s.chain.tip_hash();
                    for share in shares {
                        let _ = s
                            .mempool
                            .add_share(s.chain.state(), &tip, share, s.chain.pow());
                    }
                }
                NetMessage::Evidence(evidence) => {
                    let valid = {
                        let s = lock(&self.shared);
                        let keys = s.chain.state().staking().set_at(evidence.height());
                        evidence.verify(&s.chain.state().chain_id, &keys).is_ok()
                    };
                    if valid {
                        self.report(&evidence);
                    }
                }
                consensus => {
                    let height = message_height(&consensus).unwrap_or(0);
                    if height > self.engine.height() {
                        if self.future.len() < MAX_BUFFERED {
                            self.future.push(consensus);
                        }
                        return;
                    }
                    if height < self.engine.height() {
                        return;
                    }
                    let s = lock(&self.shared);
                    let actions = match consensus {
                        NetMessage::Proposal(p) => {
                            self.engine.on_proposal(p, &mut Checker(&s.chain))
                        }
                        NetMessage::Vote(v) => self.engine.on_vote(v, &mut Checker(&s.chain)),
                        _ => Vec::new(),
                    };
                    drop(s);
                    self.queue.extend(actions);
                }
            },
            NetEvent::Sync(peer, response) => self.on_sync(peer, *response),
        }
    }

    fn on_sync(&mut self, peer: PeerId, response: SyncResponse) {
        match response {
            SyncResponse::Blocks(blocks) => {
                let mut advanced = false;
                let mut stuck = false;
                {
                    let mut guard = lock(&self.shared);
                    for (block, cert) in blocks {
                        if block.header.height <= guard.chain.height() {
                            continue;
                        }
                        if block.header.height != guard.chain.height().saturating_add(1) {
                            stuck = true;
                            break;
                        }
                        match guard.commit(block, cert) {
                            Ok(()) => advanced = true,
                            Err(e) => {
                                warn!("rattrapage interrompu : {e:#}");
                                break;
                            }
                        }
                    }
                }
                if advanced {
                    self.new_height();
                }
                // Le pair n'a plus les blocs qui nous manquent : instantané.
                if stuck && !self.snapshot_requested {
                    self.snapshot_requested = true;
                    lock(&self.shared).request(peer, SyncRequest::Snapshot);
                }
            }
            SyncResponse::Snapshot(Some(snap)) => {
                self.snapshot_requested = false;
                let (block, cert, bytes) = *snap;
                let result = {
                    let mut guard = lock(&self.shared);
                    let trusted = guard.trusted_block;
                    let h = block.header.height;
                    guard
                        .chain
                        .install_snapshot(block, cert, &bytes, trusted)
                        .map(|()| h)
                };
                match result {
                    Ok(h) => {
                        info!(height = h, "état installé depuis un instantané");
                        self.new_height();
                    }
                    Err(e) => warn!(%peer, "instantané refusé : {e:#}"),
                }
            }
            SyncResponse::Snapshot(None) => self.snapshot_requested = false,
        }
    }
}

/// Fait tourner le consensus jusqu'à ce que `shutdown` passe à `true`.
pub async fn run(
    shared: SharedRef,
    key: Option<KeyPair>,
    mut inbound: mpsc::UnboundedReceiver<NetEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let (timeouts_tx, mut timeouts_rx) = mpsc::unbounded_channel::<Timeout>();
    let placeholder = {
        let s = lock(&shared);
        Engine::new(
            s.chain.state().chain_id.clone(),
            0,
            Vec::new(),
            None,
            s.chain.state().params.clone(),
        )
    };
    let mut r = Runner {
        shared,
        key,
        engine: placeholder,
        me: None,
        queue: VecDeque::new(),
        future: Vec::new(),
        own: VecDeque::new(),
        build: None,
        start_at: None,
        timeouts: timeouts_tx,
        snapshot_requested: false,
    };
    r.new_height();
    if r.me.is_none() {
        info!("mode observateur : ce nœud ne vote pas");
    }
    let mut rebroadcast = tokio::time::interval(REBROADCAST);
    loop {
        if r.start_at.is_some_and(|at| at <= now_ms()) {
            r.start_at = None;
            let s = lock(&r.shared);
            let actions = r.engine.start(&mut Checker(&s.chain));
            drop(s);
            r.queue.extend(actions);
        }
        r.drain()?;
        let start_in = r
            .start_at
            .map(|at| Duration::from_millis(at.saturating_sub(now_ms())));
        let build_in = r
            .build
            .map(|(_, at)| Duration::from_millis(at.saturating_sub(now_ms())));
        tokio::select! {
            _ = tokio::time::sleep(start_in.unwrap_or(Duration::MAX)), if start_in.is_some() => {}
            _ = tokio::time::sleep(build_in.unwrap_or(Duration::MAX)), if build_in.is_some() => {
                if let Some((round, _)) = r.build.take() {
                    r.build_now(round);
                }
            }
            Some(timeout) = timeouts_rx.recv() => {
                let s = lock(&r.shared);
                let actions = r.engine.on_timeout(timeout, &mut Checker(&s.chain));
                drop(s);
                r.queue.extend(actions);
            }
            Some(event) = inbound.recv() => r.on_net(event),
            _ = rebroadcast.tick() => {
                let s = lock(&r.shared);
                for msg in &r.own {
                    s.broadcast(msg.clone());
                }
            }
            _ = shutdown.changed() => return Ok(()),
        }
    }
}
