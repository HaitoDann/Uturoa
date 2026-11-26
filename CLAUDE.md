# CLAUDE.md : contexte de travail sur Uturoa

À lire en début de session. Les règles du protocole sont dans `SPEC.md`.

## 1. Règles de travail avec le propriétaire (Haito)

- **Tout en français**, avec un guidage pas à pas et des commandes prêtes à
  copier.
- **Demander confirmation avant de pousser** quoi que ce soit, et avant de
  lancer toute action visible de l'extérieur.
- **Être objectif**, sans complaisance : dire ce qui ne va pas, ce qui est
  risqué, ce qui n'est pas prouvé.
- Ne jamais présenter un résultat non vérifié comme acquis.
- Pour l'instant, on travaille directement sur `main`. Des branches viendront
  plus tard.

## 2. Le projet

Monnaie minimaliste en Rust : une seule fonction, envoyer de la valeur.

- Blocs de 12 s. **Toute l'émission est comptée en blocs** : changer la durée
  d'un bloc exige d'ajuster en même temps `MINT_PHASE_BLOCKS`,
  `MINT_PER_BLOCK` et `FORGE_DIVISOR` (version de protocole).
- Phase 1 (6 mois, 1 314 000 blocs) : preuve d'autorité et frappe RandomX par
  parts (`MintShare`), 150 M de pièces partagées tous les 50 blocs.
- Bascule vers la preuve d'enjeu par simple changement de règle de sélection
  des validateurs.
- Phase 2 : preuve d'enjeu avec délégation. Vote à poids égal entre les 21
  validateurs les mieux classés par stake (10 000 pièces de stake propre au
  minimum) ; commission sous un plafond déclaré à l'inscription (hausse
  d'1 point par jour au plus) ; forge partagée chaque jour ;
  économie « melt & forge » (`forge = réservoir / 89 478 485`, frais
  80 % fondus et 20 % au proposeur).
- Consensus BFT de type **Tendermint**. Ce n'est pas un algorithme maison :
  ne jamais le présenter comme tel.
- Aucune prévente, aucune part fondateur, aucune clé d'administration.

## 3. Arborescence

| Crate | Rôle |
|---|---|
| `uturoa-crypto` | Clés ed25519 typées, adresses `utu1…`, BLAKE3, Merkle, étiquettes de domaine `utu/…` |
| `uturoa-core` | Montants et constantes d'émission, comptes et multisignatures, transactions, parts de frappe, blocs, votes, paramètres |
| `uturoa-consensus` | Moteur Tendermint (machine à états pure), garde anti-double signature, preuves d'équivoque |
| `uturoa-state` | Machine d'état : comptes, réserve, envois, multisignatures, frais dynamiques, melt, frappe (parts, partage, difficulté ; RandomX abstrait par `PowHasher`), forge (partagée par `staking`) |
| `uturoa-state::staking` | Validateurs, délégation, présence (certificat du bloc précédent), mise à l'écart, restitution, double signature, récompenses par époque, signal de mise à jour, bascule |
| `uturoa-pow` | RandomX (crate `randomx-rs`, C++ : il faut CMake et un compilateur C++) : vérification en mode léger et minage |
| `uturoa-node` | Nœud : genèse JSON, stockage redb (blocs, certificats, instantanés, garde et verrou de consensus), chaîne, mempool, réseau libp2p **ouvert** (gossip, rattrapage, instantané), boucle de consensus, soumission automatique des preuves d'équivoque, API HTTP et explorateur (`--api`), client de test ; commandes `devnet`, `run`, `info`, `new-key`, `mine`, `send` |
| `uturoa-wallet` | Portefeuille de bureau : page locale sur 127.0.0.1 (jeton aléatoire), phrase de 12 mots chiffrée, comptes dérivés, envoi avec aperçu, mineur RandomX multicœur, délégation |
| `uturoa-keystore` | Fichiers de clés chiffrés ; mot de passe par `UTUROA_KEY_PASSWORD` ou fichier |

## 4. Commandes

```bash
cargo fmt --all                                         # toujours avant un commit
cargo clippy -q --workspace --all-targets -- -D warnings
cargo test -q --workspace
cargo deny check                                        # failles et licences
cargo test -q --release -p uturoa-state --test mint -- --ignored   # parcourt toute la phase de frappe
cargo test -q --release -p uturoa-node --test bascule -- --ignored # 8 nœuds : frappe, inscription, bascule
cd fuzz && cargo +nightly fuzz run --target x86_64-unknown-linux-gnu net_message -- -max_total_time=60

# Réseau local
export UTUROA_KEY_PASSWORD=pw
uturoa-node devnet --dir net --block-interval-ms 1000   # affiche les 4 commandes run
uturoa-node devnet --dir testnet --chain-id utu-testnet-1 --hosts IP1,IP2,IP3,IP4 --names A,B,C,D --base-port 26656   # services systemd
uturoa-node run --genesis net/genesis.json --key net/validator-1.json --data-dir net/d1 --listen /ip4/127.0.0.1/tcp/30333 --peer …
uturoa-node info --genesis net/genesis.json --data-dir net/d1
# avec --api 127.0.0.1:8545 sur un nœud : explorateur sur http://127.0.0.1:8545/
uturoa-node new-key --out alice.json
uturoa-node mine --api 127.0.0.1:8545 --address utu1…
uturoa-node send --api 127.0.0.1:8545 --key alice.json --to utu1… --amount 12,5
uturoa-wallet                       # portefeuille : ouvre le navigateur (--dir, --port, --no-browser)
```

