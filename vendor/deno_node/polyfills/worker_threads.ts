// Copyright 2018-2026 the Deno authors. MIT license.
// Copyright Joyent and Node contributors. All rights reserved. MIT license.
//
// meow-native `node:worker_threads`.
//
// meow runs each worker on its OWN OS thread with its OWN `deno_core::JsRuntime`
// (V8 isolate) + tokio runtime (see `crates/cli/src/cli/commands/worker.rs`).
// Determinism survives because every worker gets a clone of the run's hermetic
// config (same frozen clock + seed). Cross-isolate values are structured-cloned
// (`core.serialize`/`core.deserialize`); `SharedArrayBuffer`s are shared via a
// process-global store so `Atomics.wait`/`notify` work across threads, and
// `MessagePort`s transfer by carrying a registry id (`op_meow_port_*`).
//
// This replaces Deno's worker_threads polyfill, which is welded to
// `deno_runtime`'s worker host (`op_create_worker`, `op_host_*`) and newer
// `deno_node` worker ops that meow does not vendor. The host/worker ops below
// are meow-owned (`op_meow_worker_*`).
(function () {
  const { core, internals, primordials } = __bootstrap;
  // NOTE: the `op_meow_worker_*` ops are registered by a RUNTIME extension
  // (`commands::worker`), not baked into the V8 snapshot -- this module IS in the
  // snapshot, so destructuring them here would capture `undefined` (the snapshot's
  // build.rs only knows the dependency-crate extensions). Reach them lazily via
  // `core.ops.<op>` at call time, when the runtime extension has registered them.
  // `op_worker_threads_filename` IS a snapshot op (deno_node), so destructure it.
  const { op_worker_threads_filename } = core.ops;
  const ops = core.ops;
  const {
    ObjectAssign,
    ObjectDefineProperty,
    PromisePrototypeThen,
    PromiseResolve,
    SafeMap,
    Symbol,
    TypeError,
    TypedArrayPrototypeSubarray,
  } = primordials;

  const { EventEmitter } = core.loadExtScript("ext:deno_node/_events.mjs");

  const serialize = (value) => core.serialize(value);
  const deserialize = (bytes) => core.deserialize(bytes);

  // `op_meow_worker_host_recv` returns a framed Uint8Array: byte 0 is the tag
  // (CTRL_MESSAGE | CTRL_ERROR | CTRL_ONLINE), the rest is the serialized
  // payload. An EMPTY buffer means the worker isolate has exited. (Framing
  // avoids fragile option-of-tuple op return shapes across the op2 boundary.)
  // CTRL_ONLINE has no payload; it fires the `'online'` event the moment the
  // worker isolate starts (Node semantics -- pools wait for it before dispatching
  // work, so it must NOT be tied to the first message).
  const CTRL_MESSAGE = 0;
  const CTRL_ERROR = 1;
  const CTRL_ONLINE = 2;

  // The live module exports. `__initWorkerThreads` mutates the bootstrap fields
  // (isMainThread / threadId / parentPort / workerData) in place; the synthetic
  // ESM dispatch derives export names from `Object.keys(exportsObj)`.
  const exportsObj = { __proto__: null };

  // ----------------------------- host side -----------------------------

  class Worker extends EventEmitter {
    #id;
    #terminated = false;
    #onlineEmitted = false;

    constructor(specifier, options = {}) {
      super();
      const isEval = options.eval === true;
      let spec;
      if (isEval) {
        // `new Worker(code, { eval: true })`: the first argument is inline
        // source code, NOT a path. Pass it through untouched; the worker driver
        // materialises + runs it (do NOT resolve it as a filename).
        spec = typeof specifier === "string" ? specifier : String(specifier);
      } else {
        if (typeof specifier === "string") {
          spec = specifier;
        } else if (specifier && typeof specifier.href === "string") {
          spec = specifier.href; // URL
        } else {
          throw new TypeError(
            "The 'specifier' argument must be a string or URL.",
          );
        }
        // Modern TS projects pass a `.js` specifier for a `.ts` source on disk;
        // reuse the resolver's source-fallback (also normalizes a file URL).
        spec = op_worker_threads_filename(spec) ?? spec;
      }

      const workerData = options.workerData === undefined
        ? null
        : options.workerData;

      this.#id = ops.op_meow_worker_create(
        spec,
        serializeForPort(workerData, options.transferList),
        isEval,
      );
      this.threadId = this.#id;
      this.resourceLimits = { ...(options.resourceLimits ?? {}) };
      this.#pump();
    }

    #pump() {
      const step = () => {
        if (this.#terminated) return;
        PromisePrototypeThen(
          ops.op_meow_worker_host_recv(this.#id),
          (frame) => {
            if (this.#terminated) return;
            if (frame.length === 0) {
              this.#terminated = true;
              this.emit("exit", 0);
              return;
            }
            const tag = frame[0];
            if (tag === CTRL_ONLINE) {
              // Explicit start signal from the worker driver.
              if (!this.#onlineEmitted) {
                this.#onlineEmitted = true;
                this.emit("online");
              }
              step();
              return;
            }
            // Safety net: if a message/error somehow precedes the explicit online
            // frame, still emit `'online'` first (Node emits it before any event).
            if (!this.#onlineEmitted) {
              this.#onlineEmitted = true;
              this.emit("online");
            }
            const payload = TypedArrayPrototypeSubarray(frame, 1);
            if (tag === CTRL_ERROR) {
              // Errors arrive as a UTF-8 string (the Rust driver can't structured-
              // clone), messages as a `core.serialize`d value.
              this.emit("error", new Error(core.decode(payload)));
            } else {
              this.emit("message", deserializeForPort(payload));
            }
            step();
          },
          (err) => {
            if (this.#terminated) return;
            this.#terminated = true;
            this.emit("error", err);
            this.emit("exit", 1);
          },
        );
      };
      step();
    }

    postMessage(value, transferList) {
      if (this.#terminated) return;
      ops.op_meow_worker_host_post(
        this.#id,
        serializeForPort(value, transferList),
      );
    }

    terminate() {
      if (this.#terminated) return PromiseResolve(1);
      this.#terminated = true;
      ops.op_meow_worker_terminate(this.#id);
      this.emit("exit", 1);
      return PromiseResolve(1);
    }

    // Cooperative single-thread workers have no separate libuv loop to hold
    // open, so ref/unref are no-ops.
    ref() {}
    unref() {}
  }

  // ---------------------------- worker side ----------------------------

  function makeParentPort() {
    const port = new EventEmitter();
    let closed = false;
    port.postMessage = (value, transferList) => {
      if (!closed) ops.op_meow_worker_post(serializeForPort(value, transferList));
    };
    port.close = () => {
      closed = true;
    };
    port.start = () => {};
    port.ref = () => {};
    port.unref = () => {};
    // Pump host->worker messages; dispatch as EventEmitter "message" and the
    // web-style `onmessage` for compatibility.
    const step = () => {
      if (closed) return;
      PromisePrototypeThen(ops.op_meow_worker_recv(), (bytes) => {
        if (closed) return;
        if (bytes.length === 0) {
          closed = true;
          return; // host terminated us
        }
        const value = deserializeForPort(bytes);
        port.emit("message", value);
        if (typeof port.onmessage === "function") port.onmessage({ data: value });
        step();
      });
    };
    step();
    return port;
  }

  // ----------------------- bootstrap-time state -----------------------
  // `01_require.js` calls this for the main isolate (runningOnMainThread = true)
  // and for each worker isolate (false; the worker driver passes the id).
  internals.__initWorkerThreads = (
    runningOnMainThread,
    workerId,
    _maybeWorkerMetadata,
    _moduleSpecifier,
  ) => {
    const isMainThread = !!runningOnMainThread;
    internals.__isWorkerThread = !isMainThread;
    exportsObj.isMainThread = isMainThread;
    exportsObj.threadId = workerId ?? 0;

    if (!isMainThread) {
      const dataBytes = ops.op_meow_worker_data();
      exportsObj.workerData = dataBytes && dataBytes.length > 0
        ? deserializeForPort(dataBytes)
        : null;
      exportsObj.parentPort = makeParentPort();
    }
  };

  // The worker driver (Rust, `commands::worker`) calls this on the worker isolate
  // right after the normal Node bootstrap (which initialized as a main thread) to
  // flip it into worker mode + wire `parentPort`/`workerData`. Exposed on
  // globalThis because the driver runs a top-level script with no `internals` in
  // scope.
  globalThis.__meowInitWorkerThread = (workerId, moduleSpecifier) => {
    internals.__initWorkerThreads(false, workerId, null, moduleSpecifier);
  };

  // ------------------- transferable MessagePort / MessageChannel -------------
  // A `MessagePort` is backed by a PROCESS-GLOBAL registry endpoint keyed by an
  // integer id (Rust `op_meow_port_*`). Because the endpoint lives in the
  // registry by id, transferring a port to another isolate is just carrying its
  // id across the structured clone: `serializeForPort` walks the value replacing
  // each `MessagePort` with a `{ [PORT_MARKER]: id }` placeholder before
  // `core.serialize`, and `deserializeForPort` rebuilds ports from placeholders
  // after `core.deserialize`. `SharedArrayBuffer`/`ArrayBuffer` are left to
  // `core.serialize` (shared / copied); we recurse only plain objects + arrays.
  const PORT_MARKER = "__meow_transferred_port__";
  const kPortId = Symbol("meowPortId");
  const kNeuter = Symbol("meowNeuter");

  const isPlainContainer = (v) => {
    if (Array.isArray(v)) return true;
    const proto = Object.getPrototypeOf(v);
    return proto === Object.prototype || proto === null;
  };

  const hasPort = (value, seen) => {
    if (value instanceof MessagePort) return true;
    if (value === null || typeof value !== "object" || !isPlainContainer(value)) {
      return false;
    }
    if (seen.has(value)) return false;
    seen.add(value);
    if (Array.isArray(value)) {
      for (let i = 0; i < value.length; i++) {
        if (hasPort(value[i], seen)) return true;
      }
    } else {
      for (const key of Object.keys(value)) {
        if (hasPort(value[key], seen)) return true;
      }
    }
    return false;
  };

  const replacePorts = (value, seen) => {
    if (value instanceof MessagePort) {
      return { [PORT_MARKER]: value[kPortId] };
    }
    if (value === null || typeof value !== "object" || !isPlainContainer(value)) {
      return value;
    }
    if (seen.has(value)) return seen.get(value);
    const out = Array.isArray(value) ? [] : {};
    seen.set(value, out);
    if (Array.isArray(value)) {
      for (let i = 0; i < value.length; i++) {
        out[i] = replacePorts(value[i], seen);
      }
    } else {
      for (const key of Object.keys(value)) {
        out[key] = replacePorts(value[key], seen);
      }
    }
    return out;
  };

  const isPortPlaceholder = (v) =>
    v !== null && typeof v === "object" &&
    typeof v[PORT_MARKER] === "number" && Object.keys(v).length === 1;

  const restorePorts = (value, seen) => {
    if (value === null || typeof value !== "object") return value;
    if (isPortPlaceholder(value)) return new MessagePort(value[PORT_MARKER]);
    if (!isPlainContainer(value)) return value;
    if (seen.has(value)) return value;
    seen.add(value);
    if (Array.isArray(value)) {
      for (let i = 0; i < value.length; i++) {
        value[i] = restorePorts(value[i], seen);
      }
    } else {
      for (const key of Object.keys(value)) {
        value[key] = restorePorts(value[key], seen);
      }
    }
    return value;
  };

  // Node detaches transferred ports from the sender; we neuter the local object
  // WITHOUT closing the shared registry endpoint (the receiver adopts it by id).
  const neuterTransferList = (transferList) => {
    if (!Array.isArray(transferList)) return;
    for (const item of transferList) {
      if (item instanceof MessagePort) item[kNeuter]();
    }
  };

  const serializeForPort = (value, transferList) => {
    const bytes = hasPort(value, new WeakSet())
      ? serialize(replacePorts(value, new WeakMap()))
      : serialize(value);
    neuterTransferList(transferList);
    return bytes;
  };

  const deserializeForPort = (bytes) =>
    restorePorts(deserialize(bytes), new WeakSet());

  class MessagePort extends EventEmitter {
    #closed = false;
    #started = false;
    #onmessage = null;

    constructor(id) {
      super();
      this[kPortId] = id;
    }

    postMessage(value, transferList) {
      if (this.#closed) return;
      ops.op_meow_port_post(this[kPortId], serializeForPort(value, transferList));
    }

    // Node delivers nothing until the port is started (via `start()`, an
    // `onmessage` setter, or a "message" listener). `receiveMessageOnPort`
    // drains synchronously without starting the async pump.
    start() {
      if (this.#started || this.#closed) return;
      this.#started = true;
      const step = () => {
        if (this.#closed) return;
        PromisePrototypeThen(ops.op_meow_port_recv(this[kPortId]), (bytes) => {
          if (this.#closed) return;
          if (bytes.length === 0) return; // peer closed (EOF)
          const value = deserializeForPort(bytes);
          this.emit("message", value);
          if (typeof this.#onmessage === "function") {
            this.#onmessage({ data: value });
          }
          step();
        });
      };
      step();
    }

    close() {
      if (this.#closed) return;
      this.#closed = true;
      ops.op_meow_port_close(this[kPortId]);
    }

    get onmessage() {
      return this.#onmessage;
    }
    set onmessage(fn) {
      this.#onmessage = typeof fn === "function" ? fn : null;
      if (this.#onmessage) this.start();
    }

    // Web `EventTarget` shim (miniflare uses `addEventListener`): deliver
    // "message"/"messageerror" as `{ data }` events and auto-start on "message".
    addEventListener(type, listener) {
      if (typeof listener !== "function") return;
      if (type === "message") {
        this.on("message", (value) => listener({ data: value }));
        this.start();
      } else {
        this.on(type, listener);
      }
    }
    removeEventListener(type) {
      this.removeAllListeners(type);
    }

    [kNeuter]() {
      this.#closed = true;
    }

    ref() {}
    unref() {}
  }

  class MessageChannel {
    constructor() {
      const pair = ops.op_meow_port_channel_new();
      this.port1 = new MessagePort(pair[0]);
      this.port2 = new MessagePort(pair[1]);
    }
  }

  // Synchronous, non-blocking drain -- the piece that lets a thread read a port's
  // reply while it is blocked in `Atomics.wait` (miniflare's synchronous fetch).
  const receiveMessageOnPort = (port) => {
    if (!(port instanceof MessagePort)) return undefined;
    const bytes = ops.op_meow_port_recv_sync(port[kPortId]);
    if (bytes.length === 0) return undefined;
    return { message: deserializeForPort(bytes) };
  };

  const SHARE_ENV = Symbol.for("nodejs.worker_threads.SHARE_ENV");
  const environmentData = new SafeMap();
  const identity = (value) => value;

  ObjectAssign(exportsObj, {
    Worker,
    MessageChannel,
    MessagePort,
    // Bootstrap placeholders, overwritten by `__initWorkerThreads`.
    isMainThread: true,
    threadId: 0,
    parentPort: null,
    workerData: null,
    resourceLimits: {},
    SHARE_ENV,
    getEnvironmentData: (key) => environmentData.get(key),
    setEnvironmentData: (key, value) => {
      if (value === undefined) environmentData.delete(key);
      else environmentData.set(key, value);
    },
    markAsUntransferable: identity,
    markAsUncloneable: identity,
    isMarkedAsUntransferable: () => false,
    moveMessagePortToContext: identity,
    receiveMessageOnPort,
    setMaxListeners: (n, ...targets) => {
      if (typeof EventEmitter.setMaxListeners === "function") {
        EventEmitter.setMaxListeners(n, ...targets);
      }
    },
  });

  ObjectDefineProperty(exportsObj, "BroadcastChannel", {
    __proto__: null,
    configurable: true,
    enumerable: true,
    get() {
      return globalThis.BroadcastChannel;
    },
  });

  return exportsObj;
})();
