import * as nodeAssert from "node:assert";
import * as nodeAsyncHooks from "node:async_hooks";
import * as nodeBuffer from "node:buffer";
import * as nodeConsole from "node:console";
import * as nodeChildProcess from "node:child_process";
import * as nodeConstants from "node:constants";
import * as nodeCrypto from "node:crypto";
import * as nodeDns from "node:dns";
import * as nodeEvents from "node:events";
import * as nodeFs from "node:fs";
import * as nodeFsPromises from "node:fs/promises";
import * as nodeHttp from "node:http";
import * as nodeHttps from "node:https";
import * as nodeOs from "node:os";
import * as nodePath from "node:path";
import * as nodeProcess from "node:process";
import * as nodeUrl from "node:url";
import * as nodeUtil from "node:util";
import * as nodeUtilTypes from "node:util/types";
import * as nodeCluster from "node:cluster";
import * as nodeDgram from "node:dgram";
import * as nodeDiagnosticsChannel from "node:diagnostics_channel";
import * as nodeDnsPromises from "node:dns/promises";
import * as nodeDomain from "node:domain";
import * as nodeHttp2 from "node:http2";
import * as nodeInspector from "node:inspector";
import * as nodeNet from "node:net";
import * as nodePerfHooks from "node:perf_hooks";
import * as nodePunycode from "node:punycode";
import * as nodeQuerystring from "node:querystring";
import * as nodeReadline from "node:readline";
import * as nodeRepl from "node:repl";
import * as nodeStream from "node:stream";
import * as nodeStreamConsumers from "node:stream/consumers";
import * as nodeStreamPromises from "node:stream/promises";
import * as nodeStreamWeb from "node:stream/web";
import * as nodeStringDecoder from "node:string_decoder";
import * as nodeSys from "node:sys";
import * as nodeTimers from "node:timers";
import * as nodeTimersPromises from "node:timers/promises";
import * as nodeTls from "node:tls";
import * as nodeTraceEvents from "node:trace_events";
import * as nodeTty from "node:tty";
import * as nodeV8 from "node:v8";
import * as nodeVm from "node:vm";
import * as nodeWasi from "node:wasi";
import * as nodeWorkerThreads from "node:worker_threads";
import * as nodeZlib from "node:zlib";
import Module from "node:module";
const core = (globalThis as unknown as {
  Deno: {
    core: {
      ops: Record<string, unknown>;
      loadExtScript(specifier: string): unknown;
    };
  };
}).Deno.core;
const ops = core.ops;

type CjsRequire = ((specifier: unknown) => unknown) & {
  resolve(specifier: unknown): string;
  cache: Record<string, CjsModuleRecord>;
  extensions: Record<string, unknown>;
};

interface LoadedModule {
  url: string;
  filename: string;
  dirname: string;
  source: string;
  kind: "cjs" | "json" | "napi";
}

interface CjsModuleRecord {
  id: string;
  url: string;
  filename: string;
  path: string;
  exports: unknown;
  loaded: boolean;
  parent: CjsModuleRecord | undefined;
  children: CjsModuleRecord[];
  constructor: typeof Module;
  require: CjsRequire;
}

const moduleCache = new Map<string, CjsModuleRecord>();
const jsonCache = new Map<string, unknown>();
const requireCache: Record<string, CjsModuleRecord> = Object.create(null);
const requireExtensions: Record<string, unknown> = Object.create(null);

function makeNodeEnoentError(name: string, path: string): Error {
  const syscall = name.endsWith("Sync") ? name.slice(0, -4) : name;
  const syscallNode = (syscall === "readFile" || syscall === "readTextFile" || syscall === "readFileSync" || syscall === "readTextFileSync" || syscall === "read" || syscall === "readSync") ? "open" : syscall;
  const error = new Error(`ENOENT: no such file or directory, ${syscallNode} '${path}'`);
  (error as any).code = "ENOENT";
  (error as any).errno = -2;
  (error as any).syscall = syscallNode;
  (error as any).path = path ? String(path) : "";
  return error;
}

