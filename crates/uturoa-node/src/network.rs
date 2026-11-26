//! Réseau P2P : libp2p, ouvert à tous, avec quotas par pair.
//!
//! - Transport TCP chiffré et authentifié (Noise), multiplexé (Yamux).
//! - Réseau **ouvert** : n'importe qui peut lancer un nœud ou miner. Un pair
//!   qui inonde le réseau ou envoie des messages illisibles est exclu un temps.
//! - Gossipsub relaie propositions, votes, transactions, parts de frappe et
//!   preuves d'équivoque.
//! - Un protocole requête-réponse sert les blocs validés (rattrapage) et
//!   l'instantané d'état.

use crate::node::{SharedRef, lock};
use anyhow::{Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{
    Multiaddr, PeerId, StreamProtocol, Swarm, SwarmBuilder, gossipsub, identity, noise, tcp, yamux,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uturoa_consensus::{Evidence, SignedProposal, SignedVote};
use uturoa_core::{Block, CommitCertificate, MintShare, SignedTransaction, encode};

/// Messages relayés par gossip.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
// Les messages sont déplacés, rarement copiés : la taille importe peu.
#[allow(clippy::large_enum_variant)]
pub enum NetMessage {
    Proposal(SignedProposal) = 0x01,
    Vote(SignedVote) = 0x02,
    Txs(Vec<SignedTransaction>) = 0x03,
    Shares(Vec<MintShare>) = 0x04,
    Evidence(Box<Evidence>) = 0x05,
}

/// Enveloppe gossip : le numéro de séquence rend chaque envoi unique, pour
/// qu'un message retransmis ne soit pas écarté comme doublon.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
struct Envelope {
    seq: u64,
    msg: NetMessage,
}

/// Requêtes du protocole `/uturoa/sync/1`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum SyncRequest {
    /// Blocs validés à partir de cette hauteur.
    Blocks { from_height: u64 } = 0x01,
    /// Dernier instantané d'état servi par le pair.
    Snapshot = 0x02,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum SyncResponse {
    Blocks(Vec<(Block, CommitCertificate)>) = 0x01,
    /// Bloc, son certificat, et l'état après ce bloc (encodé).
    Snapshot(Option<Box<(Block, CommitCertificate, Vec<u8>)>>) = 0x02,
}

/// Blocs servis par réponse de rattrapage, au plus.
pub const SYNC_BATCH: u64 = 50;
/// Taille maximale d'un message réseau.
const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
/// Taille visée d'une réponse de rattrapage.
const SYNC_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// Transactions et parts reçues localement : diffusées par lots.
const BATCH_FLUSH: Duration = Duration::from_millis(200);
const MAX_BATCH: usize = 1_000;
/// Messages gossip par seconde et par pair, au plus.
const MAX_MESSAGES_PER_SEC: u32 = 1_000;
/// Poids d'un message illisible dans le quota.
const INVALID_WEIGHT: u32 = 100;
/// Requêtes de rattrapage par seconde et par pair, au plus.
const MAX_SYNC_PER_SEC: u32 = 4;
/// Durée d'exclusion d'un pair qui dépasse ses quotas.
const BAN: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Usage {
    window: Option<Instant>,
    messages: u32,
    sync: u32,
    banned_until: Option<Instant>,
}

#[derive(Default)]
struct Quotas {
    peers: BTreeMap<PeerId, Usage>,
}

impl Quotas {
    fn usage(&mut self, peer: PeerId, now: Instant) -> &mut Usage {
        let u = self.peers.entry(peer).or_default();
        if u.window
            .is_none_or(|w| now.duration_since(w) >= Duration::from_secs(1))
        {
            u.window = Some(now);
            u.messages = 0;
            u.sync = 0;
        }
        u
    }

    fn is_banned(&self, peer: &PeerId, now: Instant) -> bool {
        self.peers
            .get(peer)
            .and_then(|u| u.banned_until)
            .is_some_and(|t| now < t)
    }

    fn charge_message(&mut self, peer: PeerId, valid: bool, now: Instant) -> bool {
        let u = self.usage(peer, now);
        u.messages = u
            .messages
            .saturating_add(if valid { 1 } else { INVALID_WEIGHT });
        if u.messages > MAX_MESSAGES_PER_SEC {
            u.banned_until = now.checked_add(BAN);
            return false;
        }
        true
    }

    fn charge_sync(&mut self, peer: PeerId, now: Instant) -> bool {
        let u = self.usage(peer, now);
        u.sync = u.sync.saturating_add(1);
        u.sync <= MAX_SYNC_PER_SEC
    }
}

/// Ce que le réseau transmet au nœud.
#[derive(Debug)]
pub enum NetEvent {
    Message(Box<NetMessage>),
    Sync(PeerId, Box<SyncResponse>),
}

#[derive(Clone, Copy, Default)]
pub struct SyncCodec;

async fn read_borsh<T: BorshDeserialize, R: futures::AsyncRead + Unpin + Send>(
    io: &mut R,
) -> io::Result<T> {
    let mut bytes = Vec::new();
    io.take(u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .await?;
    borsh::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

async fn write_borsh<T: BorshSerialize, W: futures::AsyncWrite + Unpin + Send>(
    io: &mut W,
    value: &T,
) -> io::Result<()> {
    io.write_all(&encode(value)).await?;
    io.close().await
}

#[async_trait::async_trait]
impl request_response::Codec for SyncCodec {
    type Protocol = StreamProtocol;
    type Request = SyncRequest;
    type Response = SyncResponse;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<SyncRequest>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        read_borsh(io).await
    }

    async fn read_response<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<SyncResponse>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        read_borsh(io).await
    }

    async fn write_request<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        req: SyncRequest,
    ) -> io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        write_borsh(io, &req).await
    }

    async fn write_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        res: SyncResponse,
    ) -> io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        write_borsh(io, &res).await
    }
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    gossip: gossipsub::Behaviour,
    sync: request_response::Behaviour<SyncCodec>,
}

