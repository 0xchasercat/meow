import { Buffer } from "node:buffer";
import { EventEmitter } from "node:events";

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
  op_http_respond(slot: number, status: number, headers: Array<[string, string]>, body: Uint8Array): Promise<void>;
  op_http_shutdown(rid: number): Promise<void>;
}

type BinaryLike = string | Uint8Array | ArrayBuffer | ArrayBufferView<ArrayBufferLike> | readonly number[];
type HeaderValue = string | number | readonly string[];
type HeadersInput = Record<string, HeaderValue | undefined> | Headers | Array<readonly [string, string]>;
type RequestCallback = (response: IncomingMessage) => void;
type ServerRequestListener = (request: IncomingMessage, response: ServerResponse) => void;
type ListenCallback = () => void;
type ClientRequestCallback = (response: IncomingMessage) => void;
type BodyChunk = string | Uint8Array | ArrayBuffer | ArrayBufferView<ArrayBufferLike>;
type NodeError = Error & { code?: string };

type RequestInput = string | URL | RequestOptions;

export interface AddressInfo {
  address: string;
  family: string;
  port: number;
}

export interface RequestOptions {
  protocol?: string;
  hostname?: string;
  host?: string;
  port?: number | string;
  path?: string;
  pathname?: string;
  method?: string;
  headers?: HeadersInput;
}

export const METHODS = Object.freeze([
  "ACL",
  "BIND",
  "CHECKOUT",
  "CONNECT",
  "COPY",
  "DELETE",
  "GET",
  "HEAD",
  "LINK",
  "LOCK",
  "M-SEARCH",
  "MERGE",
  "MKACTIVITY",
  "MKCALENDAR",
  "MKCOL",
  "MOVE",
  "NOTIFY",
  "OPTIONS",
  "PATCH",
  "POST",
  "PROPFIND",
  "PROPPATCH",
  "PURGE",
  "PUT",
  "REBIND",
  "REPORT",
  "SEARCH",
  "SOURCE",
  "SUBSCRIBE",
  "TRACE",
  "UNBIND",
  "UNLINK",
  "UNLOCK",
  "UNSUBSCRIBE",
]);

const ops = rawOps as unknown as NativeHttpOps;

function strictWebError(message: string): NodeError {
  const error = new Error(message) as NodeError;
  error.code = "ERR_STRICT_WEB_WITHDRAWN";
  return error;
}

function isArrayBufferView(value: unknown): value is ArrayBufferView<ArrayBufferLike> {
  return ArrayBuffer.isView(value);
}

