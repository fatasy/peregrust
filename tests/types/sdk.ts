import { Peregrust, canvas, window } from 'peregrust';
import { loadGLTF } from 'peregrust/three';
import { attachThree } from 'peregrust/inspect/three';
import type { Object3D, Camera, WebGPURenderer } from 'three/webgpu';

Peregrust.control.registerState('test', () => ({ health: 100, position: [0, 1, 2] }));
function registerInspector(scene: Object3D, camera: Camera, renderer: WebGPURenderer) {
  return attachThree({ scene, camera, renderer });
}
void registerInspector;

const context: GPUCanvasContext = canvas.getContext('webgpu');
const dimensions: [number, number] = [window.innerWidth, window.innerHeight];
const cancel = Peregrust.onFrame(async (timestampMs) => {
  const image = await Peregrust.assets.decodeImage('assets/checker.png');
  const bytes: Uint8Array = await Peregrust.assets.read('assets/spinner.glb');
  console.log(timestampMs, context, dimensions, image.data, bytes);
});
cancel();
void canvas.requestPointerLock();
Peregrust.setFullscreen(false);
void loadGLTF('assets/textured-spinner.glb').then((asset) => asset.scene.rotation.y = 1);
void Peregrust.audio.load('assets/tone.wav').then((clip) => {
  const voice = clip.play({ volume: 0.5, loop: true });
  voice.stop();
  voice.dispose();
  clip.unload();
});
const firstPad = Peregrust.gamepads.poll()[0];
if (firstPad?.connected) console.log(firstPad.axes[0]);
const gpu: GPU = window.navigator.gpu;
console.log(gpu);
