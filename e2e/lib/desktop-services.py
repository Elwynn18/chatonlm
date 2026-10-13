#!/usr/bin/env python3
# Faux services D-Bus du bureau, sur le bus de la session virtuelle : ce que
# Plasma fournit à ChatonLM pour la dictée (étape 17) et la correction du texte
# sélectionné, joué et observé par les essais e2e.
#   - org.freedesktop.Notifications : journal des notifications reçues ;
#   - org.freedesktop.portal.Desktop : portail GlobalShortcuts (CreateSession,
#     BindShortcuts avec un ou plusieurs raccourcis, ConfigureShortcuts,
#     signaux Activated, Deactivated et ShortcutsChanged déclenchés par le
#     test) et registre des applications hors bac à sable
#     (org.freedesktop.host.portal.Registry) ; avec l'option
#     « remoteDesktop », portail RemoteDesktop (CreateSession, SelectDevices,
#     Start, NotifyKeyboardKeysym) : les touches que l'app envoie (Ctrl+V du
#     texte corrigé) sont notées, sans rien taper nulle part ;
#   - org.kde.StatusNotifierWatcher : inscription de l'icône de la zone de
#     notification, que le test pilote ensuite (Activate, menu dbusmenu).
# Les noms sont pris avant le lancement de l'app : le bus n'active donc pas les
# vrais services (xdg-desktop-portal, plasma_waitforname) de la session.
#
# Usage (dans scripts/e2e-session.sh seulement) :
#   python3 e2e/lib/desktop-services.py '<json>'
# JSON : {"notifications": bool, "watcher": bool,
#         "portal": null | {"version": 2, "bind": "ok"|"cancel"|"hang"|"empty",
#                           (« cancel » : refus, puis liaison sans touche,
#                           comme KDE ; « hang » : boîte laissée ouverte,
#                           jamais de réponse)
#                           "only": ["proofread"],
#                           (« bind » ne s'applique qu'aux liaisons qui
#                           contiennent l'un de ces raccourcis, et le refus
#                           à eux seuls ; les autres sont liées aussitôt)
#                           "trigger": "Ctrl+Alt+D",
#                           "triggers": {"proofread": "Ctrl+Alt+C", ...},
#                           (touche par identifiant, « trigger » sinon)
#                           "configureTrigger": "Ctrl+Alt+F",
#                           "remoteDesktop": "ok"|"cancel"}}
#                           (absent : pas de portail RemoteDesktop, comme un
#                           bureau qui n'envoie pas de touches ; « cancel » :
#                           autorisation refusée à Start)
# Entrée : une commande JSON par ligne, {"seq": n, "cmd": "...", ...} :
#   activated / deactivated {"id"}   signal du portail (toutes les sessions
#                                    ouvertes, même horodatage : comme KDE,
#                                    dont chaque session écoute le même
#                                    composant)
#   tap {"id"}                       Activated puis Deactivated d'un coup
#   portal-options {...}             options du portail changées (fusionnées)
#                                    pour les liaisons suivantes
#   shortcuts-changed {"trigger", "id"?}
#                                    nouvelle touche annoncée (de « id »
#                                    seulement s'il est donné)
#   item-activate                    clic gauche sur l'icône (Activate)
#   item-props                       propriétés de l'icône (titre, infobulle,
#                                    pixmaps résumées)
#   menu                             AboutToShow puis GetLayout du menu
#   menu-click {"label"}             clic sur l'entrée de ce libellé
#   menu-event {"id"}                clic sur un identifiant lu plus tôt
#   notification-action {"id", "key"}  clic sur une action d'une notification
#   notification-close {"id"}        notification fermée par l'utilisateur
#   watcher-off / watcher-on         zone de notification qui disparaît,
#                                    puis revient
# Sortie : une ligne JSON par évènement ({"event": "ready"}, "notify" (avec
# ses actions, [[clé, libellé], …]),
# "close-notification", "register-app", "create-session", "bind" (session,
# shortcuts : [[id, {description, preferred_trigger}], …], ids, et
# « answered » : touches répondues, {id: touche}, absent sans réponse),
# "configure", "close-session", "rd-select" (session, types, persistMode,
# restoreToken reçu ou null), "rd-start" (session, granted, token rendu),
# "rd-key" (keysym, state : 1 pressée, 0 relâchée),
# "item-registered", "reply" avec le même
# « seq » qu'une commande : {"result": ...} ou {"error": "..."}).
# Codes de sortie : 0 fin (entrée fermée) ; 3 hors de la session virtuelle
# (refusé avant toute connexion au bus) ; 1 autre erreur. Appelé par
# e2e/lib/desktop.mjs (startDesktopServices).
import hashlib
import json
import os
import re
import stat
import sys

