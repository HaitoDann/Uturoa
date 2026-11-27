# Remplacer VinX Ledger par Uturoa sur Proxmox

Ce guide installe le testnet Uturoa **à la place** de VinX Ledger, sur les
mêmes conteneurs et **les mêmes ports** : la box, les redirections de ports et
le Caddy de Copper restent tels quels. VinX est arrêté mais **ses données sont
gardées** : on peut revenir en arrière à tout moment (section 9).

| Machine | Adresse | Rôle Uturoa |
|---|---|---|
| Cinnamon | 192.168.1.22 | validateur 1 |
| Amber | 192.168.1.21 | validateur 2 |
| Obsidian | 192.168.1.23 | validateur 3 |
| Pearl | 192.168.1.233 | validateur 4 |
| Copper (`vinx-public`) | 192.168.1.244 | préparation, nœud public, API et explorateur |

| Élément | VinX (arrêté, gardé) | Uturoa |
|---|---|---|
| Port réseau | 26656 | **26656** (le même) |
| API | 127.0.0.1:8545 | **127.0.0.1:8545** (la même) |
| Caddy de Copper | `reverse_proxy 127.0.0.1:8545` | **inchangé** : le domaine affiche l'explorateur Uturoa |
| Service | `vinx-node` (désactivé) | `uturoa` |
| Utilisateur | `vinx` | `uturoa` |
| Programme | `/usr/local/bin/vinx-node` | `/opt/uturoa/uturoa-node` |
| Données | `/var/lib/vinx` (gardé) | `/var/lib/uturoa` |
| Clés | `/etc/vinx` (gardé) | `/opt/uturoa/validator.json`, `/etc/uturoa/env` |

Les deux réseaux ne peuvent pas tourner **en même temps** sur une machine :
ils utilisent les mêmes ports. Chaque nœud Uturoa utilise environ 300 Mo de
mémoire (RandomX en mode léger).

Toutes les commandes se lancent **depuis Copper**, comme pour VinX : on joint
les validateurs avec `ssh root@192.168.1.X`.

## 1. Publier la version

Sur GitHub : *Releases*, *Draft a new release*, tag `v0.1.0` sur `main`,
*Publish release*. Attendre que le workflow *Release* ait joint les binaires
(onglet *Actions*, une dizaine de minutes).

## 2. Préparer le testnet sur Copper

```bash
mkdir -p ~/uturoa && cd ~/uturoa
wget https://github.com/HaitoDann/Uturoa/releases/download/v0.1.0/uturoa-v0.1.0-linux-x86_64.tar.gz
wget https://github.com/HaitoDann/Uturoa/releases/download/v0.1.0/uturoa-v0.1.0-linux-x86_64.tar.gz.sha256
sha256sum -c uturoa-v0.1.0-linux-x86_64.tar.gz.sha256
tar xzf uturoa-v0.1.0-linux-x86_64.tar.gz
cp uturoa-v0.1.0-linux-x86_64/uturoa-node .
```

Choisir le mot de passe des clés des validateurs, puis créer le testnet :

```bash
read -rs -p "Mot de passe des clés : " UTUROA_KEY_PASSWORD; echo; export UTUROA_KEY_PASSWORD
./uturoa-node devnet --dir testnet --chain-id utu-testnet-1 \
  --hosts 192.168.1.22,192.168.1.21,192.168.1.23,192.168.1.233 \
  --names Cinnamon,Amber,Obsidian,Pearl \
  --base-port 26656
```

Les blocs durent **12 secondes**, comme sur le vrai réseau. Le dossier
`testnet/` contient `genesis.json`, les clés `validator-1.json` à
`validator-4.json`, les identités réseau `d1/network.key` à `d4/network.key`
et un service `uturoa-1.service` à `uturoa-4.service` par validateur, pairs
déjà renseignés (port 26656, API 127.0.0.1:8545).

## 3. Arrêter VinX sur les validateurs

VinX est arrêté et désactivé ; rien n'est effacé.

