import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';

const source = readFileSync(new URL('../../js/gpu_upload.js', import.meta.url), 'utf8');

async function fixture() {
  const calls = [];
  const bytes = (data, offset = 0, size) => {
    const view = ArrayBuffer.isView(data), unit = view ? data.BYTES_PER_ELEMENT ?? 1 : 1;
    return new Uint8Array(view ? data.buffer : data, (view ? data.byteOffset : 0) + offset * unit,
      size === undefined ? data.byteLength - offset * unit : size * unit);
  };
  class GPUBuffer {
    destroy() { calls.push('destroyBuffer'); }
    mapAsync() { calls.push('map'); return Promise.resolve(); }
    unmap() { calls.push('unmap'); }
  }
  class GPUQueue {
    writeBuffer(buffer, offset, data, dataOffset, size) { calls.push({ buffer, offset, bytes: [...bytes(data, dataOffset, size)] }); }
    submit() { calls.push('submit'); }
    writeTexture() { calls.push('texture'); }
    copyExternalImageToTexture() { calls.push('image'); }
    onSubmittedWorkDone() { calls.push('done'); return Promise.resolve(); }
  }
  const rawWrite = GPUQueue.prototype.writeBuffer;
  class GPUDevice {
    queue = new GPUQueue();
    pushErrorScope() { calls.push('push'); }
    popErrorScope() { calls.push('pop'); return Promise.resolve(null); }
    destroy() { calls.push('destroyDevice'); }
  }
  class GPUAdapter { async requestDevice() { return new GPUDevice(); } }
  const states = new Map();
  const context = { GPUBuffer, GPUQueue, GPUDevice, GPUAdapter, ArrayBuffer, Uint8Array, DOMException,
    Peregrust: { control: { registerState: (name, provider) => states.set(name, provider) } },
    Deno: { core: { ops: { op_peregrust_write_buffer_batch(device, operations) {
      calls.push('batch');
      for (let i = 0; i < operations.length; i += 5) rawWrite.call(device.queue, ...operations.slice(i, i + 5));
    } } } },
  };
  runInNewContext(source, context);
  const device = await new GPUAdapter().requestDevice();
  return { calls, device, buffer: new GPUBuffer(), gpu: context.Peregrust.gpu, states };
}

test('opt-in batching snapshots reused arrays, offsets and overlapping writes in order', async () => {
  const { calls, device, buffer, gpu } = await fixture();
  gpu.setUploadStatisticsEnabled(true);
  device.queue.setWriteBufferBatching(true);
  const data = new Uint32Array([1, 2, 3, 4]);
  device.queue.writeBuffer(buffer, 4, data, 1, 2);
  data.fill(9);
  device.queue.writeBuffer(buffer, 8, data.subarray(2), 0, 1);
  data.fill(0);
  assert.equal(calls.length, 0);
  device.queue.submit([]);
  assert.equal(calls[0], 'batch');
  assert.deepEqual(calls[1].bytes, [...new Uint8Array(new Uint32Array([2, 3]).buffer)]);
  assert.deepEqual(calls[2].bytes, [...new Uint8Array(new Uint32Array([9]).buffer)]);
  assert.equal(calls[3], 'submit');
  assert.equal(gpu.getUploadStatistics().nativeCalls, 1);
  assert.equal(gpu.getUploadStatistics().uploadBytes, 12);
  assert.equal(gpu.getUploadStatistics().uploadCalls, 2);
});

test('disabled mode uses direct writes and records the same payload units', async () => {
  const { calls, device, buffer, gpu } = await fixture();
  gpu.setUploadStatisticsEnabled(true);
  device.queue.writeBuffer(buffer, 0, new Float32Array(8), 2, 4);
  assert.equal(calls.length, 1);
  assert.equal(gpu.getUploadStatistics().nativeWriteCalls, 1);
  assert.equal(gpu.getUploadStatistics().nativeBatchCalls, 0);
  assert.equal(gpu.getUploadStatistics().uploadBytes, 16);
});

