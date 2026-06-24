import { core, internals } from "ext:core/mod.js";
import processModule from "node:process";
import * as bufferModule from "node:buffer";
import * as perfHooks from "node:perf_hooks";
import * as nodeTimers from "node:timers";
import * as nodeStreamWeb from "node:stream/web";
import * as nodeFsModule from "node:fs";
import * as nodeNet from "node:net";
import * as nodeHttpIncoming from "node:_http_incoming";
import * as nodeOs from "node:os";
import * as nodeConstantsModule from "node:constants";
const load = core.loadExtScript;
const bootstrapInfo = typeof core.ops.op_meow_node_bootstrap_info === "function"
  ? core.ops.op_meow_node_bootstrap_info()
  : { args: [], argv: [], cwd: "", mainModule: undefined, env: {} };
const hostPlatform = typeof core.ops.op_meow_host_platform === "function"
  ? core.ops.op_meow_host_platform()
  : (core.build?.os ?? "");
const hostNodeArch = typeof core.ops.op_meow_host_arch === "function"
  ? core.ops.op_meow_host_arch()
  : (core.build?.arch ?? "");
const hostDenoArch = hostNodeArch === "arm64"
  ? "aarch64"
  : hostNodeArch === "x64"
  ? "x86_64"
  : hostNodeArch;
if (core.build && (core.build.os === "unknown" || core.build.arch === "unknown")) {
  try {
    core.build.os = hostPlatform || core.build.os;
    core.build.arch = hostDenoArch || core.build.arch;
  } catch {
    // Some embedders freeze core.build; downstream process shims handle the fallback.
  }
}


function makeNodeEnoentError(name, path) {
  const syscall = name.endsWith("Sync") ? name.slice(0, -4) : name;
  const syscallNode = (syscall === "readFile" || syscall === "readTextFile" || syscall === "readFileSync" || syscall === "readTextFileSync" || syscall === "read" || syscall === "readSync") ? "open" : syscall;
  const error = new Error(`ENOENT: no such file or directory, ${syscallNode} '${path}'`);
  error.code = "ENOENT";
  error.errno = -2;
  error.syscall = syscallNode;
  error.path = path ? String(path) : "";
  return error;
}

function def(name, value, enumerable = false) {
  if (globalThis[name] !== undefined || value === undefined) return;
  Object.defineProperty(globalThis, name, {
    value,
    writable: true,
    enumerable,
    configurable: true,
  });
}

const processValue = processModule?.default ?? processModule;
let meowCwd = "";
if (bootstrapInfo.env && typeof bootstrapInfo.env === "object") {
  Object.defineProperty(globalThis, "__MEOW_BOOTSTRAP_ENV__", {
    value: { ...bootstrapInfo.env },
    writable: true,
    configurable: true,
  });
}
def("global", globalThis);
Object.defineProperty(globalThis, "process", {
  value: processValue,
  writable: true,
  configurable: true,
});

// === CLEAN-003 env overlay ===
// CJS now runs through Deno's node:module (the synthetic cjs.ts bridge was
// deleted). cjs.ts used to overlay meow's bootstrap env onto process.env; port
// that here so meow's controlled env -- e.g. the lifecycle vars
// npm_lifecycle_event / npm_lifecycle_script / INIT_CWD that `meow run <script>`
// injects -- lands on process.env for BOTH ESM and CJS modules.
if (bootstrapInfo.env && typeof bootstrapInfo.env === "object" && processValue && typeof processValue === "object") {
  try {
    const envTarget = processValue.env;
    if (envTarget && typeof envTarget === "object") {
      for (const key of Object.keys(bootstrapInfo.env)) {
        try { envTarget[key] = String(bootstrapInfo.env[key]); } catch (_e) { /* read-only env entry */ }
      }
    }
  } catch (_e) { /* process.env unavailable */ }
}
// === /CLEAN-003 env overlay ===

def("Buffer", bufferModule.Buffer);
def("performance", perfHooks.performance);
if (typeof globalThis.reportError !== "function") {
  globalThis.reportError = (error) => {
    console.error(error);
  };
}


