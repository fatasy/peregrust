import { NoColorSpace, RenderTarget, Vector2, Vector3, SRGBColorSpace, UnsignedByteType } from 'three/webgpu';

const fields = ['id', 'name', 'type', 'parent', 'position', 'rotation', 'scale', 'visible', 'worldPosition', 'tags'];
const invalid = (message) => Object.assign(new Error(message), { code: 'INVALID_ARGUMENT' });

/** Explicit, optional bridge: the native runtime never imports Three.js. */
export function attachThree({ scene, camera, renderer, render, name = 'main', runtime = globalThis.Peregrust }) {
  if (!scene?.isObject3D || !camera?.isCamera || !renderer) throw new TypeError('attachThree requires scene, camera and renderer');
  if (!runtime?.control) throw new Error('Peregrust control API is unavailable');
  const world = new Vector3();
  function serialize(node, selected) {
    const value = {};
    for (const field of selected) {
      switch (field) {
        case 'id': value.id = node.uuid; break;
        case 'parent': value.parent = node === scene ? null : node.parent?.uuid ?? null; break;
        case 'position': case 'scale': value[field] = node[field].toArray(); break;
        case 'rotation': value.rotation = [node.rotation.x, node.rotation.y, node.rotation.z]; break;
        case 'worldPosition': value.worldPosition = node.getWorldPosition(world).toArray(); break;
        case 'tags': value.tags = Array.isArray(node.userData.tags) ? node.userData.tags.filter((tag) => typeof tag === 'string') : []; break;
        default: value[field] = node[field];
      }
    }
    return value;
  }
  function query(params = {}) {
    const selected = params.fields ?? ['id', 'name', 'type', 'parent', 'position', 'visible'];
    if (!Array.isArray(selected) || selected.some((field) => !fields.includes(field))) throw invalid('unknown query field');
    for (const field of ['id', 'name', 'type', 'tag']) {
      if (params[field] !== undefined && typeof params[field] !== 'string') throw invalid(`${field} must be a string`);
    }
    const offset = params.offset ?? 0;
    const limit = params.limit ?? 50;
    if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isInteger(limit) || limit < 1 || limit > 500) {
      throw invalid('offset must be a nonnegative integer and limit must be between 1 and 500');
    }
    let total = 0;
    const objects = [];
    scene.traverse((node) => {
      if (params.id !== undefined && node.uuid !== params.id) return;
      if (params.name !== undefined && node.name !== params.name) return;
      if (params.type !== undefined && node.type !== params.type) return;
      if (params.tag !== undefined && !(Array.isArray(node.userData.tags) && node.userData.tags.includes(params.tag))) return;
      if (total >= offset && objects.length < limit) objects.push(serialize(node, selected));
      total++;
    });
    return { objects, total, offset, nextOffset: offset + objects.length < total ? offset + objects.length : null };
  }
  function update(params) {
    if (typeof params.id !== 'string') throw invalid('id is required');
    const node = scene.getObjectByProperty('uuid', params.id);
    if (!node) throw Object.assign(new Error(`object no longer exists: ${params.id}`), { code: 'OBJECT_NOT_FOUND' });
    // Validate the entire patch before mutating anything.
    for (const field of ['position', 'rotation', 'scale']) {
      if (params[field] !== undefined && (!Array.isArray(params[field]) || params[field].length !== 3 || !params[field].every(Number.isFinite))) {
        throw invalid(`${field} requires three finite numbers`);
      }
    }
    if (params.visible !== undefined && typeof params.visible !== 'boolean') throw invalid('visible must be boolean');
    if (params.name !== undefined && typeof params.name !== 'string') throw invalid('name must be a string');
    for (const field of ['position', 'rotation', 'scale']) {
      if (params[field] !== undefined) node[field].set(...params[field]);
    }
    if (params.visible !== undefined) node.visible = params.visible;
    if (params.name !== undefined) node.name = params.name;
    node.updateMatrix();
    node.updateWorldMatrix(true, true);
  }
  async function capture(params = {}) {
    const size = renderer.getDrawingBufferSize(new Vector2());
    const factor = Math.min(1, 2048 / Math.max(size.x, size.y));
    const width = params.width ?? Math.max(1, Math.round(size.x * factor));
    const height = params.height ?? Math.max(1, Math.round(size.y * factor));
    if (![width, height].every((value) => Number.isInteger(value) && value >= 1 && value <= 2048)) throw invalid('capture dimensions must be integers between 1 and 2048');
    // The capture stands in for the canvas: Three draws screen output (render
    // target null) into an output render target with the canvas's tone mapping
    // and colour-space encode. Like the canvas it is plain rgba8unorm; an sRGB
    // format encoded pipeline output (RenderPipeline, PostProcessing) twice.
    const outputs = typeof renderer.setOutputRenderTarget === 'function';
    const target = new RenderTarget(width, height, { type: UnsignedByteType,
      colorSpace: outputs ? NoColorSpace : SRGBColorSpace });
    const previous = renderer.getRenderTarget();
    const previousFace = renderer.getActiveCubeFace();
    const previousLevel = renderer.getActiveMipmapLevel();
    const previousOutput = outputs ? renderer.getOutputRenderTarget() : null;
    try {
      if (outputs) {
        renderer.setOutputRenderTarget(target);
        renderer.setRenderTarget(null);
      } else {
        renderer.setRenderTarget(target);
      }
      if (render) await render();
      else renderer.render(scene, camera);
      const readback = await renderer.readRenderTargetPixelsAsync(target, 0, 0, width, height);
      const rowBytes = width * 4;
      let pixels = readback;
      // Three's WebGPU backend returns the copy buffer with 256-byte row
      // alignment (and no padding after its final row). PNG needs packed RGBA.
      if (readback.byteLength !== rowBytes * height) {
        const stride = Math.ceil(rowBytes / 256) * 256;
        if (readback.byteLength !== stride * (height - 1) + rowBytes && readback.byteLength !== stride * height) {
          throw new Error('unexpected RGBA8 capture buffer size');
        }
        const bytes = new Uint8Array(readback.buffer, readback.byteOffset, readback.byteLength);
        pixels = new Uint8Array(rowBytes * height);
        for (let y = 0; y < height; y++) pixels.set(bytes.subarray(y * stride, y * stride + rowBytes), y * rowBytes);
      }
      return { width, height, pixels };
    } finally {
      if (outputs) renderer.setOutputRenderTarget(previousOutput);
      renderer.setRenderTarget(previous, previousFace, previousLevel);
      target.dispose();
    }
  }
  return runtime.control.registerScene(name, { query, update, capture });
}