ROOT = os.path.realpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
RUNS = os.path.join(ROOT, ".tools", "e2e-runs")


def outside_session():
    """Raison du refus si l'on n'est pas dans scripts/e2e-session.sh, sinon None.

    Mêmes contrôles que clipboard-reader.py (variable, socket Wayland et
    dossier d'exécution de la session, HOME sous .tools/e2e-runs/), plus le
    bus : celui de dbus-run-session, jamais celui du bureau réel
    (/run/user/...).
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
    bus = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    if not bus or "/run/user/" in bus:
        return f"DBUS_SESSION_BUS_ADDRESS={bus!r} n'est pas le bus de la session"
    return None


reason = outside_session()
if reason is not None:
    print(
        f"Faux services du bureau refusés hors de la session virtuelle"
        f" (scripts/e2e-session.sh) : {reason}.",
        file=sys.stderr,
    )
    sys.exit(3)

import gi  # noqa: E402

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

options = json.loads(sys.argv[1]) if len(sys.argv) > 1 else {}
portal_options = options.get("portal")


def emit(event, **fields):
    print(json.dumps({"event": event, **fields}, ensure_ascii=False), flush=True)


def plain(value):
    """Valeur D-Bus déballée → JSON (octets résumés)."""
    if isinstance(value, GLib.Variant):
        value = value.unpack()
    if isinstance(value, (bytes, bytearray)):
        return {"bytes": len(value)}
    if isinstance(value, dict):
        return {str(k): plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [plain(v) for v in value]
    return value


bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
unique = bus.get_unique_name()
loop = GLib.MainLoop()

# --- Notifications -------------------------------------------------------

NOTIFICATIONS_XML = """
<node>
  <interface name="org.freedesktop.Notifications">
    <method name="Notify">
      <arg type="s" direction="in"/><arg type="u" direction="in"/>
      <arg type="s" direction="in"/><arg type="s" direction="in"/>
      <arg type="s" direction="in"/><arg type="as" direction="in"/>
      <arg type="a{sv}" direction="in"/><arg type="i" direction="in"/>
      <arg type="u" direction="out"/>
    </method>
    <method name="CloseNotification"><arg type="u" direction="in"/></method>
    <method name="GetCapabilities"><arg type="as" direction="out"/></method>
    <method name="GetServerInformation">
      <arg type="s" direction="out"/><arg type="s" direction="out"/>
      <arg type="s" direction="out"/><arg type="s" direction="out"/>
    </method>
    <signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
    <signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
  </interface>
