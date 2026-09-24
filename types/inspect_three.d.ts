import type { Object3D, Camera, WebGPURenderer } from 'three/webgpu';
import type { PeregrustRuntime } from './index.js';

/** Register an optional Three.js scene inspector. Returns an unregister function.
 * Captures render this scene/camera offscreen; they exclude custom postprocessing.
 */
export function attachThree(options: {
  scene: Object3D;
  camera: Camera;
  renderer: WebGPURenderer;
  name?: string;
  runtime?: PeregrustRuntime;
}): () => void;
