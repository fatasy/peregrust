// Buffers created with mappedAtCreation read as zeros until their first unmap, so their
// ranges come from a zeroed copy instead of a read of (write-combined) mapped memory.
// See src/gpu_mapping.rs. Other mappings keep deno_webgpu's getMappedRange.
(function installGpuMapping(root) {
  const zeroedRange = root.Deno.core.ops.op_peregrust_buffer_zeroed_mapped_range;
  const creationMapped = new WeakSet();
  const createBuffer = root.GPUDevice.prototype.createBuffer;
  const getMappedRange = root.GPUBuffer.prototype.getMappedRange;
  root.GPUDevice.prototype.createBuffer = function (...args) {
    const buffer = createBuffer.apply(this, args);
    if (args[0]?.mappedAtCreation && buffer.mapState === 'mapped') creationMapped.add(buffer);
    return buffer;
  };
  root.GPUBuffer.prototype.getMappedRange = function (offset, size) {
    if (!creationMapped.has(this)) return getMappedRange.call(this, offset, size);
    try {
      return zeroedRange(this, offset, size);
    } catch (error) {
      if (error instanceof RangeError) throw new DOMException(error.message, 'OperationError');
      throw error;
    }
  };
  for (const method of ['unmap', 'destroy']) {
    const original = root.GPUBuffer.prototype[method];
    root.GPUBuffer.prototype[method] = function (...args) {
      creationMapped.delete(this);
      return original.apply(this, args);
    };
  }
})(globalThis);