test('DataView offsets are bytes and source subviews retain their byteOffset', async () => {
  const { calls, device, buffer } = await fixture();
  device.queue.setWriteBufferBatching(true);
  const data = Uint8Array.from({ length: 20 }, (_, i) => i);
  device.queue.writeBuffer(buffer, 0, new DataView(data.buffer, 4, 12), 4, 4);
  device.queue.flushWriteBufferBatch();
  assert.deepEqual(calls[1].bytes, [8, 9, 10, 11]);
});

for (const operation of ['writeTexture', 'copyExternalImageToTexture', 'onSubmittedWorkDone']) {
  test(`pending writes flush before queue.${operation}`, async () => {
    const { calls, device, buffer } = await fixture();
    device.queue.setWriteBufferBatching(true);
    device.queue.writeBuffer(buffer, 0, new Uint32Array([7]));
    await device.queue[operation]();
    assert.equal(calls[0], 'batch');
    assert.equal(calls.length, 3);
  });
}
for (const operation of ['pushErrorScope', 'popErrorScope', 'destroy']) {
  test(`pending validation stays before device.${operation}`, async () => {
    const { calls, device, buffer } = await fixture();
    device.queue.setWriteBufferBatching(true);
    device.queue.writeBuffer(buffer, 0, new Uint32Array([7]));
    await device[operation]('validation');
    assert.equal(calls[0], 'batch');
    assert.equal(calls.length, 3);
  });
}
for (const operation of ['mapAsync', 'unmap', 'destroy']) {
  test(`pending writes flush before buffer.${operation}`, async () => {
    const { calls, device, buffer } = await fixture();
    device.queue.setWriteBufferBatching(true);
    device.queue.writeBuffer(buffer, 0, new Uint32Array([7]));
    await buffer[operation]();
    assert.equal(calls[0], 'batch');
    assert.equal(calls.length, 3);
  });
}

test('changing modes flushes and empty flushes do not cross the bridge', async () => {
  const { calls, device, buffer, gpu } = await fixture();
  device.queue.setWriteBufferBatching(true);
  device.queue.writeBuffer(buffer, 0, new Uint32Array([1]));
  device.queue.setWriteBufferBatching(false);
  device.queue.flushWriteBufferBatch();
  device.queue.writeBufferBatch([]);
  assert.equal(calls.length, 2);
  assert.equal(gpu.getUploadStatistics().batchingQueues, 0);
});

test('arena capacity flush preserves previous snapshots; large writes take direct path', async () => {
  const { calls, device, buffer, gpu } = await fixture();
  gpu.setUploadStatisticsEnabled(true);
  device.queue.setWriteBufferBatching(true);
  const data = new Uint8Array(200 * 1024).fill(4);
  device.queue.writeBuffer(buffer, 0, data);
  data.fill(5);
  device.queue.writeBuffer(buffer, 0, data);
  assert.equal(calls[1].bytes[0], 4);
  device.queue.writeBuffer(buffer, 0, new Uint8Array(4 * 1024 * 1024 + 4));
  assert.equal(calls[3].bytes[0], 5);
  assert.equal(gpu.getUploadStatistics().nativeBatchCalls, 2);
  assert.equal(gpu.getUploadStatistics().nativeWriteCalls, 1);
});

test('invalid source ranges fail before they can corrupt the arena', async () => {
  const { calls, device, buffer } = await fixture();
  device.queue.setWriteBufferBatching(true);
  assert.throws(() => device.queue.writeBuffer(buffer, 0, new Uint32Array(2), 3), { name: 'OperationError' });
  assert.throws(() => device.queue.writeBuffer(buffer, 0, new Uint8Array(3)), { name: 'OperationError' });
  assert.throws(() => device.queue.writeBuffer(buffer, -1, new Uint32Array(1)), { name: 'TypeError' });
  const data = new ArrayBuffer(4);
  structuredClone(data, { transfer: [data] });
  assert.throws(() => device.queue.writeBuffer(buffer, 0, data));
  assert.equal(calls.length, 0);
});
