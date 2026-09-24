import * as THREE from 'three/webgpu';
import { Peregrust } from 'peregrust';
import { loadGLTF } from 'peregrust/three';

const { canvas, window, assets } = Peregrust;
Peregrust.setTitle('Peregrust · Three.js WebGPU');

// The native window surface supports opaque alpha mode.
const renderer = new THREE.WebGPURenderer({ canvas, antialias: true, alpha: false });
renderer.setPixelRatio(window.devicePixelRatio);
renderer.setSize(window.innerWidth, window.innerHeight, false);
await renderer.init();

const scene = new THREE.Scene();
scene.background = new THREE.Color(0x0c1120);
const camera = new THREE.PerspectiveCamera(55, window.innerWidth / window.innerHeight, 0.1, 100);
camera.position.set(0, 1.5, 7);
camera.lookAt(0, 0, 0);

const ambient = new THREE.AmbientLight(0xffffff, 1.25);
const keyLight = new THREE.DirectionalLight(0xffffff, 3.5);
keyLight.position.set(3, 5, 4);
scene.add(ambient, keyLight);

// Native decoding supplies RGBA pixels. Three uploads them through WebGPU.
const image = await assets.decodeImage('assets/checker.png');
const texture = new THREE.DataTexture(image.data, image.width, image.height);
texture.colorSpace = THREE.SRGBColorSpace;
texture.magFilter = THREE.NearestFilter;
texture.minFilter = THREE.NearestFilter;
texture.wrapS = texture.wrapT = THREE.RepeatWrapping;
texture.needsUpdate = true;

const cube = new THREE.Mesh(
  new THREE.BoxGeometry(2, 2, 2),
  new THREE.MeshStandardMaterial({ map: texture, metalness: 0.15, roughness: 0.5 }),
);
cube.position.x = -1.5;
scene.add(cube);

// This checked-in GLB has a real rotation animation, parsed by Three's loader.
const glb = await loadGLTF('assets/textured-spinner.glb');
glb.scene.position.x = 1.6;
glb.scene.scale.setScalar(1.5);
scene.add(glb.scene);
const mixer = new THREE.AnimationMixer(glb.scene);
if (glb.animations[0]) mixer.clipAction(glb.animations[0]).play();
const tone = await Peregrust.audio.load('assets/tone.wav');
let toneVoice = null;

let dragging = false;
let lastX = 0;
canvas.addEventListener('pointerdown', (event) => {
  dragging = true;
  lastX = event.clientX;
});
canvas.addEventListener('pointerup', () => {
  dragging = false;
});
canvas.addEventListener('pointermove', (event) => {
  if (!dragging) return;
  cube.rotation.y += (event.clientX - lastX) * 0.01;
  lastX = event.clientX;
});
canvas.addEventListener('wheel', (event) => {
  camera.position.z = THREE.MathUtils.clamp(camera.position.z + event.deltaY * 0.005, 3, 15);
});
window.addEventListener('keydown', (event) => {
  if (event.code === 'Escape') Peregrust.exit();
  if (event.code === 'Space') {
    cube.rotation.y += Math.PI / 4;
    toneVoice?.dispose();
    toneVoice = tone.play({ volume: 0.35 });
  }
});
window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setPixelRatio(window.devicePixelRatio);
  renderer.setSize(window.innerWidth, window.innerHeight, false);
});

let lastTime = 0;
Peregrust.onFrame((timestampMs) => {
  const delta = Math.min((timestampMs - lastTime) / 1000, 0.1);
  lastTime = timestampMs;
  cube.rotation.x += delta * 0.5;
  cube.rotation.y += delta * 0.65;
  mixer.update(delta);
  renderer.render(scene, camera);
});
