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

## 6. Feuille de route

| Lot | Contenu | État |
|---|---|---|
| 0 | Socle : espace de travail, crypto, keystore, CI, `SPEC.md` | fait |
| 1 | `uturoa-core` : blocs, votes, paramètres, transactions de la liste fermée ; moteur de consensus | à faire |
| 2 | État : comptes, envoi, multisignatures, frais, melt & forge | à faire |
| 3 | Frappe RandomX : `MintShare`, partage par période, difficulté | à faire |
| 4 | Preuve d'enjeu : délégation, sélection, plafond de vote, sanctions, bascule | à faire |
| 5 | Nœud : réseau, stockage, synchronisation par instantané, API, explorateur | à faire |
| 6 | Portefeuilles (bureau avec mineur intégré, mobile) | à faire |
| 7 | Testnet public, fuzzing, essais de panne, répétition complète frappe → bascule | à faire |
