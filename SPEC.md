# Spécification d'Uturoa (version 0.1, brouillon)

Uturoa est un nom de code. Le nom définitif sera choisi avant le réseau
principal.

Ce document fixe les règles du protocole. Tant qu'une règle est marquée
**à trancher**, elle n'est pas définitive.

## 1. Principe

Une monnaie, une fonction : envoyer de la valeur, vite et presque gratuitement.

- Pas de jetons, de contrats, de noms ni de séquestres.
- Aucune prévente, aucune part fondateur. La genèse contient zéro unité.
- Toute unité naît soit par la **frappe** (phase 1), soit par la **forge**
  (phase 2).
- Plafond absolu : 1 000 000 000 pièces.

## 2. Primitives cryptographiques

| Élément | Choix |
|---|---|
| Hachage | BLAKE3-256 |
| Signature | ed25519, clé publique typée (octet d'algorithme puis clé) |
| Adresse | 20 octets : `H("utu/address" ‖ clé typée)[0..20]` |
| Affichage | Bech32m, préfixe `utu`, donc `utu1…`. Casse mixte refusée |
| Séparation de domaines | Tout hachage et tout message signé est préfixé par `longueur ‖ étiquette`, étiquettes en `utu/…` |

Étiquettes en vigueur : `utu/address`, `utu/state`, `utu/tx`, `utu/txid`,
`utu/block`, `utu/vote`, `utu/proposal`, `utu/merkle/leaf`,
`utu/merkle/node`, `utu/merkle/empty`, `utu/smt/leaf`, `utu/smt/node`,
`utu/validators`, `utu/tx/approval`, `utu/table/key`, `utu/mint` (parts de
frappe), `utu/commit` (certificat du bloc précédent).

## 3. Unités

- 1 pièce = 10^9 unités (9 décimales).
- `PLAFOND` = 10^9 pièces = 10^18 unités (tient dans un `u64`, marge d'environ
  18 fois). Les calculs au prorata se font en 128 bits.
- **Réserve minimale** : 0,01 pièce. Un compte neuf doit recevoir au moins la
  réserve ; le solde d'un compte est soit nul, soit au moins égal à la
  réserve. Un compte n'est jamais supprimé, même à zéro : son nonce est
  conservé, ce qui empêche de rejouer ses anciennes transactions.
- Toute arithmétique sur les montants est vérifiée (`checked_*`). Un
  dépassement est une erreur, jamais un bouclage.

## 4. Consensus

- BFT de type Tendermint, finalité immédiate, `n = 3f + 1`.
- Bloc visé : 12 secondes, soit 2 628 000 blocs par an (365 jours).
- **Toute l'émission est comptée en blocs.** Changer la durée d'un bloc exige
  une version de protocole qui ajuste en même temps la durée de la frappe, la
  frappe par bloc et le diviseur de forge, sinon le calendrier d'émission
  change.
- Chaque bloc porte le certificat du bloc précédent (`last_commit`), avec
  toutes les signatures reçues. Il sert à mesurer la présence des validateurs.
  Son empreinte `H("utu/commit" ‖ certificat)` est dans l'en-tête
  (`last_commit_hash`, zéros au premier bloc).
- Contenu d'un bloc : l'en-tête, `last_commit`, les transactions, puis les
  parts de frappe. `tx_root` est la racine de Merkle des identifiants des
  transactions puis des parts, dans l'ordre du bloc.

### 4.1 Phase 1 : preuve d'autorité

- 4 à 7 validateurs d'amorçage, fixés dans la genèse, à poids égal.
- Les validateurs d'amorçage ne reçoivent aucune unité en plus de la frappe
  accessible à tous.

### 4.2 Bascule

- À la fin de la phase de frappe : hauteur 1 314 000 (6 mois de blocs de
  12 s).
- La bascule ne change que la règle de sélection des validateurs. Le moteur de
  consensus reste le même.
- Si la répartition du stake est jugée trop concentrée à l'approche de la
  bascule, une version de protocole publiée avant la hauteur prévue repousse
  la bascule et prolonge la frappe, par tranches d'un mois.

### 4.3 Phase 2 : preuve d'enjeu

- Validateurs actifs : les mieux classés par stake. 21 au départ, jusqu'à 100.
- Délégation ouverte à tous.
- Plafond de pouvoir de vote : aucun validateur ne pèse plus de 10 % du
  total. Le stake au-delà ne compte pas dans le vote.
- Sanctions :
  - absent de `last_commit` pendant 1 000 blocs d'affilée : mis à l'écart,
    retiré à `H + 2` ;
  - double signature prouvée : caution confisquée à 100 %, exclusion
    définitive. Les unités confisquées sont fondues (§6).
- Restitution du stake après un délai de 21 jours, pendant lequel une preuve
  de double signature reste recevable.

## 5. Phase 1 : la frappe

- Algorithme : RandomX. Les validateurs vérifient en mode léger (256 Mo) ;
  coût mesuré de l'ordre de 25 ms par part sur un serveur modeste.
- Les mineurs ne produisent pas de blocs. Ils soumettent des **parts de
  frappe** (`MintShare`), incluses dans les blocs à côté des transactions.
- Une part **n'est pas une transaction** : elle n'a ni émetteur, ni nonce, ni
  frais, pour qu'un mineur sans aucune pièce puisse frapper. Le bénéficiaire
  fait partie des données hachées : copier une part en changeant le
  bénéficiaire la rend invalide.
- Une part contient : l'identifiant de chaîne, l'adresse du bénéficiaire, la
  hauteur et l'empreinte d'un bloc d'ancrage, et un nonce.
- **Validité** d'une part incluse dans le bloc `h` :
  - même identifiant de chaîne ;
  - bloc d'ancrage de hauteur `a` avec `h − 64 ≤ a < h`, et empreinte égale à
    celle de ce bloc ;
  - jamais acceptée auparavant (identifiant `H("utu/mint" ‖ part)`) ;
  - empreinte RandomX de `"utu/mint" ‖ part`, avec la clé du bloc `h`, sous la
    cible : ses 8 premiers octets, en grand-boutiste, valent au plus
    `u64::MAX / difficulté`.
- **Clé RandomX** du bloc `h` : l'empreinte du bloc de hauteur
  `⌊(h − 64) / 2048⌋ × 2048` (0 si négatif). Elle change toutes les
  2 048 blocs (environ 6 h 50), avec 64 blocs de préavis.
- Au plus 100 parts par bloc (chacune coûte une vérification RandomX).
- **Quantité frappée** : 150 000 000 pièces sur la phase, soit
  114 155 251 141 unités par bloc (quantité entière, constante), versées au
  pot de frappe.
- **Partage** : tous les 50 blocs (10 minutes), le pot est réparti entre les
  bénéficiaires des parts de la période, au prorata du nombre de parts. Le
  reste de la division reste au pot. Sans aucune part, le pot est reporté.
  Un paiement de frappe peut créer un compte sous la réserve minimale : il
  rémunère un travail, pas un envoi.
- **Difficulté** : à chaque fin de période, `difficulté × parts reçues /
  2 000`, bornée entre le quart et le quadruple de la précédente, jamais
  sous 1. Difficulté initiale : 1 000.
- **Fin de la phase** (bloc 1 314 000) : après le dernier partage, ce qui
  reste au pot retourne au réservoir. Les parts sont ensuite refusées.

## 6. Phase 2 : melt & forge

```
réservoir = PLAFOND − offre en circulation
forge     = réservoir / 89 478 485   (à chaque bloc, division entière)
melt      = 80 % des frais + unités confisquées → retour au réservoir
proposeur = 20 % des frais
```

- La forge du bloc est partagée entre les validateurs actifs et leurs
  délégants, au prorata du stake, après la commission du validateur.
- Un validateur absent sur l'époque ne reçoit pas sa part. Sa part non versée
  reste dans le réservoir.
- Le plafond ne peut jamais être dépassé : la forge est toujours inférieure au
  réservoir.

Le diviseur vaut 2^29 / 6 : environ 9,5 pièces par bloc à la bascule.

Ordres de grandeur (150 M en circulation à la bascule, sans usage) :

| Années après la bascule | Inflation annuelle |
|---|---|
| 1 | environ 16 % |
| 10 | environ 5 % |
| 18 | la moitié du plafond est en circulation |

Avec de l'usage, le melt recharge le réservoir. L'offre se stabilise là où la
forge égale le melt : plus la monnaie est utilisée, plus l'offre d'équilibre
est basse.

## 7. Frais

- **Frais minimaux** d'une transaction = `frais de base par octet × taille de
  la transaction signée`. Un envoi simple (environ 200 octets) coûte environ
  0,0001 pièce quand le réseau est calme.
- **Frais de base** : identique pour toutes les transactions d'un bloc. Après
  chaque bloc :
  - cible = la moitié de la taille maximale d'un bloc ;
  - au-dessus de la cible, il monte de `base × écart / cible / 8` (au moins
    une unité) ; en dessous, il baisse d'autant ; soit 12,5 % au plus par
    bloc ;
  - plancher : 500 unités par octet.
- **Priorité** : on peut payer plus que les frais minimaux.
- **Répartition** de la totalité des frais : 80 % fondus (retour au
  réservoir), 20 % au proposeur, l'arrondi allant au proposeur.

## 8. Transactions (liste fermée)

| Transaction | Rôle |
|---|---|
| `Transfer` (0x01) | Envoyer, avec un mémo optionnel de 64 octets au plus. Montant non nul, pas d'envoi à soi-même |
| `BatchTransfer` (0x02) | Payer de 1 à 100 destinataires |
| `ConfigureMultisig` (0x03) | Rendre un compte multisignature (`m` sur `n`, 10 signataires au plus), changer ou retirer la règle |
| `Delegate` (0x04), `Undelegate` (0x05) | Déléguer son stake à un validateur |
| `RegisterValidator` (0x06), `UpdateValidatorInfo` (0x07), `Unjail` (0x08), `Leave` (0x09) | Cycle de vie d'un validateur |
| `ReportEquivocation` (0x0A) | Preuve de double signature |
| `SignalUpgrade` (0x0B) | Signalement d'une version de protocole |

Autorisation d'une transaction :

- compte ordinaire : signature ed25519 de `"utu/tx" ‖ transaction`, la clé
  publique étant jointe tant que l'état ne la connaît pas ;
- compte multisignature : approbations de `"utu/tx/approval" ‖ transaction`
  par au moins `m` signataires, indices strictement croissants.

Toute évolution des règles passe par une nouvelle version de protocole,
activée quand plus des 2/3 du pouvoir de vote l'ont signalée. Il n'existe
aucune clé d'administration.

## 9. Stockage

- Nœud normal : l'état complet et les blocs des 30 derniers jours.
- Démarrage d'un nœud : par instantané d'état, vérifié par les signatures des
  validateurs.
- Nœud archive : facultatif, garde tout l'historique.

## 10. Questions encore ouvertes

1. Nom définitif et nom de l'unité.
2. Commission maximale des validateurs.
3. Nombre et identité des validateurs d'amorçage.
4. Réglage de la difficulté initiale et de la cible de parts après un testnet.