function wrapFsBuiltin(namespaceValue: unknown): unknown {
  if (namespaceValue === null || typeof namespaceValue !== "object") {
    return namespaceValue;
  }
  const source = (namespaceValue as { default?: unknown }).default;
  const fsValue =
    source !== null && typeof source === "object"
      ? (source as Record<string, unknown>)
      : (namespaceValue as Record<string, unknown>);
  const wrapped = Object.defineProperties(
    Object.create(null),
    Object.getOwnPropertyDescriptors(fsValue),
  ) as Record<string, unknown>;
  for (const name of ["statSync", "lstatSync", "readFileSync", "readTextFileSync"]) {
    const original = fsValue[name];
    if (typeof original !== "function") {
      continue;
    }
    const isStat = name.includes("stat");
    wrapped[name] = (...args: unknown[]) => {
      try {
        const value = (original as (...args: unknown[]) => unknown).apply(fsValue, args);
        if (
          isStat &&
          (value === undefined ||
            (value !== null &&
              typeof value === "object" &&
              "isFile" in value &&
              "isDirectory" in value &&
              "isSymbolicLink" in value &&
              typeof (value as { isFile: () => boolean }).isFile === "function" &&
              typeof (value as { isDirectory: () => boolean }).isDirectory === "function" &&
              typeof (value as { isSymbolicLink: () => boolean }).isSymbolicLink === "function" &&
              !(value as { isFile: () => boolean }).isFile() &&
              !(value as { isDirectory: () => boolean }).isDirectory() &&
              !(value as { isSymbolicLink: () => boolean }).isSymbolicLink()))
        ) {
          throw makeNodeEnoentError(name, String(args[0]));
        }
        return value;
      } catch (error) {
        if (error === undefined) {
          throw makeNodeEnoentError(name, String(args[0]));
        }
        if (error && typeof error === "object") {
          const err = error as any;
          if (err.code === "ENOENT" || err.name === "NotFound" || String(err.message || "").includes("NotFound")) {
            try {
              const syscall = name.endsWith("Sync") ? name.slice(0, -4) : name;
              const syscallNode = (syscall === "readFile" || syscall === "readTextFile" || syscall === "readFileSync" || syscall === "readTextFileSync" || syscall === "read" || syscall === "readSync") ? "open" : syscall;
              Object.defineProperty(err, "code", { value: "ENOENT", configurable: true, writable: true, enumerable: true });
              Object.defineProperty(err, "errno", { value: -2, configurable: true, writable: true, enumerable: true });
              Object.defineProperty(err, "syscall", { value: err.syscall || syscallNode, configurable: true, writable: true, enumerable: true });
              Object.defineProperty(err, "path", { value: err.path || (args[0] ? String(args[0]) : ""), configurable: true, writable: true, enumerable: true });
              Object.defineProperty(err, "message", {
                value: `ENOENT: no such file or directory, ${err.syscall || syscallNode} '${err.path || (args[0] ? String(args[0]) : "")}'`,
                configurable: true,
                writable: true,
                enumerable: true
              });
            } catch (e) {
              // Ignore defineProperty errors
            }
          }
        }
        throw error;
      }
    };
  }
  if (typeof fsValue.createReadStream === "function") {
    wrapped.createReadStream = (pathLike: unknown, options?: unknown) => {
      const eventsModule = requireFromNamespace(nodeEvents) as {
        EventEmitter?: new () => {
          on(event: string, listener: (...args: unknown[]) => void): unknown;
          once(event: string, listener: (...args: unknown[]) => void): unknown;
          emit(event: string, ...args: unknown[]): boolean;
          removeListener(event: string, listener: (...args: unknown[]) => void): unknown;
        };
      };
      const EventEmitter = eventsModule.EventEmitter;
      const opts = (options !== null && typeof options === "object")
        ? options as Record<string, unknown>
        : {};
      const start = typeof opts.start === "number" ? opts.start : 0;
      const end = typeof opts.end === "number" ? opts.end : undefined;
      const encoding = typeof opts.encoding === "string" ? opts.encoding : undefined;
      const emitter = typeof EventEmitter === "function" ? new EventEmitter() : null;
      let closed = false;
      const loadPayload = () => {
        let data = (fsValue.readFileSync as (path: unknown) => Buffer)(pathLike);
        if (start !== 0 || end !== undefined) {
          data = data.subarray(start, end === undefined ? undefined : end + 1);
        }
        return encoding ? data.toString(encoding as BufferEncoding) : data;
      };
      const stream = {
        on(event: string, listener: (...args: unknown[]) => void) {
          emitter?.on(event, listener);
          return stream;
        },
        once(event: string, listener: (...args: unknown[]) => void) {
          emitter?.once(event, listener);
          return stream;
        },
        removeListener(event: string, listener: (...args: unknown[]) => void) {
          emitter?.removeListener(event, listener);
          return stream;
        },
        pause() {
          return stream;
        },
        resume() {
          return stream;
        },
        pipe(dest: { write(chunk: unknown): unknown; end(chunk?: unknown): unknown; emit?: (event: string, ...args: unknown[]) => unknown }) {
          queueMicrotask(() => {
            if (closed) return;
            try {
              const payload = loadPayload();
              dest.write(payload);
              dest.end();
              emitter?.emit("data", payload);
              emitter?.emit("end");
              emitter?.emit("close");
            } catch (error) {
              emitter?.emit("error", error);
              dest.emit?.("error", error);
            }
          });
          return dest;
        },
        close() {
          if (closed) return;
          closed = true;
          emitter?.emit("close");
        },
        destroy(error?: unknown) {
          if (closed) return;
          closed = true;
          if (error !== undefined) {
            emitter?.emit("error", error);
          }
          emitter?.emit("close");
        },
      };
      queueMicrotask(() => {
        if (closed) return;
        try {
          const payload = loadPayload();
          emitter?.emit("data", payload);
          emitter?.emit("end");
          emitter?.emit("close");
        } catch (error) {
          emitter?.emit("error", error);
        }
      });
      return stream;
    };
  }
  if ("default" in wrapped) {
    delete wrapped.default;
  }
  return wrapped;
}

