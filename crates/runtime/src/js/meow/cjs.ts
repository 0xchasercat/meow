import * as nodeAssert from "node:assert";
import * as nodeBuffer from "node:buffer";
import * as nodeChildProcess from "node:child_process";
import * as nodeCrypto from "node:crypto";
import * as nodeDns from "node:dns";
import * as nodeEvents from "node:events";
import * as nodeFs from "node:fs";
import * as nodeFsPromises from "node:fs/promises";
import * as nodeHttp from "node:http";
import * as nodeHttps from "node:https";
import * as nodeModule from "node:module";
import * as nodeOs from "node:os";
import * as nodePath from "node:path";
import * as nodeProcess from "node:process";
import * as nodeUrl from "node:url";
import * as nodeUtil from "node:util";
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

const ops = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

interface LoadedModule {
  url: string;
  filename: string;
  dirname: string;
  source: string;
  kind: "cjs" | "json";
}

interface CjsModuleRecord {
  id: string;
  filename: string;
  path: string;
  exports: unknown;
  loaded: boolean;
  parent: CjsModuleRecord | undefined;
  children: CjsModuleRecord[];
  require(specifier: unknown): unknown;
}

const moduleCache = new Map<string, CjsModuleRecord>();
const jsonCache = new Map<string, unknown>();

const builtins: Record<string, unknown> = {
  assert: nodeAssert,
  buffer: nodeBuffer,
  child_process: nodeChildProcess,
  crypto: nodeCrypto,
  dns: nodeDns,
  events: nodeEvents,
  fs: nodeFs,
  "fs/promises": nodeFsPromises,
  http: nodeHttp,
  https: nodeHttps,
  module: nodeModule,
  os: nodeOs,
  path: nodePath,
  process: nodeProcess,
  url: nodeUrl,
  util: nodeUtil,
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
  stream: nodeStream,
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
  zlib: nodeZlib,
};

function toRequireSpecifier(specifier: unknown): string {
  if (typeof specifier !== "string") {
    throw new TypeError("CommonJS require() specifier must be a string");
  }
  return specifier;
}


function instantiate(moduleValue: CjsModuleRecord, loaded: LoadedModule): void {
  const previousRequire = (globalThis as { __meowCurrentRequire?: unknown }).__meowCurrentRequire;
  (globalThis as { __meowCurrentRequire?: unknown }).__meowCurrentRequire = moduleValue.require;
  try {
    const closure = `(function(exports, require, module, __filename, __dirname) {\n${loaded.source}\n})\n//# sourceURL=${loaded.url.replace(/[\r\n]/g, "")}`;
    const compiled = (0, eval)(closure) as (
      exports: unknown,
      require: (specifier: unknown) => unknown,
      module: CjsModuleRecord,
      filename: string,
      dirname: string,
    ) => void;
    compiled(moduleValue.exports, moduleValue.require, moduleValue, loaded.filename, loaded.dirname);
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

  const moduleValue: CjsModuleRecord = {
    id: loaded.filename,
    filename: loaded.filename,
    path: loaded.dirname,
    exports: {},
    loaded: false,
    parent,
    children: [],
    require(childSpecifier: unknown): unknown {
      return runCjsModule(toRequireSpecifier(childSpecifier), loaded.url, moduleValue);
    },
  };
  if (parent !== undefined) {
    parent.children.push(moduleValue);
  }
  moduleCache.set(loaded.url, moduleValue);

  try {
    instantiate(moduleValue, loaded);
  } catch (error) {
    moduleCache.delete(loaded.url);
    throw error;
  }
  return moduleValue.exports;
}

export function runCjsModule(specifier: string, referrer = specifier, parent?: CjsModuleRecord): unknown {
  const normalized = specifier.startsWith("node:") ? specifier.slice(5) : specifier;
  const builtin = specifier.startsWith("node:") ? builtins[normalized] : builtins[specifier];
  if (builtin !== undefined) {
    return builtin;
  }

  const loaded = ops.op_cjs_resolve_and_load(specifier, referrer) as LoadedModule;
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
  if (
    namespaceValue !== null &&
    typeof namespaceValue === "object" &&
    "__meow_cjs_exports__" in namespaceValue
  ) {
    return (namespaceValue as { __meow_cjs_exports__: unknown }).__meow_cjs_exports__;
  }
  return namespaceValue;
}

export function requireJson(key: string, source: string): unknown {
  if (!jsonCache.has(key)) {
    jsonCache.set(key, JSON.parse(source));
  }
  return jsonCache.get(key);
}
