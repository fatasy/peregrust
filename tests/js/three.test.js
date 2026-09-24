import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { inflateSync } from 'node:zlib';
import { SRGBColorSpace, NearestFilter } from 'three/webgpu';
import { loadGLTF } from 'peregrust/three';

function decodeFixturePNG(bytes) {
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  let cursor = 8;
  const compressed = [];
  while (cursor < bytes.length) {
    const length = bytes.readUInt32BE(cursor);
    const type = bytes.toString('ascii', cursor + 4, cursor + 8);
    if (type === 'IDAT') compressed.push(bytes.subarray(cursor + 8, cursor + 8 + length));
    cursor += 12 + length;
    if (type === 'IEND') break;
  }
  const rows = inflateSync(Buffer.concat(compressed));
  const data = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++) {
    assert.equal(rows[y * (1 + width * 4)], 0);
    data.set(rows.subarray(y * (1 + width * 4) + 1, (y + 1) * (1 + width * 4)), y * width * 4);
  }
  return { width, height, data };
}

function mockRuntime() {
  const calls = [];
  return {
    calls,
    assets: {
      async read(path) {
        calls.push(`read:${path}`);
        return readFile(new URL(`../../${path}`, import.meta.url));
      },
      async decodeImage(path) {
        calls.push(`decode:${path}`);
        return decodeFixturePNG(await readFile(new URL(`../../${path}`, import.meta.url)));
      },
      async decodeImageBytes(bytes) {
        calls.push('decode:embedded');
        return decodeFixturePNG(Buffer.from(bytes));
      },
    },
  };
}

test('native adapter loads an animated GLB with embedded PNG texture', async () => {
  const runtime = mockRuntime();
  const gltf = await loadGLTF('assets/textured-spinner.glb', { runtime });
  const texture = gltf.scene.children[0].material.map;
  assert.equal(gltf.animations.length, 1);
  assert.equal(texture.isDataTexture, true);
  assert.equal(texture.image.width, 32);
  assert.equal(texture.image.height, 32);
  assert.equal(texture.flipY, false);
  assert.equal(texture.magFilter, NearestFilter);
  assert.equal(texture.colorSpace, SRGBColorSpace);
  assert.deepEqual(runtime.calls, ['read:assets/textured-spinner.glb', 'decode:embedded']);
});

test('external glTF texture path resolves inside project and uses native decoder', async () => {
  const runtime = mockRuntime();
  const gltf = await loadGLTF('assets/external-textured-spinner.glb', { runtime });
  assert.equal(gltf.scene.children[0].material.map.isDataTexture, true);
  assert.deepEqual(runtime.calls, ['read:assets/external-textured-spinner.glb', 'decode:assets/checker.png']);
});

test('unsupported compressed texture requirement is rejected explicitly', async () => {
  const runtime = {
    assets: {
      read: async () => new TextEncoder().encode(JSON.stringify({
        asset: { version: '2.0' },
        extensionsRequired: ['KHR_texture_basisu'],
      })),
      decodeImage: async () => {},
      decodeImageBytes: async () => {},
    },
  };
  await assert.rejects(loadGLTF('assets/unsupported.gltf', { runtime }), /KHR_texture_basisu/);
});