function wrapChildProcessBuiltin(namespaceValue: unknown): unknown {
  if (namespaceValue === null || typeof namespaceValue !== "object") {
    return namespaceValue;
  }
  const source = (namespaceValue as { default?: unknown }).default;
  const childProcessValue =
    source !== null && typeof source === "object"
      ? (source as Record<string, unknown>)
      : (namespaceValue as Record<string, unknown>);
  const wrapped = Object.defineProperties(
    Object.create(null),
    Object.getOwnPropertyDescriptors(childProcessValue),
  ) as Record<string, unknown>;
  const originalFork = childProcessValue.fork;
  if (typeof originalFork === "function") {
    const fs = requireFromNamespace(nodeFs) as Record<string, unknown>;
    const os = requireFromNamespace(nodeOs) as Record<string, unknown>;
    const path = requireFromNamespace(nodePath) as Record<string, unknown>;
    wrapped.fork = (modulePath: unknown, args?: unknown, options?: unknown) => {
      let forkArgs: unknown[] = [];
      let forkOptions: Record<string, unknown> = {};
      if (Array.isArray(args)) {
        forkArgs = args;
        forkOptions =
          options !== null && typeof options === "object"
            ? { ...(options as Record<string, unknown>) }
            : {};
      } else if (args !== null && typeof args === "object") {
        forkOptions = { ...(args as Record<string, unknown>) };
      }
      const parentEnv =
        nodeGlobal.process !== null &&
        typeof nodeGlobal.process === "object" &&
        "env" in (nodeGlobal.process as Record<string, unknown>) &&
        (nodeGlobal.process as { env?: Record<string, string> }).env !== undefined
          ? { ...(nodeGlobal.process as { env: Record<string, string> }).env }
          : {};
      const env = {
        ...parentEnv,
        ...((forkOptions.env as Record<string, string> | undefined) ?? {}),
      };
      if (forkOptions.cwd === undefined && typeof (nodeGlobal.process as { cwd?: unknown })?.cwd === "function") {
        forkOptions.cwd = ((nodeGlobal.process as { cwd: () => string }).cwd)();
      }
      const debugLog = env.MEOW_DEBUG_IPC;
      const mailboxRoot = (fs.mkdtempSync as (prefix: string) => string)(
        (path.join as (...parts: string[]) => string)(
          (os.tmpdir as () => string)(),
          "meow-ipc-",
        ),
      );
      const parentToChild = (path.join as (...parts: string[]) => string)(mailboxRoot, "parent-to-child.jsonl");
      const childToParent = (path.join as (...parts: string[]) => string)(mailboxRoot, "child-to-parent.jsonl");
      (fs.writeFileSync as (path: string, data: string) => void)(parentToChild, "");
      (fs.writeFileSync as (path: string, data: string) => void)(childToParent, "");
      env.MEOW_IPC_PARENT_TO_CHILD = parentToChild;
      env.MEOW_IPC_CHILD_TO_PARENT = childToParent;
      env.MEOW_FORK_PATCH_JSON = JSON.stringify(
        ((forkOptions.env as Record<string, string | undefined> | undefined) ?? {}),
      );
      if ((forkOptions.env as Record<string, string | undefined> | undefined)?.NEXT_PRIVATE_WORKER) {
        env.MEOW_FORK_NEXT_PRIVATE_WORKER = String(
          (forkOptions.env as Record<string, string | undefined>).NEXT_PRIVATE_WORKER,
        );
      }
      forkOptions.env = env;
      const child = (originalFork as (...forkArgs: unknown[]) => Record<string, unknown>)(
        modulePath,
        forkArgs,
        forkOptions,
      );
      let consumed = 0;
      let buffered = "";
      const readMessages = () => {
        let text: string;
        try {
          text = (fs.readFileSync as (path: string, encoding: string) => string)(childToParent, "utf8");
        } catch {
          return;
        }
        if (text.length < consumed) {
          consumed = 0;
          buffered = "";
        }
        const chunk = text.slice(consumed);
        if (chunk.length === 0) {
          return;
        }
        consumed = text.length;
        buffered += chunk;
        const lines = buffered.split("\n");
        buffered = lines.pop() ?? "";
        for (const line of lines) {
          if (line.length === 0) {
            continue;
          }
          if (debugLog) {
            (fs.appendFileSync as (path: string, data: string) => void)(
              String(debugLog),
              `parent<-child ${line}\n`,
            );
          }
          try {
            (child.emit as (event: string, payload: unknown) => void)("message", JSON.parse(line));
          } catch {
            // Ignore malformed mailbox writes from child code.
          }
        }
      };
      const timer = setInterval(readMessages, 20);
      if (typeof (child.on as ((event: string, listener: (...args: unknown[]) => void) => unknown)) === "function") {
        child.on("exit", () => {
          clearInterval(timer);
        });
      }
      if (debugLog) {
        (fs.appendFileSync as (path: string, data: string) => void)(
          String(debugLog),
          `fork ${String(modulePath)} ${parentToChild} ${childToParent}\n`,
        );
      }
      child.send = (message: unknown) => {
        const payload = `${JSON.stringify(message)}\n`;
        if (debugLog) {
          (fs.appendFileSync as (path: string, data: string) => void)(
            String(debugLog),
            `parent->child ${payload}`,
          );
        }
        (fs.appendFileSync as (path: string, data: string) => void)(parentToChild, payload);
        return true;
      };
      return child;
    };
  }
  if ("default" in wrapped) {
    delete wrapped.default;
  }
  return wrapped;
}

