const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
if (!adapter) throw new Error('GPU unavailable');
const device = await adapter.requestDevice();
const queue = device.queue;
function check(condition, message) { if (!condition) throw new Error(message); }
async function read(buffer) {
  const result = device.createBuffer({ size: buffer.size, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
  const encoder = device.createCommandEncoder();
  encoder.copyBufferToBuffer(buffer, 0, result, 0, buffer.size);
  queue.submit([encoder.finish()]);
  await result.mapAsync(GPUMapMode.READ);
  const values = [...new Uint32Array(result.getMappedRange())];
  result.unmap(); result.destroy();
  return values;
}
function make(size = 16) { return device.createBuffer({ size, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC }); }
check(typeof queue.writeBufferBatch === 'function', 'native batch API missing');
Peregrust.gpu.setUploadStatisticsEnabled(true);
for (const enabled of [false, true]) {
  queue.setWriteBufferBatching(enabled);
  const buffer = make(), source = new Uint32Array([1, 2, 3, 4]);
  queue.writeBuffer(buffer, 4, source, 1, 2);
  source.fill(9);
  queue.writeBuffer(buffer, 8, source.subarray(2), 0, 1);
  source.fill(0);
  check(JSON.stringify(await read(buffer)) === '[0,2,9,0]', `snapshot/offset/order mode=${enabled}`);
  buffer.destroy();
}
queue.setWriteBufferBatching(false);
const direct = make(), bytes = new Uint32Array([10, 20, 30, 40]);
queue.writeBufferBatch([direct, 0, bytes, 1, 2, direct, 8, new DataView(bytes.buffer, 8, 8), 0, 4]);
bytes.fill(0);
check(JSON.stringify(await read(direct)) === '[20,30,30,0]', 'direct batch snapshot/DataView');
for (const operations of [[direct], [direct, 0, bytes, 999999999, 1], [direct, 0, new Uint8Array(3), 0, 3]]) {
  let rejected = false;
  try { queue.writeBufferBatch(operations); } catch { rejected = true; }
  check(rejected, 'malformed batch was not rejected');
}
queue.writeBufferBatch([direct, 0, new ArrayBuffer(0), 0, 0]);
for (const enabled of [false, true]) {
  queue.setWriteBufferBatching(enabled);
  device.pushErrorScope('validation');
  queue.writeBuffer(direct, 2, new Uint32Array([1]));
  const error = await device.popErrorScope();
  check(error instanceof GPUValidationError, `validation scope mode=${enabled}`);
}
const other = await adapter.requestDevice();
device.pushErrorScope('validation');
queue.writeBufferBatch([other.createBuffer({ size: 4, usage: GPUBufferUsage.COPY_DST }), 0, new Uint32Array([1]), 0, 1]);
check(await device.popErrorScope() instanceof GPUValidationError, 'cross-device upload accepted');
other.destroy();
queue.setWriteBufferBatching(true);
const mapped = device.createBuffer({ size: 4, usage: GPUBufferUsage.COPY_DST, mappedAtCreation: true });
device.pushErrorScope('validation');
queue.writeBuffer(mapped, 0, new Uint32Array([1]));
mapped.unmap();
check(await device.popErrorScope() instanceof GPUValidationError, 'mapped upload validated after unmap');
mapped.destroy(); direct.destroy();
const missingUsage = device.createBuffer({ size: 4, usage: GPUBufferUsage.COPY_SRC });
const destroyed = make(4); destroyed.destroy();
const valid = make(4);
for (const [buffer, offset] of [[missingUsage, 0], [destroyed, 0], [valid, 4]]) {
  device.pushErrorScope('validation');
  queue.writeBufferBatch([buffer, offset, new Uint32Array([1]), 0, 1,
    valid, 0, new Uint32Array([42]), 0, 1]);
  check(await device.popErrorScope() instanceof GPUValidationError, 'GPU validation error was lost');
  check((await read(valid))[0] === 42, 'valid operation after GPU validation error was skipped');
}
try {
  queue.writeBufferBatch([valid, 0, new Uint32Array([73]), 0, 1, valid, 0, new Uint8Array(3), 0, 3]);
  throw new Error('invalid source accepted');
} catch (error) { check(error.name === 'OperationError', 'wrong source error type'); }
check((await read(valid))[0] === 73, 'earlier write was rolled back after a source error');
missingUsage.destroy(); valid.destroy();
const stats = Peregrust.gpu.getUploadStatistics();
check(stats.nativeBatchCalls > 0 && stats.nativeWriteCalls > 0 && stats.uploadBytes > 0, 'upload counters missing');
console.log('GPU_UPLOAD_BATCH_OK', JSON.stringify(stats));
device.destroy();
Peregrust.exit(0);
