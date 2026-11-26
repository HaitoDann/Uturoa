#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::expect_used
)]

//! Preuve d'enjeu : présence, bascule, récompenses, commission, mise à
//! l'écart, restitution, double signature, signal de mise à jour.

use std::collections::{BTreeMap, BTreeSet};
use uturoa_consensus::{Evidence, SignedVote};
use uturoa_core::{
    Address, CommitCertificate, Hash, Params, PublicKey, SignedTransaction, Transaction, TxBody,
    UNITS_PER_COIN, ValidatorInfo, Vote, VoteKind, encode,
};
use uturoa_crypto::{Domain, KeyPair, hash_tagged_parts};
use uturoa_state::staking::{
    ABSENCE_JAIL_BLOCKS, EPOCH_BLOCKS, JAIL_MIN_BLOCKS, MIN_VALIDATOR_STAKE, UNBONDING_BLOCKS,
};
use uturoa_state::{BlockError, BlockInput, PowHasher, State, TxError, ValidatorStatus};

const CHAIN: &str = "utu-test";

struct NoPow;
impl PowHasher for NoPow {
    fn hash(&self, _: &Hash, _: &[u8]) -> Hash {
        [0xFF; 32]
    }
}

fn info(name: &str) -> ValidatorInfo {
    ValidatorInfo {
        name: name.into(),
        site: "Tahaa".into(),
        contact: String::new(),
    }
}

/// Un réseau d'essai : l'état, les clés connues et les absents.
struct Net {
    s: State,
    keys: BTreeMap<PublicKey, KeyPair>,
    nonces: BTreeMap<Address, u64>,
    known: BTreeSet<Address>,
    absent: BTreeSet<PublicKey>,
    parent: Hash,
    /// Fait signer un autre bloc que le parent (certificat incohérent).
    commit_hash_override: Option<Hash>,
}

impl Net {
    fn new(bootstrap: usize, phase_end: u64) -> (Self, Vec<KeyPair>) {
        let kps: Vec<KeyPair> = (0..bootstrap).map(|_| KeyPair::generate()).collect();
        let boot = kps
            .iter()
            .enumerate()
            .map(|(i, k)| {
                (
                    Address::from_public_key(&k.public_key()),
                    k.public_key(),
                    info(&format!("amorce-{i}")),
                )
            })
            .collect();
        let mut s = State::genesis_with_validators(CHAIN, Params::default(), 1, boot);
        s.set_phase_end_for_tests(phase_end);
        let mut net = Net {
            s,
            keys: BTreeMap::new(),
            nonces: BTreeMap::new(),
            known: BTreeSet::new(),
            absent: BTreeSet::new(),
            parent: [0; 32],
            commit_hash_override: None,
        };
        for k in &kps {
            net.keys.insert(k.public_key(), k.clone());
        }
        (net, kps)
    }

    fn addr(k: &KeyPair) -> Address {
        Address::from_public_key(&k.public_key())
    }

    fn tx(&mut self, k: &KeyPair, body: TxBody) -> SignedTransaction {
        let addr = Self::addr(k);
        let first =
            self.s.account(&addr).is_none_or(|a| a.key.is_none()) && self.known.insert(addr);
        let n = self.nonces.entry(addr).or_insert(0);
        *n += 1;
        let mut tx = Transaction {
            chain_id: CHAIN.into(),
            sender: addr,
            nonce: *n,
            fee: 0,
            body,
        };
        tx.fee = self.s.required_fee(&tx.clone().sign(k, first)) * 2;
        tx.sign(k, first)
    }