function wrapZlibBuiltin(source: typeof nodeZlib): unknown {
  const base = ((source as { default?: unknown }).default ?? source) as Record<string, unknown>;
  const wrapped = { ...base } as Record<string, unknown>;

  for (const [key, value] of Object.entries(base)) {
    if (typeof value === "function") {
      wrapped[key] = value.bind(base);
    }
  }

  const patchTransform = (factoryName: string, syncName: string) => {
    const syncFn = wrapped[syncName];
    if (typeof syncFn !== "function" || typeof wrapped[factoryName] !== "function") {
      return;
    }
    wrapped[factoryName] = (options?: unknown) => {
      const eventsModule = requireFromNamespace(nodeEvents) as {
        EventEmitter?: new () => {
          on(event: string, listener: (...args: unknown[]) => void): unknown;
          once(event: string, listener: (...args: unknown[]) => void): unknown;
          emit(event: string, ...args: unknown[]): boolean;
          removeListener(event: string, listener: (...args: unknown[]) => void): unknown;
        };
      };
      const bufferModule = requireFromNamespace(nodeBuffer) as { Buffer?: typeof Buffer };
      const EventEmitter = eventsModule.EventEmitter;
      const BufferCtor = bufferModule.Buffer ?? Buffer;
      const emitter = typeof EventEmitter === "function" ? new EventEmitter() : null;
      const chunks: Buffer[] = [];
      let closed = false;

      const stream = {
        on(event: string, listener: (...args: unknown[]) => void) {
          emitter?.on(event, listener);
          return stream;
        },
        once(event: string, listener: (...args: unknown[]) => void) {
          emitter?.once(event, listener);
          return stream;
        },
        removeListener(event: string, listener: (...args: unknown[]) => void) {
          emitter?.removeListener(event, listener);
          return stream;
        },
        pause() {
          return stream;
        },
        resume() {
          return stream;
        },
        pipe(dest: {
          write?(chunk: unknown): unknown;
          end?(chunk?: unknown): unknown;
          emit?(event: string, ...args: unknown[]): unknown;
        }) {
          stream.on("data", (chunk) => {
            dest.write?.(chunk);
          });
          stream.on("end", () => {
            dest.end?.();
          });
          stream.on("error", (error) => {
            dest.emit?.("error", error);
          });
          return dest;
        },
        write(chunk: unknown) {
          if (closed) return false;
          chunks.push(BufferCtor.from(chunk as ArrayBufferLike));
          return true;
        },
        end(chunk?: unknown) {
          if (closed) return stream;
          if (chunk !== undefined) {
            stream.write(chunk);
          }
          queueMicrotask(() => {
            if (closed) return;
            try {
              const output = (syncFn as (input: Buffer, options?: unknown) => unknown)(
                BufferCtor.concat(chunks),
                options,
              );
              emitter?.emit("data", output);
              emitter?.emit("end");
              emitter?.emit("close");
            } catch (error) {
              emitter?.emit("error", error);
            }
          });
          return stream;
        },
        destroy(error?: unknown) {
          if (closed) return stream;
          closed = true;
          if (error !== undefined) {
            emitter?.emit("error", error);
          }
          emitter?.emit("close");
          return stream;
        },
      };
      return stream;
    };
  };

  patchTransform("createGzip", "gzipSync");
  patchTransform("createGunzip", "gunzipSync");
  patchTransform("createDeflate", "deflateSync");
  patchTransform("createInflate", "inflateSync");
  patchTransform("createDeflateRaw", "deflateRawSync");
  patchTransform("createInflateRaw", "inflateRawSync");
  patchTransform("createBrotliCompress", "brotliCompressSync");
  patchTransform("createBrotliDecompress", "brotliDecompressSync");

  if ("default" in wrapped) {
    delete wrapped.default;
  }
  return wrapped;
}

const builtins: Record<string, unknown> = {
  assert: nodeAssert,
  buffer: nodeBuffer,
  async_hooks: nodeAsyncHooks,
  child_process: wrapChildProcessBuiltin(nodeChildProcess),
  constants: nodeConstants,
  console: nodeConsole,
  crypto: nodeCrypto,
  dns: nodeDns,
  events: nodeEvents,
  fs: wrapFsBuiltin(nodeFs),
  "fs/promises": nodeFsPromises,
  http: nodeHttp,
  https: nodeHttps,
  module: Module,
  os: (nodeOs as { default?: unknown }).default ?? nodeOs,
  path: nodePath,
  process: (nodeProcess as { default?: unknown }).default ?? nodeProcess,
  url: nodeUrl,
  util: nodeUtil,
  "util/types": nodeUtilTypes,
  cluster: nodeCluster,
  dgram: nodeDgram,
  diagnostics_channel: nodeDiagnosticsChannel,
  "dns/promises": nodeDnsPromises,
  domain: nodeDomain,
  http2: nodeHttp2,
  inspector: nodeInspector,
  net: nodeNet,
  perf_hooks: nodePerfHooks,
  punycode: nodePunycode,
  querystring: nodeQuerystring,
  readline: nodeReadline,
  repl: nodeRepl,
  stream: (nodeStream as { default?: unknown }).default ?? nodeStream,
  "stream/consumers": nodeStreamConsumers,
  "stream/promises": nodeStreamPromises,
  "stream/web": nodeStreamWeb,
  string_decoder: nodeStringDecoder,
  sys: nodeSys,
  timers: nodeTimers,
  "timers/promises": nodeTimersPromises,
  tls: nodeTls,
  trace_events: nodeTraceEvents,
  tty: nodeTty,
  v8: nodeV8,
  vm: nodeVm,
  wasi: nodeWasi,
  worker_threads: nodeWorkerThreads,
  zlib: wrapZlibBuiltin(nodeZlib),
};

function getBuiltin(specifier: string): unknown {
  const normalized = specifier.startsWith("node:") ? specifier.slice("node:".length) : specifier;
  const builtin = specifier.startsWith("node:") ? builtins[normalized] : builtins[specifier];
  return builtin;
}

