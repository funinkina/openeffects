// D-Bus client for the OpenEffects daemon (session bus in.co.funinkina.Daemon).
// Wraps the three daemon interfaces, exposes a flat state snapshot via refresh(),
// and emits a single 'changed' signal whenever the daemon's state moves so the UI
// can re-sync (covers changes made by the GTK app or openeffectsctl too).

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';

export const BUS_NAME = 'in.co.funinkina.Daemon';
export const OBJECT_PATH = '/in/co/funinkina/Daemon';

// Interface XML copied verbatim from data/dbus/*.xml. Only what the UI consumes.
const DAEMON_XML = `
<node>
  <interface name="in.co.funinkina.Daemon1">
    <method name="Start" />
    <method name="Stop" />
    <method name="Quit" />
    <property name="Status" type="s" access="read" />
    <property name="Capabilities" type="a{sv}" access="read" />
    <signal name="StatusChanged"><arg name="new_status" type="s" /></signal>
  </interface>
</node>`;

const EFFECTS_XML = `
<node>
  <interface name="in.co.funinkina.Effects1">
    <method name="ListEffects"><arg name="effect_ids" type="as" direction="out" /></method>
    <method name="SetEnabled"><arg name="id" type="s" direction="in" /><arg name="on" type="b" direction="in" /></method>
    <method name="SetParam"><arg name="id" type="s" direction="in" /><arg name="key" type="s" direction="in" /><arg name="value" type="v" direction="in" /></method>
    <method name="SetBypass"><arg name="on" type="b" direction="in" /></method>
    <method name="GetBypass"><arg name="on" type="b" direction="out" /></method>
    <method name="TriggerReaction"><arg name="id" type="s" direction="in" /></method>
    <method name="GetParams"><arg name="id" type="s" direction="in" /><arg name="params" type="a{sv}" direction="out" /></method>
    <method name="GetAllState"><arg name="state" type="a{sv}" direction="out" /></method>
    <signal name="EffectChanged"><arg name="id" type="s" /><arg name="params" type="a{sv}" /></signal>
    <signal name="BypassChanged"><arg name="on" type="b" /></signal>
  </interface>
</node>`;

const DEVICES_XML = `
<node>
  <interface name="in.co.funinkina.Devices1">
    <method name="ListCameras"><arg name="cameras" type="aa{sv}" direction="out" /></method>
    <method name="SelectCamera"><arg name="id" type="s" direction="in" /></method>
    <property name="ActiveCamera" type="s" access="read" />
    <property name="VirtualCameraInfo" type="a{sv}" access="read" />
  </interface>
</node>`;

const DaemonProxy = Gio.DBusProxy.makeProxyWrapper(DAEMON_XML);
const EffectsProxy = Gio.DBusProxy.makeProxyWrapper(EFFECTS_XML);
const DevicesProxy = Gio.DBusProxy.makeProxyWrapper(DEVICES_XML);

// Normalize a GetAllState/ListCameras value (deepUnpack may leave inner `v`
// values as GLib.Variant depending on GJS version) into a native JS value.
function toNative(value) {
    return value instanceof GLib.Variant ? value.recursiveUnpack() : value;
}

function get(dict, key, fallback) {
    return key in dict ? dict[key] : fallback;
}