</node>"""

notification_ids = {"next": 1}
# Client qui a envoyé chaque notification (destinataire de ses signaux).
notification_senders = {}


def pairs(actions):
    """Actions D-Bus [clé, libellé, clé, libellé…] → [[clé, libellé], …]."""
    return [[actions[i], actions[i + 1]] for i in range(0, len(actions) - 1, 2)]


def on_notifications(connection, sender, path, interface, method, parameters, invocation):
    if method == "Notify":
        app_name, replaces, icon, summary, body, actions, hints, timeout = parameters.unpack()
        if replaces:
            nid = replaces
        else:
            nid = notification_ids["next"]
            notification_ids["next"] += 1
        notification_senders[nid] = sender
        emit(
            "notify",
            id=nid,
            appName=app_name,
            replacesId=replaces,
            icon=icon,
            summary=summary,
            body=body,
            actions=pairs(actions),
            hints=plain(hints),
            timeout=timeout,
        )
        invocation.return_value(GLib.Variant("(u)", (nid,)))
    elif method == "CloseNotification":
        (nid,) = parameters.unpack()
        emit("close-notification", id=nid)
        invocation.return_value(None)
        connection.emit_signal(
            sender, path, interface, "NotificationClosed", GLib.Variant("(uu)", (nid, 3))
        )
    elif method == "GetCapabilities":
        invocation.return_value(
            GLib.Variant("(as)", (["body", "body-markup", "icon-static", "persistence"],))
        )
    elif method == "GetServerInformation":
        invocation.return_value(
            GLib.Variant("(ssss)", ("chatonlm-e2e", "ChatonLM tests", "1.0", "1.2"))
        )
    else:
        invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method)


# --- Portail GlobalShortcuts ---------------------------------------------

PORTAL_XML = """
<node>
  <interface name="org.freedesktop.portal.GlobalShortcuts">
    <method name="CreateSession">
      <arg type="a{sv}" direction="in"/><arg type="o" direction="out"/>
    </method>
    <method name="BindShortcuts">
      <arg type="o" direction="in"/><arg type="a(sa{sv})" direction="in"/>
      <arg type="s" direction="in"/><arg type="a{sv}" direction="in"/>
      <arg type="o" direction="out"/>
    </method>
    <method name="ListShortcuts">
      <arg type="o" direction="in"/><arg type="a{sv}" direction="in"/>
      <arg type="o" direction="out"/>
    </method>
    <method name="ConfigureShortcuts">
      <arg type="o" direction="in"/><arg type="s" direction="in"/>
      <arg type="a{sv}" direction="in"/>
    </method>
    <signal name="Activated">
      <arg type="o"/><arg type="s"/><arg type="t"/><arg type="a{sv}"/>
    </signal>
    <signal name="Deactivated">
      <arg type="o"/><arg type="s"/><arg type="t"/><arg type="a{sv}"/>
    </signal>
    <signal name="ShortcutsChanged">
      <arg type="o"/><arg type="a(sa{sv})"/>
    </signal>
    <property name="version" type="u" access="read"/>
  </interface>
  <interface name="org.freedesktop.host.portal.Registry">
    <method name="Register">
      <arg type="s" direction="in"/><arg type="a{sv}" direction="in"/>
    </method>
    <property name="version" type="u" access="read"/>
  </interface>
</node>"""

SESSION_XML = """
<node>
  <interface name="org.freedesktop.portal.Session">
    <method name="Close"/>
    <signal name="Closed"><arg type="a{sv}"/></signal>
    <property name="version" type="u" access="read"/>
  </interface>
</node>"""

PORTAL_PATH = "/org/freedesktop/portal/desktop"
GLOBAL_SHORTCUTS = "org.freedesktop.portal.GlobalShortcuts"
# Sessions ouvertes : chemin → (propriétaire, identifiant d'objet).
sessions = {}
# Touche actuelle de chaque raccourci lié (id → description).
triggers = {}
# Raccourcis refusés une fois (mode « cancel »).
refused = set()


def escaped(sender):
    return sender.lstrip(":").replace(".", "_")


def token(opts, key):
    value = opts.get(key)
    return value if value else "t"


def respond(sender, request, code, results):
    """Signal Response sur l'objet Request attendu par le client."""
    bus.emit_signal(
        sender,
        request,
        "org.freedesktop.portal.Request",
        "Response",
        GLib.Variant("(ua{sv})", (code, results)),
    )


def trigger_for(sid):
    """Touche répondue pour le raccourci `sid` (option « triggers », sinon
    « trigger »)."""
    per_id = portal_options.get("triggers") or {}
    return per_id.get(sid, portal_options.get("trigger", "Ctrl+Alt+D"))


def shortcut_list(entries):
    return [
        (sid, {"description": GLib.Variant("s", desc), "trigger_description": GLib.Variant("s", trig)})
        for sid, desc, trig in entries
    ]


