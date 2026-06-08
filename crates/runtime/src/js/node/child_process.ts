import { Buffer } from "node:buffer";
import { EventEmitter } from "node:events";

const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

interface ChildProcessOutput {
  status: number | null;
  signal: string | null;
  stdout: number[] | Uint8Array;
  stderr: number[] | Uint8Array;
}

interface NativeChildProcessOps {
  op_node_child_process_sync(command: string, args: string[], options: NativeChildProcessOptions): ChildProcessOutput;
  op_node_child_process(
    command: string,
    args: string[],
    options: NativeChildProcessOptions,
  ): Promise<ChildProcessOutput>;
}

interface NativeChildProcessOptions {
  cwd?: string;
  env?: Record<string, string>;
  shell?: boolean;
  stdio?: string;
  input?: number[];
}

export interface ChildProcessOptions {
  cwd?: string | URL;
  env?: Record<string, string | undefined>;
  shell?: boolean | string;
  stdio?: string | readonly unknown[];
  input?: string | Uint8Array;
  encoding?: BufferEncoding | "buffer" | null;
}

export interface SpawnSyncReturns {
  pid: number;
  output: [null, Buffer | string, Buffer | string];
  stdout: Buffer | string;
  stderr: Buffer | string;
  status: number | null;
  signal: string | null;
  error?: Error;
}

export interface ChildProcess extends EventEmitter {
  pid: number;
  stdout: EventEmitter;
  stderr: EventEmitter;
  stdin: null;
  kill(signal?: string): boolean;
}

type ExecCallback = (error: Error | null, stdout: Buffer | string, stderr: Buffer | string) => void;
type BufferEncoding = "utf8" | "utf-8" | "ascii" | "base64" | "hex" | "latin1" | "binary";

const ops = rawOps as unknown as NativeChildProcessOps;

function toPathString(value: string | URL | undefined): string | undefined {
  if (value === undefined) return undefined;
  if (value instanceof URL) {
    if (value.protocol !== "file:") throw new TypeError("child_process cwd URL must be file:");
    return decodeURIComponent(value.pathname);
  }
  return String(value);
}

function cleanEnv(env: Record<string, string | undefined> | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  if (env === undefined) return out;
  for (const [key, value] of Object.entries(env)) {
    if (value !== undefined) out[key] = String(value);
  }
  return out;
}

function inputBytes(input: string | Uint8Array | undefined): number[] | undefined {
  if (input === undefined) return undefined;
  const bytes = typeof input === "string" ? Buffer.from(input) : Buffer.from(input);
  return Array.from(bytes);
}

function stdioMode(stdio: string | readonly unknown[] | undefined): string | undefined {
  if (stdio === "inherit") return "inherit";
  if (Array.isArray(stdio) && stdio.includes("inherit")) return "inherit";
  return undefined;
}

function nativeOptions(options: ChildProcessOptions = {}): NativeChildProcessOptions {
  return {
    cwd: toPathString(options.cwd),
    env: cleanEnv(options.env),
    shell: options.shell !== undefined && options.shell !== false,
    stdio: stdioMode(options.stdio),
    input: inputBytes(options.input),
  };
}

function bytesToBuffer(value: number[] | Uint8Array): Buffer {
  return Buffer.from(value);
}

function decode(value: Buffer, options: ChildProcessOptions | undefined): Buffer | string {
  const encoding = options?.encoding;
  if (encoding === undefined || encoding === null || encoding === "buffer") return value;
  return value.toString(encoding === "utf-8" ? "utf8" : encoding);
}

function statusError(command: string, output: ChildProcessOutput, options?: ChildProcessOptions): Error {
  const error = new Error(`Command failed: ${command}`) as Error & {
    status?: number | null;
    signal?: string | null;
    stdout?: Buffer | string;
    stderr?: Buffer | string;
  };
  error.status = output.status;
  error.signal = output.signal;
  error.stdout = decode(bytesToBuffer(output.stdout), options);
  error.stderr = decode(bytesToBuffer(output.stderr), options);
  return error;
}

function checkSuccess(command: string, output: ChildProcessOutput, options?: ChildProcessOptions): void {
  if (output.status !== null && output.status !== 0) {
    throw statusError(command, output, options);
  }
}

function normalizeArgs(argsOrOptions?: readonly string[] | ChildProcessOptions, options?: ChildProcessOptions): {
  args: string[];
  options: ChildProcessOptions;
} {
  if (Array.isArray(argsOrOptions)) {
    return { args: argsOrOptions.map(String), options: options ?? {} };
  }
  return { args: [], options: argsOrOptions ?? {} };
}