## 5. Conventions

- **Publication** : le workflow `release.yml` construit les binaires Linux,
  Windows et macOS et les joint à une release GitHub (GitHub, Releases,
  Draft, tag `vX.Y.Z` sur `main`, Publish). La version est dans `Cargo.toml`.
- Guides : `docs/TESTNET-LOCAL.md` (en local), `docs/DEPLOYER-PROXMOX.md`
  (remplace VinX sur les mêmes conteneurs et **les mêmes ports** : 26656
  pour le réseau, 127.0.0.1:8545 pour l'API ; service `uturoa`),
  `docs/EXERCICES-DE-PANNE.md`.
- **Lints stricts** : pas d'`unwrap`, pas d'`expect`, pas de `panic`, pas
  d'arithmétique non vérifiée (`checked_*` ou `saturating_*`), pas
  d'indexation directe dans le code de production. Les tests en sont
  dispensés (`clippy.toml`).
- **Changer une règle = nouvelle version de protocole**, gardée par la
  version. On ne touche jamais au plafond ni à la formule de forge.
- Toute nouvelle étiquette de domaine s'ajoute à `Domain` et au test
  `tags_are_short_and_distinct`.

## 6. Points d'attention connus

- `apply_block` clone tout l'état pour être atomique, et `state_root`
  recalcule la racine de tous les comptes : simple et correct, mais en O(n).
  À remplacer par un arbre de Merkle incrémental et un journal de
  modifications au lot 5 (nœud).
- Le vote est à poids égal (le moteur compte les validateurs, pas le stake).
  Passer à un vote pondéré demanderait de modifier le moteur de consensus.
- `Leave` est définitif : un opérateur parti ne peut pas se réinscrire avec
  la même adresse.
- Pour répéter la bascule sans 1 314 000 blocs : `mint_phase_blocks` dans la
  genèse (ou `devnet --mint-phase-blocks`), **réseaux d'essai seulement**.
- Un instantané reçu d'un pair est décodé par `decode_canonical` : le
  fuzzing a montré qu'un état aux tables désordonnées était sinon accepté.
- RandomX ne peut pas changer de fil d'exécution : `RandomXHasher` le fait
  tourner dans un fil dédié et lui envoie les calculs par message (pas de
  code `unsafe`).
- Instantané d'un pair : accepté s'il est signé par l'ensemble de validateurs
  qu'on connaît déjà, ou si l'opérateur fournit l'empreinte du bloc
  (`--trusted-block`). Après la bascule, un nouveau nœud a donc besoin d'une
  empreinte de confiance.
- Une part est vérifiée par RandomX à son arrivée ; `CachedPow` mémoïse les
  empreintes, et `Chain` garde l'état résultant du dernier bloc vérifié (clé :
  empreinte du bloc **entier**, jamais de l'en-tête seul).
- `mine`, `send` et `new-key` restent des outils de test en ligne de
  commande ; le portefeuille est `uturoa-wallet`.
- Portefeuille : comptes dérivés par `BLAKE3-derive_key("Uturoa portefeuille
  v1 compte ed25519", graine BIP-39 ‖ i)` ; ne jamais changer cette chaîne,
  sinon les mêmes 12 mots ne redonnent plus les mêmes comptes.
- Un nœud vérifie chaque part reçue avec RandomX (environ 25 ms) : un mineur
  sur la même machine que le nœud ralentit les blocs.
- Si RandomX échoue localement, `RandomXHasher` renvoie une empreinte qui ne
  satisfait aucune cible : le nœud refuse la part (risque de blocage, jamais
  d'acceptation à tort).

## 7. Feuille de route

| Lot | Contenu | État |
|---|---|---|
| 0 | Socle : espace de travail, crypto, keystore, CI, `SPEC.md` | fait |
| 1 | `uturoa-core` : blocs, votes, paramètres, transactions de la liste fermée ; moteur de consensus | fait |
| 2 | État : comptes, envoi, multisignatures, frais, melt & forge | fait |
| 3 | Frappe RandomX : `MintShare`, partage par période, difficulté | fait |
| 4 | Preuve d'enjeu : délégation, sélection, plafond de vote, sanctions, bascule | fait |
| 5a | Nœud : genèse, stockage, chaîne, `devnet` | fait |
| 5b | Nœud : réseau libp2p, boucle de consensus, mempool, preuves d'équivoque soumises d'office | fait |
| 5c | Nœud : API, synchronisation par instantané, explorateur, outils de test (`mine`, `send`) | fait |
| 6a | Portefeuille de bureau avec mineur intégré | fait |
| 6b | Portefeuille mobile | à faire |
| 7a | Déploiement Proxmox, exercices de panne (guide et test automatique `faults.rs`) | fait |
| 7b | Fuzzing (nuit et rapide), répétition de la bascule sur 8 nœuds (`bascule.rs`, phase raccourcie par `mint_phase_blocks` en genèse) | fait |
