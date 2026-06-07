// RT-001 (A4) -- minimal runtime bootstrap. MUST be 7-bit ASCII (deno_core
// requires extension code to be ASCII).
//
// Installs ONLY globalThis.console (log/error) backed by op_print, so a trivial
// program is observable. No Web globals (RT-004), no meow:* (RT-005).

import { core } from "ext:core/mod.js";

const { op_meow_print } = core.ops;

function stringify(value) {
  if (typeof value === "string") return value;
  if (typeof value === "bigint") return `${value}n`;
  if (typeof value === "symbol") return value.toString();
  if (value === undefined) return "undefined";
  if (value === null) return "null";
  if (typeof value === "object") {
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  }
  return String(value);
}

function format(args) {
  let out = "";
  for (let i = 0; i < args.length; i++) {
    if (i > 0) out += " ";
    out += stringify(args[i]);
  }
  return out;
}

globalThis.console = {
  log(...args) {
    op_meow_print(`${format(args)}\n`, false);
  },
  error(...args) {
    op_meow_print(`${format(args)}\n`, true);
  },
};