    /// Applique le bloc suivant, avec un certificat signé par les présents.
    fn step(&mut self, txs: &[SignedTransaction]) -> Result<(), BlockError> {
        let h = self.s.height + 1;
        let commit = (h >= 2).then(|| {
            let set = self.s.staking().set_at(h - 1);
            let signed = self.commit_hash_override.unwrap_or(self.parent);
            let vote = Vote {
                chain_id: CHAIN.into(),
                kind: VoteKind::Precommit,
                height: h - 1,
                round: 0,
                block_hash: Some(signed),
            };
            let signatures = set
                .iter()
                .enumerate()
                .filter(|(_, k)| !self.absent.contains(*k))
                .filter_map(|(i, k)| self.keys.get(k).map(|kp| (i as u16, vote.sign(kp))))
                .collect();
            CommitCertificate {
                height: h - 1,
                round: 0,
                block_hash: signed,
                signatures,
            }
        });
        let proposer = Address::from_bytes([0xEE; 20]);
        self.s.apply_block(
            &BlockInput {
                height: h,
                parent_hash: self.parent,
                txs,
                shares: &[],
                proposer: &proposer,
                last_commit: commit.as_ref(),
                block_bytes: 0,
            },
            &NoPow,
        )?;
        self.parent = hash_tagged_parts(Domain::Block, &[&h.to_le_bytes()]);
        Ok(())
    }

    fn run_to(&mut self, height: u64) {
        while self.s.height < height {
            self.step(&[]).unwrap();
        }
    }

    /// Inscrit un validateur doté de `coins` pièces et lui fait déléguer `stake`.
    fn join(&mut self, name: &str, coins: u64, stake: u64) -> KeyPair {
        let k = KeyPair::generate();
        self.keys.insert(k.public_key(), k.clone());
        self.s
            .credit_for_tests(&Self::addr(&k), coins * UNITS_PER_COIN);
        let reg = self.tx(
            &k,
            TxBody::RegisterValidator {
                key: k.public_key(),
                info: info(name),
                commission_bps: 500,
                max_commission_bps: 2_000,
            },
        );
        let del = self.tx(
            &k,
            TxBody::Delegate {
                validator: k.public_key(),
                amount: stake * UNITS_PER_COIN,
            },
        );
        self.step(&[reg, del]).unwrap();
        k
    }
}

#[test]
fn presence_is_counted_only_when_expected() {
    let (mut net, boot) = Net::new(4, 1_000);
    let newcomer = net.join("nouveau", 20_000, 10_000);
    net.absent.insert(boot[3].public_key());
    net.run_to(20);
    let st = net.s.staking();
    // Le nouveau n'est pas dans l'ensemble pendant la phase 1 : jamais attendu.
    let v = st.validator(&Net::addr(&newcomer)).unwrap();
    assert_eq!(v.epoch_expected, 0);
    assert_eq!(v.missed_streak, 0);
    // Les validateurs d'amorçage sont attendus à chaque bloc (sauf le premier).
    let b0 = st.validator(&Net::addr(&boot[0])).unwrap();
    assert_eq!(b0.epoch_expected, 19);
    assert_eq!(b0.epoch_signed, 19);
    let b3 = st.validator(&Net::addr(&boot[3])).unwrap();
    assert_eq!(b3.epoch_signed, 0);
    // Absent, mais pas mis à l'écart pendant la phase 1.
    assert_eq!(b3.status, ValidatorStatus::Candidate);
}

#[test]
fn commit_must_reach_quorum_and_match_parent() {
    let (mut net, boot) = Net::new(4, 1_000);
    net.run_to(3);
    // 2 absents sur 4 : pas de quorum (3 requis).
    net.absent.insert(boot[0].public_key());
    net.absent.insert(boot[1].public_key());
    let r = net.step(&[]);
    assert!(matches!(r, Err(BlockError::BadCommit(_))), "{r:?}");
    net.absent.clear();
    // Certificat d'un autre bloc que le parent.
    net.commit_hash_override = Some([1; 32]);
    assert!(matches!(net.step(&[]), Err(BlockError::BadCommit(_))));
    net.commit_hash_override = None;
    net.step(&[]).unwrap();
}

