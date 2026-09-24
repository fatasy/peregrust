import type { GLTF } from 'three/addons/loaders/GLTFLoader.js';
import type { PeregrustRuntime } from './index.js';

/** Loads a local glTF/GLB, decoding PNG/JPEG/WebP images through Peregrust. */
export function loadGLTF(path: string, options?: { runtime?: PeregrustRuntime }): Promise<GLTF>;