export const Client = GObject.registerClass({
    Signals: {
        'changed': {},
        'availability-changed': { param_types: [GObject.TYPE_BOOLEAN] },
    },
}, class OpenEffectsClient extends GObject.Object {
    _init() {
        super._init();
        this._available = false;
        this._signalIds = [];
        this._proxies = [];

        const flags = Gio.DBusProxyFlags.DO_NOT_AUTO_START;
        const bus = Gio.DBus.session;
        // Sync init is non-activating with DO_NOT_AUTO_START and cheap on the
        // session bus; guard so a hiccup never breaks enable().
        try {
            this._daemon = new DaemonProxy(bus, BUS_NAME, OBJECT_PATH, null, null, flags);
            this._effects = new EffectsProxy(bus, BUS_NAME, OBJECT_PATH, null, null, flags);
            this._devices = new DevicesProxy(bus, BUS_NAME, OBJECT_PATH, null, null, flags);
            this._proxies = [this._daemon, this._effects, this._devices];
        } catch (e) {
            logError(e, 'OpenEffects: failed to create D-Bus proxies');
        }

        // Daemon-side state changes -> re-sync the UI.
        this._connectSignal(this._effects, 'EffectChanged', () => this.emit('changed'));
        this._connectSignal(this._effects, 'BypassChanged', () => this.emit('changed'));
        this._connectSignal(this._daemon, 'StatusChanged', () => this.emit('changed'));
        if (this._devices) {
            const id = this._devices.connect('g-properties-changed', () => this.emit('changed'));
            this._signalIds.push([this._devices, id, false]);
        }

        this._watchId = Gio.bus_watch_name(
            Gio.BusType.SESSION, BUS_NAME, Gio.BusNameWatcherFlags.NONE,
            () => this._setAvailable(true),
            () => this._setAvailable(false));
    }

    get available() {
        return this._available;
    }

    _connectSignal(proxy, name, cb) {
        if (!proxy)
            return;
        const id = proxy.connectSignal(name, cb);
        this._signalIds.push([proxy, id, true]);
    }

    _setAvailable(on) {
        if (this._available === on)
            return;
        this._available = on;
        this.emit('availability-changed', on);
        this.emit('changed');
    }

    _callAsync(proxy, method, args) {
        return new Promise((resolve, reject) => {
            if (!proxy)
                return reject(new Error('proxy unavailable'));
            proxy[method](...args, (ret, err) => err ? reject(err) : resolve(ret));
        });
    }

    _fire(proxy, method, args) {
        if (!proxy || !this._available)
            return;
        try {
            proxy[method](...args, (_ret, err) => {
                if (err)
                    logError(err, `OpenEffects: ${method} failed`);
            });
        } catch (e) {
            logError(e, `OpenEffects: ${method} threw`);
        }
    }

    // --- mutating helpers (fire-and-forget; daemon echoes back via signals) ---

    setBypass(on) { this._fire(this._effects, 'SetBypassRemote', [on]); }

    setEnabled(id, on) { this._fire(this._effects, 'SetEnabledRemote', [id, on]); }

    // value must be a GLib.Variant of the exact type the daemon expects
    // (s for zoom/mode/background, u for crop/strength/intensity/contrast,
    // i for brightness/bg_brightness). The proxy auto-boxes it into the `v` arg.
    setParam(id, key, value) { this._fire(this._effects, 'SetParamRemote', [id, key, value]); }

    trigger(id) { this._fire(this._effects, 'TriggerReactionRemote', [id]); }

    selectCamera(id) { this._fire(this._devices, 'SelectCameraRemote', [id]); }

    // Activate the daemon via its D-Bus .service file (no auto-start flag set on
    // our proxies, so we ask the bus explicitly). Used when the master toggle is
    // flipped on while the daemon is down.
    start() {
        Gio.DBus.session.call(
            'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
            'StartServiceByName', new GLib.Variant('(su)', [BUS_NAME, 0]), null,
            Gio.DBusCallFlags.NONE, -1, null,
            (conn, res) => {
                try { conn.call_finish(res); }
                catch (e) { logError(e, 'OpenEffects: StartServiceByName failed'); }
            });
    }

    // Full snapshot the UI renders from.
    async refresh() {
        const state = {
            available: this._available,
            status: 'stopped',
            bypass: false,
            cs: { enabled: false, zoom: 'normal', mode: 'single' },
            blur: { enabled: false, strength: 50 },
            bg: { enabled: false, background: '' },
            studio: { enabled: false, intensity: 50, brightness: 0, contrast: 50, bg_brightness: 0 },
            reactions: { enabled: false },
            cameras: [],
            activeCamera: '',
        };
        if (!this._available)
            return state;

        try {
            const [raw] = await this._callAsync(this._effects, 'GetAllStateRemote', []);
            const d = {};
            for (const k in raw)
                d[k] = toNative(raw[k]);

            state.bypass = !!get(d, 'bypass', false);
            state.cs = {
                enabled: !!get(d, 'center_stage.enabled', false),
                zoom: get(d, 'center_stage.zoom', 'normal'),
                mode: get(d, 'center_stage.mode', 'single'),
            };
            state.blur = {
                enabled: !!get(d, 'portrait_blur.enabled', false),
                strength: Number(get(d, 'portrait_blur.strength', 50)),
            };
            state.bg = {
                enabled: !!get(d, 'bg_replace.enabled', false),
                background: get(d, 'bg_replace.background', ''),
            };
            state.studio = {
                enabled: !!get(d, 'studio_light.enabled', false),
                intensity: Number(get(d, 'studio_light.intensity', 50)),
                brightness: Number(get(d, 'studio_light.brightness', 0)),
                contrast: Number(get(d, 'studio_light.contrast', 50)),
                bg_brightness: Number(get(d, 'studio_light.bg_brightness', 0)),
            };
            state.reactions = { enabled: !!get(d, 'reactions.enabled', false) };
        } catch (e) {
            logError(e, 'OpenEffects: GetAllState failed');
        }

        try {
            const status = this._daemon?.Status;
            if (status)
                state.status = status;
        } catch { /* cached prop may be absent right after activation */ }

        try {
            const [list] = await this._callAsync(this._devices, 'ListCamerasRemote', []);
            state.cameras = (list || []).map(entry => {
                const cam = {};
                for (const k in entry)
                    cam[k] = toNative(entry[k]);
                return { id: cam.id ?? '', name: cam.name ?? cam.id ?? '', active: !!cam.active };
            });
        } catch (e) {
            logError(e, 'OpenEffects: ListCameras failed');
        }

        try {
            const active = this._devices?.ActiveCamera;
            state.activeCamera = active || state.cameras.find(c => c.active)?.id || '';
        } catch { /* ignore */ }

        return state;
    }

    destroy() {
        if (this._watchId) {
            Gio.bus_unwatch_name(this._watchId);
            this._watchId = 0;
        }
        for (const [proxy, id, isDbusSignal] of this._signalIds) {
            try {
                if (isDbusSignal)
                    proxy.disconnectSignal(id);
                else
                    proxy.disconnect(id);
            } catch { /* proxy may be gone */ }
        }
        this._signalIds = [];
        this._proxies = [];
        this._daemon = null;
        this._effects = null;
        this._devices = null;
    }
});