def on_session(connection, sender, path, interface, method, parameters, invocation):
    if method == "Close":
        invocation.return_value(None)
        close_session(path)
    else:
        invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method)


def close_session(path):
    entry = sessions.pop(path, None)
    if entry is not None:
        bus.unregister_object(entry[1])
        emit("close-session", session=path)


def on_portal(connection, sender, path, interface, method, parameters, invocation):
    args = parameters.unpack()
    if interface == "org.freedesktop.host.portal.Registry" and method == "Register":
        emit("register-app", appId=args[0], sender=sender)
        invocation.return_value(None)
        return
    if method == "CreateSession":
        (opts,) = args
        request = f"{PORTAL_PATH}/request/{escaped(sender)}/{token(opts, 'handle_token')}"
        session = f"{PORTAL_PATH}/session/{escaped(sender)}/{token(opts, 'session_handle_token')}"
        node = Gio.DBusNodeInfo.new_for_xml(SESSION_XML)
        object_id = bus.register_object(session, node.interfaces[0], on_session, on_property, None)
        sessions[session] = (sender, object_id)
        emit("create-session", session=session, sender=sender)
        invocation.return_value(GLib.Variant("(o)", (request,)))
        respond(sender, request, 0, {"session_handle": GLib.Variant("s", session)})
    elif method == "BindShortcuts":
        session, shortcuts, parent, opts = args
        request = f"{PORTAL_PATH}/request/{escaped(sender)}/{token(opts, 'handle_token')}"
        ids = [sid for sid, _ in shortcuts]
        mode = portal_options.get("bind", "ok")
        only = portal_options.get("only")
        # Raccourcis que la boîte demande (« only » : ceux-là seulement ; une
        # liaison sans eux est faite aussitôt).
        asked = ids if only is None else [sid for sid in ids if sid in only]
        if not asked:
            mode = "ok"
        # Comme KDE : un raccourci refusé reste inscrit sans touche, et une
        # nouvelle liaison répond aussitôt, sans rouvrir la boîte.
        refusing = mode == "cancel" and not all(sid in refused for sid in asked)
        answered = {}
        if mode != "hang" and not refusing:
            for sid in ids:
                answered[sid] = "" if mode == "empty" or sid in refused else trigger_for(sid)
        bind = {"session": session, "shortcuts": plain(shortcuts), "ids": ids, "parentWindow": parent}
        if mode != "hang":
            bind["answered"] = answered
        emit("bind", **bind)
        invocation.return_value(GLib.Variant("(o)", (request,)))
        if mode == "hang":
            return
        if refusing:
            refused.update(asked)
            for sid in asked:
                triggers[sid] = ""
            respond(sender, request, 1, {})
            return
        entries = []
        for sid, info in shortcuts:
            triggers[sid] = answered[sid]
            entries.append((sid, info.get("description", ""), answered[sid]))
        respond(
            sender,
            request,
            0,
            {"shortcuts": GLib.Variant("a(sa{sv})", shortcut_list(entries))},
        )
    elif method == "ListShortcuts":
        session, opts = args
        request = f"{PORTAL_PATH}/request/{escaped(sender)}/{token(opts, 'handle_token')}"
        invocation.return_value(GLib.Variant("(o)", (request,)))
        entries = [(sid, "", trig) for sid, trig in triggers.items()]
        respond(sender, request, 0, {"shortcuts": GLib.Variant("a(sa{sv})", shortcut_list(entries))})
    elif method == "ConfigureShortcuts":
        session, parent, opts = args
        emit("configure", session=session, options=plain(opts))
        invocation.return_value(None)
        trigger = portal_options.get("configureTrigger")
        if trigger:
            shortcuts_changed(trigger)
    else:
        invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method)


