import * as THREE from 'three/webgpu';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { Peregrust } from 'peregrust';
import { loadGLTF } from 'peregrust/three';

if (self.URL !== URL || typeof self.URL.createObjectURL !== 'function' ||
  typeof self.URL.revokeObjectURL !== 'function') {
  throw new Error('GPU_PROOF_FAIL: window.URL Blob URL API is unavailable');
}
const objectURL = URL.createObjectURL(new Blob([Uint8Array.of(11, 22, 33)]));
try {
  const objectResponse = await fetch(objectURL);
  const objectBytes = new Uint8Array(await objectResponse.arrayBuffer());
  if (!objectResponse.ok || objectBytes.length !== 3 || objectBytes[2] !== 33) {
    throw new Error('GPU_PROOF_FAIL: Blob URL fetch did not preserve bytes');
  }
} finally {
  URL.revokeObjectURL(objectURL);
}

// A deterministic real-GPU check: render red geometry over blue background,
// copy pixels back from a WebGPU render target, and verify both locations.
const renderer = new THREE.WebGPURenderer({ canvas: Peregrust.canvas, antialias: false, alpha: false });
renderer.setPixelRatio(1);
renderer.setSize(Peregrust.window.innerWidth, Peregrust.window.innerHeight, false);
await renderer.init();
if (renderer.backend?.isWebGPUBackend !== true) {
  throw new Error('GPU_PROOF_FAIL: Three.js selected a non-WebGPU backend');
}

const scene = new THREE.Scene();
scene.background = new THREE.Color(0x0000ff);
scene.add(new THREE.Mesh(
  new THREE.PlaneGeometry(1, 1),
  new THREE.MeshBasicMaterial({ color: 0xff0000, toneMapped: false, side: THREE.DoubleSide }),
));
const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.1, 10);
camera.position.z = 2;
const target = new THREE.RenderTarget(64, 64, { depthBuffer: false });
renderer.setRenderTarget(target);
renderer.render(scene, camera);
const center = await renderer.readRenderTargetPixelsAsync(target, 32, 32, 1, 1);
const corner = await renderer.readRenderTargetPixelsAsync(target, 2, 2, 1, 1);
renderer.setRenderTarget(null);

if (!(center[0] > 180 && center[1] < 80 && center[2] < 80)) {
  throw new Error(`GPU_PROOF_FAIL: center pixel ${Array.from(center).join(',')}`);
}
if (!(corner[2] > 180 && corner[0] < 80 && corner[1] < 80)) {
  throw new Error(`GPU_PROOF_FAIL: corner pixel ${Array.from(corner).join(',')}`);
}

// Exercise the GLB bufferView image, native PNG decoder, and WebGPU upload.
const gltf = await loadGLTF('assets/textured-spinner.glb');
const map = gltf.scene.children[0].material.map;
if (!map?.isDataTexture) throw new Error('GPU_PROOF_FAIL: GLB texture was not decoded');
const textureScene = new THREE.Scene();
textureScene.add(new THREE.Mesh(
  new THREE.PlaneGeometry(2, 2),
  new THREE.MeshBasicMaterial({ map, toneMapped: false, side: THREE.DoubleSide }),
));
renderer.setRenderTarget(target);
renderer.render(textureScene, camera);
const dark = await renderer.readRenderTargetPixelsAsync(target, 8, 8, 1, 1);
const light = await renderer.readRenderTargetPixelsAsync(target, 24, 8, 1, 1);
renderer.setRenderTarget(null);
if (!(Math.abs(light[0] - dark[0]) > 100 && (dark[2] > dark[0]) !== (light[2] > light[0]))) {
  throw new Error(`GPU_PROOF_FAIL: texture pixels ${Array.from(dark).join(',')} / ${Array.from(light).join(',')}`);
}

// Exercise Three's browser loader unchanged: FileLoader -> fetch/stream,
// bufferView -> self.URL.createObjectURL -> ImageBitmapLoader -> GPU upload.
const nativeGltf = await new GLTFLoader().loadAsync('assets/textured-spinner.glb');
const nativeMap = nativeGltf.scene.children[0]?.material?.map;
if (!nativeMap?.image || nativeMap.image.width !== 32 || nativeMap.image.height !== 32) {
  throw new Error('GPU_PROOF_FAIL: GLTFLoader did not decode the embedded image');
}
const nativeScene = new THREE.Scene();
nativeScene.add(new THREE.Mesh(
  new THREE.PlaneGeometry(2, 2),
  new THREE.MeshBasicMaterial({ map: nativeMap, toneMapped: false, side: THREE.DoubleSide }),
));
renderer.setRenderTarget(target);
renderer.render(nativeScene, camera);
const nativeDark = await renderer.readRenderTargetPixelsAsync(target, 8, 8, 1, 1);
const nativeLight = await renderer.readRenderTargetPixelsAsync(target, 24, 8, 1, 1);
renderer.setRenderTarget(null);
if (!(Math.abs(nativeLight[0] - nativeDark[0]) > 100 &&
  (nativeDark[2] > nativeDark[0]) !== (nativeLight[2] > nativeLight[0]))) {
  throw new Error(`GPU_PROOF_FAIL: GLTFLoader texture pixels ${Array.from(nativeDark).join(',')} / ${Array.from(nativeLight).join(',')}`);
}
console.log(`GLTF_NATIVE_LOADER_OK pixels=${Array.from(nativeDark).join(',')}/${Array.from(nativeLight).join(',')}`);
console.log(`GPU_PROOF_OK center=${Array.from(center).join(',')} corner=${Array.from(corner).join(',')} texture=${Array.from(dark).join(',')}/${Array.from(light).join(',')}`);

Peregrust.onFrame(() => {
  renderer.render(scene, camera);
  if (Peregrust.frameCount >= 2) Peregrust.exit(0);
});
