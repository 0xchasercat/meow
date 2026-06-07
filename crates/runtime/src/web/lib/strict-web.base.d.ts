// strict-web ambient globals — base (RT-004 · CANON §8.1).
//
// The always-present part of the committed P1 "Stateless Edge" global surface,
// typed for editors + `meow check`. CURATED from the upstream WHATWG/WinterTC
// declarations (the same shapes TypeScript's lib.dom.d.ts and Deno's .d.ts
// expose); NOT generated from Rust (I-9's generate-from-impl applies to the
// `meow:*` modules — RT-005). Scoped to EXACTLY the §8.1 set: the shadow tsconfig
// pins `lib: ["esnext"]` (no DOM), so `window`/`document`/`localStorage`/
// `WebSocket` are intentionally absent here — the types match the runtime
// surface, nothing more (CRAFT: prose matches code).
//
// The `fetch` group (`fetch`/`Headers`/`Request`/`Response`/`FormData`) lives in
// the sibling `strict-web.fetch.d.ts`, appended ONLY when the runtime is built
// with the default-on `web-fetch` feature (meow_runtime::web::STRICT_WEB_DTS). A
// no-fetch build omits those globals at runtime, so it must omit their types too,
// or code typechecks then throws `ReferenceError` (honest typing, I-9/I-11).
//
// Honest boundary: the runtime exposes a SUPERSET transitively (streams beyond
// fetch bodies, EventTarget, structuredClone, atob/btoa, performance,
// crypto.getRandomValues). Those are present-but-uncommitted: not declared here,
// not promised. `ReadableStream` is declared minimally only because `Blob.stream`
// (and the fetch bodies, when present) reference it; stream construction is not a
// committed surface.

// --- shared helper types (not globals; underpin the declarations below) ---
type BufferSource = ArrayBufferView | ArrayBuffer;
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

// --- blob (deno_web) ---
// `File` is NOT a committed global (CANON §8.1 commits only `Blob`/`FormData`);
// it ships transitively but is not promised here. Its interface is declared
// minimally in `strict-web.fetch.d.ts` only because `FormData` entries reference
// it — there is no `File` value binding, so it is not constructable as committed API.
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