def on_property(connection, sender, path, interface, name):
    if name == "version":
        if interface == GLOBAL_SHORTCUTS:
            return GLib.Variant("u", portal_options.get("version", 2))
        if interface == REMOTE_DESKTOP:
            return GLib.Variant("u", 2)
        return GLib.Variant("u", 1)
    if name == "AvailableDeviceTypes":
        # Clavier seulement.
        return GLib.Variant("u", 1)
    return None


# --- Portail RemoteDesktop (collage du texte corrigé) ---------------------

REMOTE_DESKTOP = "org.freedesktop.portal.RemoteDesktop"
REMOTE_DESKTOP_XML = """
<node>
  <interface name="org.freedesktop.portal.RemoteDesktop">
    <method name="CreateSession">
      <arg type="a{sv}" direction="in"/><arg type="o" direction="out"/>
    </method>
    <method name="SelectDevices">
      <arg type="o" direction="in"/><arg type="a{sv}" direction="in"/>
      <arg type="o" direction="out"/>
    </method>
    <method name="Start">
      <arg type="o" direction="in"/><arg type="s" direction="in"/>
      <arg type="a{sv}" direction="in"/><arg type="o" direction="out"/>
    </method>
    <method name="NotifyKeyboardKeysym">
      <arg type="o" direction="in"/><arg type="a{sv}" direction="in"/>
      <arg type="i" direction="in"/><arg type="u" direction="in"/>
    </method>
    <property name="AvailableDeviceTypes" type="u" access="read"/>
    <property name="version" type="u" access="read"/>
  </interface>
</node>"""

# Jetons d'autorisation rendus par Start : « rd-token-1 », « rd-token-2 »…
rd_tokens = {"count": 0}


def on_remote_desktop(connection, sender, path, interface, method, parameters, invocation):
    args = parameters.unpack()
    if method == "CreateSession":
        (opts,) = args
        request = f"{PORTAL_PATH}/request/{escaped(sender)}/{token(opts, 'handle_token')}"
        session = f"{PORTAL_PATH}/session/{escaped(sender)}/{token(opts, 'session_handle_token')}"
        node = Gio.DBusNodeInfo.new_for_xml(SESSION_XML)
        object_id = bus.register_object(session, node.interfaces[0], on_session, on_property, None)
        sessions[session] = (sender, object_id)
        emit("create-session", session=session, sender=sender, portal="remote-desktop")
        invocation.return_value(GLib.Variant("(o)", (request,)))
        respond(sender, request, 0, {"session_handle": GLib.Variant("s", session)})
    elif method == "SelectDevices":
        session, opts = args
        request = f"{PORTAL_PATH}/request/{escaped(sender)}/{token(opts, 'handle_token')}"
        emit(
            "rd-select",
            session=session,
            types=opts.get("types"),
            persistMode=opts.get("persist_mode"),
            restoreToken=opts.get("restore_token"),
        )
        invocation.return_value(GLib.Variant("(o)", (request,)))
        respond(sender, request, 0, {})
    elif method == "Start":
        session, parent, opts = args
        request = f"{PORTAL_PATH}/request/{escaped(sender)}/{token(opts, 'handle_token')}"
        invocation.return_value(GLib.Variant("(o)", (request,)))
        if portal_options.get("remoteDesktop") == "cancel":
            emit("rd-start", session=session, granted=False, token=None)
            respond(sender, request, 1, {})
            return
        rd_tokens["count"] += 1
        granted = f"rd-token-{rd_tokens['count']}"
        emit("rd-start", session=session, granted=True, token=granted)
        respond(
            sender,
            request,
            0,
            {"devices": GLib.Variant("u", 1), "restore_token": GLib.Variant("s", granted)},
        )
    elif method == "NotifyKeyboardKeysym":
        session, opts, keysym, state = args
        emit("rd-key", session=session, keysym=keysym, state=state)
        invocation.return_value(None)
    else:
        invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method)


def signal_all(name, shortcut_id):
    # Un appui, un horodatage, signalé à chaque session ouverte.
    timestamp = GLib.get_real_time() // 1000
    for session in list(sessions):
        bus.emit_signal(
            None,
            PORTAL_PATH,
            GLOBAL_SHORTCUTS,
            name,
            GLib.Variant("(osta{sv})", (session, shortcut_id, timestamp, {})),
        )
    return len(sessions)