const nodeGlobal = globalThis as { process?: unknown; Buffer?: unknown; global?: unknown; performance?: unknown };
if (nodeGlobal.global === undefined) {
  nodeGlobal.global = globalThis;
}
if (nodeGlobal.process === undefined) {
  nodeGlobal.process = (nodeProcess as { default?: unknown }).default ?? requireFromNamespace(nodeProcess);
}
const bufferModule = requireFromNamespace(nodeBuffer);
if (bufferModule !== null && typeof bufferModule === "object") {
  const bufferExports = bufferModule as { Buffer?: unknown; atob?: unknown; btoa?: unknown };
  if (nodeGlobal.Buffer === undefined && "Buffer" in bufferExports) {
    nodeGlobal.Buffer = bufferExports.Buffer;
  }
  const globals = nodeGlobal as Record<string, unknown>;
  if (globals.atob === undefined && typeof bufferExports.atob === "function") {
    globals.atob = bufferExports.atob;
  }
  if (globals.btoa === undefined && typeof bufferExports.btoa === "function") {
    globals.btoa = bufferExports.btoa;
  }
}
if (nodeGlobal.performance === undefined) {
  const perfHooks = requireFromNamespace(nodePerfHooks);
  if (perfHooks !== null && typeof perfHooks === "object" && "performance" in perfHooks) {
    nodeGlobal.performance = (perfHooks as { performance: unknown }).performance;
  }
}
const timerGlobals = requireFromNamespace(nodeTimers);
if (timerGlobals !== null && typeof timerGlobals === "object") {
  const globals = nodeGlobal as Record<string, unknown>;
  for (const name of [
    "setTimeout",
    "clearTimeout",
    "setInterval",
    "clearInterval",
    "setImmediate",
    "clearImmediate",
  ]) {
    if (globals[name] === undefined && name in timerGlobals) {
      globals[name] = (timerGlobals as Record<string, unknown>)[name];
    }
  }
}
const streamWebGlobals = requireFromNamespace(nodeStreamWeb);
if (streamWebGlobals !== null && typeof streamWebGlobals === "object") {
  for (const name of [
    "ReadableStream",
    "ReadableStreamDefaultReader",
    "ReadableStreamBYOBReader",
    "ReadableStreamBYOBRequest",
    "ReadableByteStreamController",
    "ReadableStreamDefaultController",
    "TransformStream",
    "TransformStreamDefaultController",
    "WritableStream",
    "WritableStreamDefaultWriter",
    "WritableStreamDefaultController",
    "TextDecoderStream",
    "CompressionStream",
    "DecompressionStream",
  ]) {
    const globals = nodeGlobal as Record<string, unknown>;
    if (globals[name] === undefined && name in streamWebGlobals) {
      globals[name] = (streamWebGlobals as Record<string, unknown>)[name];
    }
  }
}
try {
  const globals = nodeGlobal as Record<string, unknown>;
  const domException = core.loadExtScript("ext:deno_web/01_dom_exception.js") as Record<string, unknown>;
  if (globals.DOMException === undefined && "DOMException" in domException) {
    globals.DOMException = domException.DOMException;
  }
  const encoding = core.loadExtScript("ext:deno_web/08_text_encoding.js") as Record<string, unknown>;
  if (globals.TextEncoder === undefined && "TextEncoder" in encoding) {
    globals.TextEncoder = encoding.TextEncoder;
  }
  if (globals.TextDecoder === undefined && "TextDecoder" in encoding) {
    globals.TextDecoder = encoding.TextDecoder;
  }
  const structured = core.loadExtScript("ext:deno_web/02_structured_clone.js") as Record<string, unknown>;
  if (globals.structuredClone === undefined && "structuredClone" in structured) {
    globals.structuredClone = structured.structuredClone;
  }
  const base64 = core.loadExtScript("ext:deno_web/05_base64.js") as Record<string, unknown>;
  if (globals.atob === undefined && "atob" in base64) {
    globals.atob = base64.atob;
  }
  if (globals.btoa === undefined && "btoa" in base64) {
    globals.btoa = base64.btoa;
  }
  const abortSignal = core.loadExtScript("ext:deno_web/03_abort_signal.js") as Record<string, unknown>;
  if (globals.AbortController === undefined && "AbortController" in abortSignal) {
    globals.AbortController = abortSignal.AbortController;
  }
  if (globals.AbortSignal === undefined && "AbortSignal" in abortSignal) {
    globals.AbortSignal = abortSignal.AbortSignal;
  }
  const file = core.loadExtScript("ext:deno_web/09_file.js") as Record<string, unknown>;
  if (globals.Blob === undefined && "Blob" in file) {
    globals.Blob = file.Blob;
  }
  const urlGlobals = core.loadExtScript("ext:deno_web/00_url.js") as Record<string, unknown>;
  if (globals.URL === undefined && "URL" in urlGlobals) {
    globals.URL = urlGlobals.URL;
  }
  if (globals.URLSearchParams === undefined && "URLSearchParams" in urlGlobals) {
    globals.URLSearchParams = urlGlobals.URLSearchParams;
  }
  const eventGlobals = core.loadExtScript("ext:deno_web/02_event.js") as Record<string, unknown>;
  for (const name of [
    "Event",
    "EventTarget",
    "CustomEvent",
    "MessageEvent",
    "ErrorEvent",
    "CloseEvent",
    "ProgressEvent",
  ]) {
    if (globals[name] === undefined && name in eventGlobals) {
      globals[name] = eventGlobals[name];
    }
  }
} catch {
  // Leave optional web globals absent if this runtime pin does not ship the script.
}

