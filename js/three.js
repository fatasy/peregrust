import {
  ClampToEdgeWrapping,
  DataTexture,
  LinearFilter,
  LinearMipmapLinearFilter,
  LinearMipmapNearestFilter,
  MirroredRepeatWrapping,
  NearestFilter,
  NearestMipmapLinearFilter,
  NearestMipmapNearestFilter,
  RepeatWrapping,
} from 'three/webgpu';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';

const FILTERS = {
  9728: NearestFilter,
  9729: LinearFilter,
  9984: NearestMipmapNearestFilter,
  9985: LinearMipmapNearestFilter,
  9986: NearestMipmapLinearFilter,
  9987: LinearMipmapLinearFilter,
};
const WRAPS = {
  33071: ClampToEdgeWrapping,
  33648: MirroredRepeatWrapping,
  10497: RepeatWrapping,
};

function imageBytes(value) {
  if (value instanceof Uint8Array) return value;
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  throw new TypeError('image buffer must be binary data');
}

function decodeBase64(uri) {
  const comma = uri.indexOf(',');
  if (comma === -1 || !/;base64$/i.test(uri.slice(0, comma))) {
    throw new Error('glTF data image URI must use base64 encoding');
  }
  const text = uri.slice(comma + 1).replace(/\s/g, '');
  if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(text)) {
    throw new Error('glTF image has invalid base64 data');
  }
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  const output = new Uint8Array((text.length / 4) * 3 - (text.endsWith('==') ? 2 : text.endsWith('=') ? 1 : 0));
  let cursor = 0;
  for (let i = 0; i < text.length; i += 4) {
    const value = (alphabet.indexOf(text[i]) << 18)
      | (alphabet.indexOf(text[i + 1]) << 12)
      | ((text[i + 2] === '=' ? 0 : alphabet.indexOf(text[i + 2])) << 6)
      | (text[i + 3] === '=' ? 0 : alphabet.indexOf(text[i + 3]));
    if (cursor < output.length) output[cursor++] = (value >>> 16) & 255;
    if (cursor < output.length) output[cursor++] = (value >>> 8) & 255;
    if (cursor < output.length) output[cursor++] = value & 255;
  }
  return output;
}

function resolveAssetPath(modelPath, uri) {
  if (/^[a-z][a-z0-9+.-]*:/i.test(uri) || uri.startsWith('//')) {
    throw new Error(`remote glTF image URI is unsupported: ${uri}`);
  }
  const decoded = decodeURIComponent(uri);
  if (decoded.startsWith('/') || decoded.includes('\\') || /[?#]/.test(decoded)) {
    throw new Error(`invalid glTF image path: ${uri}`);
  }
  const segments = modelPath.replace(/\\/g, '/').split('/').slice(0, -1);
  for (const segment of decoded.split('/')) {
    if (!segment || segment === '.') continue;
    if (segment === '..') {
      if (!segments.length) throw new Error(`glTF image path leaves project root: ${uri}`);
      segments.pop();
    } else {
      segments.push(segment);
    }
  }
  return segments.join('/');
}

function inspectGLTF(bytes) {
  const binary = imageBytes(bytes);
  let jsonText;
  if (binary.length >= 20 && binary[0] === 103 && binary[1] === 108 && binary[2] === 84 && binary[3] === 70) {
    const view = new DataView(binary.buffer, binary.byteOffset, binary.byteLength);
    const jsonLength = view.getUint32(12, true);
    if (view.getUint32(16, true) !== 0x4e4f534a || 20 + jsonLength > binary.length) {
      throw new Error('invalid GLB JSON chunk');
    }
    jsonText = new TextDecoder().decode(binary.subarray(20, 20 + jsonLength));
  } else {
    jsonText = new TextDecoder().decode(binary);
  }
  const json = JSON.parse(jsonText);
  for (const extension of json.extensionsRequired ?? []) {
    if (['KHR_draco_mesh_compression', 'KHR_texture_basisu', 'EXT_meshopt_compression'].includes(extension)) {
      throw new Error(`${extension} requires a decoder that Peregrust does not provide`);
    }
  }
  for (const texture of json.textures ?? []) {
    if (texture.source === undefined && texture.extensions?.KHR_texture_basisu) {
      throw new Error('KTX2 texture without a PNG/JPEG/WebP fallback is unsupported');
    }
  }
  if (!binary.length) throw new Error('glTF asset is empty');
  return { json, binary: binary.buffer.slice(binary.byteOffset, binary.byteOffset + binary.byteLength) };
}

function texturePlugin(parser, modelPath, assets) {
  const images = new Map();
  return {
    name: 'PEREGRUST_native_images',
    async loadTexture(textureIndex) {
      const textureDef = parser.json.textures?.[textureIndex];
      if (!textureDef || textureDef.source === undefined) {
        throw new Error(`glTF texture ${textureIndex} has no supported image source`);
      }
      const imageIndex = textureDef.source;
      const source = parser.json.images?.[imageIndex];
      if (!source) throw new Error(`glTF image ${imageIndex} is missing`);
      let pending = images.get(imageIndex);
      if (!pending) {
        pending = (async () => {
          if (source.bufferView !== undefined) {
            const bytes = await parser.getDependency('bufferView', source.bufferView);
            return assets.decodeImageBytes(imageBytes(bytes));
          }
          if (source.uri?.startsWith('data:')) {
            return assets.decodeImageBytes(decodeBase64(source.uri));
          }
          if (source.uri) {
            return assets.decodeImage(resolveAssetPath(modelPath, source.uri));
          }
          throw new Error(`glTF image ${imageIndex} has no URI or bufferView`);
        })();
        images.set(imageIndex, pending);
      }
      const decoded = await pending;
      const texture = new DataTexture(decoded.data, decoded.width, decoded.height);
      texture.flipY = false;
      texture.name = textureDef.name || source.name || source.uri || '';
      const sampler = parser.json.samplers?.[textureDef.sampler] ?? {};
      texture.magFilter = FILTERS[sampler.magFilter] ?? LinearFilter;
      texture.minFilter = FILTERS[sampler.minFilter] ?? LinearMipmapLinearFilter;
      texture.wrapS = WRAPS[sampler.wrapS] ?? RepeatWrapping;
      texture.wrapT = WRAPS[sampler.wrapT] ?? RepeatWrapping;
      texture.generateMipmaps = texture.minFilter !== NearestFilter && texture.minFilter !== LinearFilter;
      texture.userData.mimeType = source.mimeType || (source.uri?.match(/^data:([^;,]+)/)?.[1] ?? '');
      texture.needsUpdate = true;
      parser.associations.set(texture, { textures: textureIndex });
      return texture;
    },
  };
}

/** Load a local GLB/glTF with native image decoding and Three's genuine parser. */
export async function loadGLTF(path, options = {}) {
  const runtime = options.runtime ?? globalThis.Peregrust;
  if (!runtime?.assets?.read || !runtime.assets.decodeImage || !runtime.assets.decodeImageBytes) {
    throw new Error('Peregrust image asset APIs are unavailable');
  }
  const bytes = await runtime.assets.read(path);
  const { json, binary } = inspectGLTF(bytes);
  if (json.buffers?.some((buffer) => buffer.uri && !buffer.uri.startsWith('data:'))) {
    throw new Error('external glTF buffer files are unsupported; use GLB or data URI buffers');
  }
  const loader = new GLTFLoader();
  loader.register((parser) => texturePlugin(parser, path, runtime.assets));
  return loader.parseAsync(binary, '');
}
