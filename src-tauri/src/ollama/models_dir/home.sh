# ChatonLM: Ollama home access
# Script de ChatonLM (Settings > General, bouton « Fix »), lancé en root par :
#   pkexec --disable-internal-agent /bin/sh -c "<ce texte>" chatonlm-home \
#     ollama.service <dossier .ollama du service>
# La boîte polkit affiche « Authentication is needed to run `<38 premiers
# caractères> ... <37 derniers>' as the super user » : la 1re ligne ci-dessus
# en fait le début, le dossier (dernier argument) la fin.
#
# Rend au service le dossier .ollama de son HOME (clé id_ed25519 : modèles
# d'ollama.com, compte Ollama), que ProtectHome= lui cache, avec le moins de
# droits possible : drop-in zz-chatonlm-home.conf, « ProtectHome=tmpfs » (le
# reste de /home, /root et /run/user reste caché : dossiers vides en lecture
# seule) et « BindPaths= » de ce seul dossier, créé avec les droits du
# service s'il manque.
#
# POSIX (dash sur Debian et Ubuntu) ; suppose coreutils, util-linux
# (setpriv), awk et getent, comme toute distribution à systemd.
#
# Sortie standard, une ligne par événement, lue par l'app :
#   step <prepare|config|restart>    (restart : seulement si le service tourne)
#   restored                         après une restauration réussie
#   result changed=<yes|no> restarted=<yes|no>
# Codes de sortie :
#   0      fait (changed=no : rien à faire, le dossier n'est pas caché) ;
#   10     arguments ; 11 service absent, qui tourne en root, ou changé
#          (autre HOME) ;
#   12     .ollama est un lien, n'est pas un dossier, ou appartient à un
#          autre compte ;
#   15     le service ne peut pas créer .ollama ; 17 interne ;
#   22-24  échec, état précédent restauré (configuration, autre réglage du
#          service qui l'emporte, redémarrage) ;
#   30     échec et restauration incomplète (détail sur la sortie d'erreur).
#   126 et 127 ne viennent que de pkexec (annulation ; refus ou pas d'agent).

set -u
umask 022
# L'app peut se fermer pendant l'opération : on va jusqu'au bout.
trap '' HUP PIPE
PATH=/usr/sbin:/usr/bin:/sbin:/bin
LC_ALL=C
export PATH LC_ALL

E_ARGS=10 E_UNIT=11 E_TARGET=12 E_WRITE=15 E_INTERNAL=17
E_CONFIG=22 E_OVERRIDDEN=23 E_START=24 E_ROLLBACK=30

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
# arrivent jamais.
ROOT='' TEST_HOME=''
if [ "$(id -u)" != 0 ]; then
	[ -n "${CHATONLM_TEST_ROOT:-}" ] || die "$E_ARGS" "must run as root"
	ROOT=$CHATONLM_TEST_ROOT
	PATH=$CHATONLM_TEST_BIN:$PATH
	TEST_HOME=${CHATONLM_TEST_HOME:-}
fi

