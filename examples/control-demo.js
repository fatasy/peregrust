import * as THREE from 'three/webgpu';
import { pass } from 'three/tsl';
import { attachThree } from 'peregrust/inspect/three';

const renderer = new THREE.WebGPURenderer({ canvas: Peregrust.canvas, alpha: false, antialias: false });
renderer.setPixelRatio(1);
renderer.setSize(window.innerWidth, window.innerHeight, false);
await renderer.init();
const scene = new THREE.Scene();
scene.background = new THREE.Color(0x0000ff);
const camera = new THREE.OrthographicCamera(-2, 2, 1.5, -1.5, 0.1, 10);
camera.position.z = 5;
const player = new THREE.Mesh(new THREE.BoxGeometry(0.6, 0.6, 0.6),
  new THREE.MeshBasicMaterial({ color: 0xff0000, toneMapped: false }));
player.name = 'player';
player.userData.tags = ['player', 'controllable'];
scene.add(player);
// Asymmetric marker makes image orientation verifiable.
const marker = new THREE.Mesh(new THREE.PlaneGeometry(0.4, 0.4),
  new THREE.MeshBasicMaterial({ color: 0x00ff00, toneMapped: false }));
marker.name = 'orientation-marker';
marker.position.set(-1.4, 1, 0);
scene.add(marker);
// Mid grey: 0x808080 is 128 on screen, so captures show whether colour is encoded once.
const swatch = new THREE.Mesh(new THREE.PlaneGeometry(0.4, 0.4),
  new THREE.MeshBasicMaterial({ color: 0x808080, toneMapped: false }));
swatch.name = 'grey-swatch';
swatch.position.set(1.4, -1, 0);
scene.add(swatch);
attachThree({ scene, camera, renderer });
// The same view through a RenderPipeline, whose output pass tone maps and encodes itself.
const pipeline = new THREE.RenderPipeline(renderer);
pipeline.outputNode = pass(scene, camera);
attachThree({ name: 'pipeline', scene, camera, renderer, render: () => pipeline.render() });
const held = new Set();
let pointerEvents = 0;
window.addEventListener('keydown', (event) => {
  held.add(event.code);
  if (event.code === 'Escape') Peregrust.exit();
});
window.addEventListener('keyup', (event) => held.delete(event.code));
window.addEventListener('pointerdown', () => pointerEvents++);
Peregrust.control.registerState('player', () => ({
  position: player.position.toArray(), held: [...held], pointerEvents,
}));
Peregrust.control.registerAction('player.teleport', {
  description: 'Move the demo player to an exact world position.',
  inputSchema: { type: 'object', properties: { x: { type: 'number' }, y: { type: 'number' } },
    required: ['x', 'y'], additionalProperties: false },
}, ({ x, y }) => {
  player.position.set(x, y, 0);
  console.info('Player teleported');
  return { position: player.position.toArray() };
});
window.addEventListener('resize', () => renderer.setSize(window.innerWidth, window.innerHeight, false));
await renderer.setAnimationLoop(() => {
  if (held.has('KeyW')) player.position.y += 0.02;
  if (held.has('KeyS')) player.position.y -= 0.02;
  if (held.has('KeyA')) player.position.x -= 0.02;
  if (held.has('KeyD')) player.position.x += 0.02;
  renderer.render(scene, camera);
});
