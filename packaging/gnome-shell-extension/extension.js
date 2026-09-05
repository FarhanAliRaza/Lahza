import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const CONTROL_NAME = 'lahza-input-control.json';
const SAMPLE_INTERVAL_MS = 16;

export default class LahzaInputCapture extends Extension {
    enable() {
        this._capture = null;
        this._controlPath = GLib.build_filenamev([
            GLib.get_user_runtime_dir(), CONTROL_NAME,
        ]);
        this._controlTimer = GLib.timeout_add(
            GLib.PRIORITY_DEFAULT,
            100,
            () => {
                this._syncControl();
                return GLib.SOURCE_CONTINUE;
            }
        );
        this._syncControl();
    }

    disable() {
        if (this._controlTimer) {
            GLib.source_remove(this._controlTimer);
            this._controlTimer = 0;
        }
        this._stopCapture();
    }

    _syncControl() {
        let request = null;
        try {
            const [ok, bytes] = GLib.file_get_contents(this._controlPath);
            if (ok)
                request = JSON.parse(new TextDecoder().decode(bytes));
        } catch (_) {
            request = null;
        }

        if (!request?.id || !request?.eventPath) {
            this._stopCapture();
            return;
        }
        if (this._capture?.id === request.id)
            return;
        this._stopCapture();
        this._startCapture(request);
    }

    _startCapture(request) {
        try {
            const file = Gio.File.new_for_path(request.eventPath);
            const stream = new Gio.DataOutputStream({
                base_stream: file.replace(
                    null,
                    false,
                    Gio.FileCreateFlags.PRIVATE,
                    null
                ),
            });
            this._capture = {
                id: request.id,
                stream,
                lastX: Number.NaN,
                lastY: Number.NaN,
                lastButtons: 0,
                lastHeartbeatUs: 0,
            };
            this._write({kind: 'ready'});
            this._sampleTimer = GLib.timeout_add(
                GLib.PRIORITY_HIGH_IDLE,
                SAMPLE_INTERVAL_MS,
                () => {
                    this._samplePointer();
                    return this._capture
                        ? GLib.SOURCE_CONTINUE
                        : GLib.SOURCE_REMOVE;
                }
            );
            this._eventId = global.stage.connect(
                'captured-event',
                (_actor, event) => this._captureEvent(event)
            );
        } catch (error) {
            console.error(`Lahza input capture could not start: ${error}`);
            this._stopCapture();
        }
    }

    _stopCapture() {
        if (this._sampleTimer) {
            GLib.source_remove(this._sampleTimer);
            this._sampleTimer = 0;
        }
        if (this._eventId) {
            global.stage.disconnect(this._eventId);
            this._eventId = 0;
        }
        if (this._capture?.stream) {
            try {
                this._capture.stream.flush(null);
                this._capture.stream.close(null);
            } catch (_) {
                // The recorder may have already removed an interrupted file.
            }
        }
        this._capture = null;
    }

    _samplePointer() {
        if (!this._capture)
            return;
        const [x, y, modifiers] = global.get_pointer();
        const buttons = this._buttonMask(modifiers);
        const nowUs = Number(GLib.get_monotonic_time());
        const moved = x !== this._capture.lastX || y !== this._capture.lastY;
        const heartbeat = nowUs - this._capture.lastHeartbeatUs >= 1_000_000;

        const changed = buttons ^ this._capture.lastButtons;
        for (let button = 1; button <= 5; button++) {
            const bit = 1 << (button - 1);
            if (changed & bit) {
                this._write({
                    kind: 'button',
                    x,
                    y,
                    button,
                    phase: buttons & bit ? 'down' : 'up',
                    window: this._focusedWindowRect(),
                });
            }
        }
        if (moved || heartbeat || changed) {
            this._write({
                kind: buttons ? 'drag' : 'move',
                x,
                y,
                window: this._focusedWindowRect(),
            });
            this._capture.lastHeartbeatUs = nowUs;
        }
        this._capture.lastX = x;
        this._capture.lastY = y;
        this._capture.lastButtons = buttons;
    }

    _captureEvent(event) {
        if (!this._capture || event.type() !== Clutter.EventType.KEY_PRESS)
            return Clutter.EVENT_PROPAGATE;
        const state = event.get_state();
        const modifiers = [];
        if (state & Clutter.ModifierType.CONTROL_MASK)
            modifiers.push('control');
        if (state & Clutter.ModifierType.MOD1_MASK)
            modifiers.push('alt');
        if (state & Clutter.ModifierType.SUPER_MASK)
            modifiers.push('super');
        if (state & Clutter.ModifierType.SHIFT_MASK)
            modifiers.push('shift');
        const key = Clutter.keyval_name(event.get_key_symbol()) ?? '';
        const special = /^(Tab|Escape|Return|BackSpace|Delete|Home|End|Page_|F\d+|.*Arrow)/.test(key);
        const meaningfulModifier = modifiers.some(value => value !== 'shift');
        if (special || meaningfulModifier)
            this._write({kind: 'key', key, modifiers});
        return Clutter.EVENT_PROPAGATE;
    }

    _buttonMask(modifiers) {
        let mask = 0;
        const values = [
            Clutter.ModifierType.BUTTON1_MASK,
            Clutter.ModifierType.BUTTON2_MASK,
            Clutter.ModifierType.BUTTON3_MASK,
            Clutter.ModifierType.BUTTON4_MASK,
            Clutter.ModifierType.BUTTON5_MASK,
        ];
        values.forEach((value, index) => {
            if (modifiers & value)
                mask |= 1 << index;
        });
        return mask;
    }

    _focusedWindowRect() {
        const rect = global.display.focus_window?.get_frame_rect();
        return rect ? [rect.x, rect.y, rect.width, rect.height] : null;
    }

    _write(event) {
        if (!this._capture)
            return;
        try {
            event.monoUs = Number(GLib.get_monotonic_time());
            this._capture.stream.put_string(`${JSON.stringify(event)}\n`, null);
            this._capture.stream.flush(null);
        } catch (error) {
            console.error(`Lahza input event could not be written: ${error}`);
            this._stopCapture();
        }
    }
}
