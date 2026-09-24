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

export interface PeregrustRuntime {
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
  const Peregrust: PeregrustRuntime;
}