#[test]
fn switch_to_proof_of_stake_at_phase_end() {
    let phase_end = 30;
    let (mut net, boot) = Net::new(4, phase_end);
    let stakers: Vec<KeyPair> = (0..5)
        .map(|i| net.join(&format!("v{i}"), 20_000 + i, 10_000 + i))
        .collect();
    // Un candidat sous le stake minimal n'est jamais sélectionné.
    let small = net.join("petit", 20_000, 9_999);
    net.run_to(phase_end - 1);
    let boot_keys: BTreeSet<PublicKey> = boot.iter().map(KeyPair::public_key).collect();
    assert_eq!(
        net.s.next_validators().into_iter().collect::<BTreeSet<_>>(),
        boot_keys
    );
    net.step(&[]).unwrap();
    let pos: BTreeSet<PublicKey> = net.s.staking().set_at(phase_end + 1).into_iter().collect();
    let expected: BTreeSet<PublicKey> = stakers.iter().map(KeyPair::public_key).collect();
    assert_eq!(pos, expected);
    assert!(!pos.contains(&small.public_key()));
    // Les blocs suivants sont signés par le nouvel ensemble.
    net.run_to(phase_end + 10);
}

#[test]
fn too_few_stakers_keeps_the_bootstrap_set() {
    let phase_end = 20;
    let (mut net, boot) = Net::new(4, phase_end);
    for i in 0..3 {
        net.join(&format!("v{i}"), 20_000, 10_000);
    }
    net.run_to(phase_end + 5);
    let set: BTreeSet<PublicKey> = net.s.next_validators().into_iter().collect();
    assert_eq!(set, boot.iter().map(KeyPair::public_key).collect());
}

#[test]
fn forge_is_shared_at_epoch_end_with_commission_and_availability() {
    let phase_end = 10;
    let (mut net, _) = Net::new(4, phase_end);
    let vals: Vec<KeyPair> = (0..5)
        .map(|i| net.join(&format!("v{i}"), 20_000, 10_000))
        .collect();
    // Un délégant confie 10 000 pièces à v0.
    let alice = KeyPair::generate();
    net.s
        .credit_for_tests(&Net::addr(&alice), 20_000 * UNITS_PER_COIN);
    let d = net.tx(
        &alice,
        TxBody::Delegate {
            validator: vals[0].public_key(),
            amount: 10_000 * UNITS_PER_COIN,
        },
    );
    net.step(&[d]).unwrap();
    // v4 est absent toute l'époque : moins de 97 % de présence.
    net.absent.insert(vals[4].public_key());
    // Deux validateurs signalent la version 2 : pas assez (plus des 2/3 requis).
    for v in &vals[..2] {
        let sig = net.tx(v, TxBody::SignalUpgrade { version: 2 });
        net.step(&[sig]).unwrap();
    }
    let before: Vec<u64> = vals.iter().map(|v| net.s.balance(&Net::addr(v))).collect();
    let alice_before = net.s.balance(&Net::addr(&alice));
    let supply_before_epoch = net.s.supply();
    net.run_to(EPOCH_BLOCKS);

    let gain = |i: usize, n: &Net| n.s.balance(&Net::addr(&vals[i])) - before[i];
    let alice_gain = net.s.balance(&Net::addr(&alice)) - alice_before;
    // v0 : stake 20 000 (10 000 propres + 10 000 d'Alice), commission 5 %.
    // v1 : stake 10 000. v0 reçoit donc environ le double de v1 en tout.
    assert!(gain(1, &net) > 0);
    let v0_total = gain(0, &net) + alice_gain;
    let ratio = v0_total as f64 / gain(1, &net) as f64;
    assert!((ratio - 2.0).abs() < 0.001, "{ratio}");
    // Alice touche la moitié du reste après commission de v0.
    let v0_share = v0_total as f64;
    let expected_alice = v0_share * 0.95 / 2.0;
    assert!((alice_gain as f64 - expected_alice).abs() / expected_alice < 0.001);
    // L'absent ne touche rien ; sa part retourne au réservoir.
    assert_eq!(gain(4, &net), 0);
    assert!(net.s.pending_rewards() == 0);
    let _ = supply_before_epoch;
    // v4 a été mis à l'écart après 1 000 absences : 2 sur 4 actifs, pas assez.
    assert!(matches!(
        net.s
            .staking()
            .validator(&Net::addr(&vals[4]))
            .unwrap()
            .status,
        ValidatorStatus::Jailed { .. }
    ));
    assert_eq!(net.s.protocol_version, 1);
}

