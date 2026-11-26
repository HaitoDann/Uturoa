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

### 4.1 Vote à poids égal

Le moteur de consensus compte les votes **à poids égal** : chaque validateur
actif pèse `1 / n`, et le quorum est `⌊2n/3⌋ + 1` validateurs. Le stake sert
à **sélectionner** les validateurs et à **partager** les récompenses, pas à
pondérer le vote. Avec au moins 10 validateurs actifs, aucun ne pèse plus de
10 % du vote.

Tout changement d'ensemble décidé au bloc `h` prend effet au bloc `h + 2`.

### 4.2 Phase 1 : preuve d'autorité

- 4 à 7 validateurs d'amorçage, fixés dans la genèse.
- Ils ne reçoivent aucune unité en plus de la frappe accessible à tous.
- Pas de mise à l'écart pour absence pendant la phase 1.
- Inscriptions et délégations sont déjà possibles, pour préparer la bascule.

### 4.3 Bascule

- Le bloc 1 314 001 (après la fin de la frappe) est le premier validé par
  l'ensemble à stake. La bascule ne change que la règle de sélection des
  validateurs.
- S'il y a moins de 4 candidats éligibles, les validateurs d'amorçage restent
  en place jusqu'à ce qu'il y en ait assez.
- Si la répartition du stake est jugée trop concentrée, une version de
  protocole publiée avant la hauteur prévue peut repousser la bascule.

### 4.4 Phase 2 : preuve d'enjeu

**Sélection** : à chaque fin d'époque (et après une sortie de l'ensemble),
les candidats dont le stake propre atteint **10 000 pièces** sont classés par
stake total (propre + délégué) ; les **21** premiers (paramètre, jusqu'à 100)
forment l'ensemble actif.

**Validateurs** :
- inscription (`RegisterValidator`) : clé de consensus unique, fiche
  (nom de 1 à 64 octets, site et contact de 128 octets au plus), commission,
  et **plafond de commission déclaré, définitif** (100 % au plus) ;
- **commission** : 10 % proposés par défaut par les portefeuilles ; jamais
  au-delà du plafond déclaré ; une hausse est d'1 point au plus et une fois
  par époque ; une baisse est toujours permise ;
- **départ** (`Leave`) : définitif ; tout le stake délégué au validateur part
  en restitution.

**Délégation** : n'importe qui délègue à un validateur inscrit (`Delegate`).
`Undelegate` met le montant en **restitution pendant 21 jours** (151 200
blocs) ; il est crédité automatiquement à l'échéance.

**Présence** : un validateur n'est **attendu** que pour les blocs où il fait
partie de l'ensemble actif. Chaque bloc `h` porte le certificat du bloc
`h − 1`, vérifié contre l'ensemble de `h − 1` (quorum et signatures) ; chaque
validateur de cet ensemble est compté attendu, et signataire s'il figure dans
le certificat.

**Mise à l'écart** : 1 000 blocs attendus d'affilée sans signature, en
phase 2 : le validateur est mis à l'écart (retiré à `h + 2`), sauf si
l'ensemble tomberait sous 4 validateurs. `Unjail` est possible après une
époque, avec un stake propre d'au moins 10 000 pièces.

**Double signature** (`ReportEquivocation`, preuve vérifiée par la clé seule,
contre l'ensemble de la hauteur de la faute, jusqu'à 21 jours après) :
- stake propre confisqué à 100 %, y compris ce qui est en restitution ;
- **1 % des délégations** en cours vers ce validateur, pour inciter à bien
  choisir ;
- 10 % du total confisqué à celui qui apporte la preuve, 90 % fondus ;
- exclusion définitive ; les délégations restantes partent en restitution ;
- une même preuve n'est sanctionnée qu'une fois.

**Mise à jour du protocole** : à chaque fin d'époque, si plus des 2/3 des
validateurs actifs ont signalé (`SignalUpgrade`) la version suivante, elle
devient la version en vigueur.

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

- La forge s'accumule pendant une **époque** (7 200 blocs, un jour). À la fin
  de l'époque, elle est partagée entre les validateurs **éligibles** :
  candidats, jamais mis à l'écart pendant l'époque, stake propre d'au moins
  10 000 pièces, et **au moins 97 % de signatures** sur les blocs où ils
  étaient attendus.
- Part d'un validateur : au prorata de son stake total. Il prélève sa
  commission, le reste va à ses délégants (lui compris) au prorata.
- Ce qui n'est pas versé retourne au réservoir.
- Les propositions de blocs ne sont pas rémunérées à part : le proposeur
  reçoit déjà 20 % des frais du bloc.
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
2. Nombre et identité des validateurs d'amorçage.
3. Réglage de la difficulté initiale et de la cible de parts après un testnet.