if (globalThis.Deno) {
  let denoNs = globalThis.Deno;
  try {
    const replacement = Object.assign(
      Object.create(Object.getPrototypeOf(denoNs)),
      denoNs,
    );
    Object.defineProperty(globalThis, "Deno", {
      value: replacement,
      writable: true,
      configurable: true,
    });
    denoNs = replacement;
  } catch {
    // Keep the original Deno object when the runtime refuses replacement.
  }

  try {
    const denoOs = load("ext:deno_os/30_os.js");
    const denoSignals = load("ext:deno_os/40_signals.js");
    const denoFs = load("ext:deno_fs/30_fs.js");

    denoNs.args = Array.isArray(bootstrapInfo.args) ? [...bootstrapInfo.args] : [];
    if (denoNs.version === undefined) {
      denoNs.version = {};
    }
    if (bootstrapInfo.mainModule) {
      denoNs.mainModule = bootstrapInfo.mainModule;
    }


    if (core.build !== undefined) {
      denoNs.build = core.build;
    }

    for (const name of [
      "execPath",
      "uid",
      "gid",
      "hostname",
      "loadavg",
      "networkInterfaces",
      "osRelease",
      "systemMemoryInfo",
    ]) {
      if (denoOs[name] !== undefined) {
        denoNs[name] = denoOs[name];
      }
    }

    for (const name of [
      "cwd",
      "chdir",
      "stat",
      "statSync",
      "lstat",
      "lstatSync",
      "realPath",
      "realPathSync",
      "readLink",
      "readLinkSync",
      "readDir",
      "readDirSync",
      "mkdir",
      "mkdirSync",
      "remove",
      "removeSync",
      "rename",
      "renameSync",
      "copyFile",
      "copyFileSync",
      "link",
      "linkSync",
      "chmod",
      "chmodSync",
      "chown",
      "chownSync",
      "utime",
      "utimeSync",
      "symlink",
      "symlinkSync",
      "watchFs",
    ]) {
      if (denoFs[name] !== undefined) {
        denoNs[name] = denoFs[name];
      }
    }

    if (bootstrapInfo.cwd) {
      meowCwd = bootstrapInfo.cwd;
      denoNs.cwd = () => meowCwd;
      denoNs.chdir = (nextCwd) => {
        meowCwd = String(nextCwd);
      };
    }

    const looksMissingStat = (value) =>
      value == null ||
      (typeof value === "object" &&
        value !== null &&
        "isFile" in value &&
        "isDirectory" in value &&
        "isSymlink" in value &&
        value.isFile === false &&
        value.isDirectory === false &&
        value.isSymlink === false &&
        value.size === 0);
    const wrapSync = (name, label, missing = (value) => value == null) => {
      const fn = denoNs[name];
      if (typeof fn !== "function") return;
      Object.defineProperty(denoNs, name, {
        value: (...args) => {
          try {
            const value = fn(...args);
            if (missing(value)) {
              throw makeNodeEnoentError(name, args[0]);
            }
            return value;
          } catch (error) {
            if (error === undefined) {
              throw makeNodeEnoentError(name, args[0]);
            }
            if (error && typeof error === "object") {
              if (error.code === "ENOENT" || error.name === "NotFound" || String(error.message || "").includes("NotFound")) {
                try {
                  const syscall = name.endsWith("Sync") ? name.slice(0, -4) : name;
                  const syscallNode = (syscall === "readFile" || syscall === "readTextFile" || syscall === "readFileSync" || syscall === "readTextFileSync" || syscall === "read" || syscall === "readSync") ? "open" : syscall;
                  Object.defineProperty(error, "code", { value: "ENOENT", configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "errno", { value: -2, configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "syscall", { value: error.syscall || syscallNode, configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "path", { value: error.path || (args[0] ? String(args[0]) : ""), configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "message", {
                    value: `ENOENT: no such file or directory, ${error.syscall || syscallNode} '${error.path || (args[0] ? String(args[0]) : "")}'`,
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
        },
        writable: true,
        configurable: true,
      });
    };
    const wrapAsync = (name, label, missing = (value) => value == null) => {
      const fn = denoNs[name];
      if (typeof fn !== "function") return;
      const syncName = `${name}Sync`;
      const syncFn = denoNs[syncName];
      Object.defineProperty(denoNs, name, {
        value: async (...args) => {
          try {
            const value = await fn(...args);
            if (missing(value)) {
              throw makeNodeEnoentError(name, args[0]);
            }
            return value;
          } catch (error) {
            if (error === undefined) {
              throw makeNodeEnoentError(name, args[0]);
            }
            if (
              String(error?.message ?? error) === "invalid_argument" &&
              typeof syncFn === "function"
            ) {
              const value = syncFn(...args);
              if (missing(value)) {
                throw makeNodeEnoentError(name, args[0]);
              }
              return value;
            }
            if (error && typeof error === "object") {
              if (error.code === "ENOENT" || error.name === "NotFound" || String(error.message || "").includes("NotFound")) {
                try {
                  const syscall = name.endsWith("Sync") ? name.slice(0, -4) : name;
                  const syscallNode = (syscall === "readFile" || syscall === "readTextFile" || syscall === "readFileSync" || syscall === "readTextFileSync" || syscall === "read" || syscall === "readSync") ? "open" : syscall;
                  Object.defineProperty(error, "code", { value: "ENOENT", configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "errno", { value: -2, configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "syscall", { value: error.syscall || syscallNode, configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "path", { value: error.path || (args[0] ? String(args[0]) : ""), configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "message", {
                    value: `ENOENT: no such file or directory, ${error.syscall || syscallNode} '${error.path || (args[0] ? String(args[0]) : "")}'`,
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
        },
        writable: true,
        configurable: true,
      });
    };
    wrapSync("statSync", "statSync", looksMissingStat);
    wrapSync("lstatSync", "lstatSync", looksMissingStat);
    wrapSync("realPathSync", "realPathSync");
    wrapSync("readLinkSync", "readLinkSync");
    wrapSync("readFileSync", "readFileSync");
    wrapSync("readTextFileSync", "readTextFileSync");
    wrapAsync("stat", "stat", looksMissingStat);
    wrapAsync("lstat", "lstat", looksMissingStat);
    wrapAsync("realPath", "realPath");
    wrapAsync("readLink", "readLink");
    wrapAsync("readFile", "readFile");
    wrapAsync("readTextFile", "readTextFile");
    if (denoSignals.addSignalListener !== undefined) {
      denoNs.addSignalListener = denoSignals.addSignalListener;
    }
    if (denoSignals.removeSignalListener !== undefined) {
      denoNs.removeSignalListener = denoSignals.removeSignalListener;
    }
  } catch {
    // Fall through to the light shims below when a Deno support script is absent.
  }

  if (denoNs.env === undefined) {
    // Deno.env must reflect the LIVE per-invocation host env, not a value baked
    // at snapshot-build time. Read the hermetic env ops at access time; keep a
    // write-overlay so runtime set/delete behave like Node's in-process env
    // (callers that need children to inherit pass env explicitly to spawn/fork).
    const overlay = Object.create(null); // key -> value, or null = deleted
    const liveGet = (key) =>
      typeof core.ops.op_hermetic_env_get === "function"
        ? core.ops.op_hermetic_env_get(String(key))
        : undefined;
    const liveEntries = () => {
      const out = Object.create(null);
      const entries = typeof core.ops.op_hermetic_env_entries === "function"
        ? core.ops.op_hermetic_env_entries()
        : [];
      if (Array.isArray(entries)) {
        for (const entry of entries) {
          if (Array.isArray(entry) && entry.length === 2) {
            out[String(entry[0])] = String(entry[1]);
          }
        }
      }
      return out;
    };
    denoNs.env = {
      get(key) {
        const k = String(key);
        if (k in overlay) return overlay[k] === null ? undefined : overlay[k];
        return liveGet(k);
      },
      set(key, value) {
        overlay[String(key)] = String(value);
      },
      has(key) {
        return this.get(key) !== undefined;
      },
      delete(key) {
        overlay[String(key)] = null;
      },
      toObject() {
        const out = liveEntries();
        for (const k of Object.keys(overlay)) {
          if (overlay[k] === null) delete out[k];
          else out[k] = overlay[k];
        }
        return out;
      },
    };
  }
  if (typeof denoNs.exit !== "function") {
    denoNs.exit = (code = 0) => {
      core.ops.op_meow_record_process_exit(code | 0);
      throw { __meowProcessExit: true, code: code | 0 };
    };
  }
  if (denoNs.build === undefined) {
    denoNs.build = {
      os: core.build?.os ?? "",
      arch: core.build?.arch ?? "",
    };
  }
  const osModule = nodeOs.default ?? nodeOs;
  const normalizedPlatform =
    processValue?.env?.MEOW_NODE_PLATFORM ??
    (typeof core.ops.op_meow_host_platform === "function"
      ? core.ops.op_meow_host_platform()
      : (denoNs.build?.os === "windows" ? "win32" : (denoNs.build?.os ?? "")));
  const normalizedArch =
    processValue?.env?.MEOW_NODE_ARCH ??
    (typeof core.ops.op_meow_host_arch === "function"
      ? core.ops.op_meow_host_arch()
      : (core.build?.arch ?? ""));
  if (typeof osModule?.platform === "function" && osModule.platform() === "" && normalizedPlatform) {
    osModule.platform = () => normalizedPlatform;
  }
  if (typeof osModule?.arch === "function" && osModule.arch() === "" && normalizedArch) {
    osModule.arch = () => normalizedArch;
  }
  if (
    processValue &&
    typeof processValue === "object" &&
    normalizedPlatform &&
    (!processValue.platform || processValue.platform.length === 0)
  ) {
    Object.defineProperty(processValue, "platform", {
      value: normalizedPlatform,
      writable: true,
      configurable: true,
    });
  }
  if (
    processValue &&
    typeof processValue === "object" &&
    normalizedArch &&
    (!processValue.arch || processValue.arch.length === 0)
  ) {
    Object.defineProperty(processValue, "arch", {
      value: normalizedArch,
      writable: true,
      configurable: true,
    });
  }
  if (
    processValue &&
    typeof processValue === "object" &&
    typeof denoNs.execPath === "function" &&
    (!processValue.execPath || processValue.execPath.length === 0)
  ) {
    processValue.execPath = denoNs.execPath();
  }
  if (processValue && typeof processValue === "object" && denoNs.pid !== undefined) {
    Object.defineProperty(processValue, "pid", {
      value: denoNs.pid,
      writable: true,
      configurable: true,
    });
  }
  if (processValue && typeof processValue === "object" && denoNs.ppid !== undefined) {
    Object.defineProperty(processValue, "ppid", {
      value: denoNs.ppid,
      writable: true,
      configurable: true,
    });
  }
  if (typeof denoNs.watchFs !== "function") {
    denoNs.watchFs = (paths, _options = {}) => {
      const roots = (Array.isArray(paths) ? paths : [paths]).map(String);
      const queue = [];
      const waiters = [];
      let closed = false;

      const wake = (item) => {
        const waiter = waiters.shift();
        if (waiter) {
          waiter({ done: false, value: item });
        } else {
          queue.push(item);
        }
      };
      const finish = () => {
        closed = true;
        while (waiters.length) {
          waiters.shift()({ done: true, value: undefined });
        }
      };
      const snapshotOne = (path, out) => {
        let stat;
        try {
          stat = denoNs.statSync(path);
        } catch {
          return;
        }
        const key = String(path);
        out.set(key, `${Number(stat.mtime?.getTime?.() ?? 0)}:${Number(stat.size ?? 0)}:${stat.isDirectory ? "d" : "f"}`);
        if (stat.isDirectory !== true || typeof denoNs.readDirSync !== "function") {
          return;
        }
        try {
          for (const entry of denoNs.readDirSync(path)) {
            snapshotOne(`${key}/${entry.name}`, out);
          }
        } catch {
          // Directory disappeared or cannot be read; the parent change is enough.
        }
      };
      const takeSnapshot = () => {
        const out = new Map();
        for (const root of roots) {
          snapshotOne(root, out);
        }
        return out;
      };

      let previous = takeSnapshot();
      const timer = (nodeTimers.setInterval ?? setInterval)(() => {
        if (closed) {
          return;
        }
        const next = takeSnapshot();
        for (const [path, sig] of next) {
          const old = previous.get(path);
          if (old === undefined) {
            wake({ kind: "create", paths: [path] });
          } else if (old !== sig) {
            wake({ kind: "modify", paths: [path] });
          }
        }
        for (const path of previous.keys()) {
          if (!next.has(path)) {
            wake({ kind: "remove", paths: [path] });
          }
        }
        previous = next;
      }, 500);

      return {
        close() {
          if (!closed) {
            (nodeTimers.clearInterval ?? clearInterval)(timer);
            finish();
          }
        },
        ref() {},
        unref() {},
        [Symbol.asyncIterator]() {
          return this;
        },
        next() {
          if (queue.length) {
            return Promise.resolve({ done: false, value: queue.shift() });
          }
          if (closed) {
            return Promise.resolve({ done: true, value: undefined });
          }
          return new Promise((resolve) => waiters.push(resolve));
        },
      };
    };
  }
  if (typeof denoNs.memoryUsage !== "function") {
    denoNs.memoryUsage = () => {
      try {
        const v8Module = load("ext:deno_node/v8.ts");
        const stats =
          typeof v8Module?.getHeapStatistics === "function"
            ? v8Module.getHeapStatistics()
            : {};
        return {
          rss: Number(stats.total_physical_size ?? 0),
          heapTotal: Number(stats.total_heap_size ?? 0),
          heapUsed: Number(stats.used_heap_size ?? 0),
          external: Number(stats.external_memory ?? 0),
        };
      } catch {
        return { rss: 0, heapTotal: 0, heapUsed: 0, external: 0 };
      }
    };
  }
  if (denoNs.errors === undefined) {
    const errors = Object.create(null);
    denoNs.errors = new Proxy(errors, {
      get(target, prop) {
        if (typeof prop !== "string") {
          return target[prop];
        }
        if (!(prop in target)) {
          target[prop] = class extends Error {
            constructor(message = prop) {
              super(message);
              this.name = prop;
            }
          };
        }
        return target[prop];
      },
    });
  }
  if (typeof globalThis.addEventListener !== "function") {
    globalThis.addEventListener = () => {};
  }
  if (typeof globalThis.removeEventListener !== "function") {
    globalThis.removeEventListener = () => {};
  }
  if (typeof globalThis.dispatchEvent !== "function") {
    globalThis.dispatchEvent = () => true;
  }
}

// Node process bootstrap. The genuine deno_node `nodeBootstrap` installs the real
// process.argv / execPath / cwd, wires child IPC (process.send over NODE_CHANNEL_FD),
// and registers streamBaseState. It must run with the REAL per-invocation state.
//
// Under a V8 snapshot this module body executes at snapshot-BUILD time, where the
// bootstrap state is a placeholder (argv=["meow","snapshot-placeholder"], cwd="/").
// There we only WARM the lazy Node module graph (warmup:true), which keeps
// __bootstrapNodeProcess installed and leaves `initialized` false. The host then runs
// the real bootstrap at runtime via __meowRuntimeBootstrap(), after seeding the real
// state. In eager (no-snapshot) mode this body runs at runtime with real values, so we
// run the real bootstrap directly here.
function meowApplyDenoNamespace(info) {
  const denoNs = globalThis.Deno;
  if (denoNs) {
    try {
      denoNs.args = Array.isArray(info.args) ? [...info.args] : [];
      if (info.mainModule) {
        denoNs.mainModule = info.mainModule;
      }
    } catch {
      // Frozen Deno namespace; process shims fall back to placeholder args.
    }
  }
  if (info.cwd) {
    meowCwd = info.cwd;
    if (processValue && typeof processValue === "object") {
      try {
        // process.cwd captured deno_fs.cwd at module-load (baked stale under a
        // snapshot); route it through Deno.cwd so it tracks meow's controlled cwd.
        processValue.cwd = () => globalThis.Deno.cwd();
      } catch {
        // process.cwd not reassignable
      }
    }
  }
  if (info.env && typeof info.env === "object" && processValue && processValue.env) {
    for (const key of Object.keys(info.env)) {
      try {
        processValue.env[key] = String(info.env[key]);
      } catch {
        // read-only env entry
      }
    }
  }
}

function meowRunNodeBootstrap(info, warmup) {
  if (typeof globalThis.nodeBootstrap !== "function") return;
  try {
    globalThis.nodeBootstrap({
      usesLocalNodeModulesDir: true,
      argv0: info.argv?.[0] ?? "meow",
      runningOnMainThread: true,
      nodeDebug: "",
      warmup,
      moduleSpecifier: info.mainModule ?? null,
    });
  } catch (error) {
    if (!String(error?.message ?? "").includes("already initialized")) {
      throw error;
    }
  }
}

// Invoked by the host (meow-runtime refresh_bootstrap_state) at runtime when booting
// from a snapshot: re-read the real bootstrap state and run the genuine Node bootstrap.
globalThis.__meowRuntimeBootstrap = function () {
  const info = typeof core.ops.op_meow_node_bootstrap_info === "function"
    ? core.ops.op_meow_node_bootstrap_info()
    : null;
  if (!info) return;
  meowApplyDenoNamespace(info);
  meowRunNodeBootstrap(info, false);
};

  meowRunNodeBootstrap(bootstrapInfo, bootstrapInfo.env?.MEOW_SNAPSHOT_BUILD === "1");


if (
  processValue &&
  typeof processValue === "object" &&
  Array.isArray(processValue.argv) &&
  typeof bootstrapInfo.argv?.[0] === "string" &&
  bootstrapInfo.argv[0].length > 0
) {
  processValue.argv[0] = bootstrapInfo.argv[0];
}

if (
  processValue &&
  typeof processValue === "object" &&
  bootstrapInfo.cwd
) {
  let processCwd = bootstrapInfo.cwd;
  processValue.cwd = () => processCwd;
  processValue.chdir = (nextCwd) => {
    processCwd = String(nextCwd);
  };
}


for (const target of [nodeFsModule.default]) {
  if (!target || typeof target !== "object") continue;
  const looksMissingStat = (value) =>
    value &&
    value.isFile === false &&
    value.isDirectory === false &&
    value.isSymlink === false &&
    value.size === 0;
  for (const name of ["statSync", "lstatSync", "readFileSync", "readTextFileSync"]) {
    if (typeof target[name] === "function") {
      const original = target[name].bind(target);
      const isStat = name.includes("stat");
      try {
        target[name] = (...args) => {
          try {
            const value = original(...args);
            if (isStat && looksMissingStat(value)) {
              throw makeNodeEnoentError(name, args[0]);
            }
            return value;
          } catch (error) {
            if (error && typeof error === "object") {
              if (error.code === "ENOENT" || error.name === "NotFound" || String(error.message || "").includes("NotFound")) {
                try {
                  const syscall = name.endsWith("Sync") ? name.slice(0, -4) : name;
                  const syscallNode = (syscall === "readFile" || syscall === "readTextFile" || syscall === "readFileSync" || syscall === "readTextFileSync" || syscall === "read" || syscall === "readSync") ? "open" : syscall;
                  Object.defineProperty(error, "code", { value: "ENOENT", configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "errno", { value: -2, configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "syscall", { value: error.syscall || syscallNode, configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "path", { value: error.path || (args[0] ? String(args[0]) : ""), configurable: true, writable: true, enumerable: true });
                  Object.defineProperty(error, "message", {
                    value: `ENOENT: no such file or directory, ${error.syscall || syscallNode} '${error.path || (args[0] ? String(args[0]) : "")}'`,
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
      } catch {
        // ESM namespace / frozen builtin export.
      }
    }
  }
}


for (const target of [nodeFsModule.default]) {
  if (
    !target ||
    typeof target.watch !== "function" ||
    target.watch.__meowWatchKickPatch === true
  ) continue;
  const originalWatch = target.watch.bind(target);
  target.watch = (path, options, listener) => {
    let callback = listener;
    if (typeof options === "function") {
      callback = options;
      options = undefined;
    }
    const watcher = originalWatch(path, options ?? {}, callback);
    const name = String(path).split(/[\\/]/).pop() || "";
    const timer = (nodeTimers.setTimeout ?? setTimeout)(() => {
      try {
        if (typeof callback === "function") {
          callback("change", name);
        }
        if (typeof watcher?.emit === "function") {
          watcher.emit("change", "change", name);
        }
      } catch {
        // Watch priming is best-effort; real watcher errors still surface normally.
      }
    }, 0);
    if (watcher && typeof watcher.close === "function") {
      const originalClose = watcher.close.bind(watcher);
      watcher.close = () => {
        (nodeTimers.clearTimeout ?? clearTimeout)(timer);
        return originalClose();
      };
    }
    return watcher;
  };
  target.watch.__meowWatchKickPatch = true;
}

if (
  processValue?.env?.NODE_CHANNEL_FD !== undefined &&
  !(
    processValue?.env?.MEOW_IPC_PARENT_TO_CHILD &&
    processValue?.env?.MEOW_IPC_CHILD_TO_PARENT
  )
) {
  try {
    core.loadExtScript("ext:deno_node/child_process.ts");
    if (typeof internals.__setupChildProcessIpcChannel === "function") {
      internals.__setupChildProcessIpcChannel();
    }
  } catch {
    // Child IPC is optional outside forked child processes.
  }
}

const netModule = nodeNet.default ?? nodeNet;
const serverPrototype = netModule.Server?.prototype;
if (
  serverPrototype &&
  typeof serverPrototype.listen === "function" &&
  serverPrototype.listen.__meowListeningPatch !== true
) {
  const originalListen = serverPrototype.listen;
  const patchedListen = function (...args) {
    let emitted = false;
    if (typeof this.once === "function") {
      this.once("listening", () => {
        emitted = true;
      });
    }
    const result = originalListen.apply(this, args);
    (nodeTimers.setTimeout ?? setTimeout)(() => {
      if (this.listening && !emitted && typeof this.emit === "function") {
        emitted = true;
        this.emit("listening");
      }
    }, 0);
    return result;
  };
  patchedListen.__meowListeningPatch = true;
  serverPrototype.listen = patchedListen;
}



if (
  typeof processValue?.emit === "function" &&
  processValue?.env?.MEOW_IPC_PARENT_TO_CHILD &&
  processValue?.env?.MEOW_IPC_CHILD_TO_PARENT
) {
  const fs = nodeFsModule.default ?? nodeFsModule;
  const parentToChild = String(processValue.env.MEOW_IPC_PARENT_TO_CHILD);
  const childToParent = String(processValue.env.MEOW_IPC_CHILD_TO_PARENT);
  const debugLog = processValue.env.MEOW_DEBUG_IPC;
  if (processValue.env.MEOW_FORK_PATCH_JSON) {
    try {
      Object.assign(processValue.env, JSON.parse(String(processValue.env.MEOW_FORK_PATCH_JSON)));
    } catch {
      // Ignore malformed fork patch payloads.
    }
  }
  let consumed = 0;
  let buffered = "";
  if (!processValue.env.NEXT_PRIVATE_WORKER && processValue.env.MEOW_FORK_NEXT_PRIVATE_WORKER) {
    processValue.env.NEXT_PRIVATE_WORKER = String(processValue.env.MEOW_FORK_NEXT_PRIVATE_WORKER);
  }
  if (debugLog) {
    fs.appendFileSync(
      String(debugLog),
      `child-setup worker=${String(processValue.env.NEXT_PRIVATE_WORKER)} send=${typeof processValue.send} emit=${typeof processValue.emit} on=${typeof processValue.on}\n`,
    );
  }
  let sendFn = (message) => {
    const payload = `${JSON.stringify(message)}\n`;
    if (debugLog) {
      fs.appendFileSync(String(debugLog), `child->parent ${payload}`);
    }
    fs.appendFileSync(childToParent, payload);
    return true;
  };
  Object.defineProperty(processValue, "send", {
    get() {
      if (debugLog) {
        fs.appendFileSync(String(debugLog), "child-read-send\n");
      }
      return sendFn;
    },
    set(value) {
      if (debugLog) {
        fs.appendFileSync(String(debugLog), `child-set-send ${typeof value}\n`);
      }
      sendFn = value;
    },
    configurable: true,
  });
  processValue.connected = true;
  processValue.disconnect = () => {
    processValue.connected = false;
  };
  (nodeTimers.setInterval ?? setInterval)(() => {
    if (!processValue.connected) {
      return;
    }
    let text;
    try {
      text = fs.readFileSync(parentToChild, "utf8");
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
        fs.appendFileSync(String(debugLog), `child<-parent ${line}\n`);
      }
      try {
        processValue.emit("message", JSON.parse(line));
      } catch {
        // Ignore malformed mailbox writes from parent code.
      }
    }
  }, 20);
}


// meow is a Node replacement: the GLOBAL timer functions must return Node `Timeout`
// objects (with .unref()/.ref()/.refresh()), not deno_web's numeric handles. The web
// bootstrap already defined setTimeout/clearTimeout, so def() (a no-op when the global
// already exists) would leave the numeric web timers in place -> `setTimeout(...).unref`
// is undefined and webpack/Next.js crash ("unref is not a function") or hang. Force-install
// the node:timers versions over any pre-existing web globals.
for (const name of ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "setImmediate", "clearImmediate"]) {
  if (nodeTimers[name] !== undefined) {
    Object.defineProperty(globalThis, name, {
      value: nodeTimers[name],
      writable: true,
      enumerable: true,
      configurable: true,
    });
  }
}

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
  "TextEncoderStream",
  "TextDecoderStream",
  "CompressionStream",
  "DecompressionStream",
]) {
  def(name, nodeStreamWeb[name]);
}


try {
  const domException = load("ext:deno_web/01_dom_exception.js");
  def("DOMException", domException.DOMException);

  const encoding = load("ext:deno_web/08_text_encoding.js");
  def("TextEncoder", encoding.TextEncoder);
  def("TextDecoder", encoding.TextDecoder);

  const abortSignal = load("ext:deno_web/03_abort_signal.js");
  def("AbortController", abortSignal.AbortController);
  def("AbortSignal", abortSignal.AbortSignal);

  const file = load("ext:deno_web/09_file.js");
  def("Blob", file.Blob);
  def("File", file.File);

  const url = load("ext:deno_web/00_url.js");
  def("URL", url.URL);
  def("URLSearchParams", url.URLSearchParams);

  const urlPattern = load("ext:deno_web/01_urlpattern.js");
  def("URLPattern", urlPattern.URLPattern);

  const base64 = load("ext:deno_web/05_base64.js");
  def("atob", base64.atob);
  def("btoa", base64.btoa);

  const structured = load("ext:deno_web/02_structured_clone.js");
  def("structuredClone", structured.structuredClone);

  const event = load("ext:deno_web/02_event.js");
  def("Event", event.Event);
  def("EventTarget", event.EventTarget);
  def("CustomEvent", event.CustomEvent);
  def("MessageEvent", event.MessageEvent);
  def("ErrorEvent", event.ErrorEvent);
  def("CloseEvent", event.CloseEvent);
  def("ProgressEvent", event.ProgressEvent);

  const compression = load("ext:deno_web/14_compression.js");
  def("CompressionStream", compression.CompressionStream);
  def("DecompressionStream", compression.DecompressionStream);

  const performanceModule = load("ext:deno_web/15_performance.js");
  def("Performance", performanceModule.Performance);
  def("PerformanceEntry", performanceModule.PerformanceEntry);
  def("PerformanceMark", performanceModule.PerformanceMark);
  def("PerformanceMeasure", performanceModule.PerformanceMeasure);
  def("PerformanceObserver", performanceModule.PerformanceObserver);
  def("PerformanceObserverEntryList", performanceModule.PerformanceObserverEntryList);
  def("performance", performanceModule.performance);
} catch {
  // Optional web globals depend on the current Deno pin.
}


// === RT-007 compat shims (CLEAN-003) ===
// CJS now executes through Deno's upstream node:module / NodeRequireLoader, so
// require("fs") / require("constants") yield Deno's node:* builtins directly.
// These two shims close the remaining Node-shape gaps the old synthetic wrapper
// used to paper over.

// 1) ENOENT guard for node:fs sync reads. Deno's node:fs readFileSync can signal
//    a missing path by throwing `undefined`; the earlier wrap only normalizes
//    object-shaped errors, so an `undefined` throw escaped raw. Re-wrap the sync
//    read/stat surface so a missing file always surfaces a Node ENOENT error.
for (const fsTarget of [nodeFsModule.default ?? nodeFsModule]) {
  if (!fsTarget || typeof fsTarget !== "object") continue;
  for (const name of ["statSync", "lstatSync", "readFileSync", "readTextFileSync"]) {
    const fn = fsTarget[name];
    if (typeof fn !== "function" || fn.__meowEnoentGuard === true) continue;
    const guarded = function (...args) {
      try {
        return fn.apply(fsTarget, args);
      } catch (error) {
        if (error === undefined) {
          throw makeNodeEnoentError(name, args[0]);
        }
        throw error;
      }
    };
    guarded.__meowEnoentGuard = true;
    try {
      fsTarget[name] = guarded;
    } catch {
      // Leave non-writable members alone.
    }
  }
}

// 2) Nested groups on node:constants. Deno's node:constants is a flat merge of
//    fs/os/crypto/zlib constants; Node also exposes `.fs` (and `.os`/`.crypto`)
//    sub-objects. Re-publish at least the fs access/open group so
//    `constants.fs.R_OK === constants.R_OK`.
{
  const constantsValue = nodeConstantsModule.default ?? nodeConstantsModule;
  if (constantsValue && typeof constantsValue === "object" && constantsValue.fs === undefined) {
    const fsConsts = (nodeFsModule.default ?? nodeFsModule)?.constants;
    let group;
    if (fsConsts && typeof fsConsts === "object") {
      group = fsConsts;
    } else {
      group = {};
      for (const k of [
        "F_OK", "R_OK", "W_OK", "X_OK",
        "O_RDONLY", "O_WRONLY", "O_RDWR", "O_CREAT", "O_EXCL",
        "O_TRUNC", "O_APPEND", "O_DIRECTORY", "O_NOFOLLOW", "O_SYNC",
      ]) {
        if (typeof constantsValue[k] === "number") {
          group[k] = constantsValue[k];
        }
      }
    }
    try {
      constantsValue.fs = group;
    } catch {
      // Frozen builtin export; nothing else to do.
    }
  }
}

// === Deno error-class registration (meow Node-compat ROOT FIX) ===
// deno_core pre-registers only the 6 JS-builtin error classes, so
// to_v8_error() returns `undefined` for Deno's own classes (NotFound,
// AlreadyExists, ...). Sync ops then throw `undefined`; async ops reject with
// `undefined`, which makes deno_core's __opRejectHandler call
// Error.captureStackTrace(undefined) and surface a misleading
// "TypeError: invalid_argument". Registering the Deno.errors.* builders makes
// both paths produce faithful errors -- deno_error attaches the errno `code`
// (ENOENT/EEXIST/...) as an additional property -- which deno_node maps to the
// right Node error. Deno.errors props are non-enumerable, so register by
// explicit name (matching deno_error's std::io::Error get_class output).
try {
  const __D = globalThis.Deno;
  const __reg = (typeof core !== "undefined" && core.registerErrorClass)
    || (__D && __D.core && __D.core.registerErrorClass);
  if (__D && __D.errors && typeof __reg === "function") {
    const __classes = [
      "NotFound", "PermissionDenied", "ConnectionRefused", "ConnectionReset",
      "ConnectionAborted", "NotConnected", "AddrInUse", "AddrNotAvailable",
      "BrokenPipe", "AlreadyExists", "InvalidData", "TimedOut", "Interrupted",
      "WriteZero", "WouldBlock", "UnexpectedEof", "BadResource", "Http",
      "Busy", "NotSupported", "FilesystemLoop", "IsADirectory",
      "NetworkUnreachable", "NotADirectory", "NotCapable",
    ];
    for (const __name of __classes) {
      const __ctor = __D.errors[__name];
      if (typeof __ctor === "function") {
        try { __reg(__name, __ctor); } catch (_) { /* already registered */ }
      }
    }
  }
} catch (_) { /* harness shims still cover common cases */ }
