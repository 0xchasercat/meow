const BUILTIN_MODULES = [
  // RT-007 — implemented core surface.
  "assert",
  "buffer",
  "child_process",
  "crypto",
  "dns",
  "events",
  "fs",
  "fs/promises",
  "http",
  "https",
  "module",
  "os",
  "path",
  "process",
  "url",
  "util",
  // === LOAD-003 / RT-007 ===
  // Webpack/Next.js externals: empty shims that satisfy the static
  // `require(...)` graph. `isBuiltin` returns true for them so callers that
  // gate behavior on the built-in check still treat them as built-ins.
  "cluster",
  "dgram",
  "diagnostics_channel",
  "dns/promises",
  "domain",
  "http2",
  "inspector",
  "net",
  "perf_hooks",
  "punycode",
  "querystring",
  "readline",
  "repl",
  "stream",
  "stream/consumers",
  "stream/promises",
  "stream/web",
  "string_decoder",
  "sys",
  "timers",
  "timers/promises",
  "tls",
  "trace_events",
  "tty",
  "v8",
  "vm",
  "wasi",
  "worker_threads",
  "zlib",
  // === /LOAD-003 / RT-007 ===
] as const;

export const builtinModules = Object.freeze([...BUILTIN_MODULES]);

export function isBuiltin(name: string): boolean {
  const normalized = name.startsWith("node:") ? name.slice("node:".length) : name;
  return (BUILTIN_MODULES as readonly string[]).includes(normalized);
}

export function createRequire(_filename: string | URL): (specifier: string) => unknown {
  const current = (globalThis as unknown as { __meowCurrentRequire?: unknown }).__meowCurrentRequire;
  if (typeof current !== "function") {
    throw new Error("meow: module.createRequire() is only available while executing CommonJS");
  }
  return current as (specifier: string) => unknown;
}

export default {
  builtinModules,
  isBuiltin,
  createRequire,
};
