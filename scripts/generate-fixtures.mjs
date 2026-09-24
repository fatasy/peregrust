// Recreates the small checked-in image and animated GLB used by three-demo.js.
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { deflateSync } from 'node:zlib';

const output = resolve('assets');
await mkdir(output, { recursive: true });

function chunk(type, payload) {
  const name = Buffer.from(type);
  const size = Buffer.alloc(4);
  size.writeUInt32BE(payload.length);
  const crcInput = Buffer.concat([name, payload]);
  let crc = 0xffffffff;
  for (const byte of crcInput) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  const crcBytes = Buffer.alloc(4);
  crcBytes.writeUInt32BE((crc ^ 0xffffffff) >>> 0);
  return Buffer.concat([size, crcInput, crcBytes]);
}

const width = 32;
const height = 32;
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(width, 0);
ihdr.writeUInt32BE(height, 4);
ihdr[8] = 8;
ihdr[9] = 6;
const rows = [];
for (let y = 0; y < height; y++) {
  const row = Buffer.alloc(1 + width * 4);
  for (let x = 0; x < width; x++) {
    const light = ((x >> 3) ^ (y >> 3)) & 1;
    const index = 1 + x * 4;
    row[index] = light ? 250 : 23;
    row[index + 1] = light ? 184 : 74;
    row[index + 2] = light ? 72 : 150;
    row[index + 3] = 255;
  }
  rows.push(row);
}
const png = Buffer.concat([
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
  chunk('IHDR', ihdr),
  chunk('IDAT', deflateSync(Buffer.concat(rows))),
  chunk('IEND', Buffer.alloc(0)),
]);
await writeFile(resolve(output, 'checker.png'), png);

const vertices = [
  -0.8, -0.6, 0.6,
   0.8, -0.6, 0.6,
   0.0,  0.9, 0.0,
   0.0, -0.6, -0.8,
];
const normals = [
  -0.4, -0.2, 0.9,
   0.4, -0.2, 0.9,
   0.0,  1.0, 0.0,
   0.0, -0.2, -1.0,
];
const indices = [0, 1, 2, 1, 3, 2, 3, 0, 2, 0, 3, 1];
const times = [0, 1, 2];
const rotations = [
  0, 0, 0, 1,
  0, 0.7071068, 0, 0.7071068,
  0, 1, 0, 0,
];
const pieces = [];
const bufferViews = [];
let offset = 0;
function addBuffer(data, target) {
  const bytes = Buffer.from(data.buffer, data.byteOffset, data.byteLength);
  const padding = Buffer.alloc((4 - (bytes.length % 4)) % 4);
  bufferViews.push({ buffer: 0, byteOffset: offset, byteLength: bytes.length, ...(target ? { target } : {}) });
  pieces.push(bytes, padding);
  offset += bytes.length + padding.length;
  return bufferViews.length - 1;
}
const posView = addBuffer(new Float32Array(vertices), 34962);
const normalView = addBuffer(new Float32Array(normals), 34962);
const indexView = addBuffer(new Uint16Array(indices), 34963);
const timeView = addBuffer(new Float32Array(times));
const rotationView = addBuffer(new Float32Array(rotations));
const binary = Buffer.concat(pieces);
const gltf = {
  asset: { version: '2.0', generator: 'Peregrust fixture generator' },
  scene: 0,
  scenes: [{ nodes: [0] }],
  nodes: [{ name: 'AnimatedSpinner', mesh: 0 }],
  meshes: [{ primitives: [{ attributes: { POSITION: 0, NORMAL: 1 }, indices: 2, material: 0 }] }],
  materials: [{ pbrMetallicRoughness: { baseColorFactor: [0.3, 0.9, 0.7, 1], metallicFactor: 0.1, roughnessFactor: 0.45 }, doubleSided: true }],
  buffers: [{ byteLength: binary.length }],
  bufferViews,
  accessors: [
    { bufferView: posView, componentType: 5126, count: 4, type: 'VEC3', min: [-0.8, -0.6, -0.8], max: [0.8, 0.9, 0.6] },
    { bufferView: normalView, componentType: 5126, count: 4, type: 'VEC3' },
    { bufferView: indexView, componentType: 5123, count: indices.length, type: 'SCALAR' },
    { bufferView: timeView, componentType: 5126, count: times.length, type: 'SCALAR', min: [0], max: [2] },
    { bufferView: rotationView, componentType: 5126, count: 3, type: 'VEC4' },
  ],
  animations: [{ samplers: [{ input: 3, output: 4, interpolation: 'LINEAR' }], channels: [{ sampler: 0, target: { node: 0, path: 'rotation' } }] }],
};
function glbChunk(type, body) {
  const header = Buffer.alloc(8);
  header.writeUInt32LE(body.length, 0);
  header.write(type, 4);
  return Buffer.concat([header, body]);
}
function makeGLB(document, data) {
  const documentBytes = Buffer.from(JSON.stringify(document));
  const paddedJson = Buffer.concat([documentBytes, Buffer.alloc((4 - documentBytes.length % 4) % 4, 32)]);
  const glbHeader = Buffer.alloc(12);
  glbHeader.write('glTF', 0);
  glbHeader.writeUInt32LE(2, 4);
  glbHeader.writeUInt32LE(12 + 8 + paddedJson.length + 8 + data.length, 8);
  return Buffer.concat([glbHeader, glbChunk('JSON', paddedJson), glbChunk('BIN\0', data)]);
}
await writeFile(resolve(output, 'spinner.glb'), makeGLB(gltf, binary));