[ $# -eq 2 ] || die "$E_ARGS" "usage: ollama.service DIR"
UNIT=$1 DIR=$2
[ "$UNIT" = ollama.service ] || die "$E_ARGS" "unsupported unit: $UNIT"

# Fonctions identiques à celles de models_dir.sh (vérifié par les tests).

# Absolu, sans caractère que systemd interpréterait (" \ : $ `) ni caractère
# de contrôle.
plain_path() {
	case $1 in /*) ;; *) return 1 ;; esac
	case $1 in *[\"\\:\$\`]*) return 1 ;; esac
	[ "$(printf '%s' "$1" | tr -d '\001-\037\177')" = "$1" ]
}

# Sous un dossier que ProtectHome= masque.
in_home() {
	case $1 in /home/* | /root/* | /run/user/*) return 0 ;; esac
	if [ -n "$TEST_HOME" ]; then
		case $1 in "$TEST_HOME"/*) return 0 ;; esac
	fi
	return 1
}

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

# Données du service : toujours avec ses droits, jamais en root.
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

plain_path "$DIR" || die "$E_ARGS" "invalid folder: $DIR"
command -v setpriv > /dev/null || die "$E_INTERNAL" "setpriv (util-linux) is missing"

# --- Service : faits relus ici (root), pas reçus de l'app ---------------------
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
# En root, le service suivrait (systemd suit la source d'un BindPaths=) un
# lien glissé par le propriétaire du HOME vers n'importe quel dossier, en
# écriture : l'app explique sans rien changer.
[ "$SUID" != 0 ] || die "$E_UNIT" "$UNIT runs as root: ChatonLM won't change it"
[ "$(service_ollama_dir)" = "$DIR" ] || die "$E_UNIT" "$DIR is not the folder of $UNIT"
WAS_ACTIVE=''
case $(systemctl is-active -- "$UNIT" 2> /dev/null) in
active | activating | reloading) WAS_ACTIVE=1 ;;
esac

# Rien à faire : dossier que ProtectHome= ne masque pas (no, read-only).
if ! in_home "$DIR"; then
	say "result changed=no restarted=no"
	exit 0
fi
case $(prop ProtectHome) in
yes | true | tmpfs) ;;
*)
	say "result changed=no restarted=no"
	exit 0
	;;
esac

WORK=$(mktemp -d "$ROOT/run/chatonlm-home.XXXXXX") || die "$E_INTERNAL" "mktemp failed"
trap 'rm -rf -- "$WORK"' EXIT

CREATED_HOME='' WROTE_DROPIN='' RESTARTED=''
DROPDIR=$ROOT/etc/systemd/system/$UNIT.d
DROPIN=$DROPDIR/zz-chatonlm-home.conf

rollback() {
	r=0
	if [ -n "$WROTE_DROPIN" ]; then
		rm -f -- "$DROPIN.tmp"
		if [ -f "$WORK/dropin.bak" ]; then
			cp -p -- "$WORK/dropin.bak" "$DROPIN" || r=1
		else
			rm -f -- "$DROPIN" || r=1
		fi
		systemctl daemon-reload || r=1
	fi
	# Dossier créé ici, s'il est resté vide (Ollama a pu y écrire sa clé).
	if [ -n "$CREATED_HOME" ]; then as_svc rmdir -- "$DIR" 2> /dev/null || :; fi
	if [ -n "$RESTARTED" ]; then systemctl restart -- "$UNIT" || r=1; fi
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

# BindPaths= effectif de $1 sur lui-même : une entrée « [-]$1[:$1][:rbind|
# :norbind] » entière (seule sur sa ligne, ou entre espaces), quelle que
# soit la forme de `systemctl show` ; un sous-dossier ($1/models) ne compte
# pas.
bound() {
	prop BindPaths | awk -v d="$1" '
		BEGIN {
			c[1] = d ":" d ":rbind"; c[2] = d ":" d ":norbind"; c[3] = d ":" d
			c[4] = d ":rbind"; c[5] = d ":norbind"; c[6] = d
		}
		{
			t = " " $0 " "
			for (i = 1; i <= 6; i++) if (index(t, " " c[i] " ") || index(t, " -" c[i] " ")) f = 1
		}
		END { exit !f }'
}

# Redémarré, stable (même PID deux fois de suite), et le dossier visible dans
# son espace de noms (/proc/<pid>/root : la vue du processus).
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
		if [ "$pid" = "$last" ] && [ -d "$ROOT/proc/$pid/root$DIR" ]; then
			seen=$((seen + 1))
			[ "$seen" -lt 2 ] || return 0
		else
			seen=0
		fi
		last=$pid
	done
	return 1
}

# --- 1. Dossier .ollama ----------------------------------------------------------
say step prepare
ollama_dir_ready "$DIR"
case $? in
0) ;;
12) die "$E_TARGET" "not a folder of $SUSER: $DIR" ;;
*) die "$E_WRITE" "$SUSER cannot create $DIR" ;;
esac

# --- 2. Configuration -------------------------------------------------------------
say step config
Q=$(printf '%s' "$DIR" | sed 's/%/%%/g')
if [ -e "$DROPIN" ]; then
	cp -p -- "$DROPIN" "$WORK/dropin.bak" || fail "$E_CONFIG" "cannot save $DROPIN"
fi
mkdir -p -- "$DROPDIR" || fail "$E_CONFIG" "cannot create $DROPDIR"
WROTE_DROPIN=1
printf '%s\n' "# Written by ChatonLM (Settings > General): Ollama's own folder." \
	'[Service]' 'ProtectHome=tmpfs' "BindPaths=\"$Q\"" > "$DROPIN.tmp" ||
	fail "$E_CONFIG" "cannot write $DROPIN"
if ! chmod 0644 -- "$DROPIN.tmp" || ! mv -f -- "$DROPIN.tmp" "$DROPIN"; then
	fail "$E_CONFIG" "cannot write $DROPIN"
fi
systemctl daemon-reload || fail "$E_CONFIG" "systemctl daemon-reload failed"
[ "$(prop ProtectHome)" = tmpfs ] ||
	fail "$E_OVERRIDDEN" "another drop-in of $UNIT sets ProtectHome="
bound "$DIR" ||
	fail "$E_OVERRIDDEN" "another drop-in of $UNIT resets BindPaths="

# --- 3. Redémarrage, seulement s'il tournait -----------------------------------
restarted=no
if [ -n "$WAS_ACTIVE" ]; then
	say step restart
	RESTARTED=1
	systemctl restart -- "$UNIT" || fail "$E_START" "$UNIT did not restart"
	wait_running || fail "$E_START" "$UNIT does not see $DIR (journalctl -u $UNIT)"
	restarted=yes
fi
say "result changed=yes restarted=$restarted"
exit 0