#[test]
fn upgrade_activates_when_more_than_two_thirds_signal() {
    let (mut net, boot) = Net::new(4, 1_000_000);
    for b in &boot[..3] {
        net.s.credit_for_tests(&Net::addr(b), UNITS_PER_COIN);
        let sig = net.tx(b, TxBody::SignalUpgrade { version: 2 });
        net.step(&[sig]).unwrap();
    }
    net.run_to(EPOCH_BLOCKS);
    assert_eq!(net.s.protocol_version, 2);
}

#[test]
fn commission_rules() {
    let (mut net, _) = Net::new(4, 1_000);
    let v = net.join("v", 20_000, 10_000);
    let upd = |net: &mut Net, bps: u16| {
        let t = net.tx(
            &v,
            TxBody::UpdateValidatorInfo {
                info: info("v"),
                commission_bps: bps,
            },
        );
        let r = net.s.apply_tx(&t, &Address::from_bytes([0xEE; 20]));
        if r.is_err() {
            *net.nonces.get_mut(&Net::addr(&v)).unwrap() -= 1;
        }
        r
    };
    // Même époque que l'inscription : pas de changement.
    assert!(upd(&mut net, 600).is_err());
    net.run_to(EPOCH_BLOCKS);
    assert!(upd(&mut net, 700).is_err()); // +2 points
    assert!(upd(&mut net, 600).is_ok()); // +1 point
    assert!(upd(&mut net, 700).is_err()); // déjà changé aujourd'hui
    assert!(upd(&mut net, 100).is_ok()); // une baisse est toujours permise
    // Au-delà du plafond déclaré (20 %), jamais, même un point à la fois.
    for day in 2..=21u64 {
        net.run_to(day * EPOCH_BLOCKS);
        let current = net
            .s
            .staking()
            .validator(&Net::addr(&v))
            .unwrap()
            .commission_bps;
        let r = upd(&mut net, current + 100);
        assert_eq!(r.is_ok(), current + 100 <= 2_000, "jour {day}");
    }
}

#[test]
fn long_absence_jails_after_the_switch_then_unjail() {
    let phase_end = 10;
    let (mut net, _) = Net::new(4, phase_end);
    let vals: Vec<KeyPair> = (0..5)
        .map(|i| net.join(&format!("v{i}"), 20_000, 10_000))
        .collect();
    net.run_to(phase_end + 2);
    net.absent.insert(vals[0].public_key());
    let start = net.s.height;
    net.run_to(start + ABSENCE_JAIL_BLOCKS + 1);
    let v0 = net.s.staking().validator(&Net::addr(&vals[0])).unwrap();
    assert!(matches!(v0.status, ValidatorStatus::Jailed { .. }));
    let since = match v0.status {
        ValidatorStatus::Jailed { since } => since,
        _ => unreachable!(),
    };
    // Retiré de l'ensemble deux blocs plus tard.
    net.run_to(since + 2);
    assert!(!net.s.next_validators().contains(&vals[0].public_key()));
    // Trop tôt pour revenir.
    let early = net.tx(&vals[0], TxBody::Unjail);
    assert!(
        net.s
            .apply_tx(&early, &Address::from_bytes([0xEE; 20]))
            .is_err()
    );
    *net.nonces.get_mut(&Net::addr(&vals[0])).unwrap() -= 1;
    net.run_to(since + JAIL_MIN_BLOCKS);
    net.absent.clear();
    let back = net.tx(&vals[0], TxBody::Unjail);
    net.step(&[back]).unwrap();
    assert_eq!(
        net.s
            .staking()
            .validator(&Net::addr(&vals[0]))
            .unwrap()
            .status,
        ValidatorStatus::Candidate
    );
}