def shortcuts_changed(trigger, only=None):
    """Nouvelle touche de tous les raccourcis liés, ou du seul `only` ; le
    changement liste les raccourcis touchés."""
    changed = [sid for sid in triggers if only is None or sid == only]
    for sid in changed:
        triggers[sid] = trigger
    entries = [(sid, "", triggers[sid]) for sid in changed]
    for session in list(sessions):
        bus.emit_signal(
            None,
            PORTAL_PATH,
            GLOBAL_SHORTCUTS,
            "ShortcutsChanged",
            GLib.Variant("(oa(sa{sv}))", (session, shortcut_list(entries))),
        )
    return len(sessions)


# --- Zone de notification ------------------------------------------------

WATCHER_XML = """
<node>
  <interface name="org.kde.StatusNotifierWatcher">
    <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
    <method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method>
    <signal name="StatusNotifierItemRegistered"><arg type="s"/></signal>
    <signal name="StatusNotifierItemUnregistered"><arg type="s"/></signal>
    <signal name="StatusNotifierHostRegistered"/>
    <signal name="StatusNotifierHostUnregistered"/>
    <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
    <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
    <property name="ProtocolVersion" type="i" access="read"/>
  </interface>
</node>"""

WATCHER_NAME = "org.kde.StatusNotifierWatcher"
WATCHER_PATH = "/StatusNotifierWatcher"
item = {"service": None, "path": None}


def on_watcher(connection, sender, path, interface, method, parameters, invocation):
    if method == "RegisterStatusNotifierItem":
        (service,) = parameters.unpack()
        if service.startswith("/"):
            item.update(service=sender, path=service)
        else:
            item.update(service=service, path="/StatusNotifierItem")
        emit("item-registered", service=item["service"], path=item["path"], sender=sender)
        invocation.return_value(None)
        connection.emit_signal(
            None, WATCHER_PATH, WATCHER_NAME, "StatusNotifierItemRegistered",
            GLib.Variant("(s)", (f"{item['service']}{item['path']}",)),
        )
    elif method == "RegisterStatusNotifierHost":
        invocation.return_value(None)
    else:
        invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method)


def on_watcher_property(connection, sender, path, interface, name):
    if name == "RegisteredStatusNotifierItems":
        registered = [f"{item['service']}{item['path']}"] if item["service"] else []
        return GLib.Variant("as", registered)
    if name == "IsStatusNotifierHostRegistered":
        return GLib.Variant("b", True)
    if name == "ProtocolVersion":
        return GLib.Variant("i", 0)
    return None


def call(service, path, interface, method, params, reply_type, done, fail):
    def finish(connection, result):
        try:
            reply = connection.call_finish(result)
        except GLib.Error as error:
            fail(error.message)
            return
        done(reply.unpack() if reply is not None else None)

    bus.call(service, path, interface, method, params, reply_type,
             Gio.DBusCallFlags.NONE, 10000, None, finish)


def summarize_icons(pixmaps):
    summary = []
    for width, height, data in pixmaps:
        data = bytes(data)
        # Pixel du centre de la pastille (en bas à droite).
        x, y = int(width * 0.76), int(height * 0.76)
        start = (y * width + x) * 4
        summary.append({
            "width": width,
            "height": height,
            "sha1": hashlib.sha1(data).hexdigest(),
            "badge": list(data[start:start + 4]),
        })
    return summary


def menu_items(layout):
    """Arbre dbusmenu (ia{sv}av) → liste plate des entrées du premier niveau."""
    _, _, children = layout
    items = []
    for child in children:
        if isinstance(child, GLib.Variant):
            child = child.unpack()
        cid, props, _ = child
        props = {k: (v.unpack() if isinstance(v, GLib.Variant) else v) for k, v in props.items()}
        items.append({
            "id": cid,
            "type": props.get("type", "standard"),
            "label": props.get("label", ""),
            "enabled": props.get("enabled", True),
            "visible": props.get("visible", True),
        })
    return items


