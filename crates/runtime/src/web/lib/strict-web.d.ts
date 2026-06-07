// strict-web ambient globals (RT-004 · CANON §8.1).
//
// The committed P1 "Stateless Edge" global surface, typed for editors + `meow
// check`. CURATED from the upstream WHATWG/WinterTC declarations (the same shapes
// TypeScript's lib.dom.d.ts and Deno's .d.ts expose); NOT generated from Rust
// (I-9's generate-from-impl applies to the `meow:*` modules — RT-005). Scoped to
// EXACTLY the §8.1 set: the shadow tsconfig pins `lib: ["esnext"]` (no DOM), so
// `window`/`document`/`localStorage`/`WebSocket` are intentionally absent here —
// the types match the runtime surface, nothing more (CRAFT: prose matches code).
//
// Honest boundary: the runtime exposes a SUPERSET transitively (streams beyond
// fetch bodies, EventTarget, structuredClone, atob/btoa, performance,
// crypto.getRandomValues). Those are present-but-uncommitted: not declared here,
// not promised. `ReadableStream` is declared minimally only because `Request`/
// `Response` bodies reference it; stream construction is not a committed surface.

// --- shared helper types (not globals; underpin the declarations below) ---
type BufferSource = ArrayBufferView | ArrayBuffer;
type BodyInit = string | Blob | BufferSource | FormData | URLSearchParams | ReadableStream<Uint8Array>;
type HeadersInit = Headers | Record<string, string> | [string, string][];
type AlgorithmIdentifier = string | { readonly name: string };
type BlobPart = BufferSource | Blob | string;

interface ReadableStream<R = unknown> {
  readonly locked: boolean;
  cancel(reason?: unknown): Promise<void>;
  getReader(): ReadableStreamDefaultReader<R>;
}
interface ReadableStreamDefaultReader<R = unknown> {
  read(): Promise<{ done: false; value: R } | { done: true; value: undefined }>;
  releaseLock(): void;
  cancel(reason?: unknown): Promise<void>;
}

// --- encoding (deno_web) ---
interface TextEncoder {
  readonly encoding: string;
  encode(input?: string): Uint8Array;
  encodeInto(source: string, destination: Uint8Array): { read: number; written: number };
}
declare var TextEncoder: { prototype: TextEncoder; new (): TextEncoder };

interface TextDecoder {
  readonly encoding: string;
  readonly fatal: boolean;
  readonly ignoreBOM: boolean;
  decode(input?: BufferSource, options?: { stream?: boolean }): string;
}
declare var TextDecoder: {
  prototype: TextDecoder;
  new (label?: string, options?: { fatal?: boolean; ignoreBOM?: boolean }): TextDecoder;
};

// --- DOMException (deno_web): the platform error type AbortController throws ---
interface DOMException {
  readonly name: string;
  readonly message: string;
  readonly code: number;
}
declare var DOMException: {
  prototype: DOMException;
  new (message?: string, name?: string): DOMException;
};

// --- abort (deno_web) ---
interface AbortSignal {
  readonly aborted: boolean;
  readonly reason: unknown;
  throwIfAborted(): void;
  addEventListener(type: "abort", listener: () => void): void;
  removeEventListener(type: "abort", listener: () => void): void;
}
declare var AbortSignal: {
  prototype: AbortSignal;
  new (): AbortSignal;
  abort(reason?: unknown): AbortSignal;
  timeout(milliseconds: number): AbortSignal;
};

interface AbortController {
  readonly signal: AbortSignal;
  abort(reason?: unknown): void;
}
declare var AbortController: { prototype: AbortController; new (): AbortController };

// --- blob / file (deno_web) ---
interface Blob {
  readonly size: number;
  readonly type: string;
  arrayBuffer(): Promise<ArrayBuffer>;
  bytes(): Promise<Uint8Array>;
  slice(start?: number, end?: number, contentType?: string): Blob;
  stream(): ReadableStream<Uint8Array>;
  text(): Promise<string>;
}
declare var Blob: {
  prototype: Blob;
  new (parts?: BlobPart[], options?: { type?: string; endings?: "transparent" | "native" }): Blob;
};

