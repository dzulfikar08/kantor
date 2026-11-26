# Fake xdg-desktop-portal FileChooser on a private session bus: SaveFile
# answers with the path in $FAKE_SAVE_PATH (screenshots of Save As flows).
import os, sys
from gi.repository import Gio, GLib

SAVE = os.environ["FAKE_SAVE_PATH"]
XML = """
<node>
 <interface name="org.freedesktop.portal.FileChooser">
  <method name="OpenFile"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
  <method name="SaveFile"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
  <property name="version" type="u" access="read"/>
 </interface>
</node>"""
node = Gio.DBusNodeInfo.new_for_xml(XML)

def call(conn, sender, path, iface, method, params, inv):
    opts = params.unpack()[2]
    token = opts.get("handle_token", "t")
    handle = "/org/freedesktop/portal/desktop/request/%s/%s" % (sender[1:].replace(".", "_"), token)
    print("call", method, sender, handle, opts.keys(), flush=True)
    inv.return_value(GLib.Variant("(o)", (handle,)))
    def respond():
        uri = "file://" + SAVE
        res = GLib.Variant("(ua{sv})", (0, {"uris": GLib.Variant("as", [uri])}))
        conn.emit_signal(sender, handle, "org.freedesktop.portal.Request", "Response", res)
        print("responded", uri, flush=True)
        return False
    GLib.timeout_add(300, respond)

def getprop(conn, sender, path, iface, name):
    return GLib.Variant("u", 4)

def acquired(conn, name):
    conn.register_object("/org/freedesktop/portal/desktop", node.interfaces[0], call, getprop, None)
    print("ready", flush=True)

Gio.bus_own_name(Gio.BusType.SESSION, "org.freedesktop.portal.Desktop", Gio.BusNameOwnerFlags.NONE, acquired, None, None)
GLib.MainLoop().run()
