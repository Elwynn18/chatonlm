#!/usr/bin/env python3
# Source de glisser-déposer de la session virtuelle, comme un gestionnaire de
# fichiers : fenêtre GTK 3 « chatonlm-e2e-drag-source » entièrement occupée par
# une zone qui offre text/uri-list (les fichiers donnés). Le pointeur, piloté
# par e2e/lib/keyboard.py (interface EIS de KWin), la saisit et la dépose sur
# l'app : vrai dépôt natif GTK, celui que wry transmet à Tauri.
#
# Usage (dans scripts/e2e-session.sh seulement) :
#   python3 e2e/lib/drag-source.py <chemin> [<chemin>...]
# Sortie : une ligne JSON par évènement : {"event": "shown"} fenêtre affichée,
# puis "press" (bouton appuyé sur la zone), "drag-begin", "data-get" (uris
# envoyées), "drag-end". Fin à la
# fermeture de l'entrée standard, ou après 120 s.
# Codes de sortie : 0 fin normale ; 3 hors de la session virtuelle (refusé
# avant tout accès à l'affichage : aucune fenêtre) ; 1 autre erreur. Appelé par
# e2e/lib/drag.mjs (startDragSource). Le refus hors session est vérifié par
# scripts/clipboard-reader.test.mjs (mêmes contrôles que clipboard-reader.py).
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
    (/tmp/chatonlm-e2e.XXXXXX), HOME sous .tools/e2e-runs/.
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
        f"Source de glisser refusée hors de la session virtuelle (scripts/e2e-session.sh) : {reason}.",
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
gi.require_version("Gio", "2.0")
from gi.repository import Gdk, Gio, GLib, Gtk  # noqa: E402

if Gdk.Display.get_default() is None:
    print("Affichage Wayland de la session inaccessible.", file=sys.stderr)
    sys.exit(1)

uris = [Gio.File.new_for_path(path).get_uri() for path in sys.argv[1:]]
if not uris:
    raise SystemExit("Usage : drag-source.py <chemin> [<chemin>...]")


def emit(event, **fields):
    print(json.dumps({"event": event, **fields}, ensure_ascii=False), flush=True)


def on_data_get(_widget, _context, selection, _info, _time):
    selection.set_uris(uris)
    emit("data-get", uris=uris)


def quit_main(*_):
    Gtk.main_quit()
    return False


window = Gtk.Window(title="chatonlm-e2e-drag-source")
window.set_default_size(220, 200)
area = Gtk.EventBox()
area.add(Gtk.Label(label="Drag me"))
area.drag_source_set(
    Gdk.ModifierType.BUTTON1_MASK,
    [Gtk.TargetEntry.new("text/uri-list", 0, 0)],
    Gdk.DragAction.COPY,
)
area.connect("drag-data-get", on_data_get)
area.connect("button-press-event", lambda _w, event: emit("press", x=event.x_root, y=event.y_root) or False)
area.connect("drag-begin", lambda *_: emit("drag-begin"))
area.connect("drag-end", lambda *_: emit("drag-end"))
window.add(area)
window.connect("map-event", lambda *_: emit("shown") or False)
window.connect("destroy", quit_main)
GLib.timeout_add_seconds(120, quit_main)
GLib.io_add_watch(
    GLib.IOChannel.unix_new(sys.stdin.fileno()),
    GLib.PRIORITY_DEFAULT,
    GLib.IOCondition.IN | GLib.IOCondition.HUP | GLib.IOCondition.ERR,
    quit_main,
)
window.show_all()
Gtk.main()
