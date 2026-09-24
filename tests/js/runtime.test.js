import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const bootstrap = readFileSync(new URL('../../js/bootstrap.js', import.meta.url), 'utf8');

function makeRuntime() {
  const calls = { redraw: 0, size: [], title: [], exit: [], capture: [], audio: [], gamepads: [] };
  const native = {
    getWindowState: () => ({ width: 1200, height: 800, devicePixelRatio: 2, args: ['--objects', '100'] }),
    getCanvasContext: () => ({ realContext: true }),
    setCanvasSize: (...size) => calls.size.push(size),
    setTitle: (title) => calls.title.push(title),
    requestRedraw: () => { calls.redraw++; },
    exit: (code) => calls.exit.push(code),
    focusWindow: () => true,
    setFullscreen: (value) => value,
    setCursorVisible: () => {},
    setPointerCapture: (mode) => { calls.capture.push(mode); return mode; },
    readAsset: async () => [1, 2, 3],
    decodeImage: async () => new Uint8Array([1, 0, 0, 0, 1, 0, 0, 0, 9, 8, 7, 6]),
    decodeImageBytes: async () => new Uint8Array([1, 0, 0, 0, 1, 0, 0, 0, 9, 8, 7, 6]),
    audioLoad: async (path) => { calls.audio.push(`load:${path}`); return { id: 4, durationSeconds: 1.5 }; },
    audioPlay: (id, volume, looped) => { calls.audio.push(`play:${id}:${volume}:${looped}`); return 9; },
    audioPause: (id) => calls.audio.push(`pause:${id}`),
    audioResume: (id) => calls.audio.push(`resume:${id}`),
    audioStop: (id) => calls.audio.push(`stop:${id}`),
    audioSetVolume: (id, value) => calls.audio.push(`volume:${id}:${value}`),
    audioSetLoop: (id, value) => calls.audio.push(`loop:${id}:${value}`),
    audioVoiceInfo: (id) => { calls.audio.push(`info:${id}`); return { state: 'playing', positionSeconds: 0.5 }; },
    audioDisposeVoice: (id) => calls.audio.push(`dispose:${id}`),
    audioUnload: (id) => calls.audio.push(`unload:${id}`),
    pollGamepads: () => calls.gamepads.shift() ?? [],
  };
  const context = vm.createContext({ __peregrustNative: native, setTimeout, clearTimeout, URL, Blob });
  vm.runInContext(bootstrap, context, { filename: 'bootstrap.js' });
  return { context, calls };
}

test('self exposes the same URL and Blob constructors as the runtime global', () => {
  const { context } = makeRuntime();
  assert.equal(context.self.URL, context.URL);
  assert.equal(context.self.webkitURL, context.URL);
  assert.equal(context.self.Blob, context.Blob);
  assert.equal(typeof context.self.URL.createObjectURL, 'function');
});

test('one-shot animation callbacks keep order, cancellation, and next-frame scheduling', async () => {
  const { context, calls } = makeRuntime();
  const seen = [];
  const cancelled = context.requestAnimationFrame(() => seen.push('cancelled'));
  context.cancelAnimationFrame(cancelled);
  context.requestAnimationFrame((time) => {
    seen.push(`first:${time}`);
    context.requestAnimationFrame(() => seen.push('next'));
  });
  context.requestAnimationFrame(() => seen.push('second'));
  assert.equal(calls.redraw, 1);
  await context.__peregrustDispatchFrame(12);
  assert.deepEqual(seen, ['first:12', 'second']);
  assert.equal(calls.redraw, 2);
  await context.__peregrustDispatchFrame(24);
  assert.deepEqual(seen, ['first:12', 'second', 'next']);
  assert.equal(context.Peregrust.frameCount, 2);
  assert.equal(calls.redraw, 2);
});

test('a callback can cancel another callback in the same frame', async () => {
  const { context } = makeRuntime();
  const seen = [];
  let second;
  context.requestAnimationFrame(() => {
    seen.push('first');
    context.cancelAnimationFrame(second);
  });
  second = context.requestAnimationFrame(() => seen.push('second'));
  await context.__peregrustDispatchFrame(1);
  assert.deepEqual(seen, ['first']);
});

test('async frame listeners finish before frame Promise resolves and stop when removed', async () => {
  const { context, calls } = makeRuntime();
  const seen = [];
  const off = context.Peregrust.onFrame(async () => {
    await Promise.resolve();
    seen.push('rendered');
  });
  assert.equal(calls.redraw, 1);
  await context.__peregrustDispatchFrame(0);
  assert.deepEqual(seen, ['rendered']);
  assert.equal(calls.redraw, 2);
  off();
  await context.__peregrustDispatchFrame(16);
  assert.deepEqual(seen, ['rendered']);
  assert.equal(calls.redraw, 2);
});

test('input targets, cancellation, and resize expose logical and backing dimensions', () => {
  const { context } = makeRuntime();
  const seen = [];
  context.Peregrust.canvas.addEventListener('pointermove', (event) => {
    seen.push([event.clientX, event.clientY, event.target === context.Peregrust.canvas]);
    event.preventDefault();
  });
  context.window.addEventListener('pointermove', () => seen.push('window'));
  const accepted = context.__peregrustDispatchEvent({ type: 'pointermove', clientX: 5, clientY: 8 });
  assert.equal(accepted, false);
  assert.deepEqual(seen, [[5, 8, true], 'window']);
  context.__peregrustDispatchEvent({ type: 'resize', width: 1600, height: 900, devicePixelRatio: 2 });
  assert.equal(context.Peregrust.canvas.width, 1600);
  assert.equal(context.Peregrust.canvas.clientWidth, 800);
  assert.equal(context.window.innerHeight, 450);
});

