#!/usr/bin/env python3
# Met un contenu dans le presse-papiers de la session virtuelle comme une autre
# application (une capture d'écran copiée, un fichier copié dans Dolphin) : les
# tests vérifient ce que l'app fait d'un vrai collage (Ctrl+V). Wayland
# n'accepte la sélection que du client qui a le focus clavier : petite fenêtre
# GTK 3, attente du focus, sélection posée (propriétaire : un widget invisible,
# comme GtkClipboard), puis la fenêtre est détruite et KWin rend le focus à la
# fenêtre précédente. Le script reste ensuite propriétaire du contenu (pas de
# gestionnaire de presse-papiers dans la session) jusqu'à la fermeture de son
# entrée standard, ou 120 s au plus.
# Avec "primary": true, c'est la sélection primaire (texte surligné dans une
# application, lu par le raccourci « Correct selected text ») qui est posée, et
# non le presse-papiers ; elle aussi demande le focus clavier sous Wayland.
#
# Usage (dans scripts/e2e-session.sh seulement) :
#   python3 e2e/lib/clipboard-writer.py '<json>' [attente du focus en ms, 3000]
# JSON : {"text": "..." | null, "data": {"<type MIME>": "<base64>", ...},
#         "primary": false}
#   text : proposé sous les types texte de GTK (text/plain;charset=utf-8,
#   UTF8_STRING, text/plain...) ; data : octets bruts, un type chacun ;
#   primary : sélection primaire au lieu du presse-papiers (texte et data
#   seulement) ;
#   ou {"image": "<base64 d'une image>"} : posée par GtkClipboard.set_image
#   (types demandés alors non notés).
# Sortie : une ligne JSON par évènement :
#   {"event": "ready", "targets": [...]}   sélection posée, fenêtre détruite
#   {"event": "get", "target": "..."}      type demandé par un lecteur
#   {"event": "cleared"}                   sélection prise par un autre client
# Codes de sortie : 0 fin normale ; 2 pas de focus clavier dans le délai ;
# 3 hors de la session virtuelle (refusé avant tout accès à l'affichage : aucune
# fenêtre) ; 1 autre erreur. Appelé par e2e/lib/clipboard.mjs (holdClipboard).
# Le refus hors session est vérifié par scripts/clipboard-reader.test.mjs.
import base64
import json
import os
import re
import stat
import sys

ROOT = os.path.realpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
RUNS = os.path.join(ROOT, ".tools", "e2e-runs")


def outside_session():
    """Raison du refus si l'on n'est pas dans scripts/e2e-session.sh, sinon None.

    Mêmes contrôles que clipboard-reader.py : variable de la session, socket
    Wayland chatonlm-e2e-<pid> dans son propre dossier d'exécution
    (/tmp/chatonlm-e2e.XXXXXX), HOME sous .tools/e2e-runs/. Le presse-papiers
    du bureau réel n'est jamais touché.
    """
    if os.environ.get("CHATONLM_E2E_SESSION") != "1":
        return "CHATONLM_E2E_SESSION absent"
    display = os.environ.get("WAYLAND_DISPLAY", "")
    if not re.fullmatch(r"chatonlm-e2e-[0-9]+", display):
        return f"WAYLAND_DISPLAY={display!r} n'est pas la socket de la session"
    runtime = os.environ.get("XDG_RUNTIME_DIR", "")
    if not re.fullmatch(r"/tmp/chatonlm-e2e\.[A-Za-z0-9]+", runtime):
        return f"XDG_RUNTIME_DIR={runtime!r} n'est pas celui de la session"
    try:
        is_socket = stat.S_ISSOCK(os.stat(os.path.join(runtime, display)).st_mode)
    except OSError:
        is_socket = False
    if not is_socket:
        return f"socket {display} introuvable dans {runtime}"
    home = os.path.realpath(os.environ.get("HOME", "/"))
    if not home.startswith(RUNS + os.sep):
        return f"HOME={home!r} hors de .tools/e2e-runs/"
    return None


reason = outside_session()
if reason is not None:
    print(
        f"Écriture du presse-papiers refusée hors de la session virtuelle"
        f" (scripts/e2e-session.sh) : {reason}.",
        file=sys.stderr,
    )
    sys.exit(3)

# Wayland seulement, jamais de repli sur un serveur X (celui du bureau réel) ;
# WAYLAND_SOCKET retiré comme dans clipboard-reader.py.
os.environ["GDK_BACKEND"] = "wayland"
os.environ.pop("DISPLAY", None)
os.environ.pop("WAYLAND_SOCKET", None)

import gi  # noqa: E402

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import Gdk, GdkPixbuf, GLib, Gtk  # noqa: E402

