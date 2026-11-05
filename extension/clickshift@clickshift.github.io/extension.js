import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Shell from 'gi://Shell';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'io.github.clickshift.Daemon';
const OBJECT_PATH = '/io/github/clickshift/Daemon';
const INTERFACE = 'io.github.clickshift.Focus1';
const CALL_TIMEOUT_MS = 1000;

export default class ClickshiftExtension extends Extension {
    enable() {
        this._focusSignal = global.display.connect(
            'notify::focus-window',
            () => this._sendFocus()
        );
        // Also fires when the daemon starts after the shell, so it never waits
        // for the next focus change to learn the current app.
        this._busWatch = Gio.bus_watch_name(
            Gio.BusType.SESSION,
            BUS_NAME,
            Gio.BusNameWatcherFlags.NONE,
            () => this._sendFocus(),
            null
        );
    }

    disable() {
        global.display.disconnect(this._focusSignal);
        this._focusSignal = null;
        Gio.bus_unwatch_name(this._busWatch);
        this._busWatch = null;
    }

    _sendFocus() {
        const window = global.display.focus_window;
        const app = window
            ? Shell.WindowTracker.get_default().get_window_app(window)
            : null;

        Gio.DBus.session.call(
            BUS_NAME,
            OBJECT_PATH,
            INTERFACE,
            'SetFocus',
            new GLib.Variant('(ss)', [
                app?.get_id() ?? '',
                window?.get_wm_class() ?? '',
            ]),
            null,
            Gio.DBusCallFlags.NO_AUTO_START,
            CALL_TIMEOUT_MS,
            null,
            (connection, result) => {
                try {
                    connection.call_finish(result);
                } catch {
                    // The daemon is not running; the bus watch resends on start.
                }
            }
        );
    }
}
