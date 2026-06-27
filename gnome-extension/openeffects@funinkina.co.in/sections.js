// Reusable St widgets and per-effect section builders for the popup. Each
// section builder returns { actor, sync(state) }: `actor` goes into the popup,
// `sync` re-applies the latest daemon state to the controls. Option tables are
// transcribed from gui/src/constants.rs so the popup matches the GTK app.

import St from 'gi://St';
import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import { Slider } from 'resource:///org/gnome/shell/ui/slider.js';

export const FRAMING_LEVELS = [['subtle', 'Subtle'], ['normal', 'Normal'], ['tight', 'Tight']];
export const FRAMING_MODES = [
    ['single', 'Single Face', 'avatar-default-symbolic'],
    ['group', 'Group Framing', 'system-users-symbolic'],
];
export const BG_MODES = [['blur', 'Blur'], ['replace', 'Replace']];
export const BLUR_LEVELS = [['33', 'Low'], ['66', 'Medium'], ['100', 'High']];
export const BG_PRESETS = [
    ['Charcoal', '#1e1e2e'], ['Slate', '#2e3440'], ['Deep Blue', '#1b3a5b'],
    ['Forest', '#1f3d2b'], ['Plum', '#3b2e4a'], ['Warm Gray', '#3a3a3a'],
];
export const BG_IMAGE_PRESETS = [
    ['Modern Living Room', 'modern-living-room.jpg'],
    ['Serene Nature', 'serene-nature.jpg'],
    ['Sunset Room', 'sunset-room.jpg'],
    ['Wall of Books', 'wall-of-books.jpg'],
];
export const REACTION_BUTTONS = [
    ['heart', '\u{1F496}'], ['thumbs_up', '\u{1F44D}'], ['thumbs_down', '\u{1F44E}'],
    ['tada', '\u{1F389}'], ['clap', '\u{1F44F}'], ['wave', '\u{1F44B}'],
    ['joy', '\u{1F602}'], ['open_mouth', '\u{1F62E}'], ['cry', '\u{1F622}'],
    ['thinking', '\u{1F914}'],
];

function toggleClass(actor, cls, on) {
    if (on)
        actor.add_style_class_name(cls);
    else
        actor.remove_style_class_name(cls);
}

// Horizontal pill group (Subtle | Normal | Tight). syncValue highlights one.
function Segmented(options, onSelect) {
    const box = new St.BoxLayout({ style_class: 'oe-segmented', x_expand: true });
    const buttons = new Map();
    for (const [value, label] of options) {
        const btn = new St.Button({ style_class: 'oe-segment', label, x_expand: true, can_focus: true });
        btn.connect('clicked', () => onSelect(value));
        box.add_child(btn);
        buttons.set(value, btn);
    }
    box.syncValue = value => {
        for (const [v, b] of buttons)
            toggleClass(b, 'selected', v === value);
    };
    return box;
}

// Two-up cards with stacked icon + label (Single Face | Group Framing).
function CardRow(options, onSelect) {
    const box = new St.BoxLayout({ style_class: 'oe-cards', x_expand: true });
    const buttons = new Map();
    for (const [value, label, iconName] of options) {
        const content = new St.BoxLayout({ vertical: true, x_expand: true, style_class: 'oe-card-content' });
        content.add_child(new St.Icon({
            icon_name: iconName, icon_size: 22, style_class: 'oe-card-icon',
            x_align: Clutter.ActorAlign.CENTER,
        }));
        content.add_child(new St.Label({
            text: label, style_class: 'oe-card-label', x_align: Clutter.ActorAlign.CENTER,
        }));
        const btn = new St.Button({ style_class: 'oe-card', child: content, x_expand: true, can_focus: true });
        btn.connect('clicked', () => onSelect(value));
        box.add_child(btn);
        buttons.set(value, btn);
    }
    box.syncValue = value => {
        for (const [v, b] of buttons)
            toggleClass(b, 'selected', v === value);
    };
    return box;
}

