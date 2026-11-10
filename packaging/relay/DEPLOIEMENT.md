# Mettre le relais de ChatonLM en ligne

Le relais (`chatonlm-relay`, crates `src-tauri/crates/relay-proto` et `relay-server`) sert la recherche web
de l'app sans compte : il garde les clés des moteurs de recherche (Brave, Mojeek en secours), n'admet que
des installations inscrites par preuve de travail, applique 50 recherches par jour et par installation et un
plafond de budget mensuel. Il ne garde ni adresse IP, ni requête, ni résultat.

Ce guide vaut pour un serveur Ubuntu avec nginx et certbot, qui héberge déjà d'autres sites : le relais y est
cloisonné (compte jetable de systemd, aucun droit, base à part, journal à part, mémoire et processeur bornés)
et ne touche à aucun autre service. Remplacer `<serveur>` par l'accès SSH du serveur. Les commandes passent
par `sudo -n` (aucune invite) : elles se lancent aussi d'ici par `ssh <serveur> '…'`, sauf celle des secrets
(§ 3), que le propriétaire du serveur colle lui-même.

Ce que le relais pose sur le serveur, et rien d'autre :

| Chemin                                                              | Contenu                                   |
| ------------------------------------------------------------------- | ----------------------------------------- |
| `/usr/local/lib/chatonlm-relay/chatonlm-relay`                      | binaire                                   |
| `/etc/chatonlm-relay/config.toml`, `secrets.toml`                   | réglages (lisibles), secrets (root, 0600) |
| `/etc/systemd/system/chatonlm-relay.service`                        | unité                                     |
| `/etc/systemd/journald@chatonlm-relay.conf`                         | journal à part (14 jours)                 |
| `/var/lib/private/chatonlm-relay/` (lien `/var/lib/chatonlm-relay`) | base SQLite (clés publiques, budget)      |
| `/etc/nginx/sites-{available,enabled}/chatonlm-relay.conf`          | site nginx `relay.chatonlm.com`           |
| `/var/log/nginx/chatonlm-relay.error.log`                           | erreurs graves de nginx pour ce site      |
| `/etc/letsencrypt/{live,archive,renewal}/relay.chatonlm.com*`       | certificat (certbot)                      |

## 1. Ce qu'il faut savoir du serveur (lecture seule)

```bash
uname -m                                   # x86_64 : cible x86_64-unknown-linux-musl (aarch64 : aarch64-unknown-linux-musl)
. /etc/os-release && echo "$PRETTY_NAME"   # version encore suivie par Ubuntu ? Sinon, le signaler avant tout
systemctl --version | head -1              # 248 ou plus : toutes les directives de l'unité sont comprises
stat -fc %T /sys/fs/cgroup                 # cgroup2fs : MemoryMax, MemorySwapMax et CPUQuota appliqués
nproc; free -m                             # le relais prend 1 cœur et 256 Mio au plus
nginx -v                                   # 1.25.1 ou plus : « http2 on; » (sinon « listen 443 ssl http2; »)
ls /etc/nginx/sites-enabled /etc/nginx/conf.d   # où sont déclarés les sites (sites-enabled attendu)
sudo -n ss -ltnp | grep -E ':(80|443|8790)\b'   # 8790 doit être libre (le service du site écoute sur 8787)
certbot --version
ip -6 addr show scope global               # IPv6 présente : enregistrement AAAA et « listen [::]:80 »
getent hosts api.search.brave.com api.mojeek.com api.ceramic.ai api.linkup.so   # fournisseurs (et banc R0)
```

