# ChatonLM: Ollama model dir
# Script de ChatonLM (Settings > Model location), lancé en root par :
#   pkexec --disable-internal-agent /bin/sh -c "<ce texte>" chatonlm-models \
#     ollama.service <move|keep> <ancien dossier|-> <nouveau dossier>
# La boîte polkit affiche « Authentication is needed to run `<38 premiers
# caractères> ... <37 derniers>' as the super user » : la 1re ligne ci-dessus
# en fait le début, le nouveau dossier (dernier argument) la fin.
#
# POSIX (dash sur Debian et Ubuntu) ; suppose coreutils, util-linux (setpriv)
# et findutils GNU, comme toute distribution à systemd.
#
# Sortie standard, une ligne par événement, lue par l'app :
#   step <prepare|stop|move|config|start|cleanup>
#   total <octets à copier>          (0 : renommages seulement)
#   copy <octets> <chemin relatif>   avant chaque fichier copié
#   restored                         après une restauration réussie
#   result restarted=<yes|no> renamed=<n> copied=<n> skipped=<n> cleanup=<none|ok|failed>
# Codes de sortie :
#   0      fait ;
#   10-17  refus ou échec avant toute modification (11 aussi pour un service
#          qui tourne en root, 12 pour un dossier existant d'un autre compte
#          ou un .ollama du service qui n'est pas un dossier, 15 si le
#          service ne peut pas créer son .ollama) ;
#   20-24  échec, état précédent restauré (« restored ») ;
#   30     échec et restauration incomplète (détail sur la sortie d'erreur).
#   126 et 127 ne viennent que de pkexec (annulation ; refus ou pas d'agent).

set -u
umask 022
# L'app peut se fermer pendant l'opération : on va jusqu'au bout.
trap '' HUP PIPE
PATH=/usr/sbin:/usr/bin:/sbin:/bin
LC_ALL=C
export PATH LC_ALL

E_ARGS=10 E_UNIT=11 E_TARGET=12 E_NOTEMPTY=13 E_SPACE=14 E_WRITE=15 E_ENVFILE=16 E_INTERNAL=17
E_STOP=20 E_MOVE=21 E_CONFIG=22 E_OVERRIDDEN=23 E_START=24 E_ROLLBACK=30

say() { printf '%s\n' "$*"; }
err() { printf 'chatonlm: %s\n' "$*" >&2; }
die() {
	code=$1
	shift
	err "$*"
	exit "$code"
}

# Bancs d'essai seulement : hors root. Sous pkexec, le script est root et
# l'environnement réduit à PATH, HOME, USER, LANG... : ces variables n'y
# arrivent jamais. CALLER : uid de qui a lancé pkexec (PKEXEC_UID, posé par
# pkexec lui-même).
ROOT='' FORCE_COPY='' TEST_HOME='' CALLER=${PKEXEC_UID:-}
if [ "$(id -u)" != 0 ]; then
	[ -n "${CHATONLM_TEST_ROOT:-}" ] || die "$E_ARGS" "must run as root"
	ROOT=$CHATONLM_TEST_ROOT
	PATH=$CHATONLM_TEST_BIN:$PATH
	FORCE_COPY=${CHATONLM_TEST_FORCE_COPY:-}
	TEST_HOME=${CHATONLM_TEST_HOME:-}
	CALLER=${CHATONLM_TEST_CALLER:-$(id -u)}
fi
case $CALLER in '' | *[!0-9]*) die "$E_ARGS" "unknown caller (run through pkexec)" ;; esac

[ $# -eq 4 ] || die "$E_ARGS" "usage: ollama.service move|keep OLD|- NEW"
UNIT=$1 MODE=$2 OLD=$3 NEW=$4
[ "$UNIT" = ollama.service ] || die "$E_ARGS" "unsupported unit: $UNIT"
case $MODE in move | keep) ;; *) die "$E_ARGS" "unknown mode: $MODE" ;; esac