interface File extends Blob {
  readonly name: string;
  readonly lastModified: number;
}
declare var File: {
  prototype: File;
  new (parts: BlobPart[], name: string, options?: { type?: string; lastModified?: number }): File;
};

// --- url / urlpattern (deno_web) ---
interface URLSearchParams {
  readonly size: number;
  append(name: string, value: string): void;
  delete(name: string, value?: string): void;
  get(name: string): string | null;
  getAll(name: string): string[];
  has(name: string, value?: string): boolean;
  set(name: string, value: string): void;
  sort(): void;
  forEach(callback: (value: string, key: string, parent: URLSearchParams) => void): void;
  entries(): IterableIterator<[string, string]>;
  keys(): IterableIterator<string>;
  values(): IterableIterator<string>;
  [Symbol.iterator](): IterableIterator<[string, string]>;
  toString(): string;
}
declare var URLSearchParams: {
  prototype: URLSearchParams;
  new (init?: string | Record<string, string> | [string, string][] | URLSearchParams): URLSearchParams;
};

interface URL {
  hash: string;
  host: string;
  hostname: string;
  href: string;
  toString(): string;
  readonly origin: string;
  password: string;
  pathname: string;
  port: string;
  protocol: string;
  search: string;
  readonly searchParams: URLSearchParams;
  username: string;
  toJSON(): string;
}
declare var URL: {
  prototype: URL;
  new (url: string | URL, base?: string | URL): URL;
  canParse(url: string | URL, base?: string): boolean;
  parse(url: string | URL, base?: string): URL | null;
};

interface URLPatternComponentResult {
  readonly input: string;
  readonly groups: Record<string, string | undefined>;
}
interface URLPatternResult {
  readonly inputs: (string | URLPatternInit)[];
  readonly protocol: URLPatternComponentResult;
  readonly username: URLPatternComponentResult;
  readonly password: URLPatternComponentResult;
  readonly hostname: URLPatternComponentResult;
  readonly port: URLPatternComponentResult;
  readonly pathname: URLPatternComponentResult;
  readonly search: URLPatternComponentResult;
  readonly hash: URLPatternComponentResult;
}
interface URLPatternInit {
  protocol?: string;
  username?: string;
  password?: string;
  hostname?: string;
  port?: string;
  pathname?: string;
  search?: string;
  hash?: string;
  baseURL?: string;
}
interface URLPattern {
  test(input?: string | URLPatternInit, baseURL?: string): boolean;
  exec(input?: string | URLPatternInit, baseURL?: string): URLPatternResult | null;
  readonly protocol: string;
  readonly pathname: string;
  readonly hostname: string;
  readonly search: string;
}
declare var URLPattern: {
  prototype: URLPattern;
  new (input?: string | URLPatternInit, baseURL?: string): URLPattern;
};

// --- crypto (deno_crypto): crypto.subtle is the committed surface ---
interface CryptoKey {
  readonly type: "public" | "private" | "secret";
  readonly extractable: boolean;
  readonly algorithm: { readonly name: string };
  readonly usages: string[];
}
declare var CryptoKey: { prototype: CryptoKey };

interface SubtleCrypto {
  digest(algorithm: AlgorithmIdentifier, data: BufferSource): Promise<ArrayBuffer>;
  encrypt(algorithm: AlgorithmIdentifier, key: CryptoKey, data: BufferSource): Promise<ArrayBuffer>;
  decrypt(algorithm: AlgorithmIdentifier, key: CryptoKey, data: BufferSource): Promise<ArrayBuffer>;
  sign(algorithm: AlgorithmIdentifier, key: CryptoKey, data: BufferSource): Promise<ArrayBuffer>;
  verify(
    algorithm: AlgorithmIdentifier,
    key: CryptoKey,
    signature: BufferSource,
    data: BufferSource,
  ): Promise<boolean>;
}
declare var SubtleCrypto: { prototype: SubtleCrypto };