Relevé du 10 nov. 2026 sur le serveur de chatonlm.com (lecture seule) : x86_64, Ubuntu 26.04.1 LTS, systemd 259,
cgroup2fs, 4 cœurs et 7,7 Gio, nginx 1.28.3, sites dans `sites-enabled/` (liens vers `sites-available/`),
`limit_req_zone` `chatonlm_api` déjà prise par le site (celle du relais s'appelle `chatonlm_relay`), 8790 libre
(8787 : service du site ; 3000 : autre site, fermé par ufw), ufw actif (22, 80, 443), certbot 4.0.0, IPv6
présente, les quatre fournisseurs résolus, `relay.chatonlm.com` **pas encore dans le DNS**.

Pourquoi un binaire **musl** : il est statique (ni glibc, ni OpenSSL, ni SQLite du système ; `ring` et SQLite y
sont compilés) et tourne sur tout Linux x86_64, quelle que soit la version d'Ubuntu. Un binaire glibc construit
sur une distribution plus récente que le serveur (Arch, par exemple) refuserait de démarrer (« GLIBC_2.xx not
found ») ; si l'on tient à glibc, le construire sur une version d'Ubuntu au plus égale à celle du serveur
(`ldd --version` des deux côtés).

## 2. Ce qu'il faut avant

- Un enregistrement DNS `relay.chatonlm.com` (A, et AAAA si le serveur a de l'IPv6) vers le serveur, déjà
  visible : `getent hosts relay.chatonlm.com` (depuis le serveur) doit rendre son adresse avant certbot.
- Une clé Brave Search API (offre « Search ») et, pour le secours, une clé Mojeek. Elles ne transitent que par
  le terminal du propriétaire du serveur (§ 3), jamais par un message, un fichier du dépôt ou un journal.
- Le binaire `chatonlm-relay` pour `x86_64-unknown-linux-musl`. Il est construit par le workflow
  `.github/workflows/relay.yml` (essais du relais sur la cible musl, contrôles « statique, sans faux
  fournisseurs », démarrage et `/health`) : artefact `chatonlm-relay-x86_64-linux-musl` d'un passage
  (binaire, `chatonlm-relay.sha256` et les fichiers de ce dossier), ou Release annexe `relay-v<version>`
  (`--latest=false`) quand un tag `relay-v*` est poussé :

  ```bash
  gh release download relay-v0.1.0 --repo Elwynn18/chatonlm -D .tools/ci/relay   # ou : gh run download <id> -n chatonlm-relay-x86_64-linux-musl -D .tools/ci/relay
  (cd .tools/ci/relay && sha256sum -c chatonlm-relay.sha256)                        # l'empreinte du § 3
  ```

  Ou construit ici :

  ```bash
  rustup target add x86_64-unknown-linux-musl   # une fois
  cargo build --release --locked --manifest-path src-tauri/Cargo.toml \
    -p relay-server --bin chatonlm-relay --target x86_64-unknown-linux-musl
  file "$CARGO_TARGET_DIR/x86_64-unknown-linux-musl/release/chatonlm-relay"   # « statically linked »
  sha256sum "$CARGO_TARGET_DIR/x86_64-unknown-linux-musl/release/chatonlm-relay"
  ```

  `ring` et SQLite compilent du C : il faut `musl-gcc` (paquet `musl` sous Arch, `musl-tools` sous Ubuntu).
  Sans la fonctionnalité `fake` (faux fournisseurs), jamais dans le binaire déployé.

## 3. Fichiers sur le serveur

Jamais par `/tmp` : le serveur héberge d'autres sites, donc d'autres comptes, et un fichier de `/tmp` créé
d'avance par l'un d'eux pourrait être changé entre le contrôle et l'installation. Les fichiers passent par un
dossier privé du compte SSH (0700), et l'empreinte est **comparée** sur le binaire une fois installé, à root,
avant tout démarrage. `<empreinte>` : celle relevée à la construction (§ 2).

```bash
ssh <serveur> 'install -d -m 700 ~/chatonlm-relay-upload'
scp chatonlm-relay packaging/relay/{chatonlm-relay.service,journald@chatonlm-relay.conf,nginx-relay.chatonlm.com.conf,config.example.toml} <serveur>:chatonlm-relay-upload/
```

Puis, sur le serveur (`set -e` : la suite s'arrête si l'empreinte diffère) :

```bash
set -e; U=~/chatonlm-relay-upload; L=/usr/local/lib/chatonlm-relay
sudo -n install -d -m 755 "$L" /etc/chatonlm-relay
sudo -n install -m 755 "$U/chatonlm-relay" "$L/chatonlm-relay"
echo "<empreinte>  $L/chatonlm-relay" | sha256sum -c   # « OK », sinon : sudo -n rm "$L/chatonlm-relay"
sudo -n install -m 644 "$U/config.example.toml" /etc/chatonlm-relay/config.toml
sudo -n install -m 644 "$U/chatonlm-relay.service" /etc/systemd/system/chatonlm-relay.service
sudo -n install -m 644 "$U/journald@chatonlm-relay.conf" /etc/systemd/journald@chatonlm-relay.conf
set +e
```

(Le fichier nginx attend dans le dossier privé : il n'est posé qu'au § 5, après la sauvegarde de `/etc/nginx`.)

**Secrets : le propriétaire du serveur colle lui-même** ces lignes dans sa propre session SSH (`ssh <serveur>`).
Le fichier est d'abord créé vide, à root en 0600 (`tee` garde ce mode) ; les clés sont tapées sans écho et
n'entrent ni dans l'historique du shell, ni dans la liste des processus (`printf` est interne à bash) ; la clé
du sceau des défis est tirée sur place. Une clé qu'on n'a pas : Entrée seule (fournisseur sauté). Ceramic et
Linkup ne servent qu'au banc R0 (§ 4 bis). Les mêmes lignes servent à changer de clé (puis
`sudo systemctl restart chatonlm-relay`).

```bash
sudo install -m 600 -o root -g root /dev/null /etc/chatonlm-relay/secrets.toml
read -rsp 'Clé Brave : ' BRAVE; echo; read -rsp 'Clé Mojeek : ' MOJEEK; echo
read -rsp 'Clé Ceramic (banc) : ' CERAMIC; echo; read -rsp 'Clé Linkup (banc) : ' LINKUP; echo
printf 'brave_api_key = "%s"\nmojeek_api_key = "%s"\nceramic_api_key = "%s"\nlinkup_api_key = "%s"\nchallenge_key = "%s"\n' \
  "$BRAVE" "$MOJEEK" "$CERAMIC" "$LINKUP" "$(openssl rand -hex 32)" | sudo tee /etc/chatonlm-relay/secrets.toml >/dev/null
unset BRAVE MOJEEK CERAMIC LINKUP
sudo ls -l /etc/chatonlm-relay/secrets.toml   # -rw------- root root
```

systemd en donne une copie au seul relais (`LoadCredential=`). Vérifier ensuite, sans démarrer (la commande ne
répond que « configuration OK » ou l'erreur, jamais le contenu d'une clé) :

```bash
sudo -n /usr/local/lib/chatonlm-relay/chatonlm-relay check \
  --config /etc/chatonlm-relay/config.toml --secrets /etc/chatonlm-relay/secrets.toml
sudo -n systemd-analyze verify /etc/systemd/system/chatonlm-relay.service
```

## 4. Service

```bash
sudo -n systemctl daemon-reload
sudo -n systemctl enable --now chatonlm-relay
systemctl status chatonlm-relay --no-pager
curl -s http://127.0.0.1:8790/health                          # {"status":"ok"}
sudo -n journalctl --namespace=chatonlm-relay -n 20 --no-pager  # journal à part, 14 jours au plus
sudo -n systemd-analyze security chatonlm-relay --no-pager | tail -1   # viser « OK » ou mieux
```

## 4 bis. Banc R0 des fournisseurs (sur le serveur)

Compare Brave, Mojeek, Ceramic et Linkup sur un jeu fixe de 60 requêtes (20 en français, 20 en espagnol,
20 en anglais ; actualité, pratique, technique, local) : latence (p50, p90, max), résultats par réponse,
réponses vides, titres et extraits présents, extraits supplémentaires de Brave (`extra_snippets` : demandés,
puis abandonnés si l'offre les refuse, ce que le résumé dit), domaines distincts et domaines du pays (`.fr`,
`.es`) dans les 5 premiers, accord entre fournisseurs, coût estimé. Les clés ne sortent pas de
`secrets.toml` : le banc tourne sur le serveur, sous un compte jetable de systemd qui reçoit le fichier comme
le service (`LoadCredential=`), sans aucun droit d'écriture. Le résumé ne contient ni clé, ni résultat
(conditions de Brave : pas de stockage), ni requête d'utilisateur : il peut être copié ici. Le service n'a pas
besoin de tourner. Coût d'un passage complet : moins de 1 € (60 requêtes par fournisseur), 2 à 4 minutes.

```bash
R='sudo -n systemd-run --quiet --pipe --wait --collect -p DynamicUser=yes \
  -p LoadCredential=secrets:/etc/chatonlm-relay/secrets.toml -p ProtectSystem=strict -p ProtectHome=yes \
  -p PrivateTmp=yes -p NoNewPrivileges=yes -p MemoryMax=256M -p CPUQuota=100% \
  /usr/local/lib/chatonlm-relay/chatonlm-relay bench --config /etc/chatonlm-relay/config.toml'
ssh <serveur> "$R --queries 12"                                   # essai court (3 langues × 4 catégories)
ssh <serveur> "$R" > .tools/research/103-banc-R0.txt                # passage complet, résumé gardé ici
ssh <serveur> "$R --queries 12 --show"                             # 3 premiers titres et domaines, à l'écran seulement
```

`--show` sert à noter la pertinence à la main (affiché sur la sortie d'erreur, jamais écrit). Un fournisseur
sans clé est sauté ; aucune clé : « no provider key in the secrets file », rien n'est envoyé.

## 5. nginx et certificat

Avant tout changement, sauvegarder la configuration :

```bash
B=/etc/nginx.bak-$(date +%F-%H%M); sudo -n cp -a /etc/nginx "$B"; echo "$B"
```

```bash
sudo -n install -m 644 ~/chatonlm-relay-upload/nginx-relay.chatonlm.com.conf /etc/nginx/sites-available/chatonlm-relay.conf
sudo -n ln -s /etc/nginx/sites-available/chatonlm-relay.conf /etc/nginx/sites-enabled/chatonlm-relay.conf
sudo -n nginx -t && sudo -n systemctl reload nginx
sudo -n certbot --nginx -n --cert-name relay.chatonlm.com -d relay.chatonlm.com --redirect --hsts
```

`--cert-name` et un seul `-d` : un certificat neuf pour ce seul nom, celui des autres sites n'est ni étendu ni
renouvelé. Si `/etc/nginx/sites-enabled/` n'existe pas, poser le fichier dans `/etc/nginx/conf.d/` (sans le
lien) ; si `nginx -t` refuse `listen [::]:80` (IPv6 coupé), retirer cette ligne.

Ajouter ensuite `http2 on;` sous `listen 443 ssl;` dans `/etc/nginx/sites-available/chatonlm-relay.conf`
(nginx avant 1.25.1 : remplacer plutôt par `listen 443 ssl http2;`), puis `sudo -n nginx -t` et rechargement.

Avec `--redirect`, certbot sort `listen 80` du bloc d'origine et crée un **second** bloc `server` (celui qui
contient `return 404; # managed by Certbot`), sans `access_log off;` : les accès en HTTP simple partiraient,
adresse IP comprise, dans le journal général de nginx. Ajouter `access_log off;` dans ce bloc, sous sa ligne
`server_name`, puis :

```bash
sudo -n grep -c 'access_log off;' /etc/nginx/sites-available/chatonlm-relay.conf   # 2 : un par bloc server
sudo -n nginx -t && sudo -n systemctl reload nginx
```

Contrôles : rien d'autre que les fichiers du relais n'a changé, et le relais répond de l'extérieur (`$B` ne
survit pas d'une session SSH à l'autre : reprendre le chemin affiché par la sauvegarde).

```bash
sudo -n diff -rq "$B" /etc/nginx    # seulement : sites-available/chatonlm-relay.conf, sites-enabled/chatonlm-relay.conf
curl -s https://relay.chatonlm.com/health                     # {"status":"ok"}
curl -s -o /dev/null -w '%{http_code}\n' https://relay.chatonlm.com/   # 404 : rien d'autre n'est servi
curl -sI https://chatonlm.com | head -1                       # le site, inchangé
```

Si `nginx -t` échoue à n'importe quelle étape : ne pas recharger, remettre la sauvegarde
(`sudo -n rm -rf /etc/nginx && sudo -n cp -a "$B" /etc/nginx && sudo -n nginx -t`).

## 6. Mettre à jour

Par le dossier privé du § 3 ; le nouveau binaire est posé à côté de l'ancien, son empreinte comparée, puis il
le remplace d'un coup (`mv`). Une empreinte qui diffère arrête tout (`&&`), l'ancien binaire reste en place.

```bash
scp chatonlm-relay <serveur>:chatonlm-relay-upload/
ssh <serveur> 'L=/usr/local/lib/chatonlm-relay; sudo -n install -m 755 ~/chatonlm-relay-upload/chatonlm-relay "$L/chatonlm-relay.new" && echo "<empreinte>  $L/chatonlm-relay.new" | sha256sum -c && sudo -n mv "$L/chatonlm-relay.new" "$L/chatonlm-relay" && sudo -n systemctl restart chatonlm-relay && curl -s http://127.0.0.1:8790/health'
```

Les clés inscrites, le budget du mois et la dépense du jour sont dans `/var/lib/chatonlm-relay/relay.sqlite` :
un redémarrage ne les perd pas. Les quotas du jour par installation et par réseau (en mémoire) repartent de zéro.

## 7. Exploitation

- Surveillance de l'extérieur : le workflow `relay.yml` interroge chaque jour `https://relay.chatonlm.com/health`
  (réponse `{"status":"ok"}`) et vérifie que le certificat vaut encore 14 jours, dès que la variable du dépôt
  `RELAY_HEALTH_URL` est posée (commande ci-dessous) ; un échec envoie le courriel de GitHub Actions au
  propriétaire du dépôt. Lancement à la main : `gh workflow run relay.yml`. Aucun courriel n'est envoyé du
  serveur (le service du formulaire du site n'est pas touché) : le seuil de 80 % du budget se lit au journal,
  `sudo -n journalctl --namespace=chatonlm-relay -p warning --since -7d --no-pager`.

  ```bash
  gh variable set RELAY_HEALTH_URL --repo Elwynn18/chatonlm --body https://relay.chatonlm.com/health
  ```

- Dépense du mois et recherches par fournisseur (aucune donnée personnelle) :
  `sudo -n /usr/local/lib/chatonlm-relay/chatonlm-relay stats --config /etc/chatonlm-relay/config.toml`.
  En root, car la base appartient au compte du service ; la commande l'ouvre en lecture seule et n'y crée aucun
  fichier (un `-wal` ou un `-shm` à root empêcherait le service d'écrire).
- Seuils du budget : une ligne au journal à 80 % du plafond, puis une au plafond (`budget_exhausted` jusqu'au
  1er du mois suivant, UTC). Enveloppe du jour (`daily_share`, 3 jours moyens : 3 € par jour pour 30 €) : une
  ligne au journal quand elle est atteinte, `budget_exhausted` jusqu'à minuit UTC. Une vague d'abus coûte ainsi
  une journée de recherche, pas le reste du mois.
- Limites par réseau en cascade (`[quota]`, `[étroit, moyen, large]` : IPv6 /56, /48, /32 ; IPv4 /24, /16).
  L'adresse du client est lue dans `X-Real-IP`, posé par nginx, seulement pour une connexion venue de
  `127.0.0.1` : tout processus local du serveur (le service du site, l'autre site) peut donc se dire n'importe
  quelle adresse. Ces limites ne valent que contre l'extérieur ; le quota par installation, l'enveloppe du jour et
  le plafond du mois tiennent quoi qu'il arrive.
- Interrupteurs (`config.toml`, puis `chatonlm-relay check …` et `systemctl restart`) : `paused = true`
  (recherche en pause), `banned_keys`, `min_app_version`, `monthly_limit_eur`, ordre des fournisseurs.
- Changer de fournisseur ou de prix : `[providers]`, sans mise à jour de l'app.

## 8. Retirer le relais

```bash
sudo -n rm /etc/nginx/sites-enabled/chatonlm-relay.conf && sudo -n nginx -t && sudo -n systemctl reload nginx
sudo -n systemctl disable --now chatonlm-relay
sudo -n certbot delete -n --cert-name relay.chatonlm.com
```

La base (`/var/lib/private/chatonlm-relay`), le dossier d'envoi (`~/chatonlm-relay-upload`) et les fichiers du
tableau du début s'effacent ensuite à la main ;
aucun autre service n'en dépend.
