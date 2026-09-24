import type { Object3D, Camera, WebGPURenderer } from 'three/webgpu';
import type { PeregrustControl } from './control.js';
export type { ControlJson, PeregrustControl } from './control.js';

/** Register an optional Three.js scene inspector. Returns an unregister function.
 * Captures render this scene/camera offscreen; pass render to include a custom pipeline.
 */
export function attachThree(options: {
  scene: Object3D;
  camera: Camera;
  renderer: WebGPURenderer;
  name?: string;
  runtime?: { control: PeregrustControl };
  /** Render into the current target without advancing simulation. */
  render?: () => void | Promise<void>;
}): () => void;
