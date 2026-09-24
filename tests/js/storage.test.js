import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import test from 'node:test';

const script = readFileSync(new URL('../../js/storage.js', import.meta.url), 'utf8');

test('durable storage facade forwards text and binary operations to native ops', () => {
  const text = new Map();
  const binary = new Map();
  const backups = new Map();
  const ops = {
    op_peregrust_storage_text_get: key => text.get(key) ?? null,
    op_peregrust_storage_text_key: index => [...text.keys()][index] ?? null,
    op_peregrust_storage_text_length: () => text.size,
    op_peregrust_storage_text_set: (key, value) => text.set(key, value),
    op_peregrust_storage_text_remove: key => text.delete(key),
    op_peregrust_storage_text_clear: () => text.clear(),
    op_peregrust_storage_binary_has: (key, backup) => (backup ? backups : binary).has(key),
    op_peregrust_storage_binary_get: (key, backup) => (backup ? backups : binary).get(key),
    op_peregrust_storage_binary_set: (key, value) => {
      if (binary.has(key)) backups.set(key, binary.get(key));
      binary.set(key, Uint8Array.from(value));
    },
    op_peregrust_storage_binary_remove: key => { binary.delete(key); backups.delete(key); },
  };
  const root = { Deno: { core: { ops } }, Peregrust: {}, ArrayBuffer, Uint8Array };
  root.globalThis = root;
  runInNewContext(script, root);

  assert.equal(root.localStorage.getItem('missing'), null);
  root.localStorage.setItem(2, false);
  assert.equal(root.localStorage.getItem('2'), 'false');
  assert.equal(root.localStorage.length, 1);
  assert.equal(root.localStorage.key(0), '2');
  assert.equal(root.localStorage.key(-1), null);
  root.localStorage.removeItem(2);
  assert.equal(root.localStorage.length, 0);

  assert.equal(root.Peregrust.storage.get('save'), null);
  assert.equal(root.Peregrust.storage.has('save'), false);
  assert.throws(() => root.Peregrust.storage.set('save', 'bytes'), /ArrayBuffer/);
  root.Peregrust.storage.set('save', new Uint8Array([1, 2, 3]));
  root.Peregrust.storage.set('save', new Uint8Array([4, 5]));
  assert.equal(root.Peregrust.storage.has('save'), true);
  assert.deepEqual([...root.Peregrust.storage.get('save')], [4, 5]);
  assert.deepEqual([...root.Peregrust.storage.getBackup('save')], [1, 2, 3]);
  root.Peregrust.storage.remove('save');
  assert.equal(root.Peregrust.storage.get('save'), null);
});

test('storage bootstrap fails when native extension is absent', () => {
  const root = { Deno: { core: { ops: {} } }, Peregrust: {} };
  root.globalThis = root;
  assert.throws(() => runInNewContext(script, root), /durable storage extension is unavailable/);
});
