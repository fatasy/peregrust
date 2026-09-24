import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';

const source = readFileSync(new URL('../../js/web_assets.js', import.meta.url), 'utf8');

function createFixture() {
  const files = new Map([
    ['/assets/data.bin', Uint8Array.from([0, 1, 127, 255])],
    ['/assets/data.json', new TextEncoder().encode('{"ready":true}')],
  ]);
  const calls = { paths: [], validated: 0, bitmap: null };
  class FakeImageBitmap {
    width = 2;
    height = 2;
    close() {}
  }
  class FakeGPUQueue {
    writes = [];
    writeTexture(...args) { this.writes.push(args); }
  }
  const ops = {
    async op_peregrust_asset_stat(path) {
      calls.paths.push(path);
      return files.has(path) ? { size: files.get(path).byteLength } : null;
    },
    async op_peregrust_fetch_asset(path) { return files.get(path); },
    op_peregrust_validate_image() { calls.validated++; },
    op_peregrust_bitmap_rgba(...args) {
      calls.bitmap = args;
      return Uint8Array.from({ length: args[3] * args[4] * 4 }, (_, i) => i);
    },
  };
  const fileModule = { blobFromObjectUrl: () => null };
  const extensions = new Map([
    ['ext:deno_web/06_streams.js', { ReadableStream }],
    ['ext:deno_web/02_event.js', { ProgressEvent: class ProgressEvent {} }],
    ['ext:deno_web/09_file.js', fileModule],
    ['ext:deno_image/01_image.js', {
      ImageBitmap: FakeImageBitmap,
      createImageBitmap: async () => new FakeImageBitmap(),
    }],
  ]);
  const document = { createElement: () => { throw new Error('unsupported element'); } };
  const context = {
    Deno: { core: {
      ops,
      loadExtScript: (specifier) => extensions.get(specifier),
      createLazyLoader: (specifier) => () => extensions.get(specifier),
    } },
    Blob, URL, TextEncoder, TextDecoder, DOMException, ReadableStream,
    atob, document, window: {}, GPUQueue: FakeGPUQueue,
  };
  runInNewContext(source, context, { filename: 'web_assets.js' });
  return { context, files, calls, ops, FakeImageBitmap, FakeGPUQueue };
}

test('fetch preserves binary bytes and exposes a readable stream', async () => {
  const { context } = createFixture();
  const response = await context.fetch('/assets/data.bin');
  assert.equal(response.status, 200);
  assert.equal(response.headers.get('CONTENT-LENGTH'), '4');
  assert.deepEqual([...new Uint8Array(await response.arrayBuffer())], [0, 1, 127, 255]);
  assert.equal(response.bodyUsed, true);
  const streamed = await context.fetch('/assets/data.bin');
  const reader = streamed.body.getReader();
  assert.deepEqual([...((await reader.read()).value)], [0, 1, 127, 255]);
  assert.equal((await reader.read()).done, true);
});

test('Request, Headers and JSON response preserve loader semantics', async () => {
  const { context } = createFixture();
  const headers = new context.Headers({ 'X-Asset': 'manifest' });
  headers.append('x-asset', 'cached');
  assert.equal(headers.get('X-ASSET'), 'manifest, cached');
  const request = new context.Request('/assets/data.json', { headers });
  const response = await context.fetch(request);
  assert.equal(response.headers.get('content-type'), 'application/json');
  const clone = response.clone();
  assert.equal((await response.json()).ready, true);
  assert.equal((await clone.json()).ready, true);
});

test('missing files return 404 while aborted requests reject', async () => {
  const { context, calls } = createFixture();
  assert.equal((await context.fetch('/assets/missing.bin')).status, 404);
  const controller = new AbortController();
  controller.abort(new DOMException('cancelled', 'AbortError'));
  await assert.rejects(context.fetch('/assets/data.bin', { signal: controller.signal }), { name: 'AbortError' });
  assert.deepEqual(calls.paths, ['/assets/missing.bin']);
});

test('abort interrupts a pending local read', async () => {
  const { context, ops } = createFixture();
  let finishStat;
  ops.op_peregrust_asset_stat = () => new Promise((resolve) => { finishStat = resolve; });
  const controller = new AbortController();
  const pending = context.fetch('/assets/data.bin', { signal: controller.signal });
  controller.abort(new DOMException('cancelled during read', 'AbortError'));
  await assert.rejects(pending, { name: 'AbortError' });
  finishStat({ size: 4 });
});

test('Response consumes real streams used by Three FileLoader', async () => {
  const { context } = createFixture();
  const stream = new ReadableStream({
    start(controller) { controller.enqueue(Uint8Array.from([3, 4])); controller.close(); },
  });
  const response = new context.Response(stream);
  assert.deepEqual([...new Uint8Array(await response.arrayBuffer())], [3, 4]);
  await assert.rejects(response.arrayBuffer(), /consumed/);
});

test('data URLs and image upload use decoded pixels', async () => {
  const { context, calls, FakeGPUQueue, FakeImageBitmap } = createFixture();
  const response = await context.fetch('data:application/octet-stream;base64,AAH/');
  assert.deepEqual([...new Uint8Array(await response.arrayBuffer())], [0, 1, 255]);
  const bitmap = await context.createImageBitmap(new Blob([Uint8Array.from([1, 2])], { type: 'image/png' }));
  assert.equal(calls.validated, 1);
  assert.ok(bitmap instanceof FakeImageBitmap);
  const queue = new FakeGPUQueue();
  const destination = { texture: { format: 'rgba8unorm' }, origin: { x: 0, y: 0, z: 0 } };
  queue.copyExternalImageToTexture({ source: bitmap, origin: { x: 0, y: 1 }, flipY: true }, destination,
    { width: 2, height: 1, depthOrArrayLayers: 1 });
  assert.deepEqual(calls.bitmap.slice(1), [0, 1, 2, 1, true]);
  assert.equal(queue.writes[0][2].bytesPerRow, 8);
  assert.equal(queue.writes[0][1].byteLength, 8);
  const bgra = { texture: { format: 'bgra8unorm' } };
  queue.copyExternalImageToTexture({ source: bitmap }, bgra, { width: 1, height: 1 });
  assert.deepEqual([...queue.writes[1][1].slice(0, 4)], [2, 1, 0, 3]);
});

test('TextureLoader image element loads a real bitmap and emits load', async () => {
  const { context, FakeImageBitmap } = createFixture();
  const element = context.document.createElement('img');
  const loaded = new Promise((resolve, reject) => {
    element.addEventListener('load', resolve);
    element.addEventListener('error', reject);
  });
  element.src = '/assets/data.bin';
  await loaded;
  await element.decode();
  assert.equal(element.complete, true);
  assert.equal(element.width, 2);
  assert.ok(element.__peregrustBitmap instanceof FakeImageBitmap);
});
