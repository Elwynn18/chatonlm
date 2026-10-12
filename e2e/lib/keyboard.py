#!/usr/bin/env python3
# Vraies touches du clavier dans la session virtuelle : le plugin WebDriver ne
# fait que simuler des KeyboardEvent en JavaScript (Tab ne déplace pas le
# focus, Entrée et Espace n'activent pas un bouton). Ce client libei passe par
# l'interface EIS de KWin (org.kde.KWin.EIS.RemoteDesktop, comme le portail
# « bureau à distance ») : KWin livre les touches à la fenêtre qui a le focus
# clavier, comme un vrai clavier.
#
# Usage (dans scripts/e2e-session.sh seulement) :
#   python3 e2e/lib/keyboard.py <accord> [<accord>...]
# Accords : tab, shift+tab, enter, space, escape, up, down, home, end, a-z,
# 0-9, slash, period, minus, ctrl+l, shift+a, ctrl+v, ctrl+alt+d...
# Pointeur (périphérique absolu de KWin, même interface) : pointer:<x>,<y>
# (déplacement, coordonnées logiques de l'écran virtuel), button:down et
# button:up (bouton gauche), rbutton:down et rbutton:up (bouton droit),
# wait:<ms> (pause, la connexion restant servie) ;
# un glisser-déposer natif est button:down puis des pointer: puis button:up.
# Sortie : une ligne JSON {"sent": <nombre d'accords et d'actions>}.
# Codes de sortie : 0 envoyé ; 2 pas de clavier ou de pointeur EIS dans le
# délai ; 3 hors de
# la session virtuelle, ou KWin qui n'est pas celui de la session (refusé avant
# toute connexion à KWin : jamais de touche sur le bureau réel) ; 1 autre
# erreur. Appelé par e2e/lib/keyboard.mjs (pressKeys). Le refus hors session
# et le contrôle du KWin visé sont vérifiés par scripts/keyboard.test.mjs.
import json
import os
import re
import select
import stat
import sys
import time

ROOT = os.path.realpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
RUNS = os.path.join(ROOT, ".tools", "e2e-runs")


def is_socket(path):
    try:
        return stat.S_ISSOCK(os.stat(path).st_mode)
    except OSError:
        return False


def outside_session():
    """Raison du refus si l'on n'est pas dans scripts/e2e-session.sh, sinon None.

    Mêmes contrôles que clipboard-reader.py (variable de la session, socket
    Wayland chatonlm-e2e-<pid> dans son dossier d'exécution, HOME sous
    .tools/e2e-runs/), plus le bus D-Bus : seul celui que dbus-run-session
    crée pour la session est accepté. Le KWin de ce bus est vérifié ensuite
    (kwin_owner_refusal).
    """
    if os.environ.get("CHATONLM_E2E_SESSION") != "1":
        return "CHATONLM_E2E_SESSION absent"
    display = os.environ.get("WAYLAND_DISPLAY", "")
    if not re.fullmatch(r"chatonlm-e2e-[0-9]+", display):
        return f"WAYLAND_DISPLAY={display!r} n'est pas la socket de la session"
    runtime = os.environ.get("XDG_RUNTIME_DIR", "")
    if not re.fullmatch(r"/tmp/chatonlm-e2e\.[A-Za-z0-9]+", runtime):
        return f"XDG_RUNTIME_DIR={runtime!r} n'est pas celui de la session"
    if not is_socket(os.path.join(runtime, display)):
        return f"socket {display} introuvable dans {runtime}"
    home = os.path.realpath(os.environ.get("HOME", "/"))
    if not home.startswith(RUNS + os.sep):
        return f"HOME={home!r} hors de .tools/e2e-runs/"
    # Bus de dbus-run-session (unix:tmpdir=/tmp dans session.conf) : une seule
    # adresse unix:path=/tmp/dbus-XXXXXXXXXX[,guid=...], vraie socket et pas
    # un lien. Chercher « /run/user/ » dans l'adresse laisserait passer
    # /run//user/..., un lien vers le bus du bureau ou autolaunch:.
    bus = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    match = re.fullmatch(r"unix:path=(/tmp/dbus-[A-Za-z0-9]+)(,guid=[0-9a-f]+)?", bus)
    if match is None or os.path.realpath(match[1]) != match[1] or not is_socket(match[1]):
        return f"DBUS_SESSION_BUS_ADDRESS={bus!r} n'est pas le bus de la session"
    return None


