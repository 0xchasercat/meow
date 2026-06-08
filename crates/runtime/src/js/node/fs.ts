import { Buffer } from "node:buffer";
import { fileURLToPath } from "node:url";

const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

type NodeError = Error & { code?: string };

type PathLike = string | URL;
type BinaryLike = Uint8Array | ArrayBuffer | ArrayBufferView;
type ReadFileEncoding = string | null | undefined;

type Callback<T> = (error: NodeError | null, value?: T) => void;

interface RawStat {
  readonly size: number;
  readonly readonly: boolean;
  readonly is_file: boolean;
  readonly is_directory: boolean;
  readonly is_symlink: boolean;
  readonly atime_ms: number;
  readonly mtime_ms: number;
  readonly ctime_ms: number;
  readonly birthtime_ms: number;
}

interface NativeFsOps {
  op_read_file(path: string): Promise<Uint8Array>;
  op_node_read_file_sync(path: string): Uint8Array;
  op_node_write_file(path: string, data: Uint8Array): Promise<void>;
  op_node_write_file_sync(path: string, data: Uint8Array): void;
  op_node_readdir(path: string): Promise<string[]>;
  op_node_readdir_sync(path: string): string[];
  op_node_stat(path: string): Promise<RawStat>;
  op_node_stat_sync(path: string): RawStat;
  op_node_mkdir(path: string, recursive: boolean): Promise<void>;
  op_node_mkdir_sync(path: string, recursive: boolean): void;
  op_node_access(path: string, mode: number): Promise<void>;
  op_node_access_sync(path: string, mode: number): void;
  op_node_rm(path: string, recursive: boolean, force: boolean): Promise<void>;
  op_node_rm_sync(path: string, recursive: boolean, force: boolean): void;
  op_node_exists_sync(path: string): boolean;
}

const ops = rawOps as unknown as NativeFsOps;

export class Stats {
  readonly size: number;
  readonly readonly: boolean;
  readonly atimeMs: number;
  readonly mtimeMs: number;
  readonly ctimeMs: number;
  readonly birthtimeMs: number;
  #isFile: boolean;
  #isDirectory: boolean;
  #isSymlink: boolean;

  constructor(value: RawStat) {
    this.size = value.size;
    this.readonly = value.readonly;
    this.atimeMs = value.atime_ms;
    this.mtimeMs = value.mtime_ms;
    this.ctimeMs = value.ctime_ms;
    this.birthtimeMs = value.birthtime_ms;
    this.#isFile = value.is_file;
    this.#isDirectory = value.is_directory;
    this.#isSymlink = value.is_symlink;
  }

  isFile(): boolean {
    return this.#isFile;
  }

  isDirectory(): boolean {
    return this.#isDirectory;
  }

  isSymbolicLink(): boolean {
    return this.#isSymlink;
  }
}

export const constants = {
  F_OK: 0,
  X_OK: 1,
  W_OK: 2,
  R_OK: 4,
} as const;

function toPathString(value: PathLike): string {
  return value instanceof URL ? fileURLToPath(value) : value;
}

function normalizeNodeError(error: unknown): NodeError {
  if (error instanceof Error) {
    const nodeError = error as NodeError;
    const trimmed = nodeError.message.startsWith("permission denied: ")
      ? nodeError.message.slice("permission denied: ".length)
      : nodeError.message;
    const marker = trimmed.indexOf(":");
    if (marker > 0) {
      const code = trimmed.slice(0, marker);
      if (/^[A-Z][A-Z0-9_]+$/.test(code)) {
        nodeError.code ??= code;
        nodeError.message = trimmed;
      }
    }
    return nodeError;
  }
  const wrapped = new Error(String(error)) as NodeError;
  wrapped.code = "EIO";
  return wrapped;
}

function encodingFromReadOptions(options?: ReadFileEncoding | { encoding?: ReadFileEncoding }): ReadFileEncoding {
  if (typeof options === "string" || options == null) {
    return options;
  }
  return options.encoding;
}

function encodeForWrite(
  value: string | BinaryLike,
  options?: string | { encoding?: string | null } | null,
): Uint8Array {
  if (typeof value === "string") {
    const encoding = typeof options === "string" ? options : options?.encoding ?? undefined;
    return Buffer.from(value, encoding ?? "utf8");
  }
  if (value instanceof ArrayBuffer) {
    return new Uint8Array(value);
  }
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  return value;
}

function decodeRead(value: Uint8Array, encoding: ReadFileEncoding): Buffer | string {
  const buffer = Buffer.from(value);
  if (encoding == null) {
    return buffer;
  }
  return buffer.toString(encoding);
}

function mkdirRecursive(options?: boolean | { recursive?: boolean }): boolean {
  if (typeof options === "boolean") {
    return options;
  }
  return options?.recursive ?? false;
}

function rmShape(options?: { recursive?: boolean; force?: boolean }): { recursive: boolean; force: boolean } {
  return {
    recursive: options?.recursive ?? false,
    force: options?.force ?? false,
  };
}

function callbackResult<T>(promise: Promise<T>, callback: Callback<T>): void {
  promise.then(
    (value) => callback(null, value),
    (error) => callback(normalizeNodeError(error)),
  );
}