function makeProcessStream(fd: 1 | 2): unknown {
  const stream = {
    fd,
    isTTY: false,
    writable: true,
    write(chunk: unknown, encodingOrCallback?: unknown, callback?: unknown): boolean {
      const cb = typeof encodingOrCallback === "function" ? encodingOrCallback : callback;
      const text = typeof chunk === "string"
        ? chunk
        : chunk instanceof Uint8Array
        ? new TextDecoder().decode(chunk)
        : String(chunk);
      const print = ops.op_meow_print;
      if (typeof print === "function" && text.length > 0) {
        print(text, fd === 2);
      }
      if (typeof cb === "function") {
        cb();
      }
      return true;
    },
    on(): unknown {
      return stream;
    },
    once(): unknown {
      return stream;
    },
    removeListener(): unknown {
      return stream;
    },
    emit(): boolean {
      return false;
    },
    cork(): void {},
    uncork(): void {},
    end(): unknown {
      return stream;
    },
  };
  return stream;
}

const processValue = nodeGlobal.process as { env?: unknown; stdout?: unknown; stderr?: unknown };
const bootstrapEnv =
  (globalThis as { __MEOW_BOOTSTRAP_ENV__?: Record<string, string> }).__MEOW_BOOTSTRAP_ENV__ ??
  Object.create(null);
if (processValue !== null && typeof processValue === "object") {
  const envObject: Record<string, string> = {
    ...bootstrapEnv,
    ...(processValue.env !== null && typeof processValue.env === "object"
      ? (processValue.env as Record<string, string>)
      : Object.create(null)),
  };
  const envEntries = typeof ops.op_hermetic_env_entries === "function"
    ? (ops.op_hermetic_env_entries() as unknown)
    : [];
  if (Array.isArray(envEntries)) {
    for (const entry of envEntries) {
      if (Array.isArray(entry) && entry.length === 2) {
        envObject[String(entry[0])] = String(entry[1]);
      }
    }
  }
  const denoGlobal = (globalThis as unknown as {
    Deno?: {
      env?: {
        get?: (key: string) => string | undefined;
        set?: (key: string, value: string) => void;
        has?: (key: string) => boolean;
        delete?: (key: string) => void;
        toObject?: () => Record<string, string>;
      };
    };
  }).Deno;
  if (denoGlobal !== undefined && denoGlobal.env === undefined) {
    denoGlobal.env = {
      get(key: string): string | undefined {
        return envObject[String(key)];
      },
      set(key: string, value: string): void {
        envObject[String(key)] = String(value);
      },
      has(key: string): boolean {
        return Object.prototype.hasOwnProperty.call(envObject, String(key));
      },
      delete(key: string): void {
        delete envObject[String(key)];
      },
      toObject(): Record<string, string> {
        return { ...envObject };
      },
    };
  }
  processValue.env = envObject;
  processValue.stdout = makeProcessStream(1);
  processValue.stderr = makeProcessStream(2);
}

function toRequireSpecifier(specifier: unknown): string {
  if (typeof specifier !== "string") {
    throw new TypeError("CommonJS require() specifier must be a string");
  }
  return specifier;
}

function fileUrlForPath(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const encoded = normalized
    .split("/")
    .map((segment, index) => (index === 0 && segment === "" ? "" : encodeURIComponent(segment)))
    .join("/");
  return /^[A-Za-z]:\//.test(normalized) ? `file:///${encoded}` : `file://${encoded}`;
}

function cacheUrlForMeowCachePath(specifier: string): string | undefined {
  let normalized = specifier.replace(/\\/g, "/");
  if (normalized.startsWith("/meow-cache/")) {
    normalized = normalized.slice(1);
  }
  if (!normalized.startsWith("meow-cache/")) {
    return undefined;
  }
  const tail = normalized.slice("meow-cache/".length);
  const slashIndex = tail.indexOf("/");
  if (slashIndex === -1 || slashIndex === tail.length - 1) {
    const host = slashIndex === -1 ? tail : tail.slice(0, slashIndex);
    return host.length === 0 ? undefined : `meow-cache://${host}/`;
  }
  return `meow-cache://${tail.slice(0, slashIndex)}/${tail.slice(slashIndex + 1)}`;
}

function normalizeCachePathSpecifier(specifier: string): string {
  return cacheUrlForMeowCachePath(specifier) ?? specifier;
}

function stripLeadingHashbang(source: string): string {
  if (!source.startsWith("#!")) {
    return source;
  }

  const lineBreakIndex = source.search(/[\r\n]/);
  if (lineBreakIndex === -1) {
    return "\n";
  }

  const lineBreakLength =
    source[lineBreakIndex] === "\r" && source[lineBreakIndex + 1] === "\n" ? 2 : 1;
  const lineBreak = source.slice(lineBreakIndex, lineBreakIndex + lineBreakLength);

  return `${lineBreak}${source.slice(lineBreakIndex + lineBreakLength)}`;
}

