# Exercices de panne

Quatre exercices qui vérifient, sur un vrai réseau, qu'Uturoa se comporte
comme prévu quand des validateurs tombent. Ils se rejouent après chaque
changement important (nouvelle version, nouveau validateur, nouvel
hébergement).

Prérequis : le testnet de `docs/DEPLOYER-PROXMOX.md`, et une clé SSH de Copper
vers chaque validateur (`ssh-copy-id root@192.168.1.X`).

**Lancer les commandes une par une** : plusieurs étapes demandent d'attendre
avant de mesurer. Avec des blocs de 12 s, la chaîne avance de 5 blocs par
minute.

Commande `hauteurs`, à définir une fois dans le terminal de Copper :

```bash
hauteurs() { for ip in 192.168.1.22 192.168.1.21 192.168.1.23 192.168.1.233; do printf "%-15s" $ip; ssh root@$ip "curl -s 127.0.0.1:8645/status" | grep -oE '"height":[0-9]+'; done; }
```

## 1. Un validateur en panne : la chaîne continue

Avec 4 validateurs, il faut plus des deux tiers des votes : 3 suffisent.

```bash
hauteurs                                              # tous égaux
ssh root@192.168.1.233 "systemctl stop uturoa"; date
# attendre 2 minutes, mesurer deux fois : la hauteur avance
curl -s 127.0.0.1:8645/status | grep -oE '"height":[0-9]+'
ssh root@192.168.1.233 "systemctl start uturoa"; sleep 60
hauteurs                                              # Pearl a rattrapé
ssh root@192.168.1.233 "journalctl -u uturoa --since '-10min' --no-pager | grep -iE 'error|panic' | tail"
```

Attendu : la chaîne avance pendant la panne ; un bloc sur quatre (le tour de
Pearl) passe au round 1 après un court délai ; Pearl rattrape seul à son
retour.

## 2. Deux validateurs en panne : arrêt sûr, puis reprise

Avec 2 validateurs sur 4, il n'y a plus de majorité : la chaîne doit
**s'arrêter**, sans bloc incohérent, et repartir seule.

```bash
ssh root@192.168.1.233 "systemctl stop uturoa"; ssh root@192.168.1.23 "systemctl stop uturoa"; date
# attendre 2 minutes, puis mesurer deux fois à 1 minute d'écart :
curl -s 127.0.0.1:8645/status | grep -oE '"height":[0-9]+'   # identiques
ssh root@192.168.1.23 "systemctl start uturoa"; ssh root@192.168.1.233 "systemctl start uturoa"
# après 1 minute : la hauteur repart
hauteurs
for ip in 192.168.1.22 192.168.1.21 192.168.1.23 192.168.1.233; do ssh root@$ip "journalctl -u uturoa --since '-15min' --no-pager | grep -iE 'error|panic|ÉQUIVOQUE' | tail -5"; done
```

Attendu : hauteur figée pendant la panne ; reprise automatique ; aucune
équivoque. Un envoi fait pendant la panne reste en attente et passe à la
reprise.

## 3. Perte d'un disque : reconstruction

On efface les données d'Amber en gardant son identité réseau (`network.key`) ;
sa clé de validateur, dans `/opt/uturoa`, n'est pas touchée. La mémoire
anti-double vote est effacée avec la base : attendre **au moins une minute**
avant de redémarrer, pour que la chaîne ait dépassé les hauteurs où Amber a
voté.

```bash
ssh root@192.168.1.21 "systemctl stop uturoa; date"
ssh root@192.168.1.21 "mv /var/lib/uturoa /var/lib/uturoa.sauvegarde && mkdir -m 700 /var/lib/uturoa && cp -p /var/lib/uturoa.sauvegarde/network.key /var/lib/uturoa/"
# attendre 1 minute
ssh root@192.168.1.21 "systemctl start uturoa"
# après 2 minutes
hauteurs
ssh root@192.168.1.21 "journalctl -u uturoa --since '-5min' --no-pager | grep -iE 'instantan|rattrap|error|panic' | tail"
```

Attendu : Amber rattrape les blocs auprès des autres (ils gardent tout
l'historique), puis revient à leur hauteur. Retour arrière si besoin :
arrêter le nœud et remettre `/var/lib/uturoa.sauvegarde` en place.

## 4. Coupure de courant : arrêt brutal répété

Le processus est tué sans pouvoir fermer sa base (`kill -9`), trois fois.

```bash
for i in 1 2 3; do
  ssh root@192.168.1.23 "systemctl kill -s KILL uturoa; date"; sleep 20
  ssh root@192.168.1.23 "systemctl start uturoa"; sleep 40
done
hauteurs
ssh root@192.168.1.23 "journalctl -u uturoa --since '-5min' --no-pager | grep -iE 'error|panic|corrupt' | tail"
for ip in 192.168.1.22 192.168.1.21 192.168.1.233; do ssh root@$ip "journalctl -u uturoa --since '-5min' --no-pager | grep -iE 'ÉQUIVOQUE|preuve' | tail"; done
```

Attendu : redémarrage et rattrapage à chaque fois ; aucune corruption ; aucune
équivoque chez les autres validateurs.

## Rejoués automatiquement

Les exercices 1, 2 et 4 (arrêt puis redémarrage) sont aussi rejoués par la CI
à chaque modification, sur quatre validateurs locaux
(`crates/uturoa-node/tests/faults.rs`). Les refaire sur le vrai réseau reste
utile après un changement d'infrastructure.

## Résultats

À remplir après chaque campagne : date, version, et pour chaque exercice le
résultat observé (hauteurs, durées, erreurs éventuelles).

| Date | Version | 1 | 2 | 3 | 4 | Remarques |
|---|---|---|---|---|---|---|
| | | | | | | |

Limite : toutes les machines partagent un serveur et un réseau local. Ces
résultats ne disent rien de la latence entre sites distants : à refaire quand
les validateurs seront répartis.
