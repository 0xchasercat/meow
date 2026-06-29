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
    if (value instanceof Error) {
      return value.stack || `${value.name}: ${value.message}`;
    }
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

const consoleTimers = new Map();
const consoleCounters = new Map();

function resolveConsoleLabel(label) {
  return typeof label === "string" ? label : "default";
}

function nowMs() {
  return Date.now();
}

globalThis.console = {
  log(...args) {
    op_meow_print(`${format(args)}\n`, false);
  },
  info(...args) {
    op_meow_print(`${format(args)}\n`, false);
  },
  warn(...args) {
    op_meow_print(`${format(args)}\n`, true);
  },
  error(...args) {
    op_meow_print(`${format(args)}\n`, true);
  },
  debug(...args) {
    op_meow_print(`${format(args)}\n`, false);
  },
  clear() {
    op_meow_print("\x1Bc", false);
  },
  assert(condition, ...args) {
    if (condition) return;
    if (args.length === 0) {
      op_meow_print(`Assertion failed\n`, true);
      return;
    }
    op_meow_print(`Assertion failed: ${format(args)}\n`, true);
  },
  time(label) {
    const key = resolveConsoleLabel(label);
    consoleTimers.set(key, nowMs());
  },
  timeEnd(label) {
    const key = resolveConsoleLabel(label);
    const started = consoleTimers.get(key);
    const duration =
      typeof started === "number" ? nowMs() - started : NaN;
    consoleTimers.delete(key);
    if (Number.isNaN(duration)) {
      op_meow_print(`${key}: no such label\n`, true);
      return;
    }
    op_meow_print(`${key}: ${duration}ms\n`, false);
  },
  timeLog(label, ...args) {
    const key = resolveConsoleLabel(label);
    const started = consoleTimers.get(key);
    const duration =
      typeof started === "number" ? nowMs() - started : NaN;
    if (Number.isNaN(duration)) {
      op_meow_print(`${key}: no such label\n`, true);
      return;
    }
    if (args.length === 0) {
      op_meow_print(`${key}: ${duration}ms\n`, false);
      return;
    }
    op_meow_print(`${key}: ${duration}ms ${format(args)}\n`, false);
  },
  trace(...args) {
    op_meow_print(`TRACE ${format(args)}\n`, true);
  },
  count(label) {
    const key = resolveConsoleLabel(label);
    const next = (consoleCounters.get(key) ?? 0) + 1;
    consoleCounters.set(key, next);
    op_meow_print(`${key}: ${next}\n`, false);
  },
  dir(...args) {
    if (args.length === 0) {
      op_meow_print("undefined\n", false);
      return;
    }
    op_meow_print(`${format(args)}\n`, false);
  },
};
