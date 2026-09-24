export type ControlJson = null | boolean | number | string | ControlJson[] | { [key: string]: ControlJson };

export interface PeregrustSceneAdapter {
  query(params: Record<string, unknown>): unknown;
  update(params: Record<string, unknown>): void;
  capture?(params: Record<string, unknown>): Promise<{ width: number; height: number; pixels: Uint8Array }>;
}

export interface PeregrustControl {
  /** True only when started with --control SESSION. Registration itself is always available. */
  readonly enabled: boolean;
  registerScene(name: string, adapter: PeregrustSceneAdapter): () => void;
  /** Providers return synchronous JSON at a frame boundary. */
  registerState(name: string, provider: () => ControlJson): () => void;
  /** Synchronous game actions. inputSchema uses the documented built-in schema subset. */
  registerAction(name: string, definition: { description: string; inputSchema: Record<string, unknown> },
    handler: (input: Record<string, ControlJson>) => ControlJson | void): () => void;
}

