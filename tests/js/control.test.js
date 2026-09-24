import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';
import * as THREE from 'three/webgpu';
import { attachThree } from '../../js/inspect_three.js';

const bootstrap = readFileSync(new URL('../../js/bootstrap.js', import.meta.url), 'utf8');
const control = readFileSync(new URL('../../js/control.js', import.meta.url), 'utf8');
const plain = (value) => JSON.parse(JSON.stringify(value));

function harness(enabled = true) {
  const requests = [], replies = [];
  let expired = false;
  const context = vm.createContext({
    __peregrustNative: {
      getWindowState: () => ({ width: 320, height: 240, devicePixelRatio: 1 }),
      requestRedraw() {},
    },
    __peregrustControlNative: {
      enabled: () => enabled,
      poll: () => requests.shift(),
      reply: (reply) => replies.push(plain(reply)),
      cancelled: () => expired,
      redraw() {},
      png: () => 'PNG',
    },
  });
  vm.runInContext(bootstrap, context);
  vm.runInContext(control, context);
  return { context, requests, replies, expire: () => { expired = true; },
    frame: () => context.__peregrustDispatchFrame(context.Peregrust.frameCount * 16),
  };
}

test('held input spans exactly N async callbacks and releases before observation', async () => {
  const h = harness();
  let held = false, moved = 0;
  h.context.window.addEventListener('keydown', () => { held = true; });
  h.context.window.addEventListener('keyup', () => { held = false; });
  h.context.Peregrust.onFrame(async () => { await Promise.resolve(); if (held) moved++; });
  h.context.Peregrust.control.registerState('player', () => ({ held, moved }));
  h.requests.push({ method: 'input.key', params: { code: 'KeyW', key: 'w', frames: 3, observe: { state: 'player' } } });
  await h.frame(); await h.frame();
  assert.equal(h.replies.length, 0);
  await h.frame();
  assert.deepEqual(h.replies[0], { ok: true, frame: 3, result: { frames: 3, state: { held: false, moved: 3 } } });
  await h.frame();
  assert.equal(moved, 3);
});

test('timeout releases held input and subsequent requests still work', async () => {
  const h = harness();
  let held = false;
  h.context.window.addEventListener('keydown', () => { held = true; });
  h.context.window.addEventListener('keyup', () => { held = false; });
  h.requests.push({ method: 'input.key', params: { code: 'KeyW', key: 'w', frames: 30 } });
  await h.frame();
  assert.equal(held, true);
  h.expire();
  h.requests.push({ method: 'runtime.info' });
  await h.frame();
  assert.equal(held, false);
  assert.equal(h.replies[0].error.code, 'TIMEOUT');
  assert.equal(h.replies[1].ok, true);
});

test('invalid nested observation parameters are rejected before pressing a key', async () => {
  const h = harness();
  let pressed = false;
  h.context.window.addEventListener('keydown', () => { pressed = true; });
  h.requests.push({ method: 'input.key', params: { code: 'KeyW', key: 'w', observe: { capture: { width: 0 } } } });
  await h.frame();
  assert.equal(pressed, false);
  assert.equal(h.replies[0].error.code, 'INVALID_ARGUMENT');
});

test('input can schedule animation in an otherwise idle game before observation', async () => {
  const h = harness();
  let rendered = false;
  h.context.window.addEventListener('keydown', () => {
    h.context.requestAnimationFrame(() => { rendered = true; });
  });
  h.context.Peregrust.control.registerState('rendered', () => rendered);
  h.requests.push({ method: 'input.key', params: { code: 'KeyW', key: 'w', observe: { state: 'rendered' } } });
  await h.frame();
  assert.equal(h.replies[0].result.state, true);
});

test('invalid operations and non-JSON state fail without killing the frame loop', async () => {
  const h = harness();
  const cyclic = {}; cyclic.self = cyclic;
  h.context.Peregrust.control.registerState('cyclic', () => cyclic);
  for (const request of [
    { method: 'missing' },
    { method: 'input.key', params: { code: 'KeyW', key: 'w', frames: 0 } },
    { method: 'input.dispatch', params: { event: { type: 'resize' } } },
    { method: 'state.get', params: { name: 'cyclic' } },
    { method: 'scene.query' },
    { method: 'runtime.info', params: { typo: true } },
  ]) {
    h.requests.push(request); await h.frame();
    assert.equal(h.replies.at(-1).ok, false);
  }
  h.requests.push({ method: 'control.describe' }); await h.frame();
  assert.equal(h.replies.at(-1).result.methods['scene.update'].inputSchema.required[0], 'id');
});

test('registration works without exposing an external control loop', () => {
  const h = harness(false);
  const dispose = h.context.Peregrust.control.registerState('x', () => 1);
  assert.throws(() => h.context.Peregrust.control.registerState('x', () => 2), /already registered/);
  dispose(); dispose();
  h.context.Peregrust.control.registerState('x', () => 3);
  assert.equal(h.context.__peregrustControlBeforeFrame, undefined);
});

test('scene queries paginate, distinguish duplicate names, and validate patches atomically', () => {
  let adapter;
  const scene = new THREE.Scene();
  const parent = new THREE.Group(); parent.position.x = 10; scene.add(parent);
  const a = new THREE.Object3D(), b = new THREE.Object3D();
  a.name = b.name = 'enemy'; a.userData.tags = ['enemy']; parent.add(a, b);
  attachThree({ scene, camera: new THREE.PerspectiveCamera(), renderer: {},
    runtime: { control: { registerScene: (_, value) => { adapter = value; return () => {}; } } },
  });
  const first = adapter.query({ name: 'enemy', limit: 1, fields: ['id', 'worldPosition'] });
  assert.equal(first.total, 2);
  assert.equal(first.nextOffset, 1);
  assert.deepEqual(first.objects[0].worldPosition, [10, 0, 0]);
  assert.equal(adapter.query({ name: 'enemy', offset: 1 }).objects[0].id, b.uuid);
  assert.equal(adapter.query({ tag: 'enemy' }).total, 1);
  assert.throws(() => adapter.update({ id: a.uuid, position: [2, 0, 0], visible: 'no' }), /boolean/);
  assert.equal(a.position.x, 0);
  adapter.update({ id: a.uuid, position: [2, 0, 0] });
  assert.deepEqual(adapter.query({ id: a.uuid, fields: ['worldPosition'] }).objects[0].worldPosition, [12, 0, 0]);
  parent.remove(a);
  assert.throws(() => adapter.update({ id: a.uuid, visible: true }), /no longer exists/);
});

test('captures restore the renderer target after GPU readback failure', async () => {
  let adapter, disposed = false;
  const original = {}, current = { target: original };
  const renderer = {
    getDrawingBufferSize: (out) => out.set(320, 240),
    getRenderTarget: () => current.target,
    getActiveCubeFace: () => 2,
    getActiveMipmapLevel: () => 1,
    setRenderTarget(target, face, level) {
      current.target = target;
      if (target !== original) target.addEventListener('dispose', () => { disposed = true; });
      else { assert.equal(face, 2); assert.equal(level, 1); }
    },
    render() {},
    readRenderTargetPixelsAsync: async () => { throw new Error('readback failed'); },
  };
  attachThree({ scene: new THREE.Scene(), camera: new THREE.PerspectiveCamera(), renderer,
    runtime: { control: { registerScene: (_, value) => { adapter = value; return () => {}; } } },
  });
  await assert.rejects(adapter.capture(), /readback failed/);
  assert.equal(current.target, original);
  assert.equal(disposed, true);
});
