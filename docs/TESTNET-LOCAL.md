# Lancer un réseau de test Uturoa

Ce guide explique comment faire tourner un réseau Uturoa de test, d'abord sur
un seul ordinateur, puis sur plusieurs machines (par exemple des conteneurs
Proxmox). Rien de ce qui est créé ici n'a de valeur : c'est un réseau d'essai.

## 1. Obtenir le programme

**Option A : télécharger** (recommandé). Sur la page GitHub du projet,
*Releases*, prendre l'archive de son système :

- Linux : `uturoa-vX.Y.Z-linux-x86_64.tar.gz`
- Windows : `uturoa-vX.Y.Z-windows-x86_64.zip`
- macOS (Apple Silicon) : `uturoa-vX.Y.Z-macos-arm64.tar.gz`

Vérifier l'empreinte avec le fichier `.sha256` fourni, puis extraire.

```bash
tar xzf uturoa-vX.Y.Z-linux-x86_64.tar.gz
cd uturoa-vX.Y.Z-linux-x86_64
./uturoa-node --version
```

**Option B : compiler.** Il faut Rust, CMake et un compilateur C++ (pour
RandomX).

```bash
git clone https://github.com/HaitoDann/Uturoa.git
cd Uturoa
cargo build --release -p uturoa-node
```

Le programme est alors `target/release/uturoa-node`.

## 2. Un réseau de 4 validateurs sur un seul ordinateur

### Linux ou macOS

Le mot de passe des clés de test est lu dans une variable d'environnement :

```bash
export UTUROA_KEY_PASSWORD=choisir-un-mot-de-passe
```

Créer le réseau, avec un bloc par seconde pour aller vite :

```bash
./uturoa-node devnet --dir net --block-interval-ms 1000
```

La commande crée `net/genesis.json`, les clés `net/validator-1.json` à
`validator-4.json`, et affiche **les quatre commandes de lancement**. Ouvrir
quatre terminaux et coller une commande dans chacun (sans oublier
`export UTUROA_KEY_PASSWORD=…` dans chaque terminal). Ajouter
`--api 127.0.0.1:8545` à la **première** commande pour avoir l'API et
l'explorateur.

Les blocs commencent dès que 3 validateurs sur 4 sont lancés. Ouvrir
l'explorateur : <http://127.0.0.1:8545/>.

### Windows (PowerShell)

```powershell
$env:UTUROA_KEY_PASSWORD = "choisir-un-mot-de-passe"
.\uturoa-node.exe devnet --dir net --block-interval-ms 1000
```

Puis, dans quatre fenêtres PowerShell, la même variable et une des commandes
affichées, en remplaçant `uturoa-node` par `.\uturoa-node.exe`.

## 3. Frapper et envoyer des pièces

Dans un nouveau terminal (avec `UTUROA_KEY_PASSWORD`) :

```bash
./uturoa-node new-key --out alice.json
```

La commande affiche l'adresse d'Alice (`utu1…`). Lancer la frappe pour elle :

```bash
./uturoa-node mine --api 127.0.0.1:8545 --address utu1…
```

Les pièces frappées sont partagées tous les **50 blocs** (50 secondes avec
un bloc par seconde). Ensuite, envoyer des pièces à quelqu'un :

```bash
./uturoa-node new-key --out bob.json
./uturoa-node send --api 127.0.0.1:8545 --key alice.json --to utu1… --amount 12,5 --memo "premier envoi"
```

Les soldes se vérifient dans l'explorateur (case « Rechercher un compte »).

### Avec le portefeuille

Le portefeuille de bureau fait tout cela depuis le navigateur :

```bash
./uturoa-wallet
```

Il s'ouvre dans le navigateur. Créer un portefeuille (noter les 12 mots),
puis dans **Réglages**, indiquer le nœud : `127.0.0.1:8545`. L'onglet
**Frapper** lance la frappe sur le nombre de cœurs choisi, **Envoyer** montre
les frais et le solde restant avant de confirmer.

Le portefeuille est rangé dans `~/.uturoa-wallet` (Windows :
`C:\Users\<vous>\.uturoa-wallet`). Si un mineur tourne sur la même machine
que le nœud, les blocs ralentissent : le nœud vérifie chaque part reçue.

