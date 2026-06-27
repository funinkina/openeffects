// Panel button + popup. Owns the D-Bus client, composes the sections, and
// re-syncs every control whenever the daemon's state changes or the menu opens.

import GObject from 'gi://GObject';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import {Client} from './dbus.js';
import * as Sections from './sections.js';

export const Indicator = GObject.registerClass(
class OpenEffectsIndicator extends PanelMenu.Button {
    _init(extension) {
        super._init(0.5, 'OpenEffects');
        this._extension = extension;
        this._refreshId = 0;
        this._client = new Client();

        this._iconFile = Gio.File.new_for_path(
            GLib.build_filenamev([extension.path, 'icons', 'openeffects.svg']));
        this._panelIcon = new St.Icon({
            gicon: new Gio.FileIcon({ file: this._iconFile }),
            style_class: 'oe-panel-icon',
            icon_size: 18,
        });
        this.add_child(this._panelIcon);

        this._buildMenu();

        this._changedId = this._client.connect('changed', () => this._scheduleRefresh());
        this.menu.connect('open-state-changed', (_m, open) => {
            if (open)
                this._refresh();
        });
        this._scheduleRefresh();
    }

    _buildMenu() {
        const item = new PopupMenu.PopupBaseMenuItem({ reactive: false, can_focus: false });
        item.set_style('padding: 0px; margin: 0px;');
        const root = new St.BoxLayout({ vertical: true, style_class: 'oe-popup', x_expand: true });
        item.add_child(root);
        this.menu.addMenuItem(item);

        // Header: app icon + title/subtitle + master switch.
        const header = new St.BoxLayout({ style_class: 'oe-header', x_expand: true });
        header.add_child(new St.Icon({
            gicon: new Gio.FileIcon({ file: this._iconFile }),
            icon_size: 44, style_class: 'oe-header-icon',
        }));
        const titleBox = new St.BoxLayout({
            vertical: true, x_expand: true, y_align: Clutter.ActorAlign.CENTER, style_class: 'oe-titlebox',
        });
        titleBox.add_child(new St.Label({ text: 'Openeffects', style_class: 'oe-title' }));
        this._subtitle = new St.Label({ text: '…', style_class: 'oe-subtitle' });
        titleBox.add_child(this._subtitle);
        header.add_child(titleBox);

        this._masterSwitch = new PopupMenu.Switch(false);
        const swBtn = new St.Button({
            child: this._masterSwitch, style_class: 'oe-master-switch',
            y_align: Clutter.ActorAlign.CENTER, can_focus: true,
        });
        swBtn.connect('clicked', () => this._onMasterToggled());
        header.add_child(swBtn);
        root.add_child(header);

        // Daemon-down notice (replaces the controls when unavailable).
        this._notice = new St.Label({
            text: 'OpenEffects daemon is not running.\nFlip the switch above to start it.',
            style_class: 'oe-notice',
        });
        this._notice.clutter_text.line_wrap = true;
        this._notice.visible = false;
        root.add_child(this._notice);

        // Controls.
        this._sectionsBox = new St.BoxLayout({ vertical: true, x_expand: true, style_class: 'oe-sections' });
        root.add_child(this._sectionsBox);

        this._camera = Sections.CameraRow(this._client);
        this._sectionsBox.add_child(this._camera.actor);

        this._sections = [
            Sections.CenterStageSection(this._client),
            Sections.BackgroundSection(this._client, this._extension.path),
            Sections.StudioLightSection(this._client),
            Sections.ReactionsSection(this._client),
        ];
        for (const s of this._sections)
            this._sectionsBox.add_child(s.actor);

        // Footer: launch the full GTK app.
        const footer = new St.BoxLayout({ style_class: 'oe-footer', x_expand: true });
        const openBtn = new St.Button({
            label: 'Open OpenEffects', style_class: 'oe-open-btn', x_expand: true, can_focus: true,
        });
        openBtn.connect('clicked', () => {
            this.menu.close();
            this._launchApp();
        });
        footer.add_child(openBtn);
        root.add_child(footer);
    }

    _onMasterToggled() {
        const newOn = !this._masterSwitch.state;
        this._masterSwitch.setToggleState(newOn);
        if (!this._client.available) {
            if (newOn)
                this._client.start();
            return;
        }
        this._client.setBypass(!newOn); // master on => bypass off => effects active
    }

    _launchApp() {
        try {
            const app = Gio.DesktopAppInfo.new('in.co.funinkina.OpenEffects.desktop');
            if (app)
                app.launch([], null);
            else
                Gio.Subprocess.new(['openeffects'], Gio.SubprocessFlags.NONE);
        } catch (e) {
            logError(e, 'OpenEffects: failed to launch app');
        }
    }

    _scheduleRefresh() {
        if (this._refreshId)
            return;
        this._refreshId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 80, () => {
            this._refreshId = 0;
            this._refresh();
            return GLib.SOURCE_REMOVE;
        });
    }

    async _refresh() {
        let state;
        try {
            state = await this._client.refresh();
        } catch (e) {
            logError(e, 'OpenEffects: refresh failed');
            return;
        }
        if (!this._masterSwitch)
            return; // destroyed mid-flight

        const available = state.available;
        this._panelIcon.opacity = available ? 255 : 90;
        this._notice.visible = !available;
        this._sectionsBox.visible = available;

        const active = available && !state.bypass;
        this._subtitle.text = !available ? 'Inactive' : (state.bypass ? 'Bypassed' : 'Active');
        this._masterSwitch.setToggleState(active);

        if (!available)
            return;
        this._camera.sync(state);
        for (const s of this._sections)
            s.sync(state);
    }

    destroy() {
        if (this._refreshId) {
            GLib.source_remove(this._refreshId);
            this._refreshId = 0;
        }
        if (this._changedId && this._client) {
            this._client.disconnect(this._changedId);
            this._changedId = 0;
        }
        this._client?.destroy();
        this._client = null;
        super.destroy();
    }
});
