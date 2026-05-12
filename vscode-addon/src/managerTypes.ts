export interface RuntimeManagerLike {
  isRunning(): boolean;
  sendRequest(_method: string, _params?: unknown): Promise<unknown>;
  setRuntimeEnvOverrides?(_overrides: Record<string, string>): void;
}