def refuse(reason):
    print(
        f"Touches refusées hors de la session virtuelle (scripts/e2e-session.sh) : {reason}.",
        file=sys.stderr,
    )
    sys.exit(3)


reason = outside_session()
if reason is not None:
    refuse(reason)

# Codes evdev (linux/input-event-codes.h). La session virtuelle a la
# disposition qwerty des États-Unis (aucun kxkbrc dans son HOME : défaut de
# libxkbcommon) : « slash », « period », « comma » et « minus » y donnent « / »,
# « . », « , » et « - », shift+minus « _ ».
KEYS = {
    "escape": 1,
    "minus": 12,
    "equal": 13,
    "tab": 15,
    "enter": 28,
    "ctrl": 29,
    "shift": 42,
    "alt": 56,
    "comma": 51,
    "period": 52,
    "slash": 53,
    "space": 57,
    "f5": 63,
    "home": 102,
    "up": 103,
    "left": 105,
    "right": 106,
    "end": 107,
    "down": 108,
}
# Chiffres de la rangée du haut : 1 à 9 puis 0 (codes 2 à 11).
KEYS.update({digit: code for code, digit in enumerate("1234567890", start=2)})
KEYS.update(
    {
        letter: code
        for letter, code in zip(
            "qwertyuiopasdfghjklzxcvbnm",
            [*range(16, 26), *range(30, 39), *range(44, 51)],
        )
    }
)


# Boutons du pointeur (linux/input-event-codes.h).
BTN_LEFT = 0x110
BTN_RIGHT = 0x111


def parse(chord):
    """Accord → ("key", codes) ; action du pointeur → ("pointer", x, y),
    ("button", appuyé, bouton) ou ("wait", secondes)."""
    name, _, value = chord.partition(":")
    if name == "pointer":
        x, y = (float(part) for part in value.split(","))
        return ("pointer", x, y)
    if name in ("button", "rbutton") and value in ("down", "up"):
        return ("button", value == "down", BTN_LEFT if name == "button" else BTN_RIGHT)
    if name == "wait":
        return ("wait", int(value) / 1000)
    codes = []
    for name in chord.lower().split("+"):
        if name not in KEYS:
            raise SystemExit(f"Touche inconnue : {name!r}")
        codes.append(KEYS[name])
    return ("key", codes)


chords = [parse(chord) for chord in sys.argv[1:]]
if not chords:
    raise SystemExit("Usage : keyboard.py <accord> [<accord>...]")
uses_keyboard = any(action[0] == "key" for action in chords)
uses_pointer = any(action[0] in ("pointer", "button") for action in chords)

import gi  # noqa: E402

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)


def kwin_owner_refusal():
    """Le KWin du bus doit être le KWin imbriqué de cette session : lancé avec
    --virtual et la socket WAYLAND_DISPLAY de la session."""
    pid = bus.call_sync(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "GetConnectionUnixProcessID",
        GLib.Variant("(s)", ("org.kde.KWin",)),
        GLib.VariantType.new("(u)"),
        Gio.DBusCallFlags.NONE,
        3000,
        None,
    ).unpack()[0]
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as file:
            args = file.read().decode(errors="replace").split("\0")
    except OSError:
        return f"ligne de commande de KWin (PID {pid}) illisible"
    display = os.environ["WAYLAND_DISPLAY"]
    if "--virtual" not in args:
        return f"KWin (PID {pid}) n'est pas virtuel"
    if "--socket" not in args or args[args.index("--socket") + 1] != display:
        return f"KWin (PID {pid}) n'a pas la socket {display}"
    return None


