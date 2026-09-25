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

export interface PeregrustRuntime {
  readonly gpu: PeregrustGpu;
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
