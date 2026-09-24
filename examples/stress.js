import * as THREE from 'three/webgpu';

// The runtime supplies one native canvas globally; no Peregrust module import is required.
const Peregrust = globalThis.Peregrust;

function objectCount(args) {
  const option = args.indexOf('--objects');
  const input = option >= 0 ? args[option + 1] : globalThis.PEREGRUST_STRESS_OBJECTS;
  const count = Number(input ?? 1000);
  if (!Number.isSafeInteger(count) || count < 1 || count > 100000) {
    throw new RangeError('--objects must be an integer between 1 and 100000');
  }
  return count;
}

const count = objectCount(Peregrust.args);
const modeOption = Peregrust.args.indexOf('--mode');
const mode = modeOption >= 0 ? Peregrust.args[modeOption + 1] : 'instances';
if (mode !== 'instances' && mode !== 'meshes') {
  throw new RangeError('--mode must be instances or meshes');
}
if (mode === 'meshes' && count > 5000) {
  throw new RangeError('--mode meshes is limited to 5000 objects');
}
Peregrust.setTitle(`Peregrust stress · ${count} ${mode}`);
const { canvas, window } = Peregrust;
const renderer = new THREE.WebGPURenderer({ canvas, antialias: false, alpha: false });
renderer.setPixelRatio(window.devicePixelRatio);
renderer.setSize(window.innerWidth, window.innerHeight, false);
await renderer.init();

const scene = new THREE.Scene();
scene.background = new THREE.Color(0x07101e);
const camera = new THREE.PerspectiveCamera(60, window.innerWidth / window.innerHeight, 0.1, 1000);
camera.position.set(0, 8, 28);
camera.lookAt(0, 0, 0);
scene.add(new THREE.HemisphereLight(0xffffff, 0x2b4262, 2.5));
const light = new THREE.DirectionalLight(0xffffff, 3);
light.position.set(5, 10, 8);
scene.add(light);

const geometry = new THREE.BoxGeometry(0.8, 0.8, 0.8);
const material = new THREE.MeshStandardMaterial({ color: 0x55c6ff, roughness: 0.45 });
const group = new THREE.Group();
const instances = mode === 'instances' ? new THREE.InstancedMesh(geometry, material, count) : null;
const matrix = new THREE.Matrix4();
const position = new THREE.Vector3();
const rotation = new THREE.Quaternion();
const scale = new THREE.Vector3(1, 1, 1);
const grid = Math.ceil(Math.cbrt(count));
for (let i = 0; i < count; i++) {
  position.set(i % grid, Math.floor(i / grid) % grid, Math.floor(i / (grid * grid)));
  position.addScalar(-(grid - 1) / 2);
  position.multiplyScalar(1.25);
  if (instances) {
    matrix.compose(position, rotation, scale);
    instances.setMatrixAt(i, matrix);
  } else {
    const mesh = new THREE.Mesh(geometry, material);
    mesh.position.copy(position);
    group.add(mesh);
  }
}
if (instances) {
  instances.instanceMatrix.needsUpdate = true;
  group.add(instances);
}
scene.add(group);

window.addEventListener('keydown', (event) => {
  if (event.code === 'Escape') Peregrust.exit();
});
window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setPixelRatio(window.devicePixelRatio);
  renderer.setSize(window.innerWidth, window.innerHeight, false);
});

let lastReport = 0;
let frames = 0;
Peregrust.onFrame((timestampMs) => {
  group.rotation.y = timestampMs * 0.00015;
  group.rotation.x = timestampMs * 0.00008;
  renderer.render(scene, camera);
  frames++;
  if (timestampMs - lastReport >= 1000) {
    console.log(`${count} ${mode} · ${frames} fps · ${renderer.info.render.calls} draw calls · frame ${Peregrust.frameCount}`);
    frames = 0;
    lastReport = timestampMs;
  }
});
