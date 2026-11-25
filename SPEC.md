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
`utu/validators`, `utu/tx/approval`, `utu/table/key`. Les étiquettes de la
frappe et de la forge seront ajoutées avec leur lot.

## 3. Unités

- 1 pièce = 10^8 unités.
- `PLAFOND` = 10^9 pièces = 10^17 unités (tient dans un `u64`).
- Toute arithmétique sur les montants est vérifiée (`checked_*`). Un
  dépassement est une erreur, jamais un bouclage.

## 4. Consensus

- BFT de type Tendermint, finalité immédiate, `n = 3f + 1`.
- Bloc visé : 2 secondes, soit environ 15 768 000 blocs par an.
- Chaque bloc porte le certificat du bloc précédent (`last_commit`), avec
  toutes les signatures reçues. Il sert à mesurer la présence des validateurs.

### 4.1 Phase 1 : preuve d'autorité

- 4 à 7 validateurs d'amorçage, fixés dans la genèse, à poids égal.
- Les validateurs d'amorçage ne reçoivent aucune unité en plus de la frappe
  accessible à tous.

### 4.2 Bascule

- À une hauteur fixée dans la genèse (fin du 6e mois, environ 7 884 000
  blocs).
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

- Algorithme : RandomX.
- Les mineurs ne produisent pas de blocs. Ils soumettent des **parts de
  frappe** sous forme de transactions `MintShare`.
- Une part contient : l'adresse du bénéficiaire, l'empreinte d'un bloc récent
  (fenêtre de validité **à trancher**, de l'ordre de 64 blocs) et un nonce.
  Elle n'est valide que si son empreinte RandomX est sous la cible de
  difficulté.
- Une part ne peut être acceptée qu'une seule fois.
- Quantité frappée : 150 000 000 pièces sur la phase, soit 1 902 587 519
  unités par bloc (quantité entière, constante). Le reste de la division reste
  dans le réservoir.
- Partage : toutes les 300 blocs (10 minutes), la récompense accumulée est
  répartie entre les parts valides de la période, au prorata.
- Nombre de parts par bloc plafonné ; difficulté ajustée pour viser un nombre
  cible de parts par période. Valeurs **à trancher** lors du lot 3.
- `MintShare` est refusée après la bascule.

## 6. Phase 2 : melt & forge

```
réservoir = PLAFOND − offre en circulation
forge     = réservoir >> 29          (à chaque bloc, division entière)
melt      = 80 % des frais + unités confisquées → retour au réservoir
proposeur = 20 % des frais
```

- La forge du bloc est partagée entre les validateurs actifs et leurs
  délégants, au prorata du stake, après la commission du validateur.
- Un validateur absent sur l'époque ne reçoit pas sa part. Sa part non versée
  reste dans le réservoir.
- Le plafond ne peut jamais être dépassé : la forge est toujours inférieure au
  réservoir.

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

- Frais de base dynamiques : ils montent quand les blocs sont pleins et
  baissent quand ils sont vides, avec un plancher et un supplément par octet.
- Répartition : 80 % fondus, 20 % au proposeur.
- Formule exacte et plancher **à trancher** lors du lot 2.

## 8. Transactions (liste fermée)

| Transaction | Rôle |
|---|---|
| `Transfer` | Envoyer, avec un mémo optionnel de 64 octets au plus |
| `BatchTransfer` | Payer plusieurs destinataires |
| `ConfigureMultisig` | Rendre un compte multisignature (`m` sur `n`) |
| `Delegate`, `Undelegate` | Déléguer son stake à un validateur |
| `RegisterValidator`, `UpdateValidatorInfo`, `Unjail`, `Leave` | Cycle de vie d'un validateur |
| `ReportEquivocation` | Preuve de double signature |
| `SignalUpgrade` | Signalement d'une version de protocole |
| `MintShare` | Part de frappe, phase 1 seulement |

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
2. Paramètres de la frappe : fenêtre de validité d'une part, cible de parts,
   plafond par bloc.
3. Formule des frais dynamiques.
4. Commission maximale des validateurs.
5. Nombre et identité des validateurs d'amorçage.
