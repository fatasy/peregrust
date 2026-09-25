import type { PeregrustControl } from './control.js';
export type { ControlJson, PeregrustControl, PeregrustSceneAdapter } from './control.js';

export interface PeregrustWindow extends EventTarget {
  readonly innerWidth: number;
  readonly innerHeight: number;
  readonly devicePixelRatio: number;
  readonly document: PeregrustDocument;
  readonly navigator: { readonly gpu: GPU; getGamepads(): Array<PeregrustGamepad | null> };
  requestAnimationFrame(callback: FrameRequestCallback): number;
  cancelAnimationFrame(id: number): void;
}

export interface PeregrustCanvas extends EventTarget {
  width: number;
  height: number;
  readonly clientWidth: number;
  readonly clientHeight: number;
  readonly style: { width: string; height: string };
  readonly ownerDocument: PeregrustDocument;
  getContext(type: 'webgpu'): GPUCanvasContext;
  getContext(type: string): GPUCanvasContext | null;
  getBoundingClientRect(): DOMRect;
  focus(): boolean;
  setPointerCapture(pointerId: number): 'confined' | 'locked';
  releasePointerCapture(pointerId: number): void;
  hasPointerCapture(pointerId: number): boolean;
  requestPointerLock(): Promise<void>;
}

export interface PeregrustDocument extends EventTarget {
  readonly defaultView: PeregrustWindow;
  readonly body: { appendChild(canvas: PeregrustCanvas): PeregrustCanvas };
  readonly pointerLockElement: PeregrustCanvas | null;
  exitPointerLock(): void;
  createElement(tag: string): never;
  createElementNS(namespace: string, tag: string): never;
}

export interface DecodedImage {
  width: number;
  height: number;
  data: Uint8Array;
}

export interface PeregrustAssets {
  read(path: string): Promise<Uint8Array>;
  decodeImage(path: string): Promise<DecodedImage>;
  decodeImageBytes(bytes: Uint8Array): Promise<DecodedImage>;
}

/** Durable binary values. Keys are 1–96 ASCII letters, digits, '.', '-', '_'. */
export interface PeregrustStorage {
  get(key: string): Uint8Array | null;
  getBackup(key: string): Uint8Array | null;
  /** Checks existence without loading the binary payload. */
  has(key: string): boolean;
  set(key: string, value: ArrayBuffer | ArrayBufferView): void;
  remove(key: string): void;
}

export interface PeregrustAudioVoiceInfo {
  state: 'playing' | 'pausing' | 'paused' | 'waitingToResume' | 'resuming' | 'stopping' | 'stopped';
  positionSeconds: number;
}

export interface PeregrustAudioVoice {
  readonly id: number;
  pause(): void;
  resume(): void;
  stop(): void;
  setVolume(volume: number): void;
  setLoop(looped: boolean): void;
  info(): PeregrustAudioVoiceInfo;
  dispose(): void;
}

export interface PeregrustAudioClip {
  readonly id: number;
  readonly durationSeconds: number;
  play(options?: { volume?: number; loop?: boolean }): PeregrustAudioVoice;
  unload(): void;
}

export interface PeregrustAudio {
  load(path: string): Promise<PeregrustAudioClip>;
}

export interface PeregrustGamepadButton {
  pressed: boolean;
  touched: boolean;
  value: number;
}

export interface PeregrustGamepad {
  id: string;
  index: number;
  connected: boolean;
  mapping: 'standard' | '';
  axes: number[];
  buttons: PeregrustGamepadButton[];
  timestamp: number;
}

export interface PeregrustGamepads {
  poll(): Array<PeregrustGamepad | null>;
}

/** Flat groups of [buffer, bufferOffset, source, dataOffset, size]. */
export type PeregrustUploadBatch = ReadonlyArray<GPUBuffer | number | ArrayBuffer | ArrayBufferView | undefined>;

export interface PeregrustUploadStatistics {
  enabled: boolean;
  batchingQueues: number;
  adapterInfo: { vendor: string; architecture: string; device: string; description: string } | null;
  uploadCalls: number;
  /** Requested source payload bytes, not GPU memory allocated. */
  uploadBytes: number;
  nativeWriteCalls: number;
  nativeBatchCalls: number;
  nativeCalls: number;
  batchedUploads: number;
  batchBytes: number;
  submissions: number;
}

export interface PeregrustGpu {
  /** Flushes pending writes, switches statistics and resets cumulative counters. */
  setUploadStatisticsEnabled(enabled: boolean): void;
  getUploadStatistics(): PeregrustUploadStatistics;
}