function toBuffer(value: BinaryLike, encoding = "utf8"): Buffer {
  if (typeof value === "string") return Buffer.from(value, encoding);
  if (value instanceof ArrayBuffer) return Buffer.from(value);
  if (isArrayBufferView(value)) return Buffer.from(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
  return Buffer.from(value as readonly number[]);
}

function concat(chunks: Uint8Array[]): Buffer {
  let size = 0;
  for (const chunk of chunks) size += chunk.length;
  const out = Buffer.alloc(size);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

function normalizeHeaderName(name: string): string {
  return String(name).toLowerCase();
}

function normalizeHeaderValue(value: HeaderValue): string {
  if (Array.isArray(value)) return value.map(String).join(", ");
  return String(value);
}

function headerEntries(headers: HeadersInput | undefined): Array<[string, string]> {
  if (headers === undefined) return [];
  if (headers instanceof Headers) return Array.from(headers.entries());
  if (Array.isArray(headers)) return headers.map(([name, value]) => [String(name), String(value)]);
  const entries: Array<[string, string]> = [];
  for (const [name, value] of Object.entries(headers)) {
    if (value !== undefined) entries.push([name, normalizeHeaderValue(value)]);
  }
  return entries;
}

function headersObject(entries: Array<[string, string]>): Record<string, string> {
  const headers = Object.create(null) as Record<string, string>;
  for (const [name, value] of entries) {
    const key = normalizeHeaderName(name);
    headers[key] = headers[key] === undefined ? value : `${headers[key]}, ${value}`;
  }
  return headers;
}

function rawHeaders(entries: Array<[string, string]>): string[] {
  const out: string[] = [];
  for (const [name, value] of entries) {
    out.push(name, value);
  }
  return out;
}

function urlPath(rawUrl: string): string {
  try {
    const url = new URL(rawUrl);
    return `${url.pathname}${url.search}`;
  } catch {
    return rawUrl;
  }
}

function familyFor(hostname: string): string {
  return hostname.includes(":") ? "IPv6" : "IPv4";
}

function parseListenArgs(args: unknown[]): { hostname: string; port: number; callback?: ListenCallback } {
  let hostname = "0.0.0.0";
  let port = 0;
  let callback: ListenCallback | undefined;
  for (const arg of args) {
    if (typeof arg === "function") {
      callback = arg as ListenCallback;
    } else if (typeof arg === "number") {
      port = arg;
    } else if (typeof arg === "string") {
      if (/^\d+$/.test(arg)) port = Number(arg);
      else hostname = arg;
    } else if (typeof arg === "object" && arg !== null) {
      const opts = arg as { port?: number | string; host?: string; hostname?: string };
      if (opts.port !== undefined) port = Number(opts.port);
      hostname = opts.host ?? opts.hostname ?? hostname;
    }
  }
  return { hostname, port, callback };
}

function emitError(target: EventEmitter, error: unknown): void {
  queueMicrotask(() => target.emit("error", error));
}

function toFetchUrl(input: RequestInput, options: RequestOptions | undefined, defaultProtocol: "http:" | "https:"): URL {
  if (typeof input === "string" || input instanceof URL) {
    const url = new URL(String(input));
    if (options !== undefined) applyRequestOptions(url, options, defaultProtocol);
    return url;
  }
  const url = new URL(`${defaultProtocol}//localhost/`);
  applyRequestOptions(url, input, defaultProtocol);
  if (options !== undefined) applyRequestOptions(url, options, defaultProtocol);
  return url;
}

function applyRequestOptions(url: URL, options: RequestOptions, defaultProtocol: "http:" | "https:"): void {
  url.protocol = options.protocol ?? url.protocol ?? defaultProtocol;
  const host = options.hostname ?? options.host;
  if (host !== undefined) url.hostname = host.includes(":") && host.startsWith("[") ? host.slice(1, -1) : host;
  if (options.port !== undefined) url.port = String(options.port);
  const path = options.path ?? options.pathname;
  if (path !== undefined) {
    const split = path.indexOf("?");
    url.pathname = split === -1 ? path : path.slice(0, split);
    url.search = split === -1 ? "" : path.slice(split);
  }
}

export class IncomingMessage extends EventEmitter {
  method?: string;
  url?: string;
  headers: Record<string, string>;
  rawHeaders: string[];
  statusCode?: number;
  statusMessage?: string;
  socket: Record<string, unknown>;
  connection: Record<string, unknown>;
  httpVersion = "1.1";
  httpVersionMajor = 1;
  httpVersionMinor = 1;
  complete = false;
  readableEnded = false;
  #body: Buffer;

  constructor(init: {
    method?: string;
    url?: string;
    headers?: Array<[string, string]>;
    body?: Uint8Array;
    statusCode?: number;
    statusMessage?: string;
    socket?: Record<string, unknown>;
  }) {
    super();
    const headers = init.headers ?? [];
    this.method = init.method;
    this.url = init.url;
    this.headers = headersObject(headers);
    this.rawHeaders = rawHeaders(headers);
    this.statusCode = init.statusCode;
    this.statusMessage = init.statusMessage;
    this.socket = init.socket ?? Object.create(null);
    this.connection = this.socket;
    this.#body = Buffer.from(init.body ?? new Uint8Array());
  }

  setEncoding(_encoding: string): this {
    return this;
  }

  resume(): this {
    return this;
  }

  pushBody(): void {
    if (this.readableEnded) return;
    this.complete = true;
    if (this.#body.length > 0) this.emit("data", this.#body);
    this.readableEnded = true;
    this.emit("end");
    this.emit("close");
  }
}

export class ServerResponse extends EventEmitter {
  statusCode = 200;
  statusMessage = "OK";
  headersSent = false;
  writableEnded = false;
  finished = false;
  socket: Record<string, unknown>;
  #slot: number;
  #headers = new Map<string, string>();
  #headerNames = new Map<string, string>();
  #chunks: Uint8Array[] = [];
  #resolveFinished!: () => void;
  #rejectFinished!: (reason: unknown) => void;
  readonly done: Promise<void>;

  constructor(slot: number, socket: Record<string, unknown>) {
    super();
    this.#slot = slot;
    this.socket = socket;
    this.done = new Promise<void>((resolve, reject) => {
      this.#resolveFinished = resolve;
      this.#rejectFinished = reject;
    });
  }

  setHeader(name: string, value: HeaderValue): this {
    const key = normalizeHeaderName(name);
    this.#headerNames.set(key, String(name));
    this.#headers.set(key, normalizeHeaderValue(value));
    return this;
  }

  getHeader(name: string): string | undefined {
    return this.#headers.get(normalizeHeaderName(name));
  }

  getHeaders(): Record<string, string> {
    const out = Object.create(null) as Record<string, string>;
    for (const [name, value] of this.#headers) out[name] = value;
    return out;
  }

  hasHeader(name: string): boolean {
    return this.#headers.has(normalizeHeaderName(name));
  }

  removeHeader(name: string): void {
    const key = normalizeHeaderName(name);
    this.#headers.delete(key);
    this.#headerNames.delete(key);
  }

  writeHead(statusCode: number, statusMessageOrHeaders?: string | HeadersInput, maybeHeaders?: HeadersInput): this {
    this.statusCode = Number(statusCode) | 0;
    if (typeof statusMessageOrHeaders === "string") {
      this.statusMessage = statusMessageOrHeaders;
    }
    const headers = typeof statusMessageOrHeaders === "string" ? maybeHeaders : statusMessageOrHeaders;
    for (const [name, value] of headerEntries(headers)) this.setHeader(name, value);
    this.headersSent = true;
    return this;
  }

  write(chunk: BodyChunk, encodingOrCallback?: string | (() => void), maybeCallback?: () => void): boolean {
    if (this.writableEnded) throw new Error("write after end");
    const encoding = typeof encodingOrCallback === "string" ? encodingOrCallback : "utf8";
    const callback = typeof encodingOrCallback === "function" ? encodingOrCallback : maybeCallback;
    this.#chunks.push(toBuffer(chunk, encoding));
    this.headersSent = true;
    callback?.();
    return true;
  }

  end(chunk?: BodyChunk | (() => void), encodingOrCallback?: string | (() => void), maybeCallback?: () => void): this {
    if (typeof chunk === "function") {
      maybeCallback = chunk;
      chunk = undefined;
    }
    const encoding = typeof encodingOrCallback === "string" ? encodingOrCallback : "utf8";
    const callback = typeof encodingOrCallback === "function" ? encodingOrCallback : maybeCallback;
    if (chunk !== undefined) this.write(chunk, encoding);
    if (this.writableEnded) return this;
    this.writableEnded = true;
    this.finished = true;
    this.headersSent = true;
    void this.#send().then(
      () => {
        callback?.();
        this.emit("finish");
        this.emit("close");
        this.#resolveFinished();
      },
      (error) => {
        this.emit("error", error);
        this.#rejectFinished(error);
      },
    );
    return this;
  }

  async #send(): Promise<void> {
    const headers: Array<[string, string]> = [];
    for (const [key, value] of this.#headers) headers.push([this.#headerNames.get(key) ?? key, value]);
    await ops.op_http_respond(this.#slot, this.statusCode, headers, concat(this.#chunks));
  }

  respondInternalError(error?: unknown): void {
    if (this.writableEnded) return;
    if (error !== undefined) this.emit("error", error);
    this.statusCode = 500;
    this.#headers.clear();
    this.#headerNames.clear();
    this.#chunks = [Buffer.from("Internal Server Error")];
    this.end();
  }
}

export class Server extends EventEmitter {
  #rid: number | null = null;
  #addr: AddressInfo | null = null;
  #closed = false;
  #finished: Promise<void> = Promise.resolve();

  listen(...args: unknown[]): this {
    if (this.#rid !== null) throw new Error("Server is already listening");
    const { hostname, port, callback } = parseListenArgs(args);
    let handle: NativeServeHandle;
    try {
      handle = ops.op_http_serve(hostname, port);
    } catch (error) {
      emitError(this, error);
      return this;
    }
    this.#rid = handle.rid;
    this.#addr = {
      address: handle.addr.hostname,
      family: familyFor(handle.addr.hostname),
      port: handle.addr.port,
    };
    this.#closed = false;
    if (callback !== undefined) this.once("listening", callback);
    this.#finished = this.#pump();
    queueMicrotask(() => this.emit("listening"));
    return this;
  }

  address(): AddressInfo | null {
    return this.#addr === null ? null : { ...this.#addr };
  }

  close(callback?: (error?: Error) => void): this {
    if (callback !== undefined) this.once("close", () => callback());
    if (this.#closed) return this;
    this.#closed = true;
    const rid = this.#rid;
    if (rid === null) {
      queueMicrotask(() => this.emit("close"));
      return this;
    }
    void ops.op_http_shutdown(rid).catch((error: unknown) => this.emit("error", error));
    return this;
  }

  ref(): this {
    return this;
  }

  unref(): this {
    return this;
  }

  get listening(): boolean {
    return this.#rid !== null && !this.#closed;
  }

  get finished(): Promise<void> {
    return this.#finished;
  }

  async #pump(): Promise<void> {
    const rid = this.#rid;
    if (rid === null) return;
    try {
      while (true) {
        const next = await ops.op_http_next(rid);
        if (next === null) break;
        void this.#dispatch(next);
      }
    } catch (error) {
      this.emit("error", error);
    } finally {
      this.#rid = null;
      this.#closed = true;
      this.emit("close");
    }
  }

  async #dispatch(next: NativeIncomingRequest): Promise<void> {
    const socket = Object.freeze({ localAddress: this.#addr?.address, localPort: this.#addr?.port, encrypted: false });
    const req = new IncomingMessage({
      method: next.method,
      url: urlPath(next.url),
      headers: next.headers,
      body: next.body,
      socket,
    });
    const res = new ServerResponse(next.slot, socket);
    try {
      const handled = this.emit("request", req, res);
      queueMicrotask(() => req.pushBody());
      if (!handled) {
        res.statusCode = 404;
        res.end("Not Found");
      }
    } catch (error) {
      res.respondInternalError(error);
    }
    await res.done.catch((error: unknown) => this.emit("error", error));
  }
}

export function createServer(listener?: ServerRequestListener): Server {
  const server = new Server();
  if (listener !== undefined) server.on("request", listener as (...args: unknown[]) => void);
  return server;
}

export class ClientRequest extends EventEmitter {
  #url: URL;
  #method: string;
  #headers = new Headers();
  #chunks: Uint8Array[] = [];
  #sent = false;
  #aborted = false;

  constructor(input: RequestInput, options?: RequestOptions, callback?: ClientRequestCallback, defaultProtocol: "http:" | "https:" = "http:") {
    super();
    this.#url = toFetchUrl(input, options, defaultProtocol);
    const merged = typeof input === "object" && !(input instanceof URL) ? { ...input, ...options } : options;
    this.#method = merged?.method ?? "GET";
    for (const [name, value] of headerEntries(merged?.headers)) this.#headers.set(name, value);
    if (callback !== undefined) this.once("response", callback as (...args: unknown[]) => void);
  }

  setHeader(name: string, value: HeaderValue): this {
    this.#headers.set(name, normalizeHeaderValue(value));
    return this;
  }

  getHeader(name: string): string | null {
    return this.#headers.get(name);
  }

  removeHeader(name: string): void {
    this.#headers.delete(name);
  }

  write(chunk: BodyChunk, encodingOrCallback?: string | (() => void), maybeCallback?: () => void): boolean {
    const encoding = typeof encodingOrCallback === "string" ? encodingOrCallback : "utf8";
    const callback = typeof encodingOrCallback === "function" ? encodingOrCallback : maybeCallback;
    this.#chunks.push(toBuffer(chunk, encoding));
    callback?.();
    return true;
  }

  end(chunk?: BodyChunk | (() => void), encodingOrCallback?: string | (() => void), maybeCallback?: () => void): this {
    if (typeof chunk === "function") {
      maybeCallback = chunk;
      chunk = undefined;
    }
    const encoding = typeof encodingOrCallback === "string" ? encodingOrCallback : "utf8";
    const callback = typeof encodingOrCallback === "function" ? encodingOrCallback : maybeCallback;
    if (chunk !== undefined) this.write(chunk, encoding);
    if (!this.#sent) {
      this.#sent = true;
      void this.#send().then(() => callback?.(), (error) => this.emit("error", error));
    }
    return this;
  }

  abort(): void {
    this.#aborted = true;
    this.emit("abort");
  }

  destroy(error?: Error): this {
    this.#aborted = true;
    if (error !== undefined) this.emit("error", error);
    this.emit("close");
    return this;
  }

  async #send(): Promise<void> {
    if (this.#aborted) throw strictWebError("request aborted");
    const method = this.#method.toUpperCase();
    const body = method === "GET" || method === "HEAD" ? undefined : concat(this.#chunks);
    const response = await fetch(this.#url, { method, headers: this.#headers, body });
    const bytes = new Uint8Array(await response.arrayBuffer());
    const msg = new IncomingMessage({
      statusCode: response.status,
      statusMessage: response.statusText,
      headers: Array.from(response.headers.entries()),
      body: bytes,
      socket: Object.freeze({ encrypted: this.#url.protocol === "https:" }),
    });
    this.emit("response", msg);
    queueMicrotask(() => msg.pushBody());
    this.emit("close");
  }
}

export function request(input: RequestInput, optionsOrCallback?: RequestOptions | RequestCallback, maybeCallback?: RequestCallback): ClientRequest {
  const options = typeof optionsOrCallback === "function" ? undefined : optionsOrCallback;
  const callback = typeof optionsOrCallback === "function" ? optionsOrCallback : maybeCallback;
  return new ClientRequest(input, options, callback, "http:");
}

export function get(input: RequestInput, optionsOrCallback?: RequestOptions | RequestCallback, maybeCallback?: RequestCallback): ClientRequest {
  const req = request(input, optionsOrCallback as RequestOptions | RequestCallback | undefined, maybeCallback);
  req.end();
  return req;
}

export function requestWithProtocol(
  protocol: "http:" | "https:",
  input: RequestInput,
  optionsOrCallback?: RequestOptions | RequestCallback,
  maybeCallback?: RequestCallback,
): ClientRequest {
  const options = typeof optionsOrCallback === "function" ? undefined : { ...(optionsOrCallback ?? {}), protocol };
  const callback = typeof optionsOrCallback === "function" ? optionsOrCallback : maybeCallback;
  return new ClientRequest(input, options, callback, protocol);
}

export default {
  METHODS,
  Server,
  IncomingMessage,
  ServerResponse,
  ClientRequest,
  createServer,
  request,
  get,
};
