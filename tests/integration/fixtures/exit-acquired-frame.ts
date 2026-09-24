const runtime = (globalThis as any).Peregrust;
const gpu = (navigator as any).gpu;
const adapter = await gpu.requestAdapter();
if (!adapter) throw new Error('WebGPU adapter unavailable');
const device = await adapter.requestDevice();
const context = runtime.canvas.getContext('webgpu');
context.configure({ device, format: gpu.getPreferredCanvasFormat(), alphaMode: 'opaque' });

runtime.onFrame(async () => {
  const texture = context.getCurrentTexture();
  const encoder = device.createCommandEncoder();
  const pass = encoder.beginRenderPass({ colorAttachments: [{
    view: texture.createView(), clearValue: [0.1, 0.2, 0.3, 1],
    loadOp: 'clear', storeOp: 'store',
  }] });
  pass.end();
  device.queue.submit([encoder.finish()]);
  // Shutdown interrupts the frame future before the host can present it.
  setTimeout(() => {
    console.log('EXIT_ACQUIRED_FRAME_OK');
    runtime.exit(0);
  }, 10);
  await new Promise(() => {});
});