// Labelled slider over an integer [min,max]. Sends a debounced onChange so a
// drag doesn't flood D-Bus; programmatic syncValue never re-fires onChange.
function SliderRow(label, min, max, onChange) {
    const root = new St.BoxLayout({ vertical: true, style_class: 'oe-slider-row', x_expand: true });
    const head = new St.BoxLayout({ x_expand: true });
    head.add_child(new St.Label({ text: label, x_expand: true, style_class: 'oe-slider-label' }));
    const valLabel = new St.Label({ text: '0', style_class: 'oe-slider-value' });
    head.add_child(valLabel);
    const slider = new Slider(0);
    slider.x_expand = true;
    root.add_child(head);
    root.add_child(slider);

    const toValue = () => Math.round(min + slider.value * (max - min));
    let syncing = false;
    let timeoutId = 0;
    slider.connect('notify::value', () => {
        if (syncing)
            return;
        valLabel.text = `${toValue()}`;
        if (timeoutId)
            GLib.source_remove(timeoutId);
        timeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 150, () => {
            timeoutId = 0;
            onChange(toValue());
            return GLib.SOURCE_REMOVE;
        });
    });
    root.connect('destroy', () => {
        if (timeoutId)
            GLib.source_remove(timeoutId);
        timeoutId = 0;
    });

    return {
        actor: root,
        syncValue: v => {
            syncing = true;
            slider.value = Math.max(0, Math.min(1, (v - min) / (max - min)));
            valLabel.text = `${v}`;
            syncing = false;
        },
    };
}

// Shared row chrome: leading circular enable-toggle + title + chevron that
// expands a collapsible body. Returns the pieces the effect builders wire up.
function Section({ title, iconName }) {
    const root = new St.BoxLayout({ vertical: true, style_class: 'oe-section', x_expand: true });
    const header = new St.BoxLayout({ style_class: 'oe-row', x_expand: true });

    const toggle = new St.Button({ style_class: 'oe-effect-toggle', can_focus: true });
    toggle.set_child(new St.Icon({ icon_name: iconName, icon_size: 16, style_class: 'oe-effect-toggle-icon' }));

    const main = new St.Button({ style_class: 'oe-row-main', x_expand: true, can_focus: true });
    const mainBox = new St.BoxLayout({ x_expand: true });
    mainBox.add_child(new St.Label({
        text: title, x_expand: true, y_align: Clutter.ActorAlign.CENTER, style_class: 'oe-row-title',
    }));
    const chevron = new St.Icon({ icon_name: 'pan-end-symbolic', icon_size: 16, style_class: 'oe-chevron' });
    mainBox.add_child(chevron);
    main.set_child(mainBox);

    header.add_child(toggle);
    header.add_child(main);

    const body = new St.BoxLayout({ vertical: true, style_class: 'oe-body', x_expand: true });
    body.visible = false;

    main.connect('clicked', () => {
        body.visible = !body.visible;
        chevron.icon_name = body.visible ? 'pan-down-symbolic' : 'pan-end-symbolic';
        toggleClass(root, 'expanded', body.visible);
    });

    root.add_child(header);
    root.add_child(body);

    return {
        root, toggle, body, _enabled: false,
        setEnabledVisual: on => toggleClass(toggle, 'on', on),
    };
}

// Bind the leading toggle to an effect id with optimistic visual feedback.
function bindToggle(client, section, id) {
    section.toggle.connect('clicked', () => {
        const next = !section._enabled;
        section._enabled = next;
        section.setEnabledVisual(next);
        client.setEnabled(id, next);
    });
}

export function CenterStageSection(client) {
    const s = Section({ title: 'Center Stage', iconName: 'zoom-fit-best-symbolic' });
    bindToggle(client, s, 'center_stage');
    const zoom = Segmented(FRAMING_LEVELS, v =>
        client.setParam('center_stage', 'zoom', GLib.Variant.new_string(v)));
    const mode = CardRow(FRAMING_MODES, v =>
        client.setParam('center_stage', 'mode', GLib.Variant.new_string(v)));
    s.body.add_child(zoom);
    s.body.add_child(mode);
    return {
        actor: s.root,
        sync: state => {
            s._enabled = state.cs.enabled;
            s.setEnabledVisual(state.cs.enabled);
            zoom.syncValue(state.cs.zoom);
            mode.syncValue(state.cs.mode);
        },
    };
}

function ensurePreset(extPath, file) {
    const destDir = GLib.build_filenamev([GLib.get_user_data_dir(), 'openeffects', 'backgrounds']);
    GLib.mkdir_with_parents(destDir, 0o755);
    const dest = GLib.build_filenamev([destDir, file]);
    const destFile = Gio.File.new_for_path(dest);
    if (!destFile.query_exists(null)) {
        const src = Gio.File.new_for_path(GLib.build_filenamev([extPath, 'icons', 'backgrounds', file]));
        try {
            src.copy(destFile, Gio.FileCopyFlags.NONE, null, null);
        } catch (e) {
            logError(e, 'OpenEffects: background preset copy failed');
        }
    }
    return dest;
}

