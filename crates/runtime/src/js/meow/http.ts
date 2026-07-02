const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

interface NativeNetAddr {
  readonly hostname: string;
  readonly port: number;
}

interface NativeServeHandle {
  readonly rid: number;
  readonly addr: NativeNetAddr;
}

interface NativeIncomingRequest {
  readonly slot: number;
  readonly method: string;
  readonly url: string;
  readonly headers: Array<[string, string]>;
  readonly body: Uint8Array;
}

interface NativeHttpOps {
  op_http_serve(hostname: string, port: number): NativeServeHandle;
  op_http_next(rid: number): Promise<NativeIncomingRequest | null>;
  op_http_respond(
    slot: number,
    status: number,
    headers: Array<[string, string]>,
    body: Uint8Array,
  ): Promise<void>;
  op_http_shutdown(rid: number): Promise<void>;
}

interface WireResponse {
  status: number;
  headers: Array<[string, string]>;
  body: Uint8Array;
}

const ops = rawOps as unknown as NativeHttpOps;
// Deno.core.ops is intentionally untyped in the runtime bootstrap; narrow it once here.

export interface NetAddr {
  readonly hostname: string;
  readonly port: number;
}

export interface ServeOptions {
  /** TCP port to bind. Default 8000. */
  port?: number;
  /** Interface to bind. Default "0.0.0.0". */
  hostname?: string;
  /** Abort to stop accepting and drain in-flight requests. */
  signal?: AbortSignal;
  /** Called once the listener is bound, with the resolved address. */
  onListen?: (addr: NetAddr) => void;
  /** Per-request handler (Deno-style: serve({ port, fetch }) ). */
  fetch?: Handler;
}

export interface Server {
  /** The bound address (resolved port - `0` becomes the OS-assigned port). */
  readonly addr: NetAddr;
  /** Stop accepting, drain in-flight requests, release the listener. Idempotent. */
  shutdown(): Promise<void>;
  /** Resolves when the accept loop has fully stopped (after shutdown / signal). */
  readonly finished: Promise<void>;
}

export type Handler = (request: Request) => Response | Promise<Response>;

function internalErrorResponse(): Response {
  return new Response("Internal Server Error", { status: 500 });
}


function buildRequest(parts: NativeIncomingRequest): Request {
  const body =
    parts.body.byteLength > 0 && parts.method !== "GET" && parts.method !== "HEAD"
      ? parts.body
      : undefined;
  return new Request(parts.url, {
    method: parts.method,
    headers: parts.headers,
    body,
  });
}

let responseSym: symbol | null = null;
let headersListSym: symbol | null = null;
const textEncoder = new TextEncoder();

function getResponseSym(res: Response): symbol | null {
  if (responseSym !== null) return responseSym;
  const syms = Object.getOwnPropertySymbols(res);
  const found = syms.find(s => s.toString() === "Symbol(response)");
  if (found) {
    responseSym = found;
  }
  return responseSym;
}

function getHeadersList(headers: Headers): Array<[string, string]> {
  if (headersListSym === null) {
    const syms = Object.getOwnPropertySymbols(headers);
    const found = syms.find(s => s.toString() === "Symbol(header list)");
    if (found) {
      headersListSym = found;
    }
  }
  if (headersListSym !== null) {
    return (headers as any)[headersListSym] || [];
  }
  return Array.from(headers.entries());
}

async function responseToWire(response: Response): Promise<WireResponse> {
  const sym = getResponseSym(response);
  if (sym) {
    const inner = (response as any)[sym];
    if (inner) {
      if (inner.body === null) {
        return {
          status: response.status,
          headers: getHeadersList(response.headers),
          body: new Uint8Array(0),
        };
      }
      const streamOrStatic = inner.body.streamOrStatic;
      if (streamOrStatic && streamOrStatic.body !== undefined) {
        const rawBody = streamOrStatic.body;
        let bodyBytes: Uint8Array;
        if (typeof rawBody === "string") {
          bodyBytes = textEncoder.encode(rawBody);
        } else if (rawBody instanceof Uint8Array) {
          bodyBytes = rawBody;
        } else if (rawBody instanceof ArrayBuffer) {
          bodyBytes = new Uint8Array(rawBody);
        } else if (ArrayBuffer.isView(rawBody)) {
          bodyBytes = new Uint8Array(rawBody.buffer, rawBody.byteOffset, rawBody.byteLength);
        } else {
          bodyBytes = new Uint8Array(await response.arrayBuffer());
        }
        return {
          status: response.status,
          headers: getHeadersList(response.headers),
          body: bodyBytes,
        };
      }
    }
  }

  // Fallback
  const body = new Uint8Array(await response.arrayBuffer());
  return {
    status: response.status,
    headers: Array.from(response.headers.entries()),
    body,
  };
}

