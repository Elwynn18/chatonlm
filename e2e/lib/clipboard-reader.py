#!/usr/bin/env python3
# Lit le presse-papiers de la session virtuelle comme une autre application
# (un collage dans Kate ou LibreOffice) : vérifie ce qu'une copie de l'app a
# vraiment déposé. Wayland ne transmet la sélection qu'au client qui a le focus
# clavier : petite fenêtre GTK 3, attente du focus, lecture de la liste des
# types, de text/plain et de text/html, puis une ligne JSON sur la sortie :
#   {"targets": ["text/html", ...], "text": "..." | null, "html": "..." | null}
#
# Usage (dans scripts/e2e-session.sh seulement) :
#   python3 e2e/lib/clipboard-reader.py [attente du focus en ms, 3000]
# Codes de sortie : 0 lu ; 2 pas de focus clavier dans le délai ; 3 hors de la
# session virtuelle (refusé avant tout accès à l'affichage : aucune fenêtre) ;
# 1 autre erreur. Appelé par e2e/lib/clipboard.mjs (readClipboard). Le refus
# hors session est vérifié par scripts/clipboard-reader.test.mjs.
import json
import os
import re
import stat
import sys

ROOT = os.path.realpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
RUNS = os.path.join(ROOT, ".tools", "e2e-runs")


def outside_session():
    """Raison du refus si l'on n'est pas dans scripts/e2e-session.sh, sinon None.

    La session virtuelle a sa variable, sa socket Wayland (chatonlm-e2e-<pid>)
    dans son propre dossier d'exécution (/tmp/chatonlm-e2e.XXXXXX) et un HOME
    sous .tools/e2e-runs/ : le presse-papiers du bureau réel n'est jamais lu.
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
        f"Lecture du presse-papiers refusée hors de la session virtuelle"
        f" (scripts/e2e-session.sh) : {reason}.",
        file=sys.stderr,
    )
    sys.exit(3)

focus_ms = int(sys.argv[1]) if len(sys.argv) > 1 else 3000

# Wayland seulement, jamais de repli sur un serveur X (celui du bureau réel).
# WAYLAND_SOCKET (descripteur d'une connexion déjà ouverte, hérité du parent)
# passe avant WAYLAND_DISPLAY dans libwayland : retiré, seule la socket
# vérifiée ci-dessus peut servir.
os.environ["GDK_BACKEND"] = "wayland"
os.environ.pop("DISPLAY", None)
os.environ.pop("WAYLAND_SOCKET", None)

import gi  # noqa: E402

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

if Gdk.Display.get_default() is None:
    print("Affichage Wayland de la session inaccessible.", file=sys.stderr)
    sys.exit(1)

status = {"code": 1, "started": False}


def decode(data):
    """HTML reçu : UTF-8 en général, UTF-16 s'il commence par une marque."""
    if data[:2] in (b"\xff\xfe", b"\xfe\xff"):
        return data.decode("utf-16", "replace")
    return data.decode("utf-8", "replace")


def read():
    clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
    ok, atoms = clipboard.wait_for_targets()
    targets = [atom.name() for atom in atoms] if ok and atoms else []
    html = None
    if "text/html" in targets:
        selection = clipboard.wait_for_contents(Gdk.Atom.intern("text/html", False))
        data = selection.get_data() if selection is not None else None
        html = decode(bytes(data)) if data is not None else None
    result = {"targets": targets, "text": clipboard.wait_for_text(), "html": html}
    print(json.dumps(result, ensure_ascii=False), flush=True)
    status["code"] = 0
    Gtk.main_quit()
    return False


def on_focus(*_):
    # Une seule lecture, un peu après le focus (la sélection arrive avec lui).
    if not status["started"]:
        status["started"] = True
        GLib.timeout_add(150, read)
    return False


def no_focus():
    if not status["started"]:
        print(f"Pas de focus clavier après {focus_ms} ms.", file=sys.stderr)
        status["code"] = 2
        Gtk.main_quit()
    return False


window = Gtk.Window(title="chatonlm-e2e-clipboard-reader")
window.set_default_size(160, 80)
window.connect("focus-in-event", on_focus)
GLib.timeout_add(focus_ms, no_focus)
window.show_all()
Gtk.main()
window.destroy()
sys.exit(status["code"])