#[test]
fn never_jail_below_four_active_validators() {
    let phase_end = 10;
    let (mut net, _) = Net::new(4, phase_end);
    let vals: Vec<KeyPair> = (0..4)
        .map(|i| net.join(&format!("v{i}"), 20_000, 10_000))
        .collect();
    net.run_to(phase_end + 2);
    net.absent.insert(vals[0].public_key());
    let start = net.s.height;
    net.run_to(start + ABSENCE_JAIL_BLOCKS + 5);
    assert_eq!(
        net.s
            .staking()
            .validator(&Net::addr(&vals[0]))
            .unwrap()
            .status,
        ValidatorStatus::Candidate
    );
}

#[test]
fn undelegated_stake_is_returned_after_21_days() {
    // Sans validateurs d'amorçage : pas de certificat à fournir, blocs rapides.
    let (mut net, _) = Net::new(0, 1_000_000_000);
    let v = net.join("v", 20_000, 12_000);
    let addr = Net::addr(&v);
    let before = net.s.balance(&addr);
    let und = net.tx(
        &v,
        TxBody::Undelegate {
            validator: v.public_key(),
            amount: 2_000 * UNITS_PER_COIN,
        },
    );
    net.step(&[und]).unwrap();
    let fee_paid = before - net.s.balance(&addr);
    let release = net.s.height + UNBONDING_BLOCKS;
    net.run_to(release - 1);
    assert_eq!(net.s.balance(&addr), before - fee_paid);
    net.step(&[]).unwrap();
    assert_eq!(
        net.s.balance(&addr),
        before - fee_paid + 2_000 * UNITS_PER_COIN
    );
    assert_eq!(
        net.s.staking().validator(&addr).unwrap().self_stake,
        10_000 * UNITS_PER_COIN
    );
}

#[test]
fn double_signing_is_punished_once() {
    let phase_end = 10;
    let (mut net, _) = Net::new(4, phase_end);
    let vals: Vec<KeyPair> = (0..5)
        .map(|i| net.join(&format!("v{i}"), 20_000, 10_000))
        .collect();
    net.run_to(phase_end + 5);
    let h = net.s.height;
    let set = net.s.staking().set_at(h);
    let cheater = &vals[2];
    let idx = set.iter().position(|k| *k == cheater.public_key()).unwrap() as u16;
    let vote = |hash: Hash| Vote {
        chain_id: CHAIN.into(),
        kind: VoteKind::Prevote,
        height: h,
        round: 0,
        block_hash: Some(hash),
    };
    let ev = Evidence::Votes(
        SignedVote::new(vote([1; 32]), idx, cheater),
        SignedVote::new(vote([2; 32]), idx, cheater),
    );
    let reporter = KeyPair::generate();
    net.s
        .credit_for_tests(&Net::addr(&reporter), UNITS_PER_COIN);
    let supply_before = net.s.supply();
    let rep = net.tx(
        &reporter,
        TxBody::ReportEquivocation {
            evidence: encode(&ev),
        },
    );
    let fee = rep.tx.fee;
    let before = net.s.balance(&Net::addr(&reporter));
    net.step(&[rep]).unwrap();

    let v = net.s.staking().validator(&Net::addr(cheater)).unwrap();
    assert_eq!(v.status, ValidatorStatus::Banned);
    assert_eq!(v.self_stake, 0);
    // Pas d'autre délégant ici : seule la caution propre est confisquée.
    let reward = MIN_VALIDATOR_STAKE / 10;
    assert_eq!(net.s.balance(&Net::addr(&reporter)), before - fee + reward);
    // 90 % fondus : l'offre baisse d'environ 9 000 pièces (la forge du bloc
    // en rajoute quelques-unes).
    let drop = supply_before - net.s.supply();
    assert!(
        drop > MIN_VALIDATOR_STAKE - reward - 20 * UNITS_PER_COIN,
        "{drop}"
    );
    assert!(drop <= MIN_VALIDATOR_STAKE - reward + fee);

    // Une seconde fois : refusé.
    let again = net.tx(
        &reporter,
        TxBody::ReportEquivocation {
            evidence: encode(&ev),
        },
    );
    assert!(matches!(
        net.s.apply_tx(&again, &Address::from_bytes([0xEE; 20])),
        Err(TxError::Malformed(_))
    ));
    // Le tricheur sort de l'ensemble deux blocs plus tard.
    net.run_to(h + 3);
    assert!(!net.s.next_validators().contains(&cheater.public_key()));
}