def with_menu(done, fail):
    """AboutToShow(0) comme Plasma avant d'ouvrir le menu, puis GetLayout."""
    if not item["service"]:
        fail("aucune icône inscrite")
        return

    def on_menu_path(props):
        (menu_path,) = props

        def layout():
            call(item["service"], menu_path, "com.canonical.dbusmenu", "GetLayout",
                 GLib.Variant("(iias)", (0, -1, [])), None,
                 lambda reply: done(menu_path, menu_items(reply[1])), fail)

        call(item["service"], menu_path, "com.canonical.dbusmenu", "AboutToShow",
             GLib.Variant("(i)", (0,)), None, lambda _: layout(), fail)

    call(item["service"], item["path"], "org.freedesktop.DBus.Properties", "Get",
         GLib.Variant("(ss)", ("org.kde.StatusNotifierItem", "Menu")), None,
         lambda reply: on_menu_path((reply[0],)), fail)


owners = {}
pending = set()


def own(name, path, xml, handler, getter=None):
    node = Gio.DBusNodeInfo.new_for_xml(xml)
    if path is not None:
        for interface in node.interfaces:
            bus.register_object(path, interface, handler, getter, None)
    pending.add(name)

    def acquired(connection, acquired_name):
        if acquired_name in pending:
            pending.discard(acquired_name)
            if not pending:
                emit("ready", unique=unique)

    def lost(connection, lost_name):
        emit("name-lost", name=lost_name)

    owners[name] = Gio.bus_own_name_on_connection(
        bus, name, Gio.BusNameOwnerFlags.NONE, acquired, lost
    )


def handle(command):
    seq = command.get("seq")

    def done(result=None):
        emit("reply", seq=seq, result=result)

    def fail(message):
        emit("reply", seq=seq, error=message)

    name = command.get("cmd")
    try:
        if name in ("activated", "deactivated"):
            signal = "Activated" if name == "activated" else "Deactivated"
            done(signal_all(signal, command.get("id", "dictation")))
        elif name == "tap":
            # Appui et relâchement émis d'un coup (touche à peine effleurée) :
            # ils arrivent ensemble chez le client.
            shortcut_id = command.get("id", "dictation")
            signal_all("Activated", shortcut_id)
            done(signal_all("Deactivated", shortcut_id))
        elif name == "shortcuts-changed":
            done(shortcuts_changed(command["trigger"], command.get("id")))
        elif name == "portal-options":
            if portal_options is None:
                fail("pas de portail")
                return
            portal_options.update({k: v for k, v in command.items() if k not in ("seq", "cmd")})
            done(portal_options)
        elif name == "item-activate":
            if not item["service"]:
                fail("aucune icône inscrite")
                return
            call(item["service"], item["path"], "org.kde.StatusNotifierItem", "Activate",
                 GLib.Variant("(ii)", (0, 0)), None, lambda _: done(), fail)
        elif name == "item-props":
            if not item["service"]:
                fail("aucune icône inscrite")
                return

            def got(reply):
                (props,) = reply
                tooltip = props.get("ToolTip")
                done({
                    "title": props.get("Title"),
                    "id": props.get("Id"),
                    "status": props.get("Status"),
                    "menu": props.get("Menu"),
                    "tooltip": {"title": tooltip[2], "description": tooltip[3]} if tooltip else None,
                    "icons": summarize_icons(props.get("IconPixmap", [])),
                })

            call(item["service"], item["path"], "org.freedesktop.DBus.Properties", "GetAll",
                 GLib.Variant("(s)", ("org.kde.StatusNotifierItem",)), None, got, fail)
        elif name == "menu":
            with_menu(lambda path, items: done(items), fail)
        elif name == "menu-click":
            label = command["label"]

            def click(menu_path, items):
                match = [entry for entry in items if entry["label"] == label]
                if not match:
                    fail(f"entrée introuvable : {label}")
                    return
                call(item["service"], menu_path, "com.canonical.dbusmenu", "Event",
                     GLib.Variant("(isvu)", (match[0]["id"], "clicked", GLib.Variant("i", 0), 0)),
                     None, lambda _: done(match[0]), fail)

            with_menu(click, fail)
        elif name == "menu-event":
            # Clic sur un identifiant d'un menu lu plus tôt, sans le relire
            # (menu resté ouvert pendant que l'état change).
            if not item["service"]:
                fail("aucune icône inscrite")
                return

            def on_path(reply):
                call(item["service"], reply[0], "com.canonical.dbusmenu", "Event",
                     GLib.Variant("(isvu)", (command["id"], "clicked", GLib.Variant("i", 0), 0)),
                     None, lambda _: done(), fail)

            call(item["service"], item["path"], "org.freedesktop.DBus.Properties", "Get",
                 GLib.Variant("(ss)", ("org.kde.StatusNotifierItem", "Menu")), None, on_path, fail)
        elif name in ("notification-action", "notification-close"):
            # Clic sur une action de la notification, ou fermeture (raison 2 :
            # par l'utilisateur), signalés au client qui l'a envoyée.
            nid = command["id"]
            sender = notification_senders.get(nid)
            if sender is None:
                fail(f"notification inconnue : {nid}")
                return
            if name == "notification-action":
                signal = ("ActionInvoked", GLib.Variant("(us)", (nid, command["key"])))
            else:
                signal = ("NotificationClosed", GLib.Variant("(uu)", (nid, command.get("reason", 2))))
            bus.emit_signal(sender, "/org/freedesktop/Notifications",
                            "org.freedesktop.Notifications", *signal)
            done()
        elif name == "watcher-off":
            Gio.bus_unown_name(owners.pop(WATCHER_NAME))
            item.update(service=None, path=None)
            done()
        elif name == "watcher-on":
            own(WATCHER_NAME, None, WATCHER_XML, on_watcher, on_watcher_property)
            done()
        else:
            fail(f"commande inconnue : {name}")
    except Exception as error:  # noqa: BLE001 — renvoyé au test
        fail(f"{type(error).__name__}: {error}")


