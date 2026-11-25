# Uturoa

Une monnaie, une fonction : envoyer de la valeur, vite et presque gratuitement.

- Aucune prévente, aucune part fondateur : toutes les pièces sont frappées par
  la communauté, puis forgées par les validateurs.
- Consensus BFT de type Tendermint, finalité en environ 2 secondes.
- Plafond de 1 milliard de pièces, émission « melt & forge ».

Uturoa est un nom de code. Les règles du protocole sont dans [`SPEC.md`](SPEC.md).

## État

Pré-alpha. Le dépôt contient pour l'instant :

| Crate | Rôle |
|---|---|
| `uturoa-crypto` | BLAKE3, signatures ed25519 typées, adresses `utu1…`, arbre de Merkle |
| `uturoa-keystore` | Fichiers de clés chiffrés (Argon2 et ChaCha20-Poly1305) |

Rien n'est déployé. Aucune pièce n'existe.

## Compiler et tester

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Licence

Apache-2.0. Voir [`LICENSE`](LICENSE).