if len(sys.argv) < 2:
    raise SystemExit("Usage : clipboard-writer.py '<json>' [attente du focus en ms]")
spec = json.loads(sys.argv[1])
focus_ms = int(sys.argv[2]) if len(sys.argv) > 2 else 3000
text = spec.get("text")
primary = bool(spec.get("primary"))
image = base64.b64decode(spec["image"]) if spec.get("image") else None
if primary and spec.get("image"):
    raise SystemExit("Image dans la sélection primaire : non prise en charge.")
data = {mime: base64.b64decode(value) for mime, value in (spec.get("data") or {}).items()}

if Gdk.Display.get_default() is None:
    print("Affichage Wayland de la session inaccessible.", file=sys.stderr)
    sys.exit(1)

INFO_TEXT = 1
INFO_DATA = 2
TEXT_TARGETS = [
    "UTF8_STRING",
    "COMPOUND_TEXT",
    "TEXT",
    "STRING",
    "text/plain;charset=utf-8",
    "text/plain",
]
status = {"code": 0, "started": False, "ready": False}


def emit(event):
    print(json.dumps(event, ensure_ascii=False), flush=True)


# Propriétaire de la sélection : survit à la fenêtre (comme le widget de
# GtkClipboard).
owner = Gtk.Invisible()
mimes = list(data)


def on_get(_widget, selection, info, _time):
    target = selection.get_target().name()
    emit({"event": "get", "target": target})
    if info == INFO_TEXT:
        selection.set_text(text, -1)
    elif target in data:
        selection.set(selection.get_target(), 8, data[target])


def on_clear(*_):
    emit({"event": "cleared"})
    return False


owner.connect("selection-get", on_get)
owner.connect("selection-clear-event", on_clear)


def take_image():
    """Image posée par GtkClipboard.set_image, comme une application GTK qui
    copie une image (tous les formats d'écriture de GdkPixbuf)."""
    loader = GdkPixbuf.PixbufLoader.new()
    loader.write(image)
    loader.close()
    clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
    clipboard.set_image(loader.get_pixbuf())
    window.destroy()
    status["ready"] = True
    emit({"event": "ready", "targets": ["gtk_clipboard_set_image"]})
    return False


def take_selection():
    if image is not None:
        return take_image()
    clipboard = Gdk.SELECTION_PRIMARY if primary else Gdk.SELECTION_CLIPBOARD
    if not Gtk.selection_owner_set(owner, clipboard, Gdk.CURRENT_TIME):
        print("Sélection refusée.", file=sys.stderr)
        status["code"] = 1
        Gtk.main_quit()
        return False
    # Types texte de gtk_target_list_add_text_targets, écrits ici :
    # gtk_target_table_new_from_list rend des noms illisibles sous PyGObject.
    targets = [(name, INFO_TEXT) for name in TEXT_TARGETS] if text is not None else []
    targets += [(mime, INFO_DATA) for mime in mimes]
    entries = [Gtk.TargetEntry.new(name, 0, info) for name, info in targets]
    Gtk.selection_add_targets(owner, clipboard, entries)
    # Fenêtre détruite : KWin rend le focus à la fenêtre précédente (l'app).
    window.destroy()
    status["ready"] = True
    emit({"event": "ready", "targets": [name for name, _ in targets]})
    return False


def on_focus(*_):
    # Une seule fois, un peu après le focus (comme clipboard-reader.py).
    if not status["started"]:
        status["started"] = True
        GLib.timeout_add(150, take_selection)
    return False


def no_focus():
    if not status["started"]:
        print(f"Pas de focus clavier après {focus_ms} ms.", file=sys.stderr)
        status["code"] = 2
        Gtk.main_quit()
    return False


def on_stdin(*_):
    # Entrée standard fermée (ou ligne reçue) : fin, la sélection disparaît.
    Gtk.main_quit()
    return False


def too_long():
    print("Toujours actif après 120 s : arrêt.", file=sys.stderr)
    Gtk.main_quit()
    return False


window = Gtk.Window(title="chatonlm-e2e-clipboard-writer")
window.set_default_size(160, 80)
window.connect("focus-in-event", on_focus)
GLib.timeout_add(focus_ms, no_focus)
GLib.timeout_add_seconds(120, too_long)
GLib.io_add_watch(
    GLib.IOChannel.unix_new(sys.stdin.fileno()),
    GLib.PRIORITY_DEFAULT,
    GLib.IOCondition.IN | GLib.IOCondition.HUP | GLib.IOCondition.ERR,
    on_stdin,
)
window.show_all()
Gtk.main()
sys.exit(status["code"])