stdin_buffer = bytearray()


def on_stdin(fd, condition):
    """Commandes reçues : plusieurs lignes peuvent arriver d'un coup."""
    data = os.read(fd, 65536)
    if not data:
        loop.quit()
        return False
    stdin_buffer.extend(data)
    while b"\n" in stdin_buffer:
        index = stdin_buffer.index(b"\n")
        line = stdin_buffer[:index].decode("utf-8", "replace").strip()
        del stdin_buffer[: index + 1]
        if line:
            try:
                handle(json.loads(line))
            except json.JSONDecodeError:
                emit("reply", seq=None, error=f"ligne illisible : {line}")
    return True


if options.get("notifications"):
    own("org.freedesktop.Notifications", "/org/freedesktop/Notifications",
        NOTIFICATIONS_XML, on_notifications)
if portal_options is not None:
    if portal_options.get("remoteDesktop"):
        # Même objet que GlobalShortcuts, interface à part ; inscrite avant
        # la prise du nom : présente dès que le portail répond.
        rd_node = Gio.DBusNodeInfo.new_for_xml(REMOTE_DESKTOP_XML)
        bus.register_object(
            PORTAL_PATH, rd_node.interfaces[0], on_remote_desktop, on_property, None
        )
    own("org.freedesktop.portal.Desktop", PORTAL_PATH, PORTAL_XML, on_portal, on_property)
if options.get("watcher"):
    node = Gio.DBusNodeInfo.new_for_xml(WATCHER_XML)
    bus.register_object(WATCHER_PATH, node.interfaces[0], on_watcher, on_watcher_property, None)
    own(WATCHER_NAME, None, WATCHER_XML, on_watcher, on_watcher_property)
if not owners:
    emit("ready", unique=unique)

GLib.io_add_watch(sys.stdin.fileno(), GLib.IO_IN | GLib.IO_HUP | GLib.IO_ERR, on_stdin)
loop.run()
