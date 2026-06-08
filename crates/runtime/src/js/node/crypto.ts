import { Buffer } from "node:buffer";

const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

interface NativeCryptoOps {
  op_node_crypto_hash(algorithm: string, data: number[]): number[];
  op_node_crypto_hmac(algorithm: string, key: number[], data: number[]): number[];
}

type BinaryLike = string | ArrayBuffer | ArrayBufferView<ArrayBufferLike> | readonly number[];
type BufferEncoding = "utf8" | "utf-8" | "ascii" | "base64" | "hex" | "latin1" | "binary";
type DigestEncoding = BufferEncoding | "buffer";
type RandomCallback = (error: Error | null, bytes: Buffer) => void;
type RandomFillCallback<T extends ArrayBufferView<ArrayBufferLike>> = (error: Error | null, buf: T) => void;

interface WebCryptoLike {
  getRandomValues<T extends ArrayBufferView<ArrayBufferLike>>(array: T): T;
  randomUUID?: () => string;
  subtle?: unknown;
}

export interface Hash {
  update(data: BinaryLike, inputEncoding?: BufferEncoding): Hash;
  digest(): Buffer;
  digest(encoding: DigestEncoding): Buffer | string;
}

export interface Hmac {
  update(data: BinaryLike, inputEncoding?: BufferEncoding): Hmac;
  digest(): Buffer;
  digest(encoding: DigestEncoding): Buffer | string;
}

export interface KeyObject {
  readonly type: "secret";
  export(): Buffer;
}

class SecretKeyObject implements KeyObject {
  readonly type = "secret";
  readonly #key: Buffer;

  constructor(key: BinaryLike, encoding?: BufferEncoding) {
    this.#key = toBuffer(key, encoding);
  }