// Color swatches + image thumbnails for background replace. onPick(value) gets
// the value to store in bg_replace.background (#RRGGBB or a file path).
function SwatchGrid(extPath, onPick) {
    const box = new St.BoxLayout({ vertical: true, style_class: 'oe-swatches', x_expand: true });
    const byValue = new Map();

    const colorRow = new St.BoxLayout({ style_class: 'oe-swatch-row', x_expand: true });
    for (const [, hex] of BG_PRESETS) {
        const b = new St.Button({ style_class: 'oe-swatch', can_focus: true, x_expand: true });
        b.set_style(`background-color: ${hex};`);
        b.connect('clicked', () => onPick(hex));
        colorRow.add_child(b);
        byValue.set(hex.toLowerCase(), b);
    }
    box.add_child(colorRow);

    const imgRow = new St.BoxLayout({ style_class: 'oe-swatch-row', x_expand: true });
    for (const [, file] of BG_IMAGE_PRESETS) {
        const thumb = GLib.build_filenamev([extPath, 'icons', 'backgrounds', file]);
        const b = new St.Button({ style_class: 'oe-bg-thumb', can_focus: true, x_expand: true });
        b.set_style(`background-image: url("file://${thumb}"); background-size: cover;`);
        b.connect('clicked', () => onPick(ensurePreset(extPath, file)));
        imgRow.add_child(b);
        // dest path is what gets stored; map by it for highlight
        byValue.set(GLib.build_filenamev([GLib.get_user_data_dir(), 'openeffects', 'backgrounds', file]).toLowerCase(), b);
    }
    box.add_child(imgRow);

    box.syncValue = current => {
        const cur = (current || '').toLowerCase();
        for (const [val, b] of byValue)
            toggleClass(b, 'selected', val === cur);
    };
    return box;
}

export function BackgroundSection(client, extPath) {
    const s = Section({ title: 'Background', iconName: 'view-conceal-symbolic' });
    let mode = 'blur';

    const enableMode = m => {
        if (m === 'blur') {
            client.setEnabled('portrait_blur', true);
            client.setEnabled('bg_replace', false);
        } else {
            client.setEnabled('bg_replace', true);
            client.setEnabled('portrait_blur', false);
        }
    };

    s.toggle.connect('clicked', () => {
        const next = !s._enabled;
        s._enabled = next;
        s.setEnabledVisual(next);
        if (next)
            enableMode(mode);
        else {
            client.setEnabled('portrait_blur', false);
            client.setEnabled('bg_replace', false);
        }
    });

    const modeSeg = Segmented(BG_MODES, m => {
        mode = m;
        s._enabled = true;
        s.setEnabledVisual(true);
        enableMode(m);
        updateSub();
    });
    const blurSeg = Segmented(BLUR_LEVELS, v => {
        if (mode !== 'blur') {
            mode = 'blur';
            enableMode('blur');
            updateSub();
        }
        client.setParam('portrait_blur', 'strength', GLib.Variant.new_uint32(Number(v)));
    });
    const blurBox = new St.BoxLayout({ vertical: true, x_expand: true, style_class: 'oe-subgroup' });
    blurBox.add_child(blurSeg);

    const swatches = SwatchGrid(extPath, value => {
        mode = 'replace';
        s._enabled = true;
        s.setEnabledVisual(true);
        enableMode('replace');
        client.setParam('bg_replace', 'background', GLib.Variant.new_string(value));
        updateSub();
    });

    s.body.add_child(modeSeg);
    s.body.add_child(blurBox);
    s.body.add_child(swatches);

    function updateSub() {
        blurBox.visible = mode === 'blur';
        swatches.visible = mode === 'replace';
        modeSeg.syncValue(mode);
    }

    const nearestBlur = v => {
        const opts = [33, 66, 100];
        return String(opts.reduce((a, b) => (Math.abs(b - v) < Math.abs(a - v) ? b : a)));
    };

    return {
        actor: s.root,
        sync: state => {
            s._enabled = state.blur.enabled || state.bg.enabled;
            mode = state.bg.enabled ? 'replace' : 'blur';
            s.setEnabledVisual(s._enabled);
            blurSeg.syncValue(nearestBlur(state.blur.strength));
            swatches.syncValue(state.bg.background);
            updateSub();
        },
    };
}