## 4. Un réseau sur plusieurs machines

Exemple avec 4 machines : 192.168.1.21 à 192.168.1.24.

1. **Sur une seule machine**, créer le réseau :

   ```bash
   export UTUROA_KEY_PASSWORD=choisir-un-mot-de-passe
   ./uturoa-node devnet --dir net
   ```

   Sans `--block-interval-ms`, les blocs durent 12 secondes, comme sur le
   vrai réseau.

2. **Copier sur la machine i** : `net/genesis.json`, `net/validator-i.json`
   et le dossier `net/di` (il contient l'identité réseau `network.key`, dont
   dépend l'adresse `/p2p/…` affichée par `devnet`).

   ```bash
   scp -r net/genesis.json net/validator-2.json net/d2 root@192.168.1.22:/opt/uturoa/
   ```

3. **Ouvrir le port 30333** (TCP) entre les machines.

4. **Lancer chaque nœud** en remplaçant `127.0.0.1` par les vraies adresses.
   Sur la machine 2 par exemple :

   ```bash
   ./uturoa-node run --genesis genesis.json --key validator-2.json --data-dir d2 \
     --listen /ip4/0.0.0.0/tcp/30333 \
     --peer /ip4/192.168.1.21/tcp/30333/p2p/<id-1> \
     --peer /ip4/192.168.1.23/tcp/30333/p2p/<id-3> \
     --peer /ip4/192.168.1.24/tcp/30333/p2p/<id-4>
   ```

   Les identifiants `<id-i>` sont ceux affichés par `devnet` (la partie après
   `/p2p/`).

5. **API** : ne l'ouvrir que sur l'adresse locale (`--api 127.0.0.1:8545`) et
   y accéder par un tunnel SSH depuis son PC :

   ```bash
   ssh -N -L 8545:127.0.0.1:8545 root@192.168.1.21
   ```

   puis ouvrir <http://127.0.0.1:8545/> sur le PC.

### Faire tourner le nœud en service (Linux, systemd)

Fichier `/etc/systemd/system/uturoa.service` :

```ini
[Unit]
Description=Nœud Uturoa
After=network-online.target

[Service]
WorkingDirectory=/opt/uturoa
Environment=UTUROA_KEY_PASSWORD=choisir-un-mot-de-passe
ExecStart=/opt/uturoa/uturoa-node run --genesis genesis.json --key validator-2.json --data-dir d2 --listen /ip4/0.0.0.0/tcp/30333 --api 127.0.0.1:8545 --peer … --peer … --peer …
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```bash
systemctl daemon-reload
systemctl enable --now uturoa
journalctl -u uturoa -f
```

Pour un vrai réseau, le mot de passe ne doit pas rester en clair dans ce
fichier : ce réglage n'est acceptable que pour un réseau de test.

## 5. Vérifier que tout va bien

- Les journaux affichent `bloc validé` à chaque bloc, avec `round=0` quand
  tout va bien.
- Sur chaque nœud, après l'avoir arrêté :

  ```bash
  ./uturoa-node info --genesis genesis.json --data-dir d2
  ```

  À la même hauteur, tous les nœuds doivent afficher **la même racine
  d'état**.
- Arrêter un validateur sur quatre : la chaîne continue. En arrêter deux : elle
  s'arrête (pas de quorum) et repart dès qu'un des deux revient. C'est le
  comportement attendu.

## 6. Limites connues

- `mine`, `send` et `new-key` sont des outils de test en ligne de commande ;
  le portefeuille de bureau est `uturoa-wallet`. Le portefeuille mobile
  viendra plus tard.
- La frappe de test utilise le vrai RandomX en mode léger : un PC ordinaire
  trouve quelques dizaines de parts par seconde à difficulté 1. La difficulté
  s'ajuste toutes les 50 blocs.
- Un nœud qui arrive tard rattrape les blocs ou installe un instantané d'état.
  Après la bascule vers la preuve d'enjeu (en conditions réelles, après six
  mois), il lui faudra l'empreinte d'un bloc de confiance (`--trusted-block`).
- Repartir de zéro : arrêter les nœuds et supprimer leurs dossiers `d1` à
  `d4` (pas `network.key` si on veut garder les mêmes adresses réseau).