pub struct NetConfig {
    pub listen: Multiaddr,
    /// Pairs à joindre au démarrage (`/ip4/…/tcp/…/p2p/<id>`).
    pub peers: Vec<Multiaddr>,
}

/// Clé d'identité réseau, distincte des clés de consensus. Créée au premier
/// démarrage dans `network.key`.
pub fn load_or_create_identity(data_dir: &Path) -> Result<identity::Keypair> {
    let path = data_dir.join("network.key");
    if path.exists() {
        let mut bytes =
            std::fs::read(&path).with_context(|| format!("lecture de {}", path.display()))?;
        return identity::Keypair::ed25519_from_bytes(&mut bytes).context("clé réseau invalide");
    }
    let keypair = identity::Keypair::generate_ed25519();
    let secret = keypair
        .clone()
        .try_into_ed25519()
        .context("clé réseau")?
        .secret();
    std::fs::create_dir_all(data_dir)?;
    uturoa_keystore::keyfile::write_private(&path, secret.as_ref())?;
    Ok(keypair)
}

/// Identifiant de pair d'une adresse `/…/p2p/<id>`.
pub fn peer_of(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|p| match p {
        libp2p::multiaddr::Protocol::P2p(id) => Some(id),
        _ => None,
    })
}

fn topic(chain_id: &str) -> gossipsub::IdentTopic {
    gossipsub::IdentTopic::new(format!("uturoa/{chain_id}/1"))
}