export function StudioLightSection(client) {
    const s = Section({ title: 'Studio Light', iconName: 'display-brightness-symbolic' });
    bindToggle(client, s, 'studio_light');
    const intensity = SliderRow('Intensity', 0, 100, v =>
        client.setParam('studio_light', 'intensity', GLib.Variant.new_uint32(v)));
    const brightness = SliderRow('Brightness', -100, 100, v =>
        client.setParam('studio_light', 'brightness', GLib.Variant.new_int32(v)));
    const contrast = SliderRow('Contrast', 0, 100, v =>
        client.setParam('studio_light', 'contrast', GLib.Variant.new_uint32(v)));
    const bgBright = SliderRow('Background Brightness', -100, 100, v =>
        client.setParam('studio_light', 'bg_brightness', GLib.Variant.new_int32(v)));
    for (const r of [intensity, brightness, contrast, bgBright])
        s.body.add_child(r.actor);
    return {
        actor: s.root,
        sync: state => {
            s._enabled = state.studio.enabled;
            s.setEnabledVisual(state.studio.enabled);
            intensity.syncValue(state.studio.intensity);
            brightness.syncValue(state.studio.brightness);
            contrast.syncValue(state.studio.contrast);
            bgBright.syncValue(state.studio.bg_brightness);
        },
    };
}

export function ReactionsSection(client) {
    const s = Section({ title: 'Reactions', iconName: 'face-smile-symbolic' });
    bindToggle(client, s, 'reactions');
    s.body.add_child(new St.Label({ text: 'Tap to react', style_class: 'oe-hint' }));
    const grid = new St.BoxLayout({ vertical: true, style_class: 'oe-emoji-grid', x_expand: true });
    let rowBox = null;
    REACTION_BUTTONS.forEach(([rid, glyph], i) => {
        if (i % 5 === 0) {
            rowBox = new St.BoxLayout({ style_class: 'oe-emoji-row', x_expand: true });
            grid.add_child(rowBox);
        }
        const b = new St.Button({ label: glyph, style_class: 'oe-emoji-btn', can_focus: true, x_expand: true });
        b.connect('clicked', () => client.trigger(rid));
        rowBox.add_child(b);
    });
    s.body.add_child(grid);
    return {
        actor: s.root,
        sync: state => {
            s._enabled = state.reactions.enabled;
            s.setEnabledVisual(state.reactions.enabled);
        },
    };
}

// Camera selector: name + chevron expanding an inline radio list.
export function CameraRow(client) {
    const root = new St.BoxLayout({ vertical: true, style_class: 'oe-section oe-camera', x_expand: true });
    const main = new St.Button({ style_class: 'oe-row-main oe-camera-main', x_expand: true, can_focus: true });
    const box = new St.BoxLayout({ x_expand: true });
    box.add_child(new St.Icon({ icon_name: 'camera-web-symbolic', icon_size: 16, style_class: 'oe-camera-icon' }));
    const label = new St.Label({
        text: 'Camera', x_expand: true, y_align: Clutter.ActorAlign.CENTER, style_class: 'oe-row-title',
    });
    const chevron = new St.Icon({ icon_name: 'pan-end-symbolic', icon_size: 16, style_class: 'oe-chevron' });
    box.add_child(label);
    box.add_child(chevron);
    main.set_child(box);

    const list = new St.BoxLayout({ vertical: true, style_class: 'oe-camera-list', x_expand: true });
    list.visible = false;
    main.connect('clicked', () => {
        list.visible = !list.visible;
        chevron.icon_name = list.visible ? 'pan-down-symbolic' : 'pan-end-symbolic';
    });

    root.add_child(main);
    root.add_child(list);

    return {
        actor: root,
        sync: state => {
            const active = state.cameras.find(c => c.id === state.activeCamera) || state.cameras[0];
            label.text = active ? active.name : 'No camera';
            list.destroy_all_children();
            for (const cam of state.cameras) {
                const item = new St.Button({ style_class: 'oe-camera-item', x_expand: true, can_focus: true });
                const ib = new St.BoxLayout({ x_expand: true });
                const check = new St.Icon({ icon_name: 'object-select-symbolic', icon_size: 14, style_class: 'oe-camera-check' });
                check.opacity = cam.id === (state.activeCamera || active?.id) ? 255 : 0;
                ib.add_child(check);
                ib.add_child(new St.Label({ text: cam.name, x_expand: true, y_align: Clutter.ActorAlign.CENTER }));
                item.set_child(ib);
                item.connect('clicked', () => {
                    client.selectCamera(cam.id);
                    list.visible = false;
                    chevron.icon_name = 'pan-end-symbolic';
                });
                list.add_child(item);
            }
        },
    };
}
