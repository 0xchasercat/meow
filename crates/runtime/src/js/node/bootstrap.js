// RT-007 -- native Node built-in bootstrap. MUST be 7-bit ASCII.
//
// Installs Node-style globals (`Buffer`, `process`) when node-compat is enabled.
// In strict-web mode the native modules still resolve, but the globals stay absent.

import { core } from "ext:core/mod.js";

const ops = core.ops;
const processInfo = ops.op_node_process_info();

function strictWebError(message) {
  const error = new Error(message);
  error.code = "ERR_STRICT_WEB_WITHDRAWN";
  return error;
}

function normalizeEncoding(value) {
  if (value === undefined || value === null || value === "") return "utf8";
  const normalized = String(value).toLowerCase();
  if (normalized === "utf-8") return "utf8";
  if (normalized === "ucs2") return "utf16le";
  return normalized;
}

function decodeBytes(bytes, encoding, start = 0, end = bytes.length) {
  const slice = bytes.subarray(start, end);
  switch (normalizeEncoding(encoding)) {
    case "utf8":
      return new TextDecoder().decode(slice);
    case "hex": {
      let out = "";
      for (const byte of slice) out += byte.toString(16).padStart(2, "0");
      return out;
    }
    case "base64": {
      let out = "";
      for (const byte of slice) out += String.fromCharCode(byte);
      return btoa(out);
    }
    case "ascii":
    case "latin1": {
      let out = "";
      for (const byte of slice) out += String.fromCharCode(byte & 0xff);
      return out;
    }
    case "utf16le": {
      let out = "";
      for (let i = 0; i < slice.length; i += 2) {
        const lo = slice[i] ?? 0;
        const hi = slice[i + 1] ?? 0;
        out += String.fromCharCode(lo | (hi << 8));
      }
      return out;
    }
    default:
      throw new TypeError(`Unsupported encoding: ${String(encoding)}`);
  }
}

function encodeString(text, encoding) {
  switch (normalizeEncoding(encoding)) {
    case "utf8":
      return new TextEncoder().encode(text);
    case "hex": {
      if (text.length % 2 !== 0) throw new TypeError("Invalid hex string");
      const out = new Uint8Array(text.length / 2);
      for (let i = 0; i < text.length; i += 2) {
        const value = Number.parseInt(text.slice(i, i + 2), 16);
        if (Number.isNaN(value)) throw new TypeError("Invalid hex string");
        out[i / 2] = value;
      }
      return out;
    }
    case "base64": {
      const raw = atob(text);
      const out = new Uint8Array(raw.length);
      for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
      return out;
    }
    case "ascii":
    case "latin1": {
      const out = new Uint8Array(text.length);
      for (let i = 0; i < text.length; i++) out[i] = text.charCodeAt(i) & 0xff;
      return out;
    }
    case "utf16le": {
      const out = new Uint8Array(text.length * 2);
      for (let i = 0; i < text.length; i++) {
        const code = text.charCodeAt(i);
        out[i * 2] = code & 0xff;
        out[i * 2 + 1] = code >> 8;
      }
      return out;
    }
    default:
      throw new TypeError(`Unsupported encoding: ${String(encoding)}`);
  }
}

function makeBufferClass() {
  class Buffer extends Uint8Array {
    static from(value, encoding = "utf8") {
      if (typeof value === "string") return new Buffer(encodeString(value, encoding));
      if (value instanceof ArrayBuffer) return new Buffer(new Uint8Array(value));
      if (ArrayBuffer.isView(value)) {
        return new Buffer(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
      }
      if (Array.isArray(value)) return new Buffer(value);
      throw new TypeError("Buffer.from() expects a string, ArrayBuffer, TypedArray, or byte array");
    }

    static alloc(size, fill = 0, encoding = "utf8") {
      if (!Number.isInteger(size) || size < 0) {
        throw new RangeError("Buffer.alloc() size must be a non-negative integer");
      }
      const buffer = new Buffer(size);
      if (typeof fill === "number") {
        buffer.fill(fill & 0xff);
        return buffer;
      }
      const pattern = typeof fill === "string" ? Buffer.from(fill, encoding) : Buffer.from(fill);
      if (pattern.length === 0) return buffer;
      for (let i = 0; i < buffer.length; i++) buffer[i] = pattern[i % pattern.length];
      return buffer;
    }

    static isBuffer(value) {
      return value instanceof Buffer;
    }

    static byteLength(value, encoding = "utf8") {
      if (typeof value === "string") return encodeString(value, encoding).length;
      if (value instanceof ArrayBuffer) return value.byteLength;
      if (ArrayBuffer.isView(value)) return value.byteLength;
      throw new TypeError("Buffer.byteLength() expects a string or binary value");
    }

    toString(encoding = "utf8", start = 0, end = this.length) {
      return decodeBytes(this, encoding, start, end);
    }

    equals(other) {
      if (!(other instanceof Uint8Array) || other.length !== this.length) return false;
      for (let i = 0; i < this.length; i++) {
        if (other[i] !== this[i]) return false;
      }
      return true;
    }
  }

  return Buffer;
}

function envProxy(overrides) {
  const store = Object.create(null);
  for (const [name, value] of ops.op_hermetic_env_entries()) store[name] = value;
  for (const [name, value] of overrides) store[name] = value;
  return new Proxy(store, {
    deleteProperty(target, key) {
      delete target[key];
      return true;
    },
    get(target, key) {
      return Reflect.get(target, key);
    },
    ownKeys(target) {
      return Reflect.ownKeys(target);
    },
    getOwnPropertyDescriptor(target, key) {
      const value = Reflect.get(target, key);
      if (value === undefined) return undefined;
      return {
        configurable: true,
        enumerable: true,
        value,
        writable: true,
      };
    },
    set(target, key, value) {
      target[key] = String(value);
      return true;
    },
  });
}

function toOutputString(chunk) {
  if (typeof chunk === "string") return chunk;
  if (chunk instanceof Uint8Array) return decodeBytes(chunk, "utf8");
  return String(chunk);
}

function stream(isErr) {
  return {
    write(chunk) {
      ops.op_meow_print(toOutputString(chunk), isErr);
      return true;
    },
  };
}

function makeProcess() {
  return {
    argv: [...processInfo.argv],
    env: envProxy(processInfo.env),
    cwd() {
      return processInfo.cwd;
    },
    platform: processInfo.platform,
    arch: processInfo.arch,
    version: processInfo.version,
    versions: Object.freeze({ ...processInfo.versions }),
    nextTick(callback, ...args) {
      Promise.resolve().then(() => callback(...args));
    },
    exit(code = 0) {
      ops.op_node_process_exit(Number(code) | 0);
    },
    stdout: stream(false),
    stderr: stream(true),
  };
}

if (processInfo.enabled) {
  globalThis.Buffer = makeBufferClass();
  globalThis.process = makeProcess();
}