```bash
for IP in 192.168.1.22 192.168.1.21 192.168.1.23 192.168.1.233; do
  ssh root@$IP "systemctl disable --now vinx-node"
done
```

## 4. Installer chaque validateur

Chaque validateur reçoit **sa propre** clé, son identité réseau et son
service : le numéro `N` doit correspondre à l'IP. La boucle ci-dessous fait
l'association pour les quatre ; ne pas la remplacer par un `N` fixe, sinon
les quatre machines auraient la même clé et la chaîne resterait à la
hauteur 0.

| N | Nom | IP |
|---|---|---|
| 1 | Cinnamon | 192.168.1.22 |
| 2 | Amber | 192.168.1.21 |
| 3 | Obsidian | 192.168.1.23 |
| 4 | Pearl | 192.168.1.233 |

```bash
for pair in 1:192.168.1.22 2:192.168.1.21 3:192.168.1.23 4:192.168.1.233; do
  N=${pair%%:*}; IP=${pair#*:}; echo "== validateur $N sur $IP"
  ssh root@$IP "id uturoa >/dev/null 2>&1 || useradd --system --home /var/lib/uturoa --shell /usr/sbin/nologin uturoa; mkdir -p /opt/uturoa /var/lib/uturoa /etc/uturoa"
  scp uturoa-node testnet/genesis.json root@$IP:/opt/uturoa/
  scp testnet/validator-$N.json root@$IP:/opt/uturoa/validator.json
  scp testnet/d$N/network.key root@$IP:/var/lib/uturoa/network.key
  scp testnet/uturoa-$N.service root@$IP:/etc/systemd/system/uturoa.service
  ssh root@$IP "printf 'UTUROA_KEY_PASSWORD=%s\n' '$UTUROA_KEY_PASSWORD' > /etc/uturoa/env \
    && chown -R uturoa:uturoa /var/lib/uturoa /opt/uturoa/validator.json \
    && chmod 700 /var/lib/uturoa && chmod 600 /etc/uturoa/env /opt/uturoa/validator.json /var/lib/uturoa/network.key"
done
```

Contrôle : chaque machine doit afficher **son** nom.

```bash
for IP in 192.168.1.22 192.168.1.21 192.168.1.23 192.168.1.233; do
  printf "%-15s" $IP; ssh root@$IP "grep -o 'Uturoa [A-Za-z]*' /etc/systemd/system/uturoa.service"
done
```

**Démarrer** une fois les quatre installés :

```bash
for IP in 192.168.1.22 192.168.1.21 192.168.1.23 192.168.1.233; do
  ssh root@$IP "systemctl daemon-reload && systemctl enable --now uturoa"
done
```

Les blocs commencent dès que trois validateurs sur quatre sont en marche.

## 5. Vérifier

```bash
for IP in 192.168.1.22 192.168.1.21 192.168.1.23 192.168.1.233; do
  printf "%-15s" $IP; ssh root@$IP "curl -s 127.0.0.1:8545/status" | grep -oE '"height":[0-9]+'
done
ssh root@192.168.1.22 "journalctl -u uturoa -n 20 --no-pager"
```

Attendu : quatre hauteurs égales (à un bloc près), qui augmentent de 5 par
minute ; des lignes `bloc validé ... round=0` dans les journaux.

**Si les hauteurs restent à 0** : vérifier le contrôle de la section 4. Si
deux machines portent le même nom, elles ont la même clé ; relancer la boucle
de la section 4 après `systemctl stop uturoa; rm -f /var/lib/uturoa/chain.redb`
sur les machines concernées, puis redémarrer les quatre.

## 6. Nœud public sur Copper

Copper suit la chaîne sans voter et sert l'API et l'explorateur sur
127.0.0.1:8545, là où Caddy envoie déjà le domaine.

```bash
systemctl disable --now vinx-node
id uturoa >/dev/null 2>&1 || useradd --system --home /var/lib/uturoa --shell /usr/sbin/nologin uturoa
mkdir -p /opt/uturoa /var/lib/uturoa && chown uturoa:uturoa /var/lib/uturoa
cp ~/uturoa/uturoa-node ~/uturoa/testnet/genesis.json /opt/uturoa/
PEERS=$( { grep -o -- '--peer [^ ]*' ~/uturoa/testnet/uturoa-1.service | head -1; grep -o -- '--peer [^ ]*' ~/uturoa/testnet/uturoa-2.service; } | tr '\n' ' ')
cat > /etc/systemd/system/uturoa.service <<EOF
[Unit]
Description=Nœud public Uturoa
After=network-online.target

[Service]
User=uturoa
ExecStart=/opt/uturoa/uturoa-node run --genesis /opt/uturoa/genesis.json --data-dir /var/lib/uturoa --listen /ip4/0.0.0.0/tcp/26656 --api 127.0.0.1:8545 $PEERS
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload && systemctl enable --now uturoa
curl -s 127.0.0.1:8545/status
```

L'explorateur est alors sur le domaine habituel :
<https://vinx-ledger.vinxlabs.fr/> (ou `/explorer`). Caddy n'a pas bougé.

## 7. Miner et envoyer depuis ton PC Windows

1. Télécharger `uturoa-v0.1.0-windows-x86_64.zip` depuis la release et
   l'extraire.
2. Lancer `uturoa-wallet.exe` (double-clic). Créer le portefeuille et noter
   les 12 mots.
3. Dans **Réglages**, nœud : `192.168.1.244:8545` ne fonctionne pas (l'API
   n'écoute que sur 127.0.0.1). Deux possibilités :
   - le tunnel SSH habituel, dans une fenêtre PowerShell laissée ouverte,
     puis nœud `127.0.0.1:8545` :

     ```powershell
     ssh -N -L 8545:127.0.0.1:8545 root@192.168.1.244
     ```

   - ou le domaine public, s'il est servi en HTTPS par Caddy : pas encore
     pris en charge par le portefeuille (il parle HTTP simple), donc passer
     par le tunnel.
4. Onglet **Frapper** : choisir le nombre de cœurs, **Démarrer**. Les pièces
   arrivent toutes les 50 blocs, soit **10 minutes**.

Mieux vaut miner depuis le PC que depuis un validateur : chaque part est
vérifiée par les nœuds, et un mineur sur la même machine leur prend du
processeur.

## 8. Mettre à jour

Un nœud à la fois, validateurs d'abord, Copper en dernier :

```bash
scp uturoa-node root@192.168.1.22:/opt/uturoa/uturoa-node.new
ssh root@192.168.1.22 "mv /opt/uturoa/uturoa-node.new /opt/uturoa/uturoa-node && systemctl restart uturoa"
```

Attendre que le nœud ait rattrapé (section 5) avant de passer au suivant.

## 9. Revenir à VinX

Sur chaque machine (validateurs et Copper) :

```bash
ssh root@192.168.1.22 "systemctl disable --now uturoa && systemctl enable --now vinx-node"
```

VinX repart là où il s'était arrêté : ses données et ses clés n'ont pas été
touchées. Les validateurs VinX reprennent dès que trois sur quatre sont
revenus.

## 10. Repartir de zéro avec Uturoa

```bash
for IP in 192.168.1.22 192.168.1.21 192.168.1.23 192.168.1.233; do
  ssh root@$IP "systemctl stop uturoa; rm -f /var/lib/uturoa/chain.redb"
done
```

`network.key` est gardé pour conserver les mêmes adresses réseau. Pour une
nouvelle genèse, refaire les sections 2 et 4.

## Limites

- Ce testnet n'a **aucune valeur** et peut être effacé à tout moment.
- Le mot de passe des clés est dans `/etc/uturoa/env` (root seulement). C'est
  acceptable pour un testnet, pas pour le réseau principal.
- Le portefeuille parle HTTP simple : il passe par un tunnel SSH pour joindre
  un nœud distant.
- La phase de frappe dure 6 mois de blocs : la bascule vers la preuve d'enjeu
  ne se produira pas sur ce testnet. Elle est vérifiée par les tests
  automatiques.
- Exercices de panne à faire sur ce testnet : `docs/EXERCICES-DE-PANNE.md`.