test('frame error is visible to the native host and pending state clears', async () => {
  const { context } = makeRuntime();
  context.Peregrust.onFrame(() => { throw new Error('render failed'); });
  await assert.rejects(context.__peregrustDispatchFrame(0), /render failed/);
  assert.equal(context.__peregrustFramePending, false);
  assert.match(String(context.__peregrustLastFrameError), /render failed/);
});

test('asset wrappers preserve image bytes and single native canvas context', async () => {
  const { context } = makeRuntime();
  const { Peregrust } = context;
  assert.equal(Peregrust.canvas.getContext('webgl2'), null);
  assert.equal(Peregrust.canvas.getContext('webgpu'), Peregrust.canvas.getContext('webgpu'));
  assert.deepEqual(Array.from(await Peregrust.assets.read('asset.glb')), [1, 2, 3]);
  const image = await Peregrust.assets.decodeImage('asset.png');
  assert.deepEqual(Array.from(image.data), [9, 8, 7, 6]);
  const embedded = await Peregrust.assets.decodeImageBytes(new Uint8Array([1]));
  assert.deepEqual(Array.from(embedded.data), [9, 8, 7, 6]);
  assert.deepEqual(Array.from(Peregrust.args), ['--objects', '100']);
  assert.throws(() => context.document.createElement('canvas'), /use Peregrust.canvas/);
});

test('native pointer capture and pointer lock release predictably', async () => {
  const { context, calls } = makeRuntime();
  const { canvas, window } = context.Peregrust;
  canvas.setPointerCapture(1);
  assert.equal(canvas.hasPointerCapture(1), true);
  canvas.releasePointerCapture(1);
  assert.equal(canvas.hasPointerCapture(1), false);
  await canvas.requestPointerLock();
  assert.equal(window.document.pointerLockElement, canvas);
  window.document.exitPointerLock();
  assert.equal(window.document.pointerLockElement, null);
  assert.deepEqual(calls.capture, ['confined', 'none', 'locked', 'none']);
});

test('losing focus releases native capture and reports pointer lock change', async () => {
  const { context, calls } = makeRuntime();
  let changes = 0;
  context.document.addEventListener('pointerlockchange', () => changes++);
  await context.Peregrust.canvas.requestPointerLock();
  context.__peregrustDispatchEvent({ type: 'blur' });
  assert.equal(context.document.pointerLockElement, null);
  assert.equal(changes, 2);
  assert.deepEqual(calls.capture, ['locked', 'none']);
});

test('audio clip and voice controls call the native mixer with stable IDs', async () => {
  const { context, calls } = makeRuntime();
  const clip = await context.Peregrust.audio.load('assets/tone.wav');
  assert.equal(clip.durationSeconds, 1.5);
  const voice = clip.play({ volume: 0.25, loop: true });
  voice.pause();
  voice.resume();
  voice.setVolume(0.75);
  voice.setLoop(false);
  assert.equal(voice.info().positionSeconds, 0.5);
  voice.stop();
  voice.dispose();
  clip.unload();
  assert.deepEqual(calls.audio, [
    'load:assets/tone.wav', 'play:4:0.25:true', 'pause:9', 'resume:9',
    'volume:9:0.75', 'loop:9:false', 'info:9', 'stop:9', 'dispose:9', 'unload:4',
  ]);
});

test('gamepad polling updates a retained pad and clears it on disconnect', () => {
  const { context, calls } = makeRuntime();
  const snapshot = (value) => ({
    id: 'Test Controller', index: 0, connected: true, mapping: 'standard',
    axes: [value, 0, 0, 0],
    buttons: Array.from({ length: 17 }, () => ({ pressed: true, touched: true, value: 1 })),
    timestamp: value,
  });
  calls.gamepads.push([snapshot(0.25)], [snapshot(0.5)], [null]);
  const first = context.navigator.getGamepads()[0];
  assert.equal(first.axes[0], 0.25);
  const second = context.Peregrust.gamepads.poll()[0];
  assert.equal(first, second);
  assert.equal(first.axes[0], 0.5);
  assert.equal(context.navigator.getGamepads()[0], null);
  assert.equal(first.connected, false);
  assert.equal(first.axes[0], 0);
  assert.equal(first.buttons[0].pressed, false);
});

test('replacing a gamepad slot does not reactivate the old controller object', () => {
  const { context, calls } = makeRuntime();
  const gamepad = (id) => ({ id, index: 0, connected: true, mapping: 'standard', axes: [1, 0, 0, 0], buttons: [], timestamp: 1 });
  calls.gamepads.push([gamepad('old')], [gamepad('new')]);
  const old = context.navigator.getGamepads()[0];
  const replacement = context.navigator.getGamepads()[0];
  assert.notEqual(old, replacement);
  assert.equal(old.connected, false);
  assert.equal(old.axes[0], 0);
  assert.equal(replacement.id, 'new');
});