// Compatibility bridge: rewrite CommonJS dynamic import calls so they resolve against
// the calling CJS module, because native `import()` in eval'd wrappers defaults
// to a different base.
function rewriteDynamicImportCalls(source: string): string {
  return source.replace(/(^|[^\w$.])import(\s*)\(/g, "$1__meowDynamicImport$2(");
}

function namespaceFromCjsExports(exportsValue: unknown): unknown {
  const namespace = Object.create(null) as { [key: string]: unknown };
  if (exportsValue !== null && (typeof exportsValue === "object" || typeof exportsValue === "function")) {
    Object.defineProperties(namespace, Object.getOwnPropertyDescriptors(exportsValue as object));
  }
  namespace.default = exportsValue;
  return namespace;
}

function createDynamicImportForModule(
  moduleValue: CjsModuleRecord,
): (specifier: unknown) => Promise<unknown> {
  return async (childSpecifier: unknown): Promise<unknown> => {
    const requested = toRequireSpecifier(childSpecifier);
    const builtin = getBuiltin(requested);
    if (builtin !== undefined) {
      const builtinSpecifier = requested.startsWith("node:") ? requested : `node:${requested}`;
      return import(builtinSpecifier);
    }

    if (isFullyQualifiedModuleSpecifier(requested)) {
      try {
        const loaded = resolveAndLoad(requested, moduleValue.url);
        if (loaded.kind === "json") {
          return namespaceFromCjsExports(requireJson(loaded.url, loaded.source));
        }
        return namespaceFromCjsExports(executeLoadedModule(loaded, moduleValue));
      } catch {
        return import(requested);
      }
    }

    const referrer = referrerForRequire(moduleValue, requested);
    const targetForCjs = normalizeCachePathSpecifier(requested);

    let loaded: LoadedModule;
    try {
      loaded = resolveAndLoad(targetForCjs, referrer);
    } catch {
      let importTarget = targetForCjs;
      if (isAbsolutePath(importTarget)) {
        importTarget = fileUrlForPath(importTarget);
      } else if (
        importTarget === requested &&
        !isFullyQualifiedModuleSpecifier(importTarget)
      ) {
        try {
          importTarget = resolveSpecifier(requested, referrer);
          if (isAbsolutePath(importTarget)) {
            importTarget = fileUrlForPath(importTarget);
          } else {
            importTarget = normalizeCachePathSpecifier(importTarget);
          }
        } catch {
          // Keep the original dynamic-import target; V8 will report the failure.
        }
      }
      return import(importTarget);
    }

    if (loaded.kind === "json") {
      return namespaceFromCjsExports(requireJson(loaded.url, loaded.source));
    }
    return namespaceFromCjsExports(executeLoadedModule(loaded, moduleValue));
  };
}

function isFullyQualifiedModuleSpecifier(specifier: string): boolean {
  const schemeEnd = specifier.indexOf(":");
  if (schemeEnd <= 0) {
    return false;
  }

  const firstCode = specifier.charCodeAt(0);
  if (
    (firstCode < 65 || firstCode > 90) &&
    (firstCode < 97 || firstCode > 122)
  ) {
    return false;
  }

  if (
    schemeEnd === 1 &&
    specifier.length > 2 &&
    (specifier.charCodeAt(2) === 47 || specifier.charCodeAt(2) === 92)
  ) {
    return false;
  }

  for (let i = 1; i < schemeEnd; i += 1) {
    const code = specifier.charCodeAt(i);
    if (
      (code >= 65 && code <= 90) ||
      (code >= 97 && code <= 122) ||
      (code >= 48 && code <= 57) ||
      code === 43 ||
      code === 45 ||
      code === 46
    ) {
      continue;
    }
    return false;
  }

  return true;
}


function resolveAndLoad(specifier: string, referrer: string): LoadedModule {
  try {
    return ops.op_cjs_resolve_and_load(specifier, referrer) as LoadedModule;
  } catch (cause) {
    const message = String((cause as { message?: unknown })?.message ?? cause);
    if (message.includes("could not resolve")) {
      const err = new Error(`Cannot find module '${specifier}'`);
      (err as { code?: string; cause?: unknown }).code = "MODULE_NOT_FOUND";
      (err as { code?: string; cause?: unknown }).cause = cause;
      throw err;
    }
    throw cause;
  }
}

function resolveSpecifier(specifier: string, referrer: string): string {
  const normalized = normalizeCachePathSpecifier(specifier);
  const builtin = getBuiltin(normalized);
  if (builtin !== undefined) {
    return normalized.startsWith("node:") ? normalized.slice("node:".length) : normalized;
  }
  return resolveAndLoad(normalized, referrer).filename;
}

function referrerForRequire(moduleValue: CjsModuleRecord, specifier: string): string {
  return isAbsolutePath(specifier) ? fileUrlForPath(moduleValue.filename) : moduleValue.url;
}

function isAbsolutePath(specifier: string): boolean {
  return specifier.startsWith("/") || /^[A-Za-z]:[\\/]/.test(specifier);
}


function createRequireForModule(moduleValue: CjsModuleRecord): CjsRequire {
  const require = ((childSpecifier: unknown): unknown => {
    const requested = toRequireSpecifier(childSpecifier);
    return runCjsModule(requested, referrerForRequire(moduleValue, requested), moduleValue);
  }) as CjsRequire & { __meowResolveRaw?: (specifier: string) => string; __meowReferrer?: string };
  require.resolve = (childSpecifier: unknown): string => {
    const requested = toRequireSpecifier(childSpecifier);
    return resolveSpecifier(requested, referrerForRequire(moduleValue, requested));
  };
  require.__meowResolveRaw = (childSpecifier: string): string => {
    return resolveSpecifier(childSpecifier, referrerForRequire(moduleValue, childSpecifier));
  };
  require.__meowReferrer = moduleValue.url;
  require.cache = requireCache;
  require.extensions = requireExtensions;
  return require;
}

function instantiate(moduleValue: CjsModuleRecord, loaded: LoadedModule): void {
  const previousRequire = (globalThis as { __meowCurrentRequire?: unknown }).__meowCurrentRequire;
  (globalThis as { __meowCurrentRequire?: unknown }).__meowCurrentRequire = moduleValue.require;
  try {
    const source = stripLeadingHashbang(loaded.source);
    const rewrittenSource = rewriteDynamicImportCalls(source);
    const closure = `(function(exports, require, module, __filename, __dirname, __meowDynamicImport) {\n${rewrittenSource}\n})\n//# sourceURL=${loaded.url.replace(/[\r\n]/g, "")}`;
    const compiled = (0, eval)(closure) as (
      exports: unknown,
      require: CjsRequire,
      module: CjsModuleRecord,
      filename: string,
      dirname: string,
      __meowDynamicImport: (specifier: unknown) => Promise<unknown>,
    ) => void;
    compiled(
      moduleValue.exports,
      moduleValue.require,
      moduleValue,
      loaded.filename,
      loaded.dirname,
      createDynamicImportForModule(moduleValue),
    );
    moduleValue.loaded = true;
  } finally {
    (globalThis as { __meowCurrentRequire?: unknown }).__meowCurrentRequire = previousRequire;
  }
}

function executeLoadedModule(loaded: LoadedModule, parent?: CjsModuleRecord): unknown {
  const cached = moduleCache.get(loaded.url);
  if (cached !== undefined) {
    return cached.exports;
  }

  if (loaded.kind === "napi") {
    const nativeModule: CjsModuleRecord = {
      id: loaded.filename,
      url: loaded.url,
      filename: loaded.filename,
      path: loaded.dirname,
      exports: {},
      loaded: false,
      parent,
      children: [],
      constructor: Module,
      require: null as unknown as CjsRequire,
    };
    Object.setPrototypeOf(nativeModule, Module.prototype);
    nativeModule.require = createRequireForModule(nativeModule);
    if (parent !== undefined) {
      parent.children.push(nativeModule);
    }
    moduleCache.set(loaded.url, nativeModule);
    requireCache[loaded.filename] = nativeModule;
    try {
      const processModule = ((nodeProcess as { default?: unknown }).default ?? requireFromNamespace(nodeProcess)) as {
        dlopen(module: CjsModuleRecord, filename: string, flags?: number): unknown;
      };
      processModule.dlopen(nativeModule, loaded.filename, 0);
      nativeModule.loaded = true;
    } catch (error) {
      moduleCache.delete(loaded.url);
      delete requireCache[loaded.filename];
      throw error;
    }
    return nativeModule.exports;
  }

  const moduleValue: CjsModuleRecord = {
    id: loaded.filename,
    url: loaded.url,
    filename: loaded.filename,
    path: loaded.dirname,
    exports: {},
    loaded: false,
    parent,
    children: [],
    constructor: Module,
    require: null as unknown as CjsRequire,
  };
  Object.setPrototypeOf(moduleValue, Module.prototype);
  moduleValue.require = createRequireForModule(moduleValue);
  if (parent !== undefined) {
    parent.children.push(moduleValue);
  }
  moduleCache.set(loaded.url, moduleValue);
  requireCache[loaded.filename] = moduleValue;

  try {
    instantiate(moduleValue, loaded);
  } catch (error) {
    moduleCache.delete(loaded.url);
    delete requireCache[loaded.filename];
    throw error;
  }
  return moduleValue.exports;
}

export function runCjsModule(specifier: string, referrer = specifier, parent?: CjsModuleRecord): unknown {
  const normalized = normalizeCachePathSpecifier(specifier);
  const builtin = getBuiltin(normalized);
  if (builtin !== undefined) {
    return requireFromNamespace(builtin);
  }

  const loaded = resolveAndLoad(normalized, referrer);
  if (loaded.kind === "json") {
    let parsed = jsonCache.get(loaded.url);
    if (parsed === undefined && !jsonCache.has(loaded.url)) {
      parsed = JSON.parse(loaded.source);
      jsonCache.set(loaded.url, parsed);
    }
    return parsed;
  }

  return executeLoadedModule(loaded, parent);
}

export function runCjsModuleText(
  url: string,
  filename: string,
  dirname: string,
  source: string,
): unknown {
  return executeLoadedModule({
    url,
    filename,
    dirname,
    source,
    kind: "cjs",
  });
}

export function requireFromNamespace(namespaceValue: unknown): unknown {
  if (namespaceValue !== null && typeof namespaceValue === "object") {
    const namespace = namespaceValue as { __meow_cjs_exports__?: unknown; default?: unknown };
    if ("__meow_cjs_exports__" in namespace) {
      return namespace.__meow_cjs_exports__;
    }
    if ("default" in namespace) {
      return namespace.default;
    }
  }
  return namespaceValue;
}

export function requireJson(key: string, source: string): unknown {
  if (!jsonCache.has(key)) {
    jsonCache.set(key, JSON.parse(source));
  }
  return jsonCache.get(key);
}