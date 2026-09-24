// Native integration proof for local image decoding and external-image upload.
// Run with: peregrust examples/web-assets-gpu-proof.js --root . --presented-frames 1 --timeout 30
const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAE0lEQVR4nGP4z8DwHwwZGP6DAQBJyAn3FGMynQAAAABJRU5ErkJggg==';
const pngBytes = Uint8Array.from(atob(PNG), (character) => character.charCodeAt(0));
const blob = new Blob([pngBytes], { type: 'image/png' });

function equalPixel(actual, expected, label) {
  if (actual.length !== 4 || actual.some((value, index) => value !== expected[index])) {
    throw new Error(`${label}: expected [${expected}], got [${actual}]`);
  }
}

const adapter = await navigator.gpu.requestAdapter();
if (!adapter) throw new Error('WebGPU adapter unavailable');
const device = await adapter.requestDevice();

async function readTexture(texture, width, height) {
  const bytesPerRow = 256;
  const buffer = device.createBuffer({
    size: bytesPerRow * height,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
  });
  const encoder = device.createCommandEncoder();
  encoder.copyTextureToBuffer(
    { texture },
    { buffer, bytesPerRow, rowsPerImage: height },
    { width, height, depthOrArrayLayers: 1 },
  );
  device.queue.submit([encoder.finish()]);
  await buffer.mapAsync(GPUMapMode.READ);
  const bytes = new Uint8Array(buffer.getMappedRange()).slice();
  buffer.unmap();
  buffer.destroy();
  return (x, y) => Array.from(bytes.slice(y * bytesPerRow + x * 4, y * bytesPerRow + x * 4 + 4));
}

function texture(width, height) {
  return device.createTexture({
    size: { width, height },
    format: 'rgba8unorm',
    usage: GPUTextureUsage.COPY_DST | GPUTextureUsage.COPY_SRC,
  });
}

const bitmap = await createImageBitmap(blob, { premultiplyAlpha: 'none' });
if (bitmap.width !== 2 || bitmap.height !== 2) throw new Error('ImageBitmap dimensions are incorrect');
const flipped = texture(2, 2);
device.queue.copyExternalImageToTexture(
  { source: bitmap, flipY: true }, { texture: flipped }, { width: 2, height: 2 },
);
const flippedPixel = await readTexture(flipped, 2, 2);
equalPixel(flippedPixel(0, 0), [0, 0, 255, 255], 'flipped top-left');
equalPixel(flippedPixel(1, 0), [255, 255, 255, 255], 'flipped top-right');
equalPixel(flippedPixel(0, 1), [255, 0, 0, 255], 'flipped bottom-left');
flipped.destroy();

const cropped = texture(1, 1);
device.queue.copyExternalImageToTexture(
  { source: bitmap, origin: { x: 1, y: 0 } }, { texture: cropped }, { width: 1, height: 1 },
);
equalPixel((await readTexture(cropped, 1, 1))(0, 0), [0, 255, 0, 255], 'cropped green pixel');
cropped.destroy();
bitmap.close();

const flippedBitmap = await createImageBitmap(blob, { imageOrientation: 'flipY', premultiplyAlpha: 'none' });
const orientationTexture = texture(2, 2);
device.queue.copyExternalImageToTexture(
  { source: flippedBitmap }, { texture: orientationTexture }, { width: 2, height: 2 },
);
equalPixel((await readTexture(orientationTexture, 2, 2))(0, 0), [0, 0, 255, 255], 'ImageBitmap orientation');
orientationTexture.destroy();
flippedBitmap.close();

const element = new Image();
const loaded = new Promise((resolve, reject) => {
  element.addEventListener('load', resolve);
  element.addEventListener('error', (event) => reject(event.error ?? new Error('image element failed')));
});
element.src = `data:image/png;base64,${PNG}`;
await loaded;
await element.decode();
const elementTexture = texture(2, 2);
device.queue.copyExternalImageToTexture(
  { source: element }, { texture: elementTexture }, { width: 2, height: 2 },
);
equalPixel((await readTexture(elementTexture, 2, 2))(0, 0), [255, 0, 0, 255], 'HTMLImageElement upload');
elementTexture.destroy();

console.log('web-assets-gpu-proof: ImageBitmap decode, flip, crop, HTMLImageElement and GPU readback passed');

const canvas = Peregrust.canvas;
canvas.width = 320;
canvas.height = 180;
const context = canvas.getContext('webgpu');
context.configure({ device, format: navigator.gpu.getPreferredCanvasFormat(), alphaMode: 'opaque' });
let reported = false;
Peregrust.onFrame(() => {
  const current = context.getCurrentTexture();
  if (current.width !== 320 || current.height !== 180) {
    throw new Error(`canvas backing size mismatch: ${current.width}x${current.height}`);
  }
  const encoder = device.createCommandEncoder();
  const pass = encoder.beginRenderPass({
    colorAttachments: [{
      view: current.createView(),
      loadOp: 'clear',
      storeOp: 'store',
      clearValue: { r: 0, g: 0, b: 0, a: 1 },
    }],
  });
  pass.end();
  device.queue.submit([encoder.finish()]);
  if (!reported) {
    reported = true;
    console.log('web-assets-gpu-proof: canvas 320x180 frame submitted');
  }
});
