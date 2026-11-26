# Déployer le testnet Uturoa sur Proxmox

Ce guide installe un testnet Uturoa sur les quatre conteneurs qui font déjà
tourner les validateurs de VinX Ledger, **sans toucher à VinX** : Uturoa
utilise ses propres dossiers, son propre service et ses propres ports.

| Machine | Adresse | Rôle Uturoa |
|---|---|---|
| Cinnamon | 192.168.1.22 | validateur 1 |
| Amber | 192.168.1.21 | validateur 2 |
| Obsidian | 192.168.1.23 | validateur 3 |
| Pearl | 192.168.1.233 | validateur 4 |
| Copper (`vinx-public`) | 192.168.1.244 | préparation, nœud observateur, API pour les portefeuilles |

| Élément | VinX | Uturoa |
|---|---|---|
| Programme | `/usr/local/bin/vinx-node` | `/opt/uturoa/uturoa-node` |
| Données | celles de VinX | `/var/lib/uturoa` |
| Service | `vinx-node` | `uturoa` |
| Port réseau | celui de VinX | **30433** (TCP) |
| API | 8545 | **8645** (sur 127.0.0.1 seulement) |

Chaque nœud Uturoa utilise environ 300 Mo de mémoire en plus (RandomX en mode
léger). Vérifier que chaque conteneur a au moins 1 Go libre.

Toutes les commandes se lancent **depuis Copper** (`vinx-public`), comme pour
VinX : on joint les validateurs avec `ssh root@192.168.1.X`.

## 1. Publier la version

Sur GitHub : *Releases*, *Draft a new release*, tag `v0.1.0` sur `main`,
*Publish release*. Attendre une dizaine de minutes que le workflow *Release*
joigne les binaires (onglet *Actions*).

## 2. Préparer le testnet sur Copper

```bash
mkdir -p ~/uturoa && cd ~/uturoa
wget https://github.com/HaitoDann/Uturoa/releases/download/v0.1.0/uturoa-v0.1.0-linux-x86_64.tar.gz
wget https://github.com/HaitoDann/Uturoa/releases/download/v0.1.0/uturoa-v0.1.0-linux-x86_64.tar.gz.sha256
sha256sum -c uturoa-v0.1.0-linux-x86_64.tar.gz.sha256
tar xzf uturoa-v0.1.0-linux-x86_64.tar.gz
cp uturoa-v0.1.0-linux-x86_64/uturoa-node .
```

Choisir le mot de passe des clés des validateurs (le noter : il servira sur
chaque validateur), puis créer le testnet :

```bash
read -rs -p "Mot de passe des clés : " UTUROA_KEY_PASSWORD; echo; export UTUROA_KEY_PASSWORD
./uturoa-node devnet --dir testnet --chain-id utu-testnet-1 \
  --hosts 192.168.1.22,192.168.1.21,192.168.1.23,192.168.1.233 \
  --names Cinnamon,Amber,Obsidian,Pearl \
  --base-port 30433
```

Sans `--block-interval-ms`, les blocs durent **12 secondes**, comme sur le
vrai réseau. Le dossier `testnet/` contient :

- `genesis.json` ;
- `validator-1.json` à `validator-4.json` : les clés chiffrées ;
- `d1/network.key` à `d4/network.key` : les identités réseau ;
- `uturoa-1.service` à `uturoa-4.service` : un service par validateur, avec
  les pairs déjà renseignés.

## 3. Installer chaque validateur

Les commandes ci-dessous installent **Cinnamon** (numéro 1). Pour les autres,
changer `N` et `IP` :

| N | IP |
|---|---|
| 1 | 192.168.1.22 |
| 2 | 192.168.1.21 |
| 3 | 192.168.1.23 |
| 4 | 192.168.1.233 |

```bash
N=1; IP=192.168.1.22
ssh root@$IP "mkdir -p /opt/uturoa /var/lib/uturoa /etc/uturoa && chmod 700 /var/lib/uturoa /etc/uturoa"
scp uturoa-node testnet/genesis.json root@$IP:/opt/uturoa/
scp testnet/validator-$N.json root@$IP:/opt/uturoa/validator.json
scp testnet/d$N/network.key root@$IP:/var/lib/uturoa/network.key
scp testnet/uturoa-$N.service root@$IP:/etc/systemd/system/uturoa.service
ssh root@$IP "printf 'UTUROA_KEY_PASSWORD=%s\n' '$UTUROA_KEY_PASSWORD' > /etc/uturoa/env && chmod 600 /etc/uturoa/env /opt/uturoa/validator.json /var/lib/uturoa/network.key"
```

