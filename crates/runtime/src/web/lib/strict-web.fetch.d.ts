// strict-web ambient globals — fetch group (RT-004 · CANON §8.1).
//
// Appended to `strict-web.base.d.ts` ONLY when the runtime is built with the
// default-on `web-fetch` feature (meow_runtime::web::STRICT_WEB_DTS concatenates
// the two). These globals come from the heavy `deno_fetch` crate; a
// `--no-default-features` build omits the extension, so `fetch`/`Headers`/
// `Request`/`Response`/`FormData` are absent at runtime — and therefore omitted
// here too, so a no-fetch program can't typecheck a call that would throw
// `ReferenceError` (honest typing, I-9/I-11).
//
// Depends on base types: `Blob`, `URLSearchParams`, `AbortSignal`, `URL`,
// `ReadableStream` (declared in `strict-web.base.d.ts`).

type BodyInit = string | Blob | BufferSource | FormData | URLSearchParams | ReadableStream<Uint8Array>;
type HeadersInit = Headers | Record<string, string> | [string, string][];

// `File` is declared minimally here — and only here — because `FormData` entries
// reference it (`FormDataEntryValue`). It is NOT a committed global: there is no
// `File` value binding, so it cannot be constructed as committed API. It ships
// transitively via deno_web (present-but-uncommitted, I-11 superset honesty).
interface File extends Blob {
  readonly name: string;
  readonly lastModified: number;
}

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