/** Native retained renderer. Handles are scoped to a renderer and never reused. */
export interface PeregrustKestrel {
  readonly abiVersion: 8;
  /** settings: [sampleCount (1 or 4), hdr (0 or 1)]. MSAA 4 requires HDR. */
  createRenderer(device: GPUDevice, format: 'bgra8unorm' | 'bgra8unorm-srgb' | 'rgba8unorm' | 'rgba8unorm-srgb', settings: Uint32Array): number;
  /** Vertex stride: position3, normal3, uv2, color4; bounds: min3, max3. */
  createGeometry(renderer: number, vertices: Float32Array, indices: Uint32Array, bounds: Float32Array): number;
  /** Program-defined vertex layout; position.xyz must occupy the first three floats. */
  createShaderGeometry(renderer: number, program: number, vertices: Float32Array, indices: Uint32Array, bounds: Float32Array): number;
  /** descriptor: sourceKind,width,height,colorSpace,flipY,mips,wrapU,wrapV,mag,min,mipFilter,anisotropy.
   * info receives decoded width,height,mip count. Kinds: 0 RGBA8 color, 1 R32F, 2 encoded image,
   * 3 raw RGBA8 independent fields. Kind 3 filters all four mip channels independently and requires linear colorSpace. */
  createTexture(renderer: number, descriptor: Uint32Array, data: Uint8Array, info: Uint32Array): number;
  /** descriptor: format (0 BC1-sRGB, 1 BC4, 2 BC5), size, layers, supplied mip count, anisotropy.
   * Payload is mip-major then layer-major, exactly block-compressed bytes. info receives size,size,mips. */
  createCompressedTextureArray(renderer: number, descriptor: Uint32Array, data: Uint8Array, info: Uint32Array): number;
  /** params: r,g,b,opacity,alphaTest,roughness,metalness,model,side,alphaToCoverage,0,0. textures[0]=colorMap or 0. */
  createMaterial(renderer: number, params: Float32Array, textures: Uint32Array): number;
  /** Registers WGSL and an explicit pipeline/resource descriptor once. */
  createShaderProgram(renderer: number, descriptorJson: string, vertexWgsl: string, fragmentWgsl: string): number;
  /** Parameters concatenate descriptor uniforms in their declared order. */
  createShaderMaterial(renderer: number, program: number, params: Float32Array, textures: Uint32Array): number;
  /** N matrices of 16 floats and N instance colors of 4 floats. */
  createMesh(renderer: number, geometry: number, material: number, matrices: Float32Array, colors: Float32Array): number;
  /** Frame (116 floats): viewProjection16, cameraPosition4, lightDirection4, lightColor4, ambient4,
   * hemisphereSky4, hemisphereGround4, backgroundLinearRGBA4,
   * exposure, toneMapping (0 none/1 ACES), outputColorSpace (0 linear/1 sRGB), reserved0,
   * inverseProjection16, cameraWorld16, timeSeconds, near, far, reserved0,
   * projectionMatrix16 and matrixWorldInverse16.
   * Dirty pairs: [meshId, instanceId] per corresponding 16-float matrix.
   * Stats written: [draw calls, visible instances, culled instances, GPU buffer writes]. */
  render(renderer: number, target: GPUTexture, frame: Float32Array, dirtyPairs: Uint32Array, dirtyMatrices: Float32Array, stats: Uint32Array): void;
  /** kind: 0 renderer (id=0), 1 geometry, 2 material, 3 mesh, 4 texture, 5 shader program. */
  destroy(renderer: number, kind: 0 | 1 | 2 | 3 | 4 | 5, id: number): void;
}

export interface PeregrustRuntime {
  readonly gpu: PeregrustGpu;
  readonly kestrel: PeregrustKestrel;
  readonly control: PeregrustControl;
  readonly canvas: PeregrustCanvas;
  readonly window: PeregrustWindow;
  readonly args: readonly string[];
  readonly frameCount: number;
  readonly width: number;
  readonly height: number;
  readonly devicePixelRatio: number;
  readonly fullscreen: boolean;
  readonly assets: PeregrustAssets;
  readonly storage: PeregrustStorage;
  readonly audio: PeregrustAudio;
  readonly gamepads: PeregrustGamepads;
  setTitle(title: string): void;
  focus(): boolean;
  setFullscreen(enabled: boolean): boolean;
  setCursorVisible(visible: boolean): void;
  setPointerCaptureMode(mode: 'none' | 'confined' | 'locked'): 'none' | 'confined' | 'locked';
  exit(code?: number): void;
  requestAnimationFrame(callback: FrameRequestCallback): number;
  cancelAnimationFrame(id: number): void;
  onFrame(callback: (timestampMs: number) => void | Promise<void>): () => void;
}

export const Peregrust: PeregrustRuntime;
export const canvas: PeregrustCanvas;
export const window: PeregrustWindow;

declare global {
  interface GPUQueue {
    /** Synchronous native batch. Offsets/sizes are elements for TypedArrays, bytes otherwise.
     * Sources must be non-shared; numeric offsets/sizes must be nonnegative safe integers.
     * Earlier writes remain applied if a later operation throws. GPU validation uses error scopes. */
    writeBufferBatch(operations: PeregrustUploadBatch): void;
    /** Opt-in coalescing of writeBuffer calls, with source snapshots and ordered flushes. */
    setWriteBufferBatching(enabled: boolean): boolean;
    flushWriteBufferBatch(): void;
  }
  const Peregrust: PeregrustRuntime;
}