#[test]
fn registration_rules() {
    let (mut net, boot) = Net::new(4, 1_000);
    let k = KeyPair::generate();
    net.s.credit_for_tests(&Net::addr(&k), UNITS_PER_COIN);
    // Clé de consensus déjà prise par un validateur d'amorçage.
    let dup = net.tx(
        &k,
        TxBody::RegisterValidator {
            key: boot[0].public_key(),
            info: info("x"),
            commission_bps: 0,
            max_commission_bps: 1_000,
        },
    );
    assert!(
        net.s
            .apply_tx(&dup, &Address::from_bytes([0xEE; 20]))
            .is_err()
    );
    *net.nonces.get_mut(&Net::addr(&k)).unwrap() -= 1;
    // Commission au-delà du plafond déclaré.
    let high = net.tx(
        &k,
        TxBody::RegisterValidator {
            key: k.public_key(),
            info: info("x"),
            commission_bps: 1_001,
            max_commission_bps: 1_000,
        },
    );
    assert!(
        net.s
            .apply_tx(&high, &Address::from_bytes([0xEE; 20]))
            .is_err()
    );
}

#[test]
fn delegators_of_a_double_signer_lose_one_percent() {
    let phase_end = 10;
    let (mut net, _) = Net::new(4, phase_end);
    let vals: Vec<KeyPair> = (0..5)
        .map(|i| net.join(&format!("v{i}"), 20_000, 10_000))
        .collect();
    let alice = KeyPair::generate();
    net.s
        .credit_for_tests(&Net::addr(&alice), 2_000 * UNITS_PER_COIN);
    let d = net.tx(
        &alice,
        TxBody::Delegate {
            validator: vals[2].public_key(),
            amount: 1_000 * UNITS_PER_COIN,
        },
    );
    net.step(&[d]).unwrap();
    net.run_to(phase_end + 5);
    let h = net.s.height;
    let set = net.s.staking().set_at(h);
    let idx = set.iter().position(|k| *k == vals[2].public_key()).unwrap() as u16;
    let vote = |hash: Hash| Vote {
        chain_id: CHAIN.into(),
        kind: VoteKind::Precommit,
        height: h,
        round: 0,
        block_hash: Some(hash),
    };
    let ev = Evidence::Votes(
        SignedVote::new(vote([1; 32]), idx, &vals[2]),
        SignedVote::new(vote([2; 32]), idx, &vals[2]),
    );
    let reporter = KeyPair::generate();
    net.s
        .credit_for_tests(&Net::addr(&reporter), UNITS_PER_COIN);
    let rep = net.tx(
        &reporter,
        TxBody::ReportEquivocation {
            evidence: encode(&ev),
        },
    );
    let fee = rep.tx.fee;
    net.step(&[rep]).unwrap();
    // La délégation d'Alice (99 % restants) est partie en restitution.
    assert_eq!(
        net.s
            .staking()
            .delegation(&Net::addr(&alice), &Net::addr(&vals[2])),
        0
    );
    // Le rapporteur touche 10 % de la caution et du 1 % d'Alice.
    let slashed = MIN_VALIDATOR_STAKE + 10 * UNITS_PER_COIN;
    assert_eq!(
        net.s.balance(&Net::addr(&reporter)),
        UNITS_PER_COIN - fee + slashed / 10
    );
}