reason = kwin_owner_refusal()
if reason is not None:
    refuse(reason)

# Interface EIS de KWin ; capacités au sens du portail : 1 = clavier,
# 2 = pointeur.
reply, fds = bus.call_with_unix_fd_list_sync(
    "org.kde.KWin",
    "/org/kde/KWin/EIS/RemoteDesktop",
    "org.kde.KWin.EIS.RemoteDesktop",
    "connectToEIS",
    GLib.Variant("(i)", ((1 if uses_keyboard else 0) | (2 if uses_pointer else 0),)),
    GLib.VariantType.new("(hi)"),
    Gio.DBusCallFlags.NONE,
    3000,
    None,
    None,
)
handle, cookie = reply.unpack()
fd = fds.get(handle)

# ctypes et libei seulement maintenant, une fois les deux contrôles passés.
import ctypes  # noqa: E402

ei_lib = ctypes.CDLL("libei.so.1")
P = ctypes.c_void_p


def fn(name, restype, *argtypes):
    function = getattr(ei_lib, name)
    function.restype = restype
    function.argtypes = list(argtypes)
    return function


ei_new_sender = fn("ei_new_sender", P, P)
ei_configure_name = fn("ei_configure_name", None, P, ctypes.c_char_p)
ei_setup_backend_fd = fn("ei_setup_backend_fd", ctypes.c_int, P, ctypes.c_int)
ei_get_fd = fn("ei_get_fd", ctypes.c_int, P)
ei_dispatch = fn("ei_dispatch", None, P)
ei_get_event = fn("ei_get_event", P, P)
ei_event_get_type = fn("ei_event_get_type", ctypes.c_int, P)
ei_event_get_seat = fn("ei_event_get_seat", P, P)
ei_event_get_device = fn("ei_event_get_device", P, P)
ei_event_unref = fn("ei_event_unref", P, P)
ei_seat_ref = fn("ei_seat_ref", P, P)
ei_device_ref = fn("ei_device_ref", P, P)
ei_device_has_capability = fn("ei_device_has_capability", ctypes.c_bool, P, ctypes.c_int)
ei_device_start_emulating = fn("ei_device_start_emulating", None, P, ctypes.c_uint32)
ei_device_stop_emulating = fn("ei_device_stop_emulating", None, P)
ei_device_keyboard_key = fn("ei_device_keyboard_key", None, P, ctypes.c_uint32, ctypes.c_bool)
ei_device_pointer_motion_absolute = fn(
    "ei_device_pointer_motion_absolute", None, P, ctypes.c_double, ctypes.c_double
)
ei_device_button_button = fn("ei_device_button_button", None, P, ctypes.c_uint32, ctypes.c_bool)
ei_device_frame = fn("ei_device_frame", None, P, ctypes.c_uint64)
ei_now = fn("ei_now", ctypes.c_uint64, P)
ei_unref = fn("ei_unref", P, P)
# Variadique (liste de capacités terminée par NULL) : pas d'argtypes.
ei_seat_bind_capabilities = ei_lib.ei_seat_bind_capabilities
ei_seat_bind_capabilities.restype = None

EI_EVENT_DISCONNECT = 2
EI_EVENT_SEAT_ADDED = 3
EI_EVENT_DEVICE_ADDED = 5
EI_EVENT_DEVICE_REMOVED = 6
EI_EVENT_DEVICE_PAUSED = 7
EI_EVENT_DEVICE_RESUMED = 8
EI_DEVICE_CAP_POINTER_ABSOLUTE = 1 << 1
EI_DEVICE_CAP_KEYBOARD = 1 << 2
EI_DEVICE_CAP_BUTTON = 1 << 5

