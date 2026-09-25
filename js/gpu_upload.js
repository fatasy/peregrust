// Keep source snapshots in a reusable arena, then cross JS/Rust once per batch.
// Native wgpu still performs staging per destination; this is not zero-copy.
(function installGpuUploads(root) {
  const nativeWriteBatch = root.Deno.core.ops.op_peregrust_write_buffer_batch;
  function nativeBatch(device, operations) {
    try { nativeWriteBatch(device, operations); }
    catch (error) {
      if (error instanceof RangeError) throw new DOMException(error.message, 'OperationError');
      throw error;
    }
  }
  const queues = new WeakMap();
  const devices = new WeakMap();
  const pendingQueues = new Set();
  const originalWrite = root.GPUQueue.prototype.writeBuffer;
  const initialCapacity = 256 * 1024;
  const maximumCapacity = 4 * 1024 * 1024;
  let statisticsEnabled = false;
  let batchingQueues = 0;
  let adapterInfo = null;
  const statistics = { uploadCalls: 0, uploadBytes: 0, nativeWriteCalls: 0,
    nativeBatchCalls: 0, batchedUploads: 0, batchBytes: 0, submissions: 0 };

  const stateFor = queue => {
    const state = queues.get(queue);
    if (!state) throw new TypeError('GPUQueue was not created by this Peregrust runtime');
    return state;
  };
  const uploadBytes = (data, offset = 0, size) => {
    const unit = ArrayBuffer.isView(data) ? (data.BYTES_PER_ELEMENT ?? 1) : 1;
    return size === undefined ? data.byteLength - offset * unit : size * unit;
  };
  const recordUpload = (data, offset, size) => {
    if (!statisticsEnabled) return;
    statistics.uploadCalls++;
    statistics.uploadBytes += uploadBytes(data, offset, size);
  };
  function flush(state) {
    if (state.pending.length === 0) return;
    try {
      nativeBatch(state.device, state.pending);
      if (statisticsEnabled) {
        statistics.nativeBatchCalls++;
        statistics.batchedUploads += state.pending.length / 5;
        statistics.batchBytes += state.used;
      }
    } finally {
      state.pending.length = 0;
      state.used = 0;
      pendingQueues.delete(state);
    }
  }
  function flushAll() {
    for (const state of pendingQueues) flush(state);
  }
  const integer = (value, name) => {
    if (typeof value === 'bigint') throw new TypeError(`${name} must be a number`);
    const number = Math.trunc(Number(value));
    if (!Number.isSafeInteger(number) || number < 0) throw new TypeError(`${name} is out of range`);
    return number;
  };
  function sourceBytes(data, dataOffset = 0, size) {
    const view = ArrayBuffer.isView(data);
    const buffer = view ? data.buffer : data;
    if (!(buffer instanceof ArrayBuffer)) throw new TypeError('Upload source must use a non-shared ArrayBuffer');
    const unit = view ? (data.BYTES_PER_ELEMENT ?? 1) : 1;
    const offset = integer(dataOffset, 'dataOffset') * unit;
    const length = size === undefined ? data.byteLength - offset : integer(size, 'size') * unit;
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(length) || offset > data.byteLength ||
        length < 0 || length > data.byteLength - offset || length % 4 !== 0) {
      throw new DOMException('Upload source range is invalid or its byte length is not a multiple of four', 'OperationError');
    }
    // This constructor also rejects a detached source.
    return new Uint8Array(buffer, (view ? data.byteOffset : 0) + offset, length);
  }

  Object.defineProperties(root.GPUQueue.prototype, {
    writeBufferBatch: { configurable: true, writable: true, value(operations) {
      const state = stateFor(this);
      flush(state);
      if (!Array.isArray(operations) || operations.length % 5) throw new TypeError('Expected flat groups of five upload arguments');
      if (operations.length === 0) return;
      nativeBatch(state.device, operations);
      if (statisticsEnabled) {
        let bytes = 0;
        for (let i = 0; i < operations.length; i += 5) bytes += uploadBytes(operations[i + 2], operations[i + 3], operations[i + 4]);
        statistics.uploadCalls += operations.length / 5;
        statistics.uploadBytes += bytes;
        statistics.nativeBatchCalls++;
        statistics.batchedUploads += operations.length / 5;
        statistics.batchBytes += bytes;
      }
    } },
    setWriteBufferBatching: { configurable: true, writable: true, value(enabled) {
      if (typeof enabled !== 'boolean') throw new TypeError('Batching mode must be boolean');
      const state = stateFor(this);
      flush(state);
      if (state.enabled !== enabled) batchingQueues += enabled ? 1 : -1;
      state.enabled = enabled;
      return true;
    } },
    flushWriteBufferBatch: { configurable: true, writable: true, value() { flush(stateFor(this)); } },
    writeBuffer: { configurable: true, writable: true, value(buffer, bufferOffset, data, dataOffset, size) {
      const state = stateFor(this);
      if (!state.enabled) {
        originalWrite.call(this, buffer, bufferOffset, data, dataOffset, size);
        recordUpload(data, dataOffset, size);
        if (statisticsEnabled) statistics.nativeWriteCalls++;
        return;
      }
      if (!(buffer instanceof root.GPUBuffer)) throw new TypeError('Destination must be a GPUBuffer');
      const offset = integer(bufferOffset, 'bufferOffset');
      const bytes = sourceBytes(data, dataOffset, size);
      // Large transfers already amortize the bridge. Avoid an extra large copy.
      if (bytes.byteLength > maximumCapacity) {
        flush(state);
        originalWrite.call(this, buffer, offset, data, dataOffset, size);
        recordUpload(data, dataOffset, size);
        if (statisticsEnabled) statistics.nativeWriteCalls++;
        return;
      }
      state.arena ??= new Uint8Array(initialCapacity);
      if (state.used + bytes.byteLength > state.arena.byteLength || state.pending.length >= 4096 * 5) flush(state);
      if (bytes.byteLength > state.arena.byteLength) {
        state.arena = new Uint8Array(2 ** Math.ceil(Math.log2(bytes.byteLength)));
      }
      state.arena.set(bytes, state.used);
      state.pending.push(buffer, offset, state.arena, state.used, bytes.byteLength);
      state.used += bytes.byteLength;
      pendingQueues.add(state);
      recordUpload(bytes, 0, bytes.byteLength);
    } },
  });

  for (const method of ['submit', 'writeTexture', 'copyExternalImageToTexture', 'onSubmittedWorkDone']) {
    const original = root.GPUQueue.prototype[method];
    root.GPUQueue.prototype[method] = function (...args) {
      flush(stateFor(this));
      if (statisticsEnabled && method === 'submit') statistics.submissions++;
      return original.apply(this, args);
    };
  }
  // These operations change validation scope or buffer availability. Delaying
  // uploads past them would change errors, mapping and destruction semantics.
  for (const method of ['destroy', 'mapAsync', 'unmap']) {
    const original = root.GPUBuffer.prototype[method];
    root.GPUBuffer.prototype[method] = function (...args) {
      flushAll();
      return original.apply(this, args);
    };
  }
  for (const method of ['pushErrorScope', 'popErrorScope', 'destroy']) {
    const original = root.GPUDevice.prototype[method];
    root.GPUDevice.prototype[method] = function (...args) {
      const state = devices.get(this);
      if (state) {
        flush(state);
        if (method === 'destroy' && state.enabled) {
          state.enabled = false;
          batchingQueues--;
          state.arena = null;
        }
      }
      return original.apply(this, args);
    };
  }
  const requestDevice = root.GPUAdapter.prototype.requestDevice;
  root.GPUAdapter.prototype.requestDevice = async function (...args) {
    const device = await requestDevice.apply(this, args);
    const info = this.info;
    if (info) adapterInfo = { vendor: info.vendor, architecture: info.architecture,
      device: info.device, description: info.description };
    const state = { device, enabled: false, arena: null, used: 0, pending: [] };
    devices.set(device, state);
    queues.set(device.queue, state);
    return device;
  };
  root.Peregrust.gpu = Object.freeze({
    setUploadStatisticsEnabled(enabled) {
      flushAll();
      statisticsEnabled = Boolean(enabled);
      for (const key of Object.keys(statistics)) statistics[key] = 0;
    },
    getUploadStatistics() {
      return { enabled: statisticsEnabled, batchingQueues, adapterInfo, ...statistics,
        nativeCalls: statistics.nativeWriteCalls + statistics.nativeBatchCalls };
    },
  });
  root.Peregrust.control.registerState('gpuUploads', () => root.Peregrust.gpu.getUploadStatistics());
})(globalThis);
