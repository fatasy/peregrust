// The game is loaded by peregrust.exe. Only Three.js is imported here;
// the native runtime provides the window, canvas and animation clock.
import * as THREE from 'three/webgpu';

const renderer = new THREE.WebGPURenderer({
  canvas: Peregrust.canvas,
  alpha: false,
  antialias: true,
});
renderer.setPixelRatio(window.devicePixelRatio);
renderer.setSize(window.innerWidth, window.innerHeight, false);
await renderer.init();

const scene = new THREE.Scene();
scene.background = new THREE.Color(0x101828);
const camera = new THREE.PerspectiveCamera(60, window.innerWidth / window.innerHeight, 0.1, 100);
camera.position.z = 4;
const cube = new THREE.Mesh(new THREE.BoxGeometry(), new THREE.MeshNormalMaterial());
scene.add(cube);

window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setPixelRatio(window.devicePixelRatio);
  renderer.setSize(window.innerWidth, window.innerHeight, false);
});

await renderer.setAnimationLoop((time) => {
  cube.rotation.x = time * 0.0004;
  cube.rotation.y = time * 0.0007;
  renderer.render(scene, camera);
});