ei = ei_new_sender(None)
ei_configure_name(ei, b"chatonlm-e2e-keyboard")
if ei_setup_backend_fd(ei, fd) != 0:
    raise SystemExit("ei_setup_backend_fd a échoué")


def pump(timeout):
    """Lit les messages de KWin ; renvoie la liste (type, seat, device)."""
    ready, _, _ = select.select([ei_get_fd(ei)], [], [], timeout)
    if ready:
        ei_dispatch(ei)
    events = []
    while True:
        event = ei_get_event(ei)
        if not event:
            return events
        kind = ei_event_get_type(event)
        seat = ei_event_get_seat(event)
        device = ei_event_get_device(event)
        if kind == EI_EVENT_SEAT_ADDED:
            capabilities = []
            if uses_keyboard:
                capabilities.append(ctypes.c_int(EI_DEVICE_CAP_KEYBOARD))
            if uses_pointer:
                capabilities += [
                    ctypes.c_int(EI_DEVICE_CAP_POINTER_ABSOLUTE),
                    ctypes.c_int(EI_DEVICE_CAP_BUTTON),
                ]
            ei_seat_bind_capabilities(P(ei_seat_ref(seat)), *capabilities, P(None))
        if kind in (EI_EVENT_DEVICE_ADDED, EI_EVENT_DEVICE_RESUMED):
            ei_device_ref(device)
        events.append((kind, device))
        ei_event_unref(event)


keyboard = None
pointer = None
deadline = time.monotonic() + 3
while (uses_keyboard and keyboard is None) or (uses_pointer and pointer is None):
    left = deadline - time.monotonic()
    if left <= 0:
        print("Pas de clavier ou de pointeur EIS après 3 s.", file=sys.stderr)
        sys.exit(2)
    for kind, device in pump(left):
        if kind == EI_EVENT_DISCONNECT:
            print("KWin a fermé la connexion EIS.", file=sys.stderr)
            sys.exit(1)
        if kind != EI_EVENT_DEVICE_RESUMED:
            continue
        if ei_device_has_capability(device, EI_DEVICE_CAP_KEYBOARD):
            keyboard = device
        if ei_device_has_capability(device, EI_DEVICE_CAP_POINTER_ABSOLUTE) and ei_device_has_capability(
            device, EI_DEVICE_CAP_BUTTON
        ):
            pointer = device

sequence = 1
devices = [device for device in (keyboard, pointer) if device is not None]


def frame(device):
    ei_device_frame(device, ei_now(ei))


for device in devices:
    ei_device_start_emulating(device, sequence)
for action in chords:
    if action[0] == "key":
        for code in action[1]:
            ei_device_keyboard_key(keyboard, code, True)
            frame(keyboard)
        for code in reversed(action[1]):
            ei_device_keyboard_key(keyboard, code, False)
            frame(keyboard)
    elif action[0] == "pointer":
        ei_device_pointer_motion_absolute(pointer, action[1], action[2])
        frame(pointer)
    elif action[0] == "button":
        ei_device_button_button(pointer, action[2], action[1])
        frame(pointer)
    elif action[0] == "wait":
        end = time.monotonic() + action[1]
        while time.monotonic() < end:
            pump(min(0.05, max(0.0, end - time.monotonic())))
    # Une action à la fois : laisse KWin et la page traiter la précédente.
    pump(0.04)
for device in devices:
    ei_device_stop_emulating(device)
pump(0.05)
# Ferme aussi le descripteur, dont libei a pris possession.
ei_unref(ei)
bus.call_sync(
    "org.kde.KWin",
    "/org/kde/KWin/EIS/RemoteDesktop",
    "org.kde.KWin.EIS.RemoteDesktop",
    "disconnect",
    GLib.Variant("(i)", (cookie,)),
    None,
    Gio.DBusCallFlags.NONE,
    3000,
    None,
)
print(json.dumps({"sent": len(chords)}), flush=True)