/// Fait tourner le réseau : transmet au nœud les messages reçus, diffuse
/// ceux du nœud, sert et demande les blocs manquants.
pub async fn run(
    identity: identity::Keypair,
    config: NetConfig,
    shared: SharedRef,
    mut outbound: mpsc::UnboundedReceiver<NetMessage>,
    mut requests: mpsc::UnboundedReceiver<(PeerId, SyncRequest)>,
    inbound: mpsc::UnboundedSender<NetEvent>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let chain_id = lock(&shared).chain.state().chain_id.clone();
    let topic = topic(&chain_id);
    let mut swarm: Swarm<Behaviour> = SwarmBuilder::with_existing_identity(identity)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            let gossip_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_millis(500))
                .validation_mode(gossipsub::ValidationMode::Strict)
                .message_id_fn(|m: &gossipsub::Message| {
                    gossipsub::MessageId::from(blake3::hash(&m.data).as_bytes().to_vec())
                })
                .max_transmit_size(MAX_MESSAGE_BYTES)
                .validate_messages()
                .build()
                .map_err(|e| io::Error::other(e.to_string()))?;
            let gossip = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossip_config,
            )
            .map_err(|e| io::Error::other(e.to_string()))?;
            let sync = request_response::Behaviour::with_codec(
                SyncCodec,
                [(StreamProtocol::new("/uturoa/sync/1"), ProtocolSupport::Full)],
                request_response::Config::default().with_request_timeout(Duration::from_secs(20)),
            );
            Ok(Behaviour { gossip, sync })
        })
        .map_err(|e| anyhow::anyhow!("réseau : {e}"))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(3600)))
        .build();

    swarm.behaviour_mut().gossip.subscribe(&topic)?;
    let dial_targets: Vec<Multiaddr> = config.peers.clone();
    swarm.listen_on(config.listen.clone())?;
    info!(peer_id = %swarm.local_peer_id(), listen = %config.listen, "réseau démarré");

    let mut connected: BTreeSet<PeerId> = BTreeSet::new();
    let mut quotas = Quotas::default();
    let mut seq: u64 = 0;
    let mut sync_turn: usize = 0;
    let mut redial = tokio::time::interval(Duration::from_secs(3));
    let mut sync_tick = tokio::time::interval(Duration::from_secs(2));
    let mut flush = tokio::time::interval(BATCH_FLUSH);
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut txs: Vec<SignedTransaction> = Vec::new();
    let mut shares: Vec<MintShare> = Vec::new();
    let mut publish = |swarm: &mut Swarm<Behaviour>, msg: NetMessage| {
        seq = seq.wrapping_add(1);
        let data = encode(&Envelope { seq, msg });
        if let Err(e) = swarm.behaviour_mut().gossip.publish(topic.clone(), data) {
            debug!("diffusion impossible : {e}");
        }
    };
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            Some(msg) = outbound.recv() => match msg {
                NetMessage::Txs(mut batch) => {
                    txs.append(&mut batch);
                    if txs.len() >= MAX_BATCH {
                        publish(&mut swarm, NetMessage::Txs(std::mem::take(&mut txs)));
                    }
                }
                NetMessage::Shares(mut batch) => {
                    shares.append(&mut batch);
                    if shares.len() >= MAX_BATCH {
                        publish(&mut swarm, NetMessage::Shares(std::mem::take(&mut shares)));
                    }
                }
                msg => publish(&mut swarm, msg),
            },
            _ = flush.tick() => {
                if !txs.is_empty() {
                    publish(&mut swarm, NetMessage::Txs(std::mem::take(&mut txs)));
                }
                if !shares.is_empty() {
                    publish(&mut swarm, NetMessage::Shares(std::mem::take(&mut shares)));
                }
            }
            Some((peer, request)) = requests.recv() => {
                swarm.behaviour_mut().sync.send_request(&peer, request);
            }
            _ = redial.tick() => {
                for addr in &dial_targets {
                    if peer_of(addr).is_none_or(|p| !connected.contains(&p)) {
                        let _ = swarm.dial(addr.clone());
                    }
                }
            }
            _ = sync_tick.tick() => {
                // Demande périodique des blocs suivants, à un pair différent à
                // chaque fois : un pair aussi en retard que nous ne bloque rien.
                let next = lock(&shared).chain.height().saturating_add(1);
                let peers: Vec<PeerId> = connected.iter().copied().collect();
                if !peers.is_empty() {
                    sync_turn = sync_turn.wrapping_add(1);
                    let pick = sync_turn.checked_rem(peers.len()).unwrap_or(0);
                    if let Some(peer) = peers.get(pick) {
                        swarm.behaviour_mut().sync.send_request(peer, SyncRequest::Blocks { from_height: next });
                    }
                }
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    if quotas.is_banned(&peer_id, Instant::now()) {
                        let _ = swarm.disconnect_peer_id(peer_id);
                    } else {
                        info!(%peer_id, "pair connecté");
                        connected.insert(peer_id);
                        lock(&shared).peers = connected.len();
                    }
                }
                SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
                    info!(%peer_id, "pair déconnecté");
                    connected.remove(&peer_id);
                    lock(&shared).peers = connected.len();
                }
                SwarmEvent::Behaviour(BehaviourEvent::Gossip(gossipsub::Event::Message {
                    propagation_source,
                    message_id,
                    message,
                })) => {
                    let decoded = borsh::from_slice::<Envelope>(&message.data).ok().map(|e| e.msg);
                    let within = quotas.charge_message(propagation_source, decoded.is_some(), Instant::now());
                    let verdict = match (decoded, within) {
                        (Some(msg), true) => {
                            let _ = inbound.send(NetEvent::Message(Box::new(msg)));
                            gossipsub::MessageAcceptance::Accept
                        }
                        (None, true) => gossipsub::MessageAcceptance::Reject,
                        (_, false) => {
                            warn!(peer_id = %propagation_source, "quota dépassé : pair exclu {} s", BAN.as_secs());
                            let _ = swarm.disconnect_peer_id(propagation_source);
                            gossipsub::MessageAcceptance::Ignore
                        }
                    };
                    let _ = swarm
                        .behaviour_mut()
                        .gossip
                        .report_message_validation_result(&message_id, &propagation_source, verdict);
                }
                SwarmEvent::Behaviour(BehaviourEvent::Sync(request_response::Event::Message { peer, message, .. })) => {
                    match message {
                        request_response::Message::Request { request, channel, .. } => {
                            if quotas.charge_sync(peer, Instant::now()) {
                                let response = serve(&shared, request);
                                let _ = swarm.behaviour_mut().sync.send_response(channel, response);
                            }
                        }
                        request_response::Message::Response { response, .. } => {
                            let _ = inbound.send(NetEvent::Sync(peer, Box::new(response)));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn serve(shared: &SharedRef, request: SyncRequest) -> SyncResponse {
    let s = lock(shared);
    match request {
        SyncRequest::Blocks { from_height } => {
            let mut out = Vec::new();
            let mut h = from_height;
            let mut bytes = 0usize;
            while u64::try_from(out.len()).unwrap_or(u64::MAX) < SYNC_BATCH && h <= s.chain.height()
            {
                match (s.chain.store().block(h), s.chain.store().commit(h)) {
                    (Ok(Some(b)), Ok(Some(c))) => {
                        bytes = bytes.saturating_add(b.encoded_len());
                        if !out.is_empty() && bytes > SYNC_RESPONSE_BYTES {
                            break;
                        }
                        out.push((b, c));
                    }
                    _ => break,
                }
                h = h.saturating_add(1);
            }
            SyncResponse::Blocks(out)
        }
        SyncRequest::Snapshot => {
            SyncResponse::Snapshot(s.chain.snapshot().ok().flatten().map(Box::new))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flooding_peer_is_banned_then_released() {
        let mut q = Quotas::default();
        let peer = PeerId::random();
        let t0 = Instant::now();
        for _ in 0..MAX_MESSAGES_PER_SEC {
            assert!(q.charge_message(peer, true, t0));
        }
        assert!(!q.charge_message(peer, true, t0));
        assert!(q.is_banned(&peer, t0));
        assert!(!q.is_banned(&peer, t0 + BAN + Duration::from_millis(1)));
        assert!(q.charge_message(PeerId::random(), true, t0));
    }

    #[test]
    fn invalid_messages_exhaust_the_quota_quickly() {
        let mut q = Quotas::default();
        let peer = PeerId::random();
        let t0 = Instant::now();
        let allowed = (0..20)
            .take_while(|_| q.charge_message(peer, false, t0))
            .count();
        assert_eq!(allowed, (MAX_MESSAGES_PER_SEC / INVALID_WEIGHT) as usize);
    }
}
