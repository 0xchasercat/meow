import { core, internals } from "ext:core/mod.js";
import processModule from "node:process";
import * as bufferModule from "node:buffer";
import * as perfHooks from "node:perf_hooks";
import * as nodeTimers from "node:timers";
import * as nodeStreamWeb from "node:stream/web";
import * as nodeFsModule from "node:fs";
import * as nodeNet from "node:net";
import * as nodeHttp from "node:http";
import * as nodeHttpIncoming from "node:_http_incoming";
import * as nodeOs from "node:os";
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
      let currentCwd = bootstrapInfo.cwd;
      denoNs.cwd = () => currentCwd;
      denoNs.chdir = (nextCwd) => {
        currentCwd = String(nextCwd);
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
    const envObject = Object.create(null);
    const entries = typeof core.ops.op_hermetic_env_entries === "function"
      ? core.ops.op_hermetic_env_entries()
      : [];
    if (Array.isArray(entries)) {
      for (const entry of entries) {
        if (Array.isArray(entry) && entry.length === 2) {
          envObject[String(entry[0])] = String(entry[1]);
        }
      }
    }
    denoNs.env = {
      get(key) {
        return envObject[String(key)];
      },
      set(key, value) {
        envObject[String(key)] = String(value);
      },
      has(key) {
        return Object.prototype.hasOwnProperty.call(envObject, String(key));
      },
      delete(key) {
        delete envObject[String(key)];
      },
      toObject() {
        return { ...envObject };
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

if (typeof globalThis.nodeBootstrap === "function") {
  try {
    globalThis.nodeBootstrap({
      usesLocalNodeModulesDir: true,
      argv0: bootstrapInfo.argv?.[0] ?? "meow",
      runningOnMainThread: true,
      nodeDebug: "",
      warmup: false,
      moduleSpecifier: bootstrapInfo.mainModule ?? null,
    });
  } catch (error) {
    if (!String(error?.message ?? "").includes("already initialized")) {
      throw error;
    }
  }
}
if (typeof core.ops.op_stream_base_register_state === "function") {
  try {
    const { streamBaseState } = load("ext:deno_node/internal_binding/stream_wrap.ts");
    core.ops.op_stream_base_register_state(streamBaseState);
  } catch {
    // If stream-wrap bootstrap is unavailable, keep the runtime alive; net sockets may still fail.
  }
}


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

const serverEmitPrototype = netModule.Server?.prototype;
if (
  serverEmitPrototype &&
  typeof serverEmitPrototype.emit === "function" &&
  serverEmitPrototype.emit.__meowHttpConsumePatch !== true
) {
  const originalEmit = serverEmitPrototype.emit;
  const patchedEmit = function (event, ...args) {
    if (event === "connection") {
      return originalEmit.call(this, event, ...args);
    }
    if (event === "request") {
      const req = args[0];
      const result = originalEmit.call(this, event, ...args);
      if (
        req &&
        req.headers?.["content-length"] === undefined &&
        req.headers?.["transfer-encoding"] === undefined &&
        typeof req.push === "function"
      ) {
        (processValue.nextTick ?? queueMicrotask)(() => {
          if (req.complete !== true) {
            req.complete = true;
            req.push(null);
          }
          if (req.__meowEmptyEndEmitted !== true && typeof req.emit === "function") {
            req.__meowEmptyEndEmitted = true;
            req.emit("end");
          }
        });
      }
      return result;
    }
    return originalEmit.call(this, event, ...args);
  };
  patchedEmit.__meowHttpConsumePatch = true;
  serverEmitPrototype.emit = patchedEmit;
}

const httpModule = nodeHttp.default ?? nodeHttp;
const httpServerPrototype = httpModule.Server?.prototype;
if (
  httpServerPrototype &&
  typeof httpServerPrototype.emit === "function" &&
  httpServerPrototype.emit.__meowEmptyRequestPatch !== true
) {
  const originalHttpEmit = httpServerPrototype.emit;
  const patchedHttpEmit = function (event, ...args) {
    if (event !== "request") {
      return originalHttpEmit.call(this, event, ...args);
    }
    const req = args[0];
    const result = originalHttpEmit.call(this, event, ...args);
    if (
      req &&
      req.headers?.["content-length"] === undefined &&
      req.headers?.["transfer-encoding"] === undefined &&
      typeof req.push === "function"
    ) {
      (processValue.nextTick ?? queueMicrotask)(() => {
        if (req.complete !== true) {
          req.complete = true;
          req.push(null);
        }
        if (req.__meowEmptyEndEmitted !== true && typeof req.emit === "function") {
          req.__meowEmptyEndEmitted = true;
          req.emit("end");
        }
      });
    }
    return result;
  };
  patchedHttpEmit.__meowEmptyRequestPatch = true;
  httpServerPrototype.emit = patchedHttpEmit;
}
const responsePrototype = httpModule.ServerResponse?.prototype;
{
  const IncomingMessage = nodeHttpIncoming.IncomingMessage;
  const incomingPrototype = IncomingMessage?.prototype;
  if (
    incomingPrototype &&
    typeof incomingPrototype.on === "function" &&
    incomingPrototype.on.__meowEmptyEndPatch !== true
  ) {
    const originalOn = incomingPrototype.on;
    const patchedOn = function (event, listener) {
      const isKnownEmpty =
        this.headers?.["content-length"] === undefined &&
        this.headers?.["transfer-encoding"] === undefined;
      if (event === "data" && isKnownEmpty) {
        this.complete = true;
        return this;
      }
      const result = originalOn.call(this, event, listener);
      if (event === "end" && typeof listener === "function" && isKnownEmpty) {
        (nodeTimers.setTimeout ?? setTimeout)(() => {
          this.__meowEmptyEndEmitted = true;
          this.complete = true;
          listener.call(this);
        }, 0);
      }
      return result;
    };
    patchedOn.__meowEmptyEndPatch = true;
    incomingPrototype.on = patchedOn;
    incomingPrototype.addListener = patchedOn;
    if (
      typeof incomingPrototype._read === "function" &&
      incomingPrototype._read.__meowEmptyReadPatch !== true
    ) {
      const originalRead = incomingPrototype._read;
      const patchedRead = function (size) {
        if (
          this.headers?.["content-length"] === undefined &&
          this.headers?.["transfer-encoding"] === undefined
        ) {
          this.complete = true;
          this.push(null);
          return;
        }
        return originalRead.call(this, size);
      };
      patchedRead.__meowEmptyReadPatch = true;
      incomingPrototype._read = patchedRead;
    }
  }
}

if (
  responsePrototype &&
  typeof responsePrototype.end === "function" &&
  responsePrototype.end.__meowDirectEndPatch !== true
) {
  const originalEnd = responsePrototype.end;
  const patchedEnd = function (chunk, encoding, callback) {
    if (typeof chunk === "function") {
      return originalEnd.call(this, null, null, chunk);
    }
    if (typeof encoding === "function") {
      callback = encoding;
      encoding = null;
    }
    if (chunk !== null && chunk !== undefined && chunk !== "") {
      this.write(chunk, encoding);
      (nodeTimers.setTimeout ?? setTimeout)(() => {
        originalEnd.call(this, null, null, callback);
      }, 0);
      return this;
    }
    return originalEnd.call(this, chunk, encoding, callback);
  };
  patchedEnd.__meowDirectEndPatch = true;
  responsePrototype.end = patchedEnd;
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


for (const name of ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "setImmediate", "clearImmediate"]) {
  def(name, nodeTimers[name], true);
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
