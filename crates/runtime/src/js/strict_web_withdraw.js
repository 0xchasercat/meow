// strict-web host-access withdrawal (RT-004 / RT-007).
//
// Runs ONLY under NodeMode::StrictWeb, as an extension entry point pushed after
// node_globals. node:fs and node:process operations are withdrawn at use time
// (throwing ERR_STRICT_WEB_WITHDRAWN) and the ambient `process` global is
// removed, so a strict-web program sees the Stateless-Edge surface only. CJS
// still instantiates because node:module / NodeRequireLoader stay intact.
import Module from "node:module";
import * as nodeFs from "node:fs";
import * as nodeProcess from "node:process";

function withdrawError(moduleName) {
  const error = new Error("strict-web mode withdraws " + moduleName + " access");
  error.code = "ERR_STRICT_WEB_WITHDRAWN";
  return error;
}

function withdrawFunctions(target, moduleName) {
  if (target === null || typeof target !== "object") {
    return;
  }
  const thrower = () => {
    throw withdrawError(moduleName);
  };
  for (const name of Object.getOwnPropertyNames(target)) {
    let descriptor;
    try {
      descriptor = Object.getOwnPropertyDescriptor(target, name);
    } catch {
      continue;
    }
    if (descriptor === undefined || typeof descriptor.value !== "function") {
      continue;
    }
    if (descriptor.configurable === false && descriptor.writable === false) {
      continue;
    }
    try {
      Object.defineProperty(target, name, {
        value: thrower,
        writable: true,
        configurable: true,
        enumerable: descriptor.enumerable,
      });
    } catch {
      // Leave non-redefinable members alone; the common surface is covered.
    }
  }
}

function withdrawEnv(target) {
  if (target === null || typeof target !== "object") {
    return;
  }
  const envProxy = new Proxy(
    {},
    {
      get() {
        throw withdrawError("node:process");
      },
      set() {
        throw withdrawError("node:process");
      },
      has() {
        throw withdrawError("node:process");
      },
    },
  );
  try {
    Object.defineProperty(target, "env", {
      get() {
        return envProxy;
      },
      configurable: true,
    });
    return;
  } catch {
    // Fall back to a plain assignment below.
  }
  try {
    target.env = envProxy;
  } catch {
    // If env is locked we cannot withdraw it; nothing else to do.
  }
}

let requireFn;
try {
  requireFn = Module.createRequire("/meow-strict-web.js");
} catch {
  requireFn = undefined;
}

const fsTargets = new Set();
if (nodeFs && typeof nodeFs === "object") {
  if (nodeFs.default) {
    fsTargets.add(nodeFs.default);
  }
  fsTargets.add(nodeFs);
}
if (requireFn !== undefined) {
  try {
    fsTargets.add(requireFn("fs"));
  } catch {
    // The namespace targets still cover the import view.
  }
}
for (const target of fsTargets) {
  withdrawFunctions(target, "node:fs");
}

withdrawEnv(nodeProcess && nodeProcess.default ? nodeProcess.default : nodeProcess);
if (requireFn !== undefined) {
  try {
    withdrawEnv(requireFn("process"));
  } catch {
    // Builtin require unavailable; import view already handled.
  }
}

try {
  delete globalThis.process;
} catch {
  // Fall through to redefining as undefined.
}
try {
  Object.defineProperty(globalThis, "process", {
    value: undefined,
    writable: true,
    configurable: true,
  });
} catch {
  // If the global cannot be redefined, the delete above stands.
}