Si un pare-feu tourne dans les conteneurs, ouvrir le port 30433 en TCP entre
eux (par exemple `ufw allow from 192.168.1.0/24 to any port 30433 proto tcp`).

**Démarrer** une fois les quatre installés, un par un :

```bash
for IP in 192.168.1.22 192.168.1.21 192.168.1.23 192.168.1.233; do
  ssh root@$IP "systemctl daemon-reload && systemctl enable --now uturoa"
done
```

Les blocs commencent dès que trois validateurs sur quatre sont en marche.

## 4. Vérifier

```bash
for IP in 192.168.1.22 192.168.1.21 192.168.1.23 192.168.1.233; do
  printf "%-15s" $IP; ssh root@$IP "curl -s 127.0.0.1:8645/status" | grep -oE '"height":[0-9]+'
done
ssh root@192.168.1.22 "journalctl -u uturoa -n 20 --no-pager"
```

Attendu : les quatre hauteurs sont égales (à un bloc près) et augmentent de 5
par minute ; les journaux montrent `bloc validé ... round=0`.

## 5. Nœud observateur et API sur Copper

Copper suit la chaîne sans voter et sert l'API aux portefeuilles :

```bash
mkdir -p /opt/uturoa /var/lib/uturoa-public
cp ~/uturoa/uturoa-node ~/uturoa/testnet/genesis.json /opt/uturoa/
P=$(grep -o -- '--peer [^ ]*' ~/uturoa/testnet/uturoa-2.service | head -3 | tr '\n' ' ')
P1=$(grep -o -- '--peer [^ ]*' ~/uturoa/testnet/uturoa-1.service | head -1)
cat > /etc/systemd/system/uturoa.service <<EOF
[Unit]
Description=Nœud public Uturoa (observateur)
After=network-online.target

[Service]
ExecStart=/opt/uturoa/uturoa-node run --genesis /opt/uturoa/genesis.json --data-dir /var/lib/uturoa-public --listen /ip4/0.0.0.0/tcp/30433 --api 127.0.0.1:8645 $P1 $P
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload && systemctl enable --now uturoa
curl -s 127.0.0.1:8645/status
```

(`$P1 $P` réunit les adresses des quatre validateurs, prises dans les fichiers
de service.)

## 6. Miner et envoyer depuis ton PC Windows

1. Télécharger `uturoa-v0.1.0-windows-x86_64.zip` depuis la release et
   l'extraire.
2. Ouvrir un tunnel vers l'API de Copper (PowerShell, laisser la fenêtre
   ouverte) :

   ```powershell
   ssh -N -L 8645:127.0.0.1:8645 root@192.168.1.244
   ```

3. Lancer `uturoa-wallet.exe` (double-clic). Créer le portefeuille, noter les
   12 mots, puis dans **Réglages**, nœud : `127.0.0.1:8645`.
4. Onglet **Frapper** : choisir le nombre de cœurs, **Démarrer**. Les pièces
   arrivent toutes les 50 blocs, soit **10 minutes** avec des blocs de 12 s.

Il vaut mieux miner depuis le PC que depuis un validateur : chaque part est
vérifiée par les nœuds, et un mineur sur la même machine leur prend du
processeur.

## 7. Mettre à jour

Un nœud à la fois, validateurs d'abord, Copper en dernier :

```bash
scp uturoa-node root@192.168.1.22:/opt/uturoa/uturoa-node.new
ssh root@192.168.1.22 "mv /opt/uturoa/uturoa-node.new /opt/uturoa/uturoa-node && systemctl restart uturoa"
```

Attendre que le nœud ait rattrapé (section 4) avant de passer au suivant.

## 8. Tout effacer (repartir de zéro)

```bash
for IP in 192.168.1.22 192.168.1.21 192.168.1.23 192.168.1.233; do
  ssh root@$IP "systemctl disable --now uturoa; rm -f /var/lib/uturoa/chain.redb"
done
```

`network.key` est gardé pour conserver les mêmes adresses réseau. Pour une
nouvelle genèse, refaire les sections 2 et 3. VinX n'est jamais touché.

## Limites

- Ce testnet n'a **aucune valeur** et peut être effacé à tout moment.
- Le mot de passe des clés est dans `/etc/uturoa/env` (lisible par root
  seulement). C'est acceptable pour un testnet, pas pour le réseau principal.
- La phase de frappe dure 6 mois de blocs (1 314 000 blocs) : la bascule vers
  la preuve d'enjeu ne se produira pas pendant ces essais. Elle est vérifiée
  par les tests automatiques, et sera répétée sur un réseau à phase courte.
- Exercices de panne à faire sur ce testnet : `docs/EXERCICES-DE-PANNE.md`.
