import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';

const source = readFileSync(new URL('../../js/gpu_mapping.js', import.meta.url), 'utf8');

function fixture() {
  const calls = [];
  class GPUBuffer {
    constructor(mapped) { this.mapState = mapped ? 'mapped' : 'unmapped'; }
    getMappedRange(offset, size) { calls.push(['deno', offset, size]); return new ArrayBuffer(size ?? 8); }
    unmap() { calls.push('unmap'); this.mapState = 'unmapped'; }
    destroy() { calls.push('destroy'); }
    mapAsync() { this.mapState = 'mapped'; return Promise.resolve(); }
  }
  class GPUDevice {
    createBuffer(descriptor) { return new GPUBuffer(Boolean(descriptor.mappedAtCreation) && !descriptor.invalid); }
  }
  const context = { GPUBuffer, GPUDevice, ArrayBuffer, RangeError, DOMException,
    Deno: { core: { ops: { op_peregrust_buffer_zeroed_mapped_range(buffer, offset, size) {
      calls.push(['zeroed', offset, size]);
      if (offset === 3) throw new RangeError('misaligned');
      return new ArrayBuffer(size ?? 8);
    } } } } };
  runInNewContext(source, context);
  return { calls, device: new GPUDevice() };
}

test('creation mappings use the zeroed range until unmap; later mappings use deno_webgpu', async () => {
  const { calls, device } = fixture();
  const buffer = device.createBuffer({ size: 16, mappedAtCreation: true });
  buffer.getMappedRange(0, 8);
  buffer.getMappedRange(8);
  buffer.unmap();
  await buffer.mapAsync();
  buffer.getMappedRange();
  assert.deepEqual(calls, [['zeroed', 0, 8], ['zeroed', 8, undefined], 'unmap', ['deno', undefined, undefined]]);
});

test('unmapped creation, destroy and range errors', () => {
  const { calls, device } = fixture();
  device.createBuffer({ size: 16 }).getMappedRange();
  const destroyed = device.createBuffer({ size: 16, mappedAtCreation: true });
  destroyed.destroy();
  destroyed.getMappedRange();
  // A descriptor the device rejected yields an unmapped (error) buffer.
  device.createBuffer({ size: 16, mappedAtCreation: true, invalid: true }).getMappedRange();
  assert.deepEqual(calls, [['deno', undefined, undefined], 'destroy', ['deno', undefined, undefined], ['deno', undefined, undefined]]);
  const buffer = device.createBuffer({ size: 16, mappedAtCreation: true });
  assert.throws(() => buffer.getMappedRange(3), error => error instanceof DOMException && error.name === 'OperationError');
});