interface Crypto {
  readonly subtle: SubtleCrypto;
}
declare var Crypto: { prototype: Crypto };
declare var crypto: Crypto;

// --- fetch group (deno_fetch): Headers / Request / Response / FormData / fetch ---
interface Headers {
  append(name: string, value: string): void;
  delete(name: string): void;
  get(name: string): string | null;
  getSetCookie(): string[];
  has(name: string): boolean;
  set(name: string, value: string): void;
  forEach(callback: (value: string, key: string, parent: Headers) => void): void;
  entries(): IterableIterator<[string, string]>;
  keys(): IterableIterator<string>;
  values(): IterableIterator<string>;
  [Symbol.iterator](): IterableIterator<[string, string]>;
}
declare var Headers: { prototype: Headers; new (init?: HeadersInit): Headers };

type FormDataEntryValue = File | string;
interface FormData {
  append(name: string, value: string): void;
  delete(name: string): void;
  get(name: string): FormDataEntryValue | null;
  getAll(name: string): FormDataEntryValue[];
  has(name: string): boolean;
  set(name: string, value: string): void;
  forEach(callback: (value: FormDataEntryValue, key: string, parent: FormData) => void): void;
  entries(): IterableIterator<[string, FormDataEntryValue]>;
  keys(): IterableIterator<string>;
  values(): IterableIterator<FormDataEntryValue>;
  [Symbol.iterator](): IterableIterator<[string, FormDataEntryValue]>;
}
declare var FormData: { prototype: FormData; new (): FormData };

type RequestMethod = string;
interface RequestInit {
  method?: RequestMethod;
  headers?: HeadersInit;
  body?: BodyInit | null;
  signal?: AbortSignal | null;
  redirect?: "follow" | "error" | "manual";
}
interface Body {
  readonly body: ReadableStream<Uint8Array> | null;
  readonly bodyUsed: boolean;
  arrayBuffer(): Promise<ArrayBuffer>;
  bytes(): Promise<Uint8Array>;
  blob(): Promise<Blob>;
  formData(): Promise<FormData>;
  json(): Promise<unknown>;
  text(): Promise<string>;
}
interface Request extends Body {
  readonly method: string;
  readonly url: string;
  readonly headers: Headers;
  readonly redirect: "follow" | "error" | "manual";
  readonly signal: AbortSignal;
  clone(): Request;
}
declare var Request: {
  prototype: Request;
  new (input: string | URL | Request, init?: RequestInit): Request;
};

interface ResponseInit {
  status?: number;
  statusText?: string;
  headers?: HeadersInit;
}
interface Response extends Body {
  readonly ok: boolean;
  readonly status: number;
  readonly statusText: string;
  readonly headers: Headers;
  readonly redirected: boolean;
  readonly url: string;
  clone(): Response;
}
declare var Response: {
  prototype: Response;
  new (body?: BodyInit | null, init?: ResponseInit): Response;
  error(): Response;
  json(data: unknown, init?: ResponseInit): Response;
  redirect(url: string | URL, status?: number): Response;
};

declare function fetch(input: string | URL | Request, init?: RequestInit): Promise<Response>;

// --- timers (deno_web) ---
declare function setTimeout(handler: (...args: unknown[]) => void, timeout?: number, ...args: unknown[]): number;
declare function clearTimeout(id?: number): void;

// --- console (RT-001's op_print-backed global; typed here since lib:["esnext"]
// omits it). The committed methods route to stdout/stderr (CANON §8.1). ---
interface Console {
  log(...data: unknown[]): void;
  info(...data: unknown[]): void;
  warn(...data: unknown[]): void;
  error(...data: unknown[]): void;
  debug(...data: unknown[]): void;
}
declare var console: Console;
