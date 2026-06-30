// Copyright 2018-2026 the Deno authors. MIT license.
// Copyright Joyent and Node contributors. All rights reserved. MIT license.
//
// meow-native `node:worker_threads`.
//
// meow runs workers as COOPERATIVE ISOLATES: each worker is its own
// `deno_core::JsRuntime` (V8 isolate) driven as a `spawn_local` task on the SAME
// OS thread as the main isolate (see `crates/cli/src/cli/commands/worker.rs`).
// That keeps the runtime single-threaded (so meow's frozen-clock / seeded-RNG
// determinism survives) and lets workers share the resolver + Oxc module graph
// by `Rc` clone -- but cross-isolate values must be structured-cloned
// (`core.serialize`/`core.deserialize`), never shared by reference.
//
// This replaces Deno's worker_threads polyfill, which is welded to
// `deno_runtime`'s worker host (`op_create_worker`, `op_host_*`) and newer
// `deno_node` worker ops that meow does not vendor. The host/worker ops below
// are meow-owned (`op_meow_worker_*`).
(function () {
  const { core, internals, primordials } = __bootstrap;
  const {
    op_meow_worker_create,
    op_meow_worker_host_post,
    op_meow_worker_host_recv,
    op_meow_worker_terminate,
    op_meow_worker_post,
    op_meow_worker_recv,
    op_meow_worker_data,
    op_worker_threads_filename,
  } = core.ops;
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
  // (CTRL_MESSAGE | CTRL_ERROR), the rest is the serialized payload. An EMPTY
  // buffer means the worker isolate has exited. (Framing avoids fragile
  // option-of-tuple op return shapes across the op2 boundary.)
  const CTRL_MESSAGE = 0;
  const CTRL_ERROR = 1;

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
      let spec;
      if (typeof specifier === "string") {
        spec = specifier;
      } else if (specifier && typeof specifier.href === "string") {
        spec = specifier.href; // URL
      } else {
        throw new TypeError("The 'specifier' argument must be a string or URL.");
      }
      // Modern TS projects pass a `.js` specifier for a `.ts` source on disk;
      // reuse the resolver's source-fallback (also normalizes a file URL).
      spec = op_worker_threads_filename(spec) ?? spec;

      const workerData = options.workerData === undefined
        ? null
        : options.workerData;

      this.#id = op_meow_worker_create(spec, serialize(workerData));
      this.threadId = this.#id;
      this.resourceLimits = { ...(options.resourceLimits ?? {}) };
      this.#pump();
    }

    #pump() {
      const step = () => {
        if (this.#terminated) return;
        PromisePrototypeThen(
          op_meow_worker_host_recv(this.#id),
          (frame) => {
            if (this.#terminated) return;
            // First event observed => the worker isolate is live.
            if (!this.#onlineEmitted) {
              this.#onlineEmitted = true;
              this.emit("online");
            }
            if (frame.length === 0) {
              this.#terminated = true;
              this.emit("exit", 0);
              return;
            }
            const payload = TypedArrayPrototypeSubarray(frame, 1);
            if (frame[0] === CTRL_ERROR) {
              // Errors arrive as a UTF-8 string (the Rust driver can't structured-
              // clone), messages as a `core.serialize`d value.
              this.emit("error", new Error(core.decode(payload)));
            } else {
              this.emit("message", deserialize(payload));
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

    postMessage(value) {
      if (this.#terminated) return;
      op_meow_worker_host_post(this.#id, serialize(value));
    }

    terminate() {
      if (this.#terminated) return PromiseResolve(1);
      this.#terminated = true;
      op_meow_worker_terminate(this.#id);
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
    port.postMessage = (value) => {
      if (!closed) op_meow_worker_post(serialize(value));
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
      PromisePrototypeThen(op_meow_worker_recv(), (bytes) => {
        if (closed) return;
        if (bytes.length === 0) {
          closed = true;
          return; // host terminated us
        }
        const value = deserialize(bytes);
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
      const dataBytes = op_meow_worker_data();
      exportsObj.workerData = dataBytes && dataBytes.length > 0
        ? deserialize(dataBytes)
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

  // ----------------- minimal MessagePort / MessageChannel -----------------
  // SvelteKit/Vite drive workers through `Worker` + `parentPort` only; these
  // give same-isolate channels for libraries that destructure the names at
  // import time. Cross-isolate port transfer is not supported yet.
  class MessagePort extends EventEmitter {
    #peer = null;
    #closed = false;
    _link(peer) {
      this.#peer = peer;
    }
    postMessage(value) {
      if (this.#closed || !this.#peer) return;
      const cloned = deserialize(serialize(value));
      const peer = this.#peer;
      PromisePrototypeThen(PromiseResolve(), () => {
        peer.emit("message", cloned);
        if (typeof peer.onmessage === "function") peer.onmessage({ data: cloned });
      });
    }
    start() {}
    close() {
      this.#closed = true;
    }
    ref() {}
    unref() {}
  }

  class MessageChannel {
    constructor() {
      this.port1 = new MessagePort();
      this.port2 = new MessagePort();
      this.port1._link(this.port2);
      this.port2._link(this.port1);
    }
  }

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
    receiveMessageOnPort: () => undefined,
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