const textured = structuredClone(gltf);
const uv = Buffer.from(new Float32Array([0, 0, 1, 0, 0.5, 1, 0.5, 0]).buffer);
const imagePadding = Buffer.alloc((4 - png.length % 4) % 4);
const texturedBinary = Buffer.concat([binary, uv, png, imagePadding]);
textured.bufferViews.push({ buffer: 0, byteOffset: binary.length, byteLength: uv.length, target: 34962 });
const uvIndex = textured.bufferViews.length - 1;
textured.bufferViews.push({ buffer: 0, byteOffset: binary.length + uv.length, byteLength: png.length });
const imageIndex = textured.bufferViews.length - 1;
textured.accessors.push({ bufferView: uvIndex, componentType: 5126, count: 4, type: 'VEC2' });
textured.meshes[0].primitives[0].attributes.TEXCOORD_0 = textured.accessors.length - 1;
textured.images = [{ bufferView: imageIndex, mimeType: 'image/png' }];
textured.samplers = [{ magFilter: 9728, minFilter: 9728, wrapS: 10497, wrapT: 10497 }];
textured.textures = [{ source: 0, sampler: 0 }];
textured.materials[0].pbrMetallicRoughness.baseColorTexture = { index: 0 };
textured.buffers[0].byteLength = texturedBinary.length;
await writeFile(resolve(output, 'textured-spinner.glb'), makeGLB(textured, texturedBinary));

const external = structuredClone(textured);
external.images = [{ uri: 'checker.png', mimeType: 'image/png' }];
external.bufferViews.pop();
const externalBinary = Buffer.concat([binary, uv]);
external.buffers[0].byteLength = externalBinary.length;
await writeFile(resolve(output, 'external-textured-spinner.glb'), makeGLB(external, externalBinary));

const sampleRate = 8000;
const sampleCount = sampleRate / 4;
const wav = Buffer.alloc(44 + sampleCount * 2);
wav.write('RIFF', 0);
wav.writeUInt32LE(wav.length - 8, 4);
wav.write('WAVEfmt ', 8);
wav.writeUInt32LE(16, 16);
wav.writeUInt16LE(1, 20);
wav.writeUInt16LE(1, 22);
wav.writeUInt32LE(sampleRate, 24);
wav.writeUInt32LE(sampleRate * 2, 28);
wav.writeUInt16LE(2, 32);
wav.writeUInt16LE(16, 34);
wav.write('data', 36);
wav.writeUInt32LE(sampleCount * 2, 40);
for (let i = 0; i < sampleCount; i++) {
  wav.writeInt16LE(Math.round(Math.sin(i * 2 * Math.PI * 440 / sampleRate) * 8000), 44 + i * 2);
}
await writeFile(resolve(output, 'tone.wav'), wav);
