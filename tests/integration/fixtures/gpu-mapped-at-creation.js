const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
if (!adapter) throw new Error('GPU unavailable');
const device = await adapter.requestDevice();
function check(condition, message) { if (!condition) throw new Error(message); }
async function read(buffer) {
  const result = device.createBuffer({ size: buffer.size, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
  const encoder = device.createCommandEncoder();
  encoder.copyBufferToBuffer(buffer, 0, result, 0, buffer.size);
  device.queue.submit([encoder.finish()]);
  await result.mapAsync(GPUMapMode.READ);
  const values = new Uint32Array(result.getMappedRange().slice(0));
  result.unmap(); result.destroy();
  return values;
}

// Creation mappings read as zeros and write back on unmap, whole or in ranges.
const size = 8 << 20, whole = device.createBuffer({ size, usage: GPUBufferUsage.COPY_SRC, mappedAtCreation: true });
const started = performance.now();
const range = new Uint32Array(whole.getMappedRange());
check(range.every(value => value === 0), 'creation mapping is not zeroed');
for (let i = 0; i < range.length; i++) range[i] = i * 2654435761 >>> 0;
whole.unmap();
const uploadMs = performance.now() - started;
check(range.length === 0, 'mapped range stayed attached after unmap');
const back = await read(whole);
for (let i = 0; i < back.length; i += 4099) check(back[i] === (i * 2654435761 >>> 0), `writeback mismatch at ${i}`);
check(back[back.length - 1] === ((back.length - 1) * 2654435761 >>> 0), 'writeback tail mismatch');
whole.destroy();

const split = device.createBuffer({ size: 16, usage: GPUBufferUsage.COPY_SRC, mappedAtCreation: true });
new Uint32Array(split.getMappedRange(0, 8)).set([1, 2]);
new Uint32Array(split.getMappedRange(8)).set([3, 4]);
split.unmap();
check(JSON.stringify([...await read(split)]) === '[1,2,3,4]', 'range writeback');
let rejected = null;
try { split.getMappedRange(); } catch (error) { rejected = error; }
// deno_webgpu's own error once unmapped; only the rejection matters here.
check(rejected !== null, 'unmapped buffer returned a range');
split.destroy();

const misaligned = device.createBuffer({ size: 16, usage: GPUBufferUsage.COPY_SRC, mappedAtCreation: true });
rejected = null;
try { misaligned.getMappedRange(3); } catch (error) { rejected = error; }
check(rejected instanceof DOMException && rejected.name === 'OperationError', 'misaligned range accepted');
misaligned.destroy();

// A later mapAsync(WRITE) exposes the buffer's contents again, through deno_webgpu's own path.
const writable = device.createBuffer({ size: 8, usage: GPUBufferUsage.MAP_WRITE | GPUBufferUsage.COPY_SRC, mappedAtCreation: true });
new Uint32Array(writable.getMappedRange()).set([5, 6]);
writable.unmap();
await writable.mapAsync(GPUMapMode.WRITE);
check(JSON.stringify([...new Uint32Array(writable.getMappedRange())]) === '[5,6]', 'mapAsync(WRITE) lost the contents');
writable.unmap();
writable.destroy();

const dropped = device.createBuffer({ size: 64, usage: GPUBufferUsage.COPY_SRC, mappedAtCreation: true });
const view = new Uint8Array(dropped.getMappedRange());
dropped.destroy();
check(view.length === 0, 'destroy left the range attached');

console.log(`GPU_MAPPED_AT_CREATION_OK ${Math.round(uploadMs * 10) / 10} ms for ${size >> 20} MiB`);
device.destroy();
Peregrust.exit(0);
