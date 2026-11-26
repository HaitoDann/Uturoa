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

- Phase 1 (6 mois) : preuve d'autorité et frappe RandomX par parts
  (`MintShare`), 150 M de pièces.
- Bascule vers la preuve d'enjeu par simple changement de règle de sélection
  des validateurs.
- Phase 2 : preuve d'enjeu avec délégation, plafond de 10 % de vote par
  validateur, économie « melt & forge » (`forge = réservoir >> 29`, frais
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
| `uturoa-state` | Machine d'état : comptes, réserve, envois, multisignatures, frais dynamiques, melt, émission (frappe puis forge) mise de côté dans `pending_rewards` |
| `uturoa-keystore` | Fichiers de clés chiffrés ; mot de passe par `UTUROA_KEY_PASSWORD` ou fichier |

## 4. Commandes

```bash
cargo fmt --all                                         # toujours avant un commit
cargo clippy -q --workspace --all-targets -- -D warnings
cargo test -q --workspace
cargo deny check                                        # failles et licences
```

## 5. Conventions

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
- Les unités émises s'accumulent dans `pending_rewards` : leur partage
  viendra aux lots 3 (parts de frappe) et 4 (validateurs et délégants).
- Les transactions de validation (0x04 à 0x0B) renvoient `NotYetSupported`
  jusqu'au lot 4.

## 7. Feuille de route

| Lot | Contenu | État |
|---|---|---|
| 0 | Socle : espace de travail, crypto, keystore, CI, `SPEC.md` | fait |
| 1 | `uturoa-core` : blocs, votes, paramètres, transactions de la liste fermée ; moteur de consensus | fait |
| 2 | État : comptes, envoi, multisignatures, frais, melt & forge | fait |
| 3 | Frappe RandomX : `MintShare`, partage par période, difficulté | à faire |
| 4 | Preuve d'enjeu : délégation, sélection, plafond de vote, sanctions, bascule | à faire |
| 5 | Nœud : réseau, stockage, synchronisation par instantané, API, explorateur | à faire |
| 6 | Portefeuilles (bureau avec mineur intégré, mobile) | à faire |
| 7 | Testnet public, fuzzing, essais de panne, répétition complète frappe → bascule | à faire |