function reportHandlerError(error: unknown): void {
  const detail = error instanceof Error ? error.message : String(error);
  console.error(`meow:http handler failed: ${detail}`);
}

/**
 * Start a Web-standard HTTP/1.1 server. `handler` is invoked per request with a
 * `Request` and must return a `Response`. Requires the network capability; with no
 * grant the bind rejects through `server.finished` with a fix-pointing error.
 *
 * Accepts either form:
 *   serve(handler, { port })       — classic
 *   serve({ port, fetch })         — Deno-style
 */
export function serve(
  handlerOrOptions: Handler | ServeOptions,
  maybeOptions?: ServeOptions,
): Server {
  let handler: Handler;
  let options: ServeOptions;
  if (typeof handlerOrOptions === "function") {
    handler = handlerOrOptions;
    options = maybeOptions ?? {};
  } else {
    options = handlerOrOptions;
    handler = options.fetch ?? (() => new Response("No handler", { status: 500 }));
  }
  const hostname = options.hostname ?? "0.0.0.0";
  const port = options.port ?? 8000;
  const addr: { hostname: string; port: number } = { hostname, port };

  let shutdownRequested = false;
  let shutdownOp: Promise<void> | null = null;
  const inFlight = new Set<Promise<void>>();

  let resolveFinished = (): void => {};
  let rejectFinished = (_reason: unknown): void => {};
  const finished = new Promise<void>((resolve, reject) => {
    resolveFinished = resolve;
    rejectFinished = reject;
  });

  // Bind synchronously so `server.addr` carries the RESOLVED port the moment serve()
  // returns (A1: port 0 -> the OS-assigned port). A bind failure (e.g. a denied
  // network capability) rejects `finished` with a fix-pointing error rather than
  // throwing from serve() — the documented contract, observed via `server.finished`.
  let rid: number;
  try {
    const handle = ops.op_http_serve(hostname, port);
    rid = handle.rid;
    addr.hostname = handle.addr.hostname;
    addr.port = handle.addr.port;
  } catch (error) {
    rejectFinished(error);
    return {
      addr,
      shutdown: () => finished.catch(() => {}),
      finished,
    };
  }

  async function requestShutdown(): Promise<void> {
    shutdownRequested = true;
    if (shutdownOp !== null) {
      return shutdownOp;
    }
    shutdownOp = ops.op_http_shutdown(rid).catch((error: unknown) => {
      shutdownOp = null;
      throw error;
    });
    return shutdownOp;
  }

  const onAbort = (): void => {
    void requestShutdown();
  };

  options.onListen?.(addr);

  const run = (async () => {
    try {
      if (shutdownRequested) {
        await requestShutdown();
      }
      while (true) {
        const next = await ops.op_http_next(rid);
        if (next === null) {
          break;
        }
        const task = (async () => {
          let response = internalErrorResponse();
          try {
            const result = await handler(buildRequest(next));
            if (result instanceof Response) {
              response = result;
            } else {
              reportHandlerError(new TypeError("meow:http handlers must return a Response"));
            }
          } catch (error) {
            reportHandlerError(error);
          }
          try {
            const wire = await responseToWire(response);
            await ops.op_http_respond(next.slot, wire.status, wire.headers, wire.body);
          } catch (error) {
            reportHandlerError(error);
            try {
              const wire = await responseToWire(internalErrorResponse());
              await ops.op_http_respond(next.slot, wire.status, wire.headers, wire.body);
            } catch (respondError) {
              reportHandlerError(respondError);
            }
          }
        })();
        inFlight.add(task);
        void task.finally(() => {
          inFlight.delete(task);
        });
      }
      if (inFlight.size > 0) {
        await Promise.allSettled([...inFlight]);
      }
      resolveFinished();
    } catch (error) {
      rejectFinished(error);
    } finally {
      options.signal?.removeEventListener("abort", onAbort);
    }
  })();

  if (options.signal?.aborted) {
    void requestShutdown();
  } else {
    options.signal?.addEventListener("abort", onAbort);
  }

  void run;

  return {
    addr,
    async shutdown(): Promise<void> {
      await requestShutdown();
      await finished;
    },
    finished,
  };
}
