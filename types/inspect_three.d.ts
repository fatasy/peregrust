import type { Object3D, Camera, WebGPURenderer } from 'three/webgpu';
import type { PeregrustControl } from './control.js';
export type { ControlJson, PeregrustControl } from './control.js';

/** Register an optional Three.js scene inspector. Returns an unregister function.
 * Captures render this scene/camera offscreen as screen output (the capture is the renderer's
 * output render target), so PNG bytes match the window; pass render to include a custom pipeline.
 */
export function attachThree(options: {
  scene: Object3D;
  camera: Camera;
  renderer: WebGPURenderer;
  name?: string;
  runtime?: { control: PeregrustControl };
  /** Draw the frame as for the screen (render target null) without advancing simulation. */
  render?: () => void | Promise<void>;
}): () => void;