# Absolu, sans caractère que systemd interpréterait (" \ : $ `) ni caractère
# de contrôle.
plain_path() {
	case $1 in /*) ;; *) return 1 ;; esac
	case $1 in *[\"\\:\$\`]*) return 1 ;; esac
	[ "$(printf '%s' "$1" | tr -d '\001-\037\177')" = "$1" ]
}

# Et canonique (ni lien symbolique, ni . ou .., ni / final).
clean_path() {
	plain_path "$1" || return 1
	[ "$(realpath -m -- "$1")" = "$1" ]
}

# Dossiers du système, et racines dont on changerait le propriétaire.
forbidden() {
	case $1 in
	/run/media/?*/?*) return 1 ;;
	/ | /home | /media | /mnt | /opt | /srv | /var | /var/lib) return 0 ;;
	/bin | /bin/* | /boot | /boot/* | /dev | /dev/* | /efi | /efi/* | /etc | /etc/* | \
		/lib | /lib/* | /lib32 | /lib32/* | /lib64 | /lib64/* | /nix | /nix/* | \
		/proc | /proc/* | /root | /root/* | /run | /run/* | /sbin | /sbin/* | \
		/snap | /snap/* | /sys | /sys/* | /tmp | /tmp/* | /usr | /usr/* | \
		/var/cache | /var/cache/* | /var/lock | /var/lock/* | /var/log | /var/log/* | \
		/var/run | /var/run/* | /var/tmp | /var/tmp/*) return 0 ;;
	esac
	return 1
}

# Sous un dossier que ProtectHome= masque.
in_home() {
	case $1 in /home/* | /root/* | /run/user/*) return 0 ;; esac
	if [ -n "$TEST_HOME" ]; then
		case $1 in "$TEST_HOME"/*) return 0 ;; esac
	fi
	return 1
}

clean_path "$NEW" || die "$E_ARGS" "invalid folder: $NEW"
forbidden "$NEW" && die "$E_TARGET" "system folder: $NEW"
if [ "$OLD" != - ]; then
	clean_path "$OLD" || die "$E_ARGS" "invalid folder: $OLD"
fi
command -v setpriv > /dev/null || die "$E_INTERNAL" "setpriv (util-linux) is missing"

# --- Service : faits relus ici (root), pas reçus de l'app ---------------------
prop() { systemctl show -p "$1" --value -- "$UNIT" 2> /dev/null; }

# Éléments de `systemctl show -p Environment`, un par ligne : systemd cite
# (") ceux qui contiennent une espace ou ( ) ' ; ..., et y échappe " \ ` $.
env_items() {
	prop Environment | awk '{
		item = ""; any = 0; q = 0
		for (i = 1; i <= length($0); i++) {
			c = substr($0, i, 1)
			if (q && c == "\\") { i++; item = item substr($0, i, 1) }
			else if (c == "\"") { q = !q; any = 1 }
			else if (!q && c == " ") { if (any) print item; item = ""; any = 0 }
			else { item = item c; any = 1 }
		}
		if (any) print item
	}'
}

# Dossier .ollama du service (clé id_ed25519 d'Ollama) : $HOME/.ollama, avec
# le HOME que systemd lui donne (Environment=, sinon répertoire de User=) ;
# rien sans HOME (service root sans User=, que systemd laisse sans HOME).
service_ollama_dir() {
	h=$(env_items | sed -n 's/^HOME=//p' | tail -n 1)
	if [ -z "$h" ] && [ -n "$(prop User)" ]; then
		h=$(getent passwd "$SUSER" | cut -d: -f6)
	fi
	h=${h%/}
	[ -z "$h" ] || printf '%s\n' "$h/.ollama"
}

[ "$(prop LoadState)" = loaded ] || die "$E_UNIT" "$UNIT is not installed"
[ "$(prop DynamicUser)" != yes ] || die "$E_UNIT" "DynamicUser= is not supported"
SUSER=$(prop User)
[ -n "$SUSER" ] || SUSER=root
SGROUP=$(prop Group)
[ -n "$SGROUP" ] || SGROUP=$(id -gn -- "$SUSER" 2> /dev/null) || die "$E_UNIT" "unknown user: $SUSER"
SUID=$(id -u -- "$SUSER" 2> /dev/null) || die "$E_UNIT" "unknown user: $SUSER"
SGID=$(getent group "$SGROUP" | cut -d: -f3)
case $SUID in '' | *[!0-9]*) die "$E_UNIT" "unknown user: $SUSER" ;; esac
case $SGID in '' | *[!0-9]*) die "$E_UNIT" "unknown group: $SGROUP" ;; esac
# En root, le service lirait, écrirait et élaguerait un dossier qu'un compte
# sans privilège peut remplacer : l'app donne alors les commandes à la place.
[ "$SUID" != 0 ] || die "$E_UNIT" "$UNIT runs as root: change its folder in a terminal"
PHOME=$(prop ProtectHome)
PSYS=$(prop ProtectSystem)
WAS_ACTIVE=''
case $(systemctl is-active -- "$UNIT" 2> /dev/null) in
active | activating | reloading) WAS_ACTIVE=1 ;;
esac

WORK=$(mktemp -d "$ROOT/run/chatonlm-models.XXXXXX") || die "$E_INTERNAL" "mktemp failed"
trap 'rm -rf -- "$WORK"' EXIT

# Un EnvironmentFile= l'emporte sur Environment= (systemd.exec(5)) : refus.
prop EnvironmentFiles | sed 's/ (ignore_errors=[a-z]*)$//' > "$WORK/envfiles"
while IFS= read -r f; do
	if [ -f "$f" ] && grep -Eq '^[[:space:]]*(export[[:space:]]+)?OLLAMA_MODELS=' "$f"; then
		die "$E_ENVFILE" "OLLAMA_MODELS is set in $f"
	fi
done < "$WORK/envfiles"

# --- Nouveau dossier : vide, ou dossier de modèles d'Ollama -------------------
kind() {
	k=empty
	for e in "$1"/* "$1"/.[!.]* "$1"/..?*; do
		[ -e "$e" ] || [ -L "$e" ] || continue
		case ${e##*/} in
		blobs | manifests | metadata) k=models ;;
		*)
			echo other
			return
			;;
		esac
	done
	echo "$k"
}
if [ -e "$NEW" ] || [ -L "$NEW" ]; then
	{ [ -d "$NEW" ] && [ ! -L "$NEW" ]; } || die "$E_TARGET" "not a folder: $NEW"
	NEW_EXISTS=1
	[ "$(kind "$NEW")" != other ] || die "$E_NOTEMPTY" "folder not empty: $NEW"
	# Il sera donné au service : seulement un dossier de l'appelant ou du
	# service (pas /var/empty, /srv/x... de root ou d'un autre compte).
	NEW_UID=$(stat -c %u -- "$NEW")
	[ "$NEW_UID" = "$CALLER" ] || [ "$NEW_UID" = "$SUID" ] ||
		die "$E_TARGET" "folder of another user: $NEW"
	AT=$NEW
