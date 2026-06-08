const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

type NodeError = Error & { code?: string };

interface ProcessVersions {
  readonly node: string;
  readonly v8: string;
  readonly meow: string;
}

interface ProcessInfo {
  readonly enabled: boolean;
  readonly argv: readonly string[];
  readonly cwd: string;
  // === RUN-001 ===
  readonly env: ReadonlyArray<readonly [string, string]>;
  // === /RUN-001 ===
  readonly platform: string;
  readonly arch: string;
  readonly version: string;
  readonly versions: ProcessVersions;
}

interface NativeProcessOps {
  op_node_process_info(): ProcessInfo;
  op_node_cwd(): string;
  op_node_process_exit(code: number): void;
  op_hermetic_env_entries(): Array<[string, string]>;
  op_meow_print(message: string, isErr: boolean): void;
}

const ops = rawOps as unknown as NativeProcessOps;

export interface NodeWriteStream {
  write(chunk: string | Uint8Array): boolean;
}

export interface Process {
  readonly argv: string[];
  readonly env: Record<string, string>;
  readonly platform: string;
  readonly arch: string;
  readonly version: string;
  readonly versions: Readonly<ProcessVersions>;
  readonly stdout: NodeWriteStream;
  readonly stderr: NodeWriteStream;
  cwd(): string;
  nextTick<TArgs extends unknown[]>(callback: (...args: TArgs) => void, ...args: TArgs): void;
  exit(code?: number): never;
}

function strictWebError(message: string): NodeError {
  const error = new Error(message) as NodeError;
  error.code = "ERR_STRICT_WEB_WITHDRAWN";
  return error;
}

function decodeUtf8(bytes: Uint8Array): string {
  return new TextDecoder().decode(bytes);
}

function toOutputString(chunk: string | Uint8Array): string {
  return typeof chunk === "string" ? chunk : decodeUtf8(chunk);
}

function envProxy(
  enabled: boolean,
  overrides: ReadonlyArray<readonly [string, string]>,
): Record<string, string> {
  if (!enabled) {
    return new Proxy(Object.create(null) as Record<string, string>, {
      deleteProperty() {
        throw strictWebError("strict-web mode withdraws process.env");
      },
      get(_target, key) {
        if (typeof key === "symbol") {
          return undefined;
        }
        throw strictWebError(`strict-web mode withdraws process.env.${String(key)}`);
      },
      ownKeys() {
        throw strictWebError("strict-web mode withdraws process.env");
      },
      getOwnPropertyDescriptor() {
        throw strictWebError("strict-web mode withdraws process.env");
      },
      set() {
        throw strictWebError("strict-web mode withdraws process.env");
      },
    });
  }

  const store = Object.create(null) as Record<string, string>;
  for (const [name, value] of ops.op_hermetic_env_entries()) {
    store[name] = value;
  }
  for (const [name, value] of overrides) {
    store[name] = value;
  }
  return new Proxy(store, {
    deleteProperty(target, key) {
      delete target[key as keyof typeof target];
      return true;
    },
    get(target, key) {
      return Reflect.get(target, key) as string | undefined;
    },
    ownKeys(target) {
      return Reflect.ownKeys(target);
    },
    getOwnPropertyDescriptor(target, key) {
      const value = Reflect.get(target, key) as string | undefined;
      if (value === undefined) {
        return undefined;
      }
      return {
        configurable: true,
        enumerable: true,
        value,
        writable: true,
      };
    },
    set(target, key, value) {
      target[key as keyof typeof target] = String(value);
      return true;
    },
  });
}

function makeStream(isErr: boolean): NodeWriteStream {
  return {
    write(chunk) {
      ops.op_meow_print(toOutputString(chunk), isErr);
      return true;
    },
  };
}

function createProcess(info: ProcessInfo): Process {
  const value: Process = {
    argv: [...info.argv],
    env: envProxy(info.enabled, info.env),
    platform: info.platform,
    arch: info.arch,
    version: info.version,
    versions: Object.freeze({ ...info.versions }),
    stdout: makeStream(false),
    stderr: makeStream(true),
    cwd() {
      return info.cwd;
    },
    nextTick(callback, ...args) {
      Promise.resolve().then(() => callback(...args));
    },
    exit(code = 0): never {
      ops.op_node_process_exit(Number(code) | 0);
      throw strictWebError("process.exit returned unexpectedly");
    },
  };
  return value;
}

const processInfo = ops.op_node_process_info();
const globals = globalThis as unknown as { process?: Process };
const processValue = globals.process ?? createProcess(processInfo);

if (globals.process === undefined && processInfo.enabled) {
  globals.process = processValue;
}

export default processValue;
export const argv = processValue.argv;
export const env = processValue.env;
export const platform = processValue.platform;
export const arch = processValue.arch;
export const version = processValue.version;
export const versions = processValue.versions;
export const stdout = processValue.stdout;
export const stderr = processValue.stderr;
export const cwd = processValue.cwd.bind(processValue);
export const nextTick = processValue.nextTick.bind(processValue);
export const exit = processValue.exit.bind(processValue);
