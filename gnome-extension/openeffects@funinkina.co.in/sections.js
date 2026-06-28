// St widgets + per-effect section builders, styled to match the Figma design
// (flat Control-Center rows; expanded settings drop into a darker inset panel).
// Each builder returns { actor, sync(state) }. Option tables come from
// gui/src/constants.rs so the popup matches the GTK app.

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
export const BG_MODES = [['blur', 'Blur', 'view-conceal-symbolic'], ['replace', 'Replace', 'image-x-generic-symbolic']];
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
    ['joy', '\u{1F602}'], ['cry', '\u{1F622}'], ['open_mouth', '\u{1F62E}'],
    ['tada', '\u{1F389}'], ['wave', '\u{1F44B}'], ['clap', '\u{1F44F}'],
    ['thinking', '\u{1F914}'],
];

function toggleClass(actor, cls, on) {
    if (on)
        actor.add_style_class_name(cls);
    else
        actor.remove_style_class_name(cls);
}

// Custom iOS-style switch (grey off / accent on, white knob slides side to side).
// A spacer pushes the fixed-size knob to the left (off) or right (on).
export function ToggleSwitch(onToggle, small = false) {
    const track = new St.Button({
        style_class: small ? 'oe-switch oe-switch-sm' : 'oe-switch',
        can_focus: true,
    });
    const inner = new St.BoxLayout({ x_expand: true, y_expand: true });
    const knob = new St.Bin({ style_class: 'oe-switch-knob', y_align: Clutter.ActorAlign.CENTER });
    const spacer = new St.Widget({ x_expand: true });
    track.set_child(inner);
    let state = false;
    const render = () => {
        toggleClass(track, 'on', state);
        inner.remove_all_children();
        if (state) {
            inner.add_child(spacer);
            inner.add_child(knob);
        } else {
            inner.add_child(knob);
            inner.add_child(spacer);
        }
    };
    track.connect('clicked', () => {
        state = !state;
        render();
        onToggle(state);
    });
    render();
    return {
        actor: track,
        setState: v => { state = !!v; render(); },
        get state() { return state; },
    };
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

// Two-up cards with stacked icon + label (Single Face | Group Framing, Blur | Replace).
function CardRow(options, onSelect) {
    const box = new St.BoxLayout({ style_class: 'oe-cards', x_expand: true });
    const buttons = new Map();
    for (const [value, label, iconName] of options) {
        const content = new St.BoxLayout({ vertical: true, x_expand: true, style_class: 'oe-card-content' });
        content.add_child(new St.Icon({
            icon_name: iconName, icon_size: 28, style_class: 'oe-card-icon',
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

// Row: label | slider | value | [+][-]. Debounced onChange; syncValue never re-fires it.
function SliderRow(label, min, max, onChange) {
    const STEP = 5;
    const root = new St.BoxLayout({ style_class: 'oe-slider-row', x_expand: true });
    root.add_child(new St.Label({
        text: label, style_class: 'oe-slider-label', y_align: Clutter.ActorAlign.CENTER,
    }));
    const slider = new Slider(0);
    slider.add_style_class_name('oe-slider');
    slider.x_expand = true;
    slider.y_align = Clutter.ActorAlign.CENTER;
    root.add_child(slider);

    const spin = new St.BoxLayout({ style_class: 'oe-spin', y_align: Clutter.ActorAlign.CENTER });
    const valLabel = new St.Label({ text: '0', style_class: 'oe-spin-value', y_align: Clutter.ActorAlign.CENTER });
    const plus = new St.Button({
        style_class: 'oe-spin-btn oe-spin-plus', can_focus: true,
        child: new St.Icon({ icon_name: 'list-add-symbolic', icon_size: 13 }),
    });
    const minus = new St.Button({
        style_class: 'oe-spin-btn oe-spin-minus', can_focus: true,
        child: new St.Icon({ icon_name: 'list-remove-symbolic', icon_size: 13 }),
    });
    spin.add_child(valLabel);
    spin.add_child(plus);
    spin.add_child(minus);
    root.add_child(spin);

    let value = min, syncing = false, timeoutId = 0;
    const clamp = v => Math.max(min, Math.min(max, Math.round(v)));
    const schedule = () => {
        if (timeoutId)
            GLib.source_remove(timeoutId);
        timeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 150, () => {
            timeoutId = 0;
            onChange(value);
            return GLib.SOURCE_REMOVE;
        });
    };
    const apply = (v, emit) => {
        value = clamp(v);
        valLabel.text = `${value}`;
        syncing = true;
        slider.value = (value - min) / (max - min);
        syncing = false;
        if (emit)
            schedule();
    };
    slider.connect('notify::value', () => {
        if (syncing)
            return;
        value = clamp(min + slider.value * (max - min));
        valLabel.text = `${value}`;
        schedule();
    });
    plus.connect('clicked', () => apply(value + STEP, true));
    minus.connect('clicked', () => apply(value - STEP, true));
    root.connect('destroy', () => {
        if (timeoutId)
            GLib.source_remove(timeoutId);
        timeoutId = 0;
    });

    return { actor: root, syncValue: v => apply(v, false) };
}

// Shared row chrome: circular enable-toggle + title + chevron expanding a body.
function Section({ title, iconName }) {
    const root = new St.BoxLayout({ vertical: true, style_class: 'oe-section', x_expand: true });
    const header = new St.BoxLayout({ style_class: 'oe-row', x_expand: true });

    const toggle = new St.Button({ style_class: 'oe-effect-toggle', can_focus: true });
    toggle.set_child(new St.Icon({ icon_name: iconName, icon_size: 15, style_class: 'oe-effect-toggle-icon' }));

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
    });

    root.add_child(header);
    root.add_child(body);

    return {
        root, toggle, body, _enabled: false,
        setEnabledVisual: on => toggleClass(toggle, 'on', on),
    };
}

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

// Image thumbnails + color swatches + an "open app" add button.
function SwatchGrid(extPath, onPick, onAdd) {
    const box = new St.BoxLayout({ vertical: true, style_class: 'oe-swatches', x_expand: true });
    const byValue = new Map();

    const imgRow = new St.BoxLayout({ style_class: 'oe-swatch-row', x_expand: true });
    for (const [, file] of BG_IMAGE_PRESETS) {
        const thumb = GLib.build_filenamev([extPath, 'icons', 'backgrounds', file]);
        const b = new St.Button({ style_class: 'oe-bg-thumb', x_expand: true, can_focus: true });
        b.set_style(`background-image: url("file://${thumb}"); background-size: cover;`);
        b.connect('clicked', () => onPick(ensurePreset(extPath, file)));
        imgRow.add_child(b);
        byValue.set(GLib.build_filenamev([GLib.get_user_data_dir(), 'openeffects', 'backgrounds', file]).toLowerCase(), b);
    }
    box.add_child(imgRow);

    const colorRow = new St.BoxLayout({ style_class: 'oe-swatch-row', x_expand: true });
    for (const [, hex] of BG_PRESETS) {
        const b = new St.Button({ style_class: 'oe-swatch', x_expand: true, can_focus: true });
        b.set_style(`background-color: ${hex};`);
        b.connect('clicked', () => onPick(hex));
        colorRow.add_child(b);
        byValue.set(hex.toLowerCase(), b);
    }
    const add = new St.Button({
        style_class: 'oe-swatch-add', x_expand: true, can_focus: true,
        child: new St.Icon({ icon_name: 'list-add-symbolic', icon_size: 16 }),
    });
    add.connect('clicked', () => onAdd());
    colorRow.add_child(add);
    box.add_child(colorRow);

    box.syncValue = current => {
        const cur = (current || '').toLowerCase();
        for (const [v, b] of byValue)
            toggleClass(b, 'selected', v === cur);
    };
    return box;
}

export function BackgroundSection(client, extPath, onAdd) {
    const s = Section({ title: 'Backgrounds', iconName: 'view-conceal-symbolic' });
    let mode = 'blur';

    const enableMode = m => {
        client.setEnabled('portrait_blur', m === 'blur');
        client.setEnabled('bg_replace', m === 'replace');
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

    const modeCards = CardRow(BG_MODES, m => {
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
    const swatches = SwatchGrid(extPath, value => {
        mode = 'replace';
        s._enabled = true;
        s.setEnabledVisual(true);
        enableMode('replace');
        client.setParam('bg_replace', 'background', GLib.Variant.new_string(value));
        updateSub();
    }, onAdd);

    s.body.add_child(modeCards);
    s.body.add_child(blurSeg);
    s.body.add_child(swatches);

    function updateSub() {
        blurSeg.visible = mode === 'blur';
        swatches.visible = mode === 'replace';
        modeCards.syncValue(mode);
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
    const bgBright = SliderRow('Background', -100, 100, v =>
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
    s.toggle.connect('clicked', () => {
        const next = !s._enabled;
        s._enabled = next;
        s.setEnabledVisual(next);
        client.setEnabled('reactions', next);
    });

    const grid = new St.BoxLayout({ vertical: true, style_class: 'oe-emoji-grid', x_expand: true });
    let rowBox = null;
    REACTION_BUTTONS.forEach(([rid, glyph], i) => {
        if (i % 5 === 0) {
            rowBox = new St.BoxLayout({ style_class: 'oe-emoji-row', x_expand: true });
            grid.add_child(rowBox);
        }
        const b = new St.Button({ label: glyph, style_class: 'oe-emoji-btn', x_expand: true, can_focus: true });
        b.connect('clicked', () => client.trigger(rid));
        rowBox.add_child(b);
    });
    s.body.add_child(grid);

    const gestureRow = new St.BoxLayout({ style_class: 'oe-gesture-row', x_expand: true });
    gestureRow.add_child(new St.Label({
        text: 'Automatically trigger on hand gesture', x_expand: true,
        y_align: Clutter.ActorAlign.CENTER, style_class: 'oe-gesture-label',
    }));
    const gesture = ToggleSwitch(v => client.setEnabled('reactions', v), true);
    gestureRow.add_child(gesture.actor);
    s.body.add_child(gestureRow);

    return {
        actor: s.root,
        sync: state => {
            s._enabled = state.reactions.enabled;
            s.setEnabledVisual(state.reactions.enabled);
            gesture.setState(state.reactions.enabled);
        },
    };
}

// Camera selector: name + chevron expanding an inline radio list.
export function CameraRow(client) {
    const root = new St.BoxLayout({ vertical: true, style_class: 'oe-section oe-camera', x_expand: true });
    const main = new St.Button({ style_class: 'oe-row-main oe-camera-main', x_expand: true, can_focus: true });
    const box = new St.BoxLayout({ x_expand: true });
    box.add_child(new St.Icon({ icon_name: 'camera-video-symbolic', icon_size: 16, style_class: 'oe-camera-icon' }));
    const label = new St.Label({
        text: 'Camera', x_expand: true, y_align: Clutter.ActorAlign.CENTER, style_class: 'oe-row-title',
    });
    const chevron = new St.Icon({ icon_name: 'pan-down-symbolic', icon_size: 16, style_class: 'oe-chevron' });
    box.add_child(label);
    box.add_child(chevron);
    main.set_child(box);

    const list = new St.BoxLayout({ vertical: true, style_class: 'oe-camera-list', x_expand: true });
    list.visible = false;
    main.connect('clicked', () => {
        list.visible = !list.visible;
        chevron.icon_name = list.visible ? 'pan-up-symbolic' : 'pan-down-symbolic';
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
                    chevron.icon_name = 'pan-down-symbolic';
                });
                list.add_child(item);
            }
        },
    };
}