else
	NEW_EXISTS=''
	AT=${NEW%/*}
	[ -n "$AT" ] || AT=/
	[ -d "$AT" ] || die "$E_TARGET" "missing folder: $AT"
fi
DEV_NEW=$(stat -c %d -- "$AT")

# --- Plan : renommer (même système de fichiers) ou copier ---------------------
RENAME='' COPY=''
if [ "$MODE" = move ] && [ "$OLD" != - ]; then
	[ "$OLD" != "$NEW" ] || die "$E_TARGET" "same folder"
	case $NEW/ in "$OLD"/*) die "$E_TARGET" "new folder inside the old one" ;; esac
	case $OLD/ in "$NEW"/*) die "$E_TARGET" "old folder inside the new one" ;; esac
	for x in blobs manifests metadata; do
		[ ! -L "$OLD/$x" ] || die "$E_TARGET" "symbolic link: $OLD/$x"
		[ -d "$OLD/$x" ] || continue
		if [ -z "$FORCE_COPY" ] && [ ! -e "$NEW/$x" ] &&
			[ "$(stat -c %d -- "$OLD/$x")" = "$DEV_NEW" ]; then
			RENAME="$RENAME $x"
		else
			COPY="$COPY $x"
		fi
	done
fi

# Place (copies vers un autre système de fichiers ; marge de 100 Mio).
need=0
for x in $COPY; do
	if [ -z "$FORCE_COPY" ] && [ "$(stat -c %d -- "$OLD/$x")" = "$DEV_NEW" ]; then
		continue # liens physiques
	fi
	need=$((need + $(du -sk -- "$OLD/$x" | cut -f1)))
done
if [ "$need" -gt 0 ]; then
	avail=$(stat -f -c '%a %S' -- "$AT" | awk '{ printf "%.0f\n", $1 * $2 / 1024 }')
	[ $((need + 102400)) -le "$avail" ] ||
		die "$E_SPACE" "not enough space: ${need} KiB needed, ${avail} KiB free"
fi

SANDBOX=''
if in_home "$NEW"; then
	case $PHOME in yes | true | tmpfs) SANDBOX=bind ;; read-only) SANDBOX=rw ;; esac
fi
[ -n "$SANDBOX" ] || [ "$PSYS" != strict ] || SANDBOX=rw

# Avec ProtectHome=tmpfs, le reste de /home est un dossier vide en lecture
# seule : le dossier .ollama du service (clé id_ed25519, sans laquelle
# `ollama serve` ne démarre pas) est rendu lui aussi, s'il est sous /home et
# hors du nouveau dossier.
OLLAMA_DIR=''
if [ "$SANDBOX" = bind ]; then
	OLLAMA_DIR=$(service_ollama_dir)
	case $OLLAMA_DIR/ in "$NEW"/*) OLLAMA_DIR='' ;; esac
	if [ -n "$OLLAMA_DIR" ] && ! in_home "$OLLAMA_DIR"; then OLLAMA_DIR=''; fi
	if [ -n "$OLLAMA_DIR" ] && ! plain_path "$OLLAMA_DIR"; then
		die "$E_UNIT" "unsupported home folder: $OLLAMA_DIR"
	fi
fi

# --- Outils des étapes qui modifient ------------------------------------------
# Données de modèles : toujours avec les droits du service, jamais en root
# (un lien symbolique glissé dans ces dossiers ne donne rien de plus).
as_svc() { setpriv --reuid="$SUID" --regid="$SGID" --init-groups -- "$@"; }

# Dossier .ollama prêt pour BindPaths= (sans lui, le service ne démarre
# pas) : un dossier du service, créé avec ses droits (0755, comme Ollama)
# s'il manque. Jamais un lien symbolique : systemd suit la source d'un
# BindPaths= à chaque démarrage, le propriétaire du HOME pourrait la faire
# pointer n'importe où. 12 : lien, autre chose qu'un dossier, ou dossier
# d'un autre compte ; 15 : le service ne peut pas le créer.
ollama_dir_ready() {
	[ ! -L "$1" ] || return 12
	if [ -d "$1" ]; then
		[ "$(stat -c %u -- "$1")" = "$SUID" ] || return 12
		return 0
	fi
	[ ! -e "$1" ] || return 12
	as_svc mkdir -m 0755 -- "$1" 2> /dev/null || return 15
	CREATED_HOME=1
}

# À appeler dans un sous-shell : se place dans le dossier, qui ne peut plus
# être remplacé, et vérifie que c'est bien lui.
pin() { cd -P -- "$1" 2> /dev/null && [ "$(pwd -P)" = "$1" ]; }

CREATED_NEW='' NEW_OWNER='' STOPPED='' STARTED='' WROTE_DROPIN='' CREATED_HOME=''
RENAMED='' COPIED='' renamed=0 copied=0 skipped=0
DIR=$ROOT/etc/systemd/system/$UNIT.d
DROPIN=$DIR/zz-chatonlm-models.conf

rollback() {
	r=0
	if [ -n "$STARTED" ]; then systemctl stop -- "$UNIT" || r=1; fi
	if [ -n "$WROTE_DROPIN" ]; then
		rm -f -- "$DROPIN.tmp"
		if [ -f "$WORK/dropin.bak" ]; then
			cp -p -- "$WORK/dropin.bak" "$DROPIN" || r=1
		else
			rm -f -- "$DROPIN" || r=1
		fi
		systemctl daemon-reload || r=1
	fi
	for x in $RENAMED; do as_svc mv -- "$NEW/$x" "$OLD/$x" || r=1; done
	for x in $COPIED; do
		if [ -f "$WORK/files-made-$x" ]; then
			while IFS= read -r t; do as_svc rm -f -- "$t" || r=1; done < "$WORK/files-made-$x"
		fi
		if [ -f "$WORK/dirs-made-$x" ]; then
			sort -r "$WORK/dirs-made-$x" > "$WORK/dirs-undo-$x"
			while IFS= read -r t; do as_svc rmdir -- "$t" || r=1; done < "$WORK/dirs-undo-$x"
		fi
	done
	# Dossier .ollama créé ici, s'il est resté vide.
	if [ -n "$CREATED_HOME" ]; then as_svc rmdir -- "$OLLAMA_DIR" 2> /dev/null || :; fi
	if [ -n "$NEW_OWNER" ]; then (pin "$NEW" && chown -h -- "$NEW_OWNER" .) || r=1; fi
	if [ -n "$CREATED_NEW" ]; then rmdir -- "$NEW" || r=1; fi
	if [ -n "$STOPPED" ]; then systemctl start -- "$UNIT" || r=1; fi
	return "$r"
}

fail() {
	code=$1
	shift
	err "$*"
	if rollback; then
		say restored
		exit "$code"
	fi
	err "could not restore everything; see the messages above"
	exit "$E_ROLLBACK"
}

# Copie fichier par fichier, sans rien écraser (fusion : un blob de même nom a
# le même contenu ; un manifeste déjà présent dans la cible est gardé). Liens
# physiques sur le même système de fichiers. Les téléchargements interrompus
# (*-partial*) et les dossiers temporaires .ollama-split-* sont laissés.
list_tree() {
	(pin "$OLD/$1" && find . -name '.ollama-split-*' -prune -o -type d -print) > "$WORK/dirs-$1" &&
		(pin "$OLD/$1" && find . -name '.ollama-split-*' -prune -o -type f ! -name '*-partial*' -print) > "$WORK/files-$1"
}
copy_tree() {
	src=$OLD/$1 dst=$NEW/$1 link=''
	[ -n "$FORCE_COPY" ] || [ "$(stat -c %d -- "$src")" != "$DEV_NEW" ] || link=1
	: > "$WORK/dirs-made-$1"
	: > "$WORK/files-made-$1"
	while IFS= read -r d; do
		case $d in .) t=$dst ;; *) t=$dst/${d#./} ;; esac
		[ -d "$t" ] && continue
		as_svc mkdir -- "$t" || return 1
		printf '%s\n' "$t" >> "$WORK/dirs-made-$1"
	done < "$WORK/dirs-$1"
	while IFS= read -r f; do
		f=${f#./}
		t=$dst/$f
		if [ -e "$t" ] || [ -L "$t" ]; then
			skipped=$((skipped + 1))
			continue
		fi
		say "copy $(stat -c %s -- "$src/$f") $1/$f"
		if [ -n "$link" ] && as_svc ln -P -- "$src/$f" "$t" 2> /dev/null; then
			:
		else
			as_svc cp -P -p -- "$src/$f" "$t" || return 1
		fi
		printf '%s\n' "$t" >> "$WORK/files-made-$1"
		copied=$((copied + 1))
	done < "$WORK/files-$1"
}

# Démarré, stable (même PID deux fois de suite), sur le nouveau dossier.
wait_running() {
	i=0 seen=0 last=''
	while [ "$i" -lt 30 ]; do
		sleep 1
		i=$((i + 1))
		case $(systemctl is-active -- "$UNIT") in
		active) ;;
		activating | reloading)
			seen=0 last=''
			continue
			;;
		*) return 1 ;;
		esac
		pid=$(prop MainPID)
		if [ "$pid" = "$last" ] && [ -d "$NEW/blobs" ] &&
			tr '\0' '\n' < "$ROOT/proc/$pid/environ" 2> /dev/null | grep -Fxq -- "OLLAMA_MODELS=$NEW"; then
			seen=$((seen + 1))
			[ "$seen" -lt 2 ] || return 0
		else
			seen=0
		fi
		last=$pid
	done
	return 1
}

# systemctl show cite (") une valeur qui contient une espace ou ( ) ' ; ...
env_has_new() {
	case " $(systemctl show -p Environment --value -- "$UNIT") " in
	*" OLLAMA_MODELS=$NEW "* | *" \"OLLAMA_MODELS=$NEW\" "*) return 0 ;;
	esac
	return 1
}

# --- 1. Dossier, sans toucher au service ---------------------------------------
say step prepare
if [ -z "$NEW_EXISTS" ]; then
	mkdir -m 0755 -- "$NEW" || die "$E_TARGET" "cannot create $NEW"
	CREATED_NEW=1
fi
owner=$(stat -c %u:%g -- "$NEW")
if [ "$owner" != "$SUID:$SGID" ]; then
	[ -n "$CREATED_NEW" ] || NEW_OWNER=$owner
	(pin "$NEW" && chown -h -- "$SUID:$SGID" . && chmod u+rwx .) ||
		fail "$E_WRITE" "cannot give $NEW to $SUSER"
fi
# Modèles déjà présents dans la cible : au service (non restauré).
for x in blobs manifests metadata; do
	[ -e "$NEW/$x" ] || continue
	if [ -n "$( (pin "$NEW" && find "./$x" ! -user "$SUID" -print -quit))" ]; then
		(pin "$NEW" && chown -R -h -- "$SUID:$SGID" "./$x") ||
			fail "$E_WRITE" "cannot give $NEW/$x to $SUSER"
	fi
done
# Écriture par le service : depuis le dossier (vaut pour BindPaths=, dont les
# parents ne sont pas traversés), puis par le chemin complet.
(pin "$NEW" && as_svc /bin/sh -c 't=.chatonlm-write-test.$$ && mkdir -- "$t" && rmdir -- "$t"') ||
	fail "$E_WRITE" "$SUSER cannot write to $NEW (file system without Unix permissions?)"
if [ "$SANDBOX" != bind ]; then
	as_svc /bin/sh -c 'cd -- "$1"' sh "$NEW" ||
		fail "$E_WRITE" "$SUSER cannot reach $NEW (permissions of a parent folder)"
fi
if [ -n "$OLLAMA_DIR" ]; then
	ollama_dir_ready "$OLLAMA_DIR"
	case $? in
	0) ;;
	12) fail "$E_TARGET" "not a folder of $SUSER: $OLLAMA_DIR" ;;
	*) fail "$E_WRITE" "$SUSER cannot create $OLLAMA_DIR" ;;
	esac
fi

# --- 2. Arrêt -------------------------------------------------------------------
say step stop
if [ -n "$WAS_ACTIVE" ]; then
	systemctl stop -- "$UNIT" || fail "$E_STOP" "cannot stop $UNIT"
	STOPPED=1
fi

# --- 3. Modèles -----------------------------------------------------------------
say step move
total=0
for x in $COPY; do
	list_tree "$x" || fail "$E_MOVE" "cannot read $OLD/$x"
	while IFS= read -r f; do
		f=${f#./}
		[ -e "$NEW/$x/$f" ] || [ -L "$NEW/$x/$f" ] || total=$((total + $(stat -c %s -- "$OLD/$x/$f")))
	done < "$WORK/files-$x"
done
say "total $total"
for x in $RENAME; do
	as_svc mv -- "$OLD/$x" "$NEW/$x" || fail "$E_MOVE" "cannot move $OLD/$x"
	RENAMED="$RENAMED $x"
	renamed=$((renamed + 1))
done
for x in $COPY; do
	COPIED="$COPIED $x"
	copy_tree "$x" || fail "$E_MOVE" "cannot copy $OLD/$x"
done

# --- 4. Configuration -----------------------------------------------------------
say step config
Q=$(printf '%s' "$NEW" | sed 's/%/%%/g')
if [ -e "$DROPIN" ]; then
	cp -p -- "$DROPIN" "$WORK/dropin.bak" || fail "$E_CONFIG" "cannot save $DROPIN"
fi
mkdir -p -- "$DIR" || fail "$E_CONFIG" "cannot create $DIR"
WROTE_DROPIN=1
{
	printf '%s\n' "# Written by ChatonLM (Settings > Model location)." \
		'[Unit]' "RequiresMountsFor=\"$Q\"" '' '[Service]' "Environment=\"OLLAMA_MODELS=$Q\""
	case $SANDBOX in
	bind)
		printf '%s\n' 'ProtectHome=tmpfs' "BindPaths=\"$Q\""
		if [ -n "$OLLAMA_DIR" ]; then
			printf '%s\n' "BindPaths=\"$(printf '%s' "$OLLAMA_DIR" | sed 's/%/%%/g')\""
		fi
		;;
	rw) printf '%s\n' "ReadWritePaths=\"$Q\"" ;;
	esac
} > "$DROPIN.tmp" || fail "$E_CONFIG" "cannot write $DROPIN"
if ! chmod 0644 -- "$DROPIN.tmp" || ! mv -f -- "$DROPIN.tmp" "$DROPIN"; then
	fail "$E_CONFIG" "cannot write $DROPIN"
fi
systemctl daemon-reload || fail "$E_CONFIG" "systemctl daemon-reload failed"
env_has_new || fail "$E_OVERRIDDEN" "another drop-in of $UNIT sets OLLAMA_MODELS"

# --- 5. Redémarrage, seulement s'il tournait -----------------------------------
say step start
restarted=no
if [ -n "$STOPPED" ]; then
	STARTED=1
	systemctl start -- "$UNIT" || fail "$E_START" "$UNIT did not start"
	wait_running || fail "$E_START" "$UNIT did not keep running with $NEW (journalctl -u $UNIT)"
	restarted=yes
fi

# --- 6. Anciennes copies : plus de retour en arrière après ce point -------------
say step cleanup
cleanup=none
if [ -n "$COPIED" ]; then
	cleanup=ok
	for x in $COPIED; do as_svc rm -rf -- "$OLD/$x" || cleanup=failed; done
fi
say "result restarted=$restarted renamed=$renamed copied=$copied skipped=$skipped cleanup=$cleanup"
exit 0
