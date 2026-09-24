import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';

test('the checked-in GLB loads as an animated Three scene', async () => {
  const bytes = await readFile(new URL('../../assets/spinner.glb', import.meta.url));
  const glb = await new GLTFLoader().parseAsync(
    bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
    '',
  );
  assert.equal(glb.scene.children.length, 1);
  assert.equal(glb.animations.length, 1);
  assert.equal(glb.animations[0].duration, 2);
  assert.equal(glb.scene.children[0].isMesh, true);
});

test('the texture fixture is a 32 by 32 PNG', async () => {
  const bytes = await readFile(new URL('../../assets/checker.png', import.meta.url));
  assert.equal(bytes.subarray(0, 8).toString('hex'), '89504e470d0a1a0a');
  assert.equal(bytes.readUInt32BE(16), 32);
  assert.equal(bytes.readUInt32BE(20), 32);
});
