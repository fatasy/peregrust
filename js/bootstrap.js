// Evaluated as a classic script before the bundled application module.
// Only the primary native canvas is exposed. WebGPU itself comes from deno_webgpu.
((root) => {
  function installPeregrust(native) {
    if (!native || typeof native.getWindowState !== 'function') {
      throw new Error('Peregrust native host is missing');
    }
    if (root.Peregrust) return root.Peregrust;

    const state = native.getWindowState();
    const pixelRatio = state.devicePixelRatio || state.dpr || 1;
    const listeners = new WeakMap();
    const raf = new Map();
    const frameListeners = new Set();
    const capturedPointers = new Set();
    let nextRafId = 1;
    let redrawRequested = false;
    let context = null;
    let backingWidth = state.width;
    let backingHeight = state.height;

    class EventTarget {
      addEventListener(type, listener, options) {
        if (!listener) return;
        const byType = listeners.get(this) ?? new Map();
        const entries = byType.get(type) ?? [];
        if (!entries.some((entry) => entry.listener === listener)) {
          entries.push({ listener, once: options === true || !!options?.once });
        }
        byType.set(type, entries);
        listeners.set(this, byType);
      }

      removeEventListener(type, listener) {
        const entries = listeners.get(this)?.get(type);
        if (!entries) return;
        const index = entries.findIndex((entry) => entry.listener === listener);
        if (index !== -1) entries.splice(index, 1);
      }

      dispatchEvent(event) {
        if (!event || typeof event.type !== 'string') throw new TypeError('event.type must be a string');
        if (!event.target) event.target = this;
        event.currentTarget = this;
        const entries = [...(listeners.get(this)?.get(event.type) ?? [])];
        for (const entry of entries) {
          if (!(listeners.get(this)?.get(event.type) ?? []).includes(entry)) continue;
          if (typeof entry.listener === 'function') entry.listener.call(this, event);
          else entry.listener?.handleEvent?.(event);
          if (entry.once) this.removeEventListener(event.type, entry.listener);
          if (event.immediatePropagationStopped) break;
        }
        return !event.defaultPrevented;
      }
    }

    function eventFromNative(data) {
      return {
        ...data,
        target: null,
        currentTarget: null,
        defaultPrevented: false,
        propagationStopped: false,
        immediatePropagationStopped: false,
        preventDefault() { this.defaultPrevented = true; },
        stopPropagation() { this.propagationStopped = true; },
        stopImmediatePropagation() {
          this.immediatePropagationStopped = true;
          this.propagationStopped = true;
        },
      };
    }

    const window = new EventTarget();
    window.innerWidth = state.width / pixelRatio;
    window.innerHeight = state.height / pixelRatio;
    window.devicePixelRatio = pixelRatio;
    window.performance = root.performance;
    window.setTimeout = root.setTimeout?.bind(root);
    window.clearTimeout = root.clearTimeout?.bind(root);
    // `self` is this window facade. Browser loaders such as GLTFLoader read
    // self.URL.createObjectURL for embedded bufferView images.
    window.URL = root.URL;
    window.webkitURL = root.URL;
    window.Blob = root.Blob;
    window.File = root.File;
    window.focus = () => native.focusWindow();

    const canvas = new EventTarget();
    canvas.style = { width: `${window.innerWidth}px`, height: `${window.innerHeight}px` };
    canvas.nodeName = 'CANVAS';
    canvas.tagName = 'CANVAS';
    canvas.width = backingWidth;
    canvas.height = backingHeight;
    canvas.clientWidth = window.innerWidth;
    canvas.clientHeight = window.innerHeight;
    canvas.getContext = (type) => {
      if (type !== 'webgpu') return null;
      if (!context) context = native.getCanvasContext(canvas);
      return context;
    };
    canvas.getBoundingClientRect = () => ({
      x: 0, y: 0, left: 0, top: 0,
      width: canvas.clientWidth, height: canvas.clientHeight,
      right: canvas.clientWidth, bottom: canvas.clientHeight,
      toJSON() { return { x: 0, y: 0, width: this.width, height: this.height }; },
    });
    canvas.focus = () => native.focusWindow();
    canvas.setPointerCapture = (pointerId) => {
      const mode = native.setPointerCapture('confined');
      capturedPointers.add(pointerId);
      if (mode === 'locked') {
        document.pointerLockElement = canvas;
        document.dispatchEvent(eventFromNative({ type: 'pointerlockchange' }));
      }
      return mode;
    };
    canvas.releasePointerCapture = (pointerId) => {
      capturedPointers.delete(pointerId);
      if (!capturedPointers.size) {
        native.setPointerCapture('none');
        if (document.pointerLockElement) {
          document.pointerLockElement = null;
          document.dispatchEvent(eventFromNative({ type: 'pointerlockchange' }));
        }
      }
    };
    canvas.hasPointerCapture = (pointerId) => capturedPointers.has(pointerId);
    canvas.requestPointerLock = async () => {
      const mode = native.setPointerCapture('locked');
      if (mode !== 'locked') {
        native.setPointerCapture('none');
        throw new Error(`pointer lock unavailable (native mode: ${mode})`);
      }
      document.pointerLockElement = canvas;
      document.dispatchEvent(eventFromNative({ type: 'pointerlockchange' }));
    };
    Object.defineProperties(canvas, {
      width: {
        get: () => backingWidth,
        set(value) {
          const next = Math.max(1, Math.floor(Number(value)));
          if (!Number.isFinite(next)) throw new RangeError('canvas width must be finite');
          backingWidth = next;
          native.setCanvasSize(backingWidth, backingHeight);
        },
        configurable: true,
      },
      height: {
        get: () => backingHeight,
        set(value) {
          const next = Math.max(1, Math.floor(Number(value)));
          if (!Number.isFinite(next)) throw new RangeError('canvas height must be finite');
          backingHeight = next;
          native.setCanvasSize(backingWidth, backingHeight);
        },
        configurable: true,
      },
    });

    const document = new EventTarget();
    document.defaultView = window;
    document.pointerLockElement = null;
    document.exitPointerLock = () => {
      native.setPointerCapture('none');
      capturedPointers.clear();
      document.pointerLockElement = null;
      document.dispatchEvent(eventFromNative({ type: 'pointerlockchange' }));
    };
    document.body = {
      appendChild(element) {
        if (element !== canvas) throw new Error('Peregrust exposes one native canvas');
        return element;
      },
    };
    document.createElement = (tag) => {
      throw new Error(`Peregrust does not create DOM ${tag} elements; use Peregrust.canvas`);
    };
    document.createElementNS = (_namespace, tag) => document.createElement(tag);
    canvas.ownerDocument = document;
    window.document = document;

    function requestRedraw() {
      if (root.__peregrustControlShouldSchedule && !root.__peregrustControlShouldSchedule()) return;
      if (redrawRequested) return;
      redrawRequested = true;
      native.requestRedraw();
    }
    function requestAnimationFrame(callback) {
      if (typeof callback !== 'function') throw new TypeError('requestAnimationFrame needs a callback');
      const id = nextRafId++;
      raf.set(id, callback);
      requestRedraw();
      return id;
    }
    function cancelAnimationFrame(id) {
      if (!raf.delete(id)) cancelledDuringFrame.add(id);
    }

    window.requestAnimationFrame = requestAnimationFrame;
    window.cancelAnimationFrame = cancelAnimationFrame;

    function unpackImage(image) {
      if (ArrayBuffer.isView(image) || Array.isArray(image)) {
        const bytes = ArrayBuffer.isView(image)
          ? new Uint8Array(image.buffer, image.byteOffset, image.byteLength)
          : new Uint8Array(image);
        if (bytes.byteLength < 8) throw new Error('decoded image header is truncated');
        const header = new DataView(bytes.buffer, bytes.byteOffset, 8);
        const width = header.getUint32(0, true);
        const height = header.getUint32(4, true);
        if (!width || !height || bytes.byteLength !== 8 + width * height * 4) {
          throw new Error('decoded image pixel data has an invalid length');
        }
        return { width, height, data: bytes.subarray(8) };
      }
      return {
        width: image.width,
        height: image.height,
        data: image.data instanceof Uint8Array ? image.data : new Uint8Array(image.data),
      };
    }

    function makeAudioVoice(id) {
      return Object.freeze({
        id,
        pause() { native.audioPause(id); },
        resume() { native.audioResume(id); },
        stop() { native.audioStop(id); },
        setVolume(volume) { native.audioSetVolume(id, Number(volume)); },
        setLoop(looped) { native.audioSetLoop(id, !!looped); },
        info() { return native.audioVoiceInfo(id); },
        dispose() { native.audioDisposeVoice(id); },
      });
    }

    const gamepadCache = [];
    function pollGamepads() {
      if (!native.pollGamepads) return [];
      const snapshots = native.pollGamepads();
      const output = new Array(snapshots.length);
      const count = Math.max(snapshots.length, gamepadCache.length);
      for (let index = 0; index < count; index++) {
        const snapshot = snapshots[index];
        const previous = gamepadCache[index];
        if (!snapshot) {
          if (previous) {
            previous.connected = false;
            previous.axes.fill(0);
            for (const button of previous.buttons) {
              button.pressed = false;
              button.touched = false;
              button.value = 0;
            }
          }
          if (index < output.length) output[index] = null;
          continue;
        }
        if (previous?.connected && previous.id !== snapshot.id) {
          previous.connected = false;
          previous.axes.fill(0);
          for (const button of previous.buttons) {
            button.pressed = false;
            button.touched = false;
            button.value = 0;
          }
        }
        const pad = previous?.connected ? previous : {};
        pad.id = snapshot.id;
        pad.index = snapshot.index;
        pad.connected = true;
        pad.mapping = snapshot.mapping;
        pad.axes = [...snapshot.axes];
        pad.buttons = snapshot.buttons.map((button) => ({ ...button }));
        pad.timestamp = snapshot.timestamp;
        gamepadCache[index] = pad;
        output[index] = pad;
      }
      return output;
    }

    const navigator = root.navigator ?? {};
    navigator.getGamepads = pollGamepads;
    root.navigator = navigator;
    window.navigator = navigator;

    const Peregrust = {
      canvas,
      window,
      args: Object.freeze([...(state.args ?? [])]),
      assets: {
        async read(path) {
          if (!native.readAsset) throw new Error('native asset reading is unavailable');
          const bytes = await native.readAsset(String(path));
          return bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
        },
        async decodeImage(path) {
          if (!native.decodeImage) throw new Error('native image decoding is unavailable');
          return unpackImage(await native.decodeImage(String(path)));
        },
        async decodeImageBytes(bytes) {
          if (!native.decodeImageBytes) throw new Error('native image byte decoding is unavailable');
          const source = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
          return unpackImage(await native.decodeImageBytes(source));
        },
      },
      audio: {
        async load(path) {
          if (!native.audioLoad) throw new Error('native audio is unavailable');
          const clip = await native.audioLoad(String(path));
          return Object.freeze({
            id: clip.id,
            durationSeconds: clip.durationSeconds,
            play({ volume = 1, loop = false } = {}) {
              return makeAudioVoice(native.audioPlay(clip.id, Number(volume), !!loop));
            },
            unload() { native.audioUnload(clip.id); },
          });
        },
      },
      gamepads: { poll: pollGamepads },
      get frameCount() { return Peregrust._frameCount; },
      _frameCount: 0,
      get width() { return window.innerWidth; },
      get height() { return window.innerHeight; },
      get devicePixelRatio() { return window.devicePixelRatio; },
      get fullscreen() { return !!native.getWindowState().fullscreen; },
      setTitle(title) { native.setTitle(String(title)); },
      focus() { return native.focusWindow(); },
      setFullscreen(enabled) { return native.setFullscreen(!!enabled); },
      setCursorVisible(visible) { native.setCursorVisible(!!visible); },
      setPointerCaptureMode(mode) {
        if (!['none', 'confined', 'locked'].includes(mode)) throw new TypeError('invalid pointer capture mode');
        const actual = native.setPointerCapture(mode);
        const nextElement = actual === 'locked' ? canvas : null;
        if (document.pointerLockElement !== nextElement) {
          document.pointerLockElement = nextElement;
          document.dispatchEvent(eventFromNative({ type: 'pointerlockchange' }));
        }
        if (actual === 'none') capturedPointers.clear();
        return actual;
      },
      exit(code = 0) { native.exit(code | 0); },
      requestAnimationFrame,
      cancelAnimationFrame,
      onFrame(callback) {
        if (typeof callback !== 'function') throw new TypeError('onFrame needs a callback');
        frameListeners.add(callback);
        requestRedraw();
        return () => frameListeners.delete(callback);
      },
    };

    root.__peregrustDispatchEvent = (data) => {
      if (!data || typeof data.type !== 'string') throw new TypeError('native event requires type');
      const event = eventFromNative(data);
      if (data.type === 'resize') {
        const width = Number(data.width);
        const height = Number(data.height);
        if (Number.isFinite(width) && width > 0) backingWidth = width;
        if (Number.isFinite(height) && height > 0) backingHeight = height;
        const dpr = Number(data.devicePixelRatio || data.dpr || window.devicePixelRatio);
        if (Number.isFinite(dpr) && dpr > 0) window.devicePixelRatio = dpr;
        window.innerWidth = backingWidth / window.devicePixelRatio;
        window.innerHeight = backingHeight / window.devicePixelRatio;
        canvas.clientWidth = window.innerWidth;
        canvas.clientHeight = window.innerHeight;
        canvas.style.width = `${window.innerWidth}px`;
        canvas.style.height = `${window.innerHeight}px`;
        canvas.dispatchEvent(event);
        if (!event.propagationStopped) window.dispatchEvent(event);
      } else if (data.type === 'blur') {
        if (document.pointerLockElement || capturedPointers.size) {
          native.setPointerCapture('none');
          document.pointerLockElement = null;
          capturedPointers.clear();
          document.dispatchEvent(eventFromNative({ type: 'pointerlockchange' }));
        }
        window.dispatchEvent(event);
      } else if (data.type.startsWith('pointer') || data.type === 'wheel' || data.type === 'click') {
        canvas.dispatchEvent(event);
        if (!event.propagationStopped) window.dispatchEvent(event);
      } else {
        window.dispatchEvent(event);
      }
      return !event.defaultPrevented;
    };

    root.__peregrustFramePending = false;
    root.__peregrustLastFrameError = null;
    root.__peregrustDispatchFrame = async (timestampMs) => {
      if (root.__peregrustFramePending) throw new Error('overlapping Peregrust frames');
      root.__peregrustFramePending = true;
      redrawRequested = false;
      try {
        const advance = !root.__peregrustControlBeforeFrame || await root.__peregrustControlBeforeFrame();
        const frameTime = root.__peregrustControlClock?.(timestampMs, advance) ?? timestampMs;
        if (advance) {
          Peregrust._frameCount++;
          root.__peregrustControlFrameStarted?.();
          const callbacks = [...raf];
          for (const [id] of callbacks) raf.delete(id);
          for (const [id, callback] of callbacks) {
            if (callback && !cancelledDuringFrame.has(id)) await callback(frameTime);
          }
          for (const callback of [...frameListeners]) {
            if (frameListeners.has(callback)) await callback(frameTime);
          }
        }
        if (root.__peregrustControlAfterFrame) await root.__peregrustControlAfterFrame(advance);
      } catch (error) {
        root.__peregrustLastFrameError = error;
        throw error;
      } finally {
        cancelledDuringFrame.clear();
        root.__peregrustFramePending = false;
        if ((!root.__peregrustControlShouldSchedule || root.__peregrustControlShouldSchedule())
          && (raf.size || frameListeners.size)) requestRedraw();
      }
    };

    root.Peregrust = Peregrust;
    root.window = window;
    root.self = window;
    root.document = document;
    root.requestAnimationFrame = requestAnimationFrame;
    root.cancelAnimationFrame = cancelAnimationFrame;
    return Peregrust;
  }

  const cancelledDuringFrame = new Set();
  root.__peregrustInstall = installPeregrust;
  if (root.__peregrustNative) installPeregrust(root.__peregrustNative);
  else if (root.Deno?.core?.ops) {
    const ops = root.Deno.core.ops;
    installPeregrust({
      getWindowState: () => ops.op_peregrust_get_window_state(),
      getCanvasContext: (canvas) => ops.op_peregrust_get_canvas_context(canvas),
      setCanvasSize: (width, height) => ops.op_peregrust_set_canvas_size(width, height),
      setTitle: (title) => ops.op_peregrust_set_title(title),
      requestRedraw: () => ops.op_peregrust_request_redraw(),
      exit: (code) => ops.op_peregrust_exit(code),
      focusWindow: () => ops.op_peregrust_focus_window(),
      setFullscreen: (enabled) => ops.op_peregrust_set_fullscreen(enabled),
      setCursorVisible: (visible) => ops.op_peregrust_set_cursor_visible(visible),
      setPointerCapture: (mode) => ops.op_peregrust_set_pointer_capture(mode),
      readAsset: (path) => ops.op_peregrust_read_asset(path),
      decodeImage: (path) => ops.op_peregrust_decode_image(path),
      decodeImageBytes: (bytes) => ops.op_peregrust_decode_image_bytes(bytes),
      audioLoad: (path) => ops.op_peregrust_audio_load(path),
      audioPlay: (clipId, volume, looped) => ops.op_peregrust_audio_play(clipId, volume, looped),
      audioPause: (voiceId) => ops.op_peregrust_audio_pause(voiceId),
      audioResume: (voiceId) => ops.op_peregrust_audio_resume(voiceId),
      audioStop: (voiceId) => ops.op_peregrust_audio_stop(voiceId),
      audioSetVolume: (voiceId, volume) => ops.op_peregrust_audio_set_volume(voiceId, volume),
      audioSetLoop: (voiceId, looped) => ops.op_peregrust_audio_set_loop(voiceId, looped),
      audioVoiceInfo: (voiceId) => ops.op_peregrust_audio_voice_info(voiceId),
      audioDisposeVoice: (voiceId) => ops.op_peregrust_audio_dispose_voice(voiceId),
      audioUnload: (clipId) => ops.op_peregrust_audio_unload(clipId),
      pollGamepads: () => ops.op_peregrust_gamepad_poll(),
    });
  }
})(globalThis);
