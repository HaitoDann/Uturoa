# Uturoa

Une monnaie, une fonction : envoyer de la valeur, vite et presque gratuitement.

- Aucune prévente, aucune part fondateur : toutes les pièces sont frappées par
  la communauté, puis forgées par les validateurs.
- Consensus BFT de type Tendermint, blocs de 12 secondes, finalité immédiate.
- Plafond de 1 milliard de pièces, émission « melt & forge ».

Uturoa est un nom de code. Les règles du protocole sont dans [`SPEC.md`](SPEC.md).

## État

Pré-alpha. Le dépôt contient pour l'instant :

| Crate | Rôle |
|---|---|
| `uturoa-crypto` | BLAKE3, signatures ed25519 typées, adresses `utu1…`, arbre de Merkle |
| `uturoa-core` | Types du protocole : transactions, parts de frappe, blocs, votes, constantes d'émission |
| `uturoa-consensus` | Moteur de consensus BFT de type Tendermint |
| `uturoa-state` | Machine d'état : comptes, envois, multisignatures, frais dynamiques, frappe, melt & forge |
| `uturoa-pow` | Preuve de travail RandomX : vérification et minage des parts de frappe |
| `uturoa-node` | Nœud : genèse, stockage, chaîne ; `uturoa-node devnet` crée un réseau local |
| `uturoa-keystore` | Fichiers de clés chiffrés (Argon2 et ChaCha20-Poly1305) |

Rien n'est déployé. Aucune pièce n'existe.

## Compiler et tester

Il faut Rust, CMake et un compilateur C++ (pour RandomX).

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Licence

Apache-2.0. Voir [`LICENSE`](LICENSE).