  export(): Buffer {
    return Buffer.from(this.#key);
  }
}

const ops = rawOps as unknown as NativeCryptoOps;
const HASHES = Object.freeze(["md5", "sha1", "sha256", "sha384", "sha512"]);

function normalizeEncoding(encoding: BufferEncoding | undefined): BufferEncoding | undefined {
  if (encoding === "binary") return "latin1";
  return encoding;
}

function webCrypto(): WebCryptoLike {
  const crypto = (globalThis as unknown as { crypto?: WebCryptoLike }).crypto;
  if (crypto === undefined || typeof crypto.getRandomValues !== "function") {
    throw new Error("node:crypto requires globalThis.crypto.getRandomValues");
  }
  return crypto;
}

function toBuffer(value: BinaryLike | KeyObject, encoding?: BufferEncoding): Buffer {
  if (isKeyObject(value)) {
    return value.export();
  }
  return Buffer.from(value, normalizeEncoding(encoding));
}

function isKeyObject(value: unknown): value is KeyObject {
  return typeof value === "object" && value !== null && typeof (value as KeyObject).export === "function";
}

function toByteArray(value: BinaryLike | KeyObject, encoding?: BufferEncoding): number[] {
  return Array.from(toBuffer(value, encoding));
}

function concatChunks(chunks: Buffer[]): Buffer {
  let length = 0;
  for (const chunk of chunks) length += chunk.length;
  const out = Buffer.alloc(length);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

function digestOutput(bytes: number[], encoding?: DigestEncoding): Buffer | string {
  const out = Buffer.from(bytes);
  if (encoding === undefined || encoding === "buffer") return out;
  return out.toString(normalizeEncoding(encoding));
}

class HashImpl implements Hash {
  readonly #algorithm: string;
  readonly #chunks: Buffer[] = [];
  #finished = false;

  constructor(algorithm: string) {
    this.#algorithm = String(algorithm);
  }

  update(data: BinaryLike, inputEncoding?: BufferEncoding): Hash {
    if (this.#finished) throw new Error("Digest already called");
    this.#chunks.push(toBuffer(data, inputEncoding));
    return this;
  }

  digest(encoding?: DigestEncoding): Buffer | string {
    if (this.#finished) throw new Error("Digest already called");
    this.#finished = true;
    const data = concatChunks(this.#chunks);
    return digestOutput(ops.op_node_crypto_hash(this.#algorithm, Array.from(data)), encoding);
  }
}

class HmacImpl implements Hmac {
  readonly #algorithm: string;
  readonly #key: Buffer;
  readonly #chunks: Buffer[] = [];
  #finished = false;

  constructor(algorithm: string, key: BinaryLike | KeyObject, encoding?: BufferEncoding) {
    this.#algorithm = String(algorithm);
    this.#key = toBuffer(key, encoding);
  }

  update(data: BinaryLike, inputEncoding?: BufferEncoding): Hmac {
    if (this.#finished) throw new Error("Digest already called");
    this.#chunks.push(toBuffer(data, inputEncoding));
    return this;
  }

  digest(encoding?: DigestEncoding): Buffer | string {
    if (this.#finished) throw new Error("Digest already called");
    this.#finished = true;
    const data = concatChunks(this.#chunks);
    return digestOutput(ops.op_node_crypto_hmac(this.#algorithm, Array.from(this.#key), Array.from(data)), encoding);
  }
}

function assertSize(size: number): void {
  if (!Number.isInteger(size) || size < 0) {
    throw new RangeError("size must be a non-negative integer");
  }
}

function fillRandom(view: Uint8Array): Uint8Array {
  const crypto = webCrypto();
  for (let offset = 0; offset < view.length; offset += 65536) {
    crypto.getRandomValues(view.subarray(offset, Math.min(offset + 65536, view.length)));
  }
  return view;
}

export function createHash(algorithm: string): Hash {
  return new HashImpl(algorithm);
}

export function createHmac(algorithm: string, key: BinaryLike | KeyObject, options?: { encoding?: BufferEncoding } | BufferEncoding): Hmac {
  const encoding = typeof options === "string" ? options : options?.encoding;
  return new HmacImpl(algorithm, key, encoding);
}

export function createSecretKey(key: BinaryLike, encoding?: BufferEncoding): KeyObject {
  return new SecretKeyObject(key, encoding);
}

export function randomBytes(size: number, callback?: RandomCallback): Buffer {
  assertSize(size);
  const out = Buffer.alloc(size);
  fillRandom(out);
  if (callback !== undefined) queueMicrotask(() => callback(null, out));
  return out;
}

export const pseudoRandomBytes = randomBytes;
export const rng = randomBytes;
export const prng = randomBytes;

export function randomFillSync<T extends ArrayBufferView<ArrayBufferLike>>(buffer: T, offset = 0, size?: number): T {
  const view = new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
  if (!Number.isInteger(offset) || offset < 0 || offset > view.length) {
    throw new RangeError("offset is out of range");
  }
  const byteLength = size ?? view.length - offset;
  assertSize(byteLength);
  if (offset + byteLength > view.length) {
    throw new RangeError("size is out of range");
  }
  fillRandom(view.subarray(offset, offset + byteLength));
  return buffer;
}

export function randomFill<T extends ArrayBufferView<ArrayBufferLike>>(
  buffer: T,
  offsetOrCallback?: number | RandomFillCallback<T>,
  sizeOrCallback?: number | RandomFillCallback<T>,
  maybeCallback?: RandomFillCallback<T>,
): T {
  const offset = typeof offsetOrCallback === "number" ? offsetOrCallback : 0;
  const size = typeof sizeOrCallback === "number" ? sizeOrCallback : undefined;
  const callback = (typeof offsetOrCallback === "function" ? offsetOrCallback :
    typeof sizeOrCallback === "function" ? sizeOrCallback : maybeCallback) as RandomFillCallback<T> | undefined;
  randomFillSync(buffer, offset, size);
  if (callback !== undefined) queueMicrotask(() => callback(null, buffer));
  return buffer;
}

export function randomUUID(): string {
  const crypto = webCrypto();
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const bytes = randomBytes(16);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = bytes.toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export function timingSafeEqual(left: Uint8Array, right: Uint8Array): boolean {
  if (!(left instanceof Uint8Array) || !(right instanceof Uint8Array)) {
    throw new TypeError("timingSafeEqual expects Uint8Array values");
  }
  if (left.length !== right.length) {
    throw new RangeError("Input buffers must have the same byte length");
  }
  let diff = 0;
  for (let index = 0; index < left.length; index++) diff |= (left[index] ?? 0) ^ (right[index] ?? 0);
  return diff === 0;
}

export function getHashes(): string[] {
  return [...HASHES];
}

export const webcrypto = (globalThis as unknown as { crypto?: WebCryptoLike }).crypto;
export const subtle = webcrypto?.subtle;

export default {
  createHash,
  createHmac,
  createSecretKey,
  randomBytes,
  pseudoRandomBytes,
  rng,
  prng,
  randomFill,
  randomFillSync,
  randomUUID,
  timingSafeEqual,
  getHashes,
  webcrypto,
  subtle,
};
