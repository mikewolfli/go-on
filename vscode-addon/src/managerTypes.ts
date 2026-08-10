export interface StreamingCallbacks {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  onToken: (_token: string) => void;
  onDone: () => void;
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  onError: (_error: Error) => void;
}

export interface StreamRequestOptions {
  signal?: AbortSignal;
  skipProviderGuard?: boolean;
  callbacks?: StreamingCallbacks;
  /** Request timeout in milliseconds (default: 30_000). */
  timeout?: number;
}

export interface RuntimeManagerLike {
  isRunning(): boolean;
  sendRequest(_method: string, _params?: unknown): Promise<unknown>;
  /**
   * Send a request with SSE streaming support.
   * Opens an HTTP SSE connection to the backend, emits incremental tokens
   * via callbacks, and falls back to sendRequest if streaming is unavailable.
   */
  sendStreamingRequest?(
    _method: string,
    _params?: unknown,
    _options?: StreamRequestOptions,
  ): Promise<string>;
  /** Send a cancel notification to abort an in-flight streaming request. */
  sendCancelRequest?(): Promise<void>;
  setRuntimeEnvOverrides?(_overrides: Record<string, string>): void;
  /**
   * Trigger reconnection from external observer (e.g., StatusMonitor).
   * Only initiates reconnection if the manager is not running.
   * Does nothing if the manager is already running or shutting down.
   */
  triggerReconnectFromObserver?(): void;
}
