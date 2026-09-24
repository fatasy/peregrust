import type { ControlJson } from './control.js';

export interface ControlResponse<T = ControlJson> {
  ok: true;
  sessionId: string;
  frame: number;
  result: T;
}
export interface RequestOptions { timeoutMs?: number }
export interface CaptureResult {
  capture: { mimeType: 'image/png'; width: number; height: number; data?: string; path?: string };
}
export class ControlError extends Error {
  readonly code: string;
  readonly response: { ok: false; frame?: number; sessionId?: string; error: { code: string; message: string } };
}
export class PeregrustClient {
  constructor(session: { version: 1; address: string; token: string; sessionId: string });
  static connect(sessionFile: string): Promise<PeregrustClient>;
  readonly sessionId: string;
  call<T = ControlJson>(method: string, params?: Record<string, ControlJson>, options?: RequestOptions): Promise<ControlResponse<T>>;
  batch(operations: Array<{ method: string; params?: Record<string, ControlJson> }>, options?: RequestOptions): Promise<ControlResponse[]>;
  capture(params?: { scene?: string; width?: number; height?: number }, output?: string, options?: RequestOptions): Promise<ControlResponse<CaptureResult>>;
}
export function connect(sessionFile: string): Promise<PeregrustClient>;
