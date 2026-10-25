import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';

const DIRECTORY = 'lahza-prompter';
const EFFECT_NAME = 'lahza-prompter-capture-exclusion';
const POLL_MS = 100;

// Mutter's shared-memory screencast path repaints the stage into a separate
// framebuffer. The normal stage framebuffers are learned from before-paint,
// which is not emitted by paint_to_buffer/paint_to_framebuffer. Comparing the
// actual framebuffer also works with fractional scaling and color management,
// where a physical display may itself render into a Cogl.Offscreen.
//
// This MUST be paired with a SystemMemory PipeWire stream. DMA-BUF monitor
// capture can copy the already-painted display instead of repainting it.
const CaptureExclusion = GObject.registerClass(
class LahzaPrompterCaptureExclusion extends Clutter.Effect {
    _init(displayFramebuffers) {
        super._init();
        this._displayFramebuffers = displayFramebuffers;
    }

    vfunc_paint(node, paintContext, flags) {
        if (this._displayFramebuffers.has(paintContext.get_framebuffer()))
            super.vfunc_paint(node, paintContext, flags);
    }
});

function readJson(path) {
    const [ok, bytes] = GLib.file_get_contents(path);
    if (!ok || bytes.length > 4096)
        return null;
    return JSON.parse(new TextDecoder().decode(bytes));
}

function removeFile(path) {
    try {
        Gio.File.new_for_path(path).delete(null);
    } catch (_) {
        // The recorder also removes its own acknowledgement on shutdown.
    }
}

export class PrompterExclusion {
    constructor() {
        this._directory = GLib.build_filenamev([
            GLib.get_user_runtime_dir(), DIRECTORY,
        ]);
        this._requests = new Map();
        this._displayFramebuffers = new Set();
        this._effects = new Map();
        // A normal signal handler runs before the RUN_LAST native stage
        // handler, including Mutter's screencast watchers. Install protection
        // before any newly mapped prompter can be painted into a recording.
        this._beforePaintId = global.stage.connect('before-paint', (_stage, view) => {
            this._displayFramebuffers.add(view.get_framebuffer());
            this._protectWindows();
        });
        this._timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, POLL_MS, () => {
            this._sync();
            return GLib.SOURCE_CONTINUE;
        });
        this._sync();
        global.stage.queue_redraw();
    }

    destroy() {
        if (this._timer) {
            GLib.source_remove(this._timer);
            this._timer = 0;
        }
        if (this._beforePaintId) {
            global.stage.disconnect(this._beforePaintId);
            this._beforePaintId = 0;
        }
        for (const request of this._requests.values())
            removeFile(request.ackPath);
        this._requests.clear();
        // Never expose a still-visible script by removing its effect. The
        // recorder observes the missing heartbeat and closes the prompter;
        // actor destruction then releases the effect. The retained framebuffer
        // set is intentionally frozen, so an unknown/new display paints no
        // script if the extension is disabled during a monitor change.
        this._effects.clear();
    }

    _sync() {
        const seen = new Set();
        let enumerator;
        try {
            enumerator = Gio.File.new_for_path(this._directory).enumerate_children(
                'standard::name,standard::type',
                Gio.FileQueryInfoFlags.NOFOLLOW_SYMLINKS,
                null
            );
            let info;
            while ((info = enumerator.next_file(null)) !== null) {
                const name = info.get_name();
                const match = /^([a-zA-Z0-9_-]{1,80})\.json$/.exec(name);
                if (!match || info.get_file_type() !== Gio.FileType.REGULAR)
                    continue;
                const id = match[1];
                const path = GLib.build_filenamev([this._directory, name]);
                let request;
                try {
                    request = readJson(path);
                } catch (_) {
                    continue;
                }
                if (request?.id !== id ||
                    typeof request.windowTitle !== 'string' ||
                    !request.windowTitle || request.windowTitle.length > 256 ||
                    !Number.isSafeInteger(request.pid) || request.pid <= 0)
                    continue;
                const existing = this._requests.get(id);
                if (!existing || existing.windowTitle !== request.windowTitle ||
                    existing.pid !== request.pid) {
                    this._requests.set(id, {
                        ...request,
                        ackPath: GLib.build_filenamev([this._directory, `${id}.ready.json`]),
                    });
                }
                seen.add(id);
            }
        } catch (_) {
            // The runtime directory exists only while a recorder uses it.
        } finally {
            enumerator?.close(null);
        }

        for (const [id, request] of this._requests) {
            if (!seen.has(id)) {
                removeFile(request.ackPath);
                this._requests.delete(id);
            }
        }
        this._protectWindows();
        this._writeAcknowledgements();
    }

    _protectWindows() {
        if (this._requests.size === 0)
            return;
        for (const actor of global.get_window_actors()) {
            const window = actor.meta_window;
            if (!window)
                continue;
            const request = [...this._requests.values()].find(candidate =>
                candidate.windowTitle === window.get_title() &&
                candidate.pid === window.get_pid()
            );
            if (!request)
                continue;
            let effect = actor.get_effect(EFFECT_NAME);
            if (!effect) {
                effect = new CaptureExclusion(this._displayFramebuffers);
                actor.add_effect_with_name(EFFECT_NAME, effect);
                actor.queue_redraw();
            } else if (effect instanceof CaptureExclusion) {
                // A helper disabled during a take leaves protection attached.
                // Re-enabling it can safely resume tracking display changes.
                effect._displayFramebuffers = this._displayFramebuffers;
            }
            if (!this._effects.has(actor)) {
                this._effects.set(actor, {id: request.id, effect});
                actor.connect('destroy', () => this._effects.delete(actor));
            }
        }
    }

    _writeAcknowledgements() {
        // Armed means interception is installed before opening the prompter;
        // protected additionally confirms the exact title+PID window has its
        // effect. No script should be shown until protected is received.
        if (this._displayFramebuffers.size === 0)
            return;
        for (const request of this._requests.values()) {
            const protectedWindow = [...this._effects.entries()].some(([actor, item]) =>
                item.id === request.id && actor.get_effect(EFFECT_NAME) === item.effect
            );
            try {
                GLib.file_set_contents(request.ackPath, JSON.stringify({
                    id: request.id,
                    state: protectedWindow ? 'protected' : 'armed',
                    heartbeatUs: Number(GLib.get_monotonic_time()),
                }));
            } catch (error) {
                console.error(`Lahza prompter acknowledgement failed: ${error}`);
            }
        }
    }
}
