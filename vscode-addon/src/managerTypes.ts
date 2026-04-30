export interface RuntimeManagerLike {
  isRunning(): boolean;
  sendRequest(_method: string, _params?: unknown): Promise<unknown>;
}