async function readFilePromise(
  path: PathLike,
  options?: ReadFileEncoding | { encoding?: ReadFileEncoding },
): Promise<Buffer | string> {
  try {
    return decodeRead(await ops.op_read_file(toPathString(path)), encodingFromReadOptions(options));
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

async function writeFilePromise(
  path: PathLike,
  data: string | BinaryLike,
  options?: string | { encoding?: string | null } | null,
): Promise<void> {
  try {
    await ops.op_node_write_file(toPathString(path), encodeForWrite(data, options));
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

async function readdirPromise(path: PathLike): Promise<string[]> {
  try {
    return await ops.op_node_readdir(toPathString(path));
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

async function statPromise(path: PathLike): Promise<Stats> {
  try {
    return new Stats(await ops.op_node_stat(toPathString(path)));
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

async function mkdirPromise(path: PathLike, options?: boolean | { recursive?: boolean }): Promise<void> {
  try {
    await ops.op_node_mkdir(toPathString(path), mkdirRecursive(options));
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

async function accessPromise(path: PathLike, mode = constants.F_OK): Promise<void> {
  try {
    await ops.op_node_access(toPathString(path), mode);
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

async function rmPromise(path: PathLike, options?: { recursive?: boolean; force?: boolean }): Promise<void> {
  const shape = rmShape(options);
  try {
    await ops.op_node_rm(toPathString(path), shape.recursive, shape.force);
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

export function readFile(
  path: PathLike,
  options: ReadFileEncoding | { encoding?: ReadFileEncoding } | Callback<Buffer | string>,
  callback?: Callback<Buffer | string>,
): void {
  if (typeof options === "function") {
    callbackResult(readFilePromise(path), options);
    return;
  }
  if (callback === undefined) {
    throw new TypeError("fs.readFile() requires a callback");
  }
  callbackResult(readFilePromise(path, options), callback);
}

export function readFileSync(
  path: PathLike,
  options?: ReadFileEncoding | { encoding?: ReadFileEncoding },
): Buffer | string {
  try {
    return decodeRead(ops.op_node_read_file_sync(toPathString(path)), encodingFromReadOptions(options));
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

export function writeFile(
  path: PathLike,
  data: string | BinaryLike,
  options: string | { encoding?: string | null } | null | Callback<void>,
  callback?: Callback<void>,
): void {
  if (typeof options === "function") {
    callbackResult(writeFilePromise(path, data), options);
    return;
  }
  if (callback === undefined) {
    throw new TypeError("fs.writeFile() requires a callback");
  }
  callbackResult(writeFilePromise(path, data, options), callback);
}

export function writeFileSync(
  path: PathLike,
  data: string | BinaryLike,
  options?: string | { encoding?: string | null } | null,
): void {
  try {
    ops.op_node_write_file_sync(toPathString(path), encodeForWrite(data, options));
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

export function readdir(path: PathLike, callback: Callback<string[]>): void {
  callbackResult(readdirPromise(path), callback);
}

export function readdirSync(path: PathLike): string[] {
  try {
    return ops.op_node_readdir_sync(toPathString(path));
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

export function stat(path: PathLike, callback: Callback<Stats>): void {
  callbackResult(statPromise(path), callback);
}

export function statSync(path: PathLike): Stats {
  try {
    return new Stats(ops.op_node_stat_sync(toPathString(path)));
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

export function mkdir(
  path: PathLike,
  options: boolean | { recursive?: boolean } | Callback<void>,
  callback?: Callback<void>,
): void {
  if (typeof options === "function") {
    callbackResult(mkdirPromise(path), options);
    return;
  }
  if (callback === undefined) {
    throw new TypeError("fs.mkdir() requires a callback");
  }
  callbackResult(mkdirPromise(path, options), callback);
}

export function mkdirSync(path: PathLike, options?: boolean | { recursive?: boolean }): void {
  try {
    ops.op_node_mkdir_sync(toPathString(path), mkdirRecursive(options));
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

export function access(path: PathLike, mode: number | Callback<void>, callback?: Callback<void>): void {
  if (typeof mode === "function") {
    callbackResult(accessPromise(path), mode);
    return;
  }
  if (callback === undefined) {
    throw new TypeError("fs.access() requires a callback");
  }
  callbackResult(accessPromise(path, mode), callback);
}

export function accessSync(path: PathLike, mode = constants.F_OK): void {
  try {
    ops.op_node_access_sync(toPathString(path), mode);
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

export function rm(
  path: PathLike,
  options: { recursive?: boolean; force?: boolean } | Callback<void>,
  callback?: Callback<void>,
): void {
  if (typeof options === "function") {
    callbackResult(rmPromise(path), options);
    return;
  }
  if (callback === undefined) {
    throw new TypeError("fs.rm() requires a callback");
  }
  callbackResult(rmPromise(path, options), callback);
}

export function rmSync(path: PathLike, options?: { recursive?: boolean; force?: boolean }): void {
  const shape = rmShape(options);
  try {
    ops.op_node_rm_sync(toPathString(path), shape.recursive, shape.force);
  } catch (error) {
    throw normalizeNodeError(error);
  }
}

export function existsSync(path: PathLike): boolean {
  return ops.op_node_exists_sync(toPathString(path));
}

export const promises = {
  access: accessPromise,
  mkdir: mkdirPromise,
  readFile: readFilePromise,
  readdir: readdirPromise,
  rm: rmPromise,
  stat: statPromise,
  writeFile: writeFilePromise,
};

const api = {
  Stats,
  access,
  accessSync,
  constants,
  existsSync,
  mkdir,
  mkdirSync,
  promises,
  readFile,
  readFileSync,
  readdir,
  readdirSync,
  rm,
  rmSync,
  stat,
  statSync,
  writeFile,
  writeFileSync,
};

export default api;
