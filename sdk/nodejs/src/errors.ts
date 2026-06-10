//! Error types for the go-on Node.js SDK.

/**
 * Base error class for go-on SDK client errors.
 */
export class GoOnClientError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "GoOnClientError";
  }
}

/**
 * JSON-RPC protocol-level error.
 */
export class GoOnJsonRpcError extends GoOnClientError {
  /** JSON-RPC error code (e.g. -32601 for method not found). */
  code: number;
  /** Human-readable error message from the server. */
  messageText: string;

  constructor(code: number, messageText: string) {
    super(`JSON-RPC error [${code}]: ${messageText}`);
    this.name = "GoOnJsonRpcError";
    this.code = code;
    this.messageText = messageText;
  }
}

/**
 * HTTP-level transport error (non-2xx response without valid JSON-RPC body).
 */
export class GoOnHttpError extends GoOnClientError {
  statusCode: number;
  statusText: string;

  constructor(statusCode: number, statusText: string) {
    super(`HTTP error ${statusCode}: ${statusText}`);
    this.name = "GoOnHttpError";
    this.statusCode = statusCode;
    this.statusText = statusText;
  }
}