export function spawnSync(
  command: string,
  argsOrOptions?: readonly string[] | ChildProcessOptions,
  maybeOptions?: ChildProcessOptions,
): SpawnSyncReturns {
  const { args, options } = normalizeArgs(argsOrOptions, maybeOptions);
  const output = ops.op_node_child_process_sync(String(command), args, nativeOptions(options));
  const stdout = decode(bytesToBuffer(output.stdout), options);
  const stderr = decode(bytesToBuffer(output.stderr), options);
  return {
    pid: 0,
    output: [null, stdout, stderr],
    stdout,
    stderr,
    status: output.status,
    signal: output.signal,
  };
}

export function execFileSync(
  file: string,
  argsOrOptions?: readonly string[] | ChildProcessOptions,
  maybeOptions?: ChildProcessOptions,
): Buffer | string {
  const { args, options } = normalizeArgs(argsOrOptions, maybeOptions);
  const output = ops.op_node_child_process_sync(String(file), args, nativeOptions(options));
  checkSuccess(String(file), output, options);
  return decode(bytesToBuffer(output.stdout), options);
}

export function execSync(command: string, options: ChildProcessOptions = {}): Buffer | string {
  const shaped = { ...options, shell: true };
  const output = ops.op_node_child_process_sync(String(command), [], nativeOptions(shaped));
  checkSuccess(String(command), output, shaped);
  return decode(bytesToBuffer(output.stdout), shaped);
}

function createChildProcess(): ChildProcess {
  const child = new EventEmitter() as ChildProcess;
  child.pid = 0;
  child.stdout = new EventEmitter();
  child.stderr = new EventEmitter();
  child.stdin = null;
  child.kill = () => false;
  return child;
}

function emitAsyncResult(child: ChildProcess, promise: Promise<ChildProcessOutput>, callback?: ExecCallback): void {
  promise.then((output) => {
    const stdout = bytesToBuffer(output.stdout);
    const stderr = bytesToBuffer(output.stderr);
    if (stdout.length > 0) child.stdout.emit("data", stdout);
    if (stderr.length > 0) child.stderr.emit("data", stderr);
    const error = output.status !== null && output.status !== 0 ? statusError("child_process", output) : null;
    callback?.(error, stdout, stderr);
    child.emit("exit", output.status, output.signal);
    child.emit("close", output.status, output.signal);
  }).catch((error) => {
    child.emit("error", error);
    callback?.(error instanceof Error ? error : new Error(String(error)), Buffer.alloc(0), Buffer.alloc(0));
  });
}

export function spawn(
  command: string,
  argsOrOptions?: readonly string[] | ChildProcessOptions,
  maybeOptions?: ChildProcessOptions,
): ChildProcess {
  const { args, options } = normalizeArgs(argsOrOptions, maybeOptions);
  const child = createChildProcess();
  emitAsyncResult(child, ops.op_node_child_process(String(command), args, nativeOptions(options)));
  return child;
}

export function execFile(
  file: string,
  argsOrOptions?: readonly string[] | ChildProcessOptions | ExecCallback,
  optionsOrCallback?: ChildProcessOptions | ExecCallback,
  maybeCallback?: ExecCallback,
): ChildProcess {
  const args = Array.isArray(argsOrOptions) ? argsOrOptions.map(String) : [];
  const options = (Array.isArray(argsOrOptions) ? optionsOrCallback : argsOrOptions) as ChildProcessOptions | undefined;
  const callback = (typeof optionsOrCallback === "function" ? optionsOrCallback : maybeCallback) as ExecCallback | undefined;
  const child = createChildProcess();
  emitAsyncResult(child, ops.op_node_child_process(String(file), args, nativeOptions(options ?? {})), callback);
  return child;
}

export function exec(command: string, optionsOrCallback?: ChildProcessOptions | ExecCallback, maybeCallback?: ExecCallback): ChildProcess {
  const options = typeof optionsOrCallback === "function" ? {} : optionsOrCallback ?? {};
  const callback = typeof optionsOrCallback === "function" ? optionsOrCallback : maybeCallback;
  const child = createChildProcess();
  const shaped = { ...options, shell: true };
  emitAsyncResult(child, ops.op_node_child_process(String(command), [], nativeOptions(shaped)), callback);
  return child;
}

export default {
  spawn,
  spawnSync,
  exec,
  execSync,
  execFile,
  execFileSync,
};
