import { Buffer } from "node:buffer";
export const TextEncoder = globalThis.TextEncoder;
export const TextDecoder = globalThis.TextDecoder;


type InspectSeen = Set<object>;
type Callback<T> = (error: unknown, value: T) => void;

type Promisified<TArgs extends unknown[], TResult> = (...args: TArgs) => Promise<TResult>;

function inspectValue(value: unknown, depth: number, seen: InspectSeen): string {
  if (typeof value === "string") {
    return JSON.stringify(value);
  }
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    return String(value);
  }
  if (typeof value === "symbol") {
    return value.toString();
  }
  if (value === undefined) {
    return "undefined";
  }
  if (value === null) {
    return "null";
  }
  if (typeof value === "function") {
    return `[Function ${value.name || "anonymous"}]`;
  }
  if (Buffer.isBuffer(value)) {
    return `Buffer <${value.toString("hex")}>`;
  }
  if (value instanceof Date) {
    return value.toISOString();
  }
  if (value instanceof RegExp) {
    return value.toString();
  }
  if (depth <= 0) {
    return Array.isArray(value) ? "[Array]" : "[Object]";
  }
  if (typeof value === "object") {
    if (seen.has(value)) {
      return "[Circular]";
    }
    seen.add(value);
    if (Array.isArray(value)) {
      const items = value.map((item) => inspectValue(item, depth - 1, seen));
      seen.delete(value);
      return `[ ${items.join(", ")} ]`;
    }
    const entries = Object.entries(value).map(
      ([key, nested]) => `${key}: ${inspectValue(nested, depth - 1, seen)}`,
    );
    seen.delete(value);
    return `{ ${entries.join(", ")} }`;
  }
  return String(value);
}

export function inspect(value: unknown, options?: { depth?: number }): string {
  return inspectValue(value, options?.depth ?? 3, new Set<object>());
}

export function promisify<TArgs extends unknown[], TResult>(
  fn: (...args: [...TArgs, Callback<TResult>]) => void,
): Promisified<TArgs, TResult> {
  return (...args) =>
    new Promise<TResult>((resolve, reject) => {
      fn(...args, (error, value) => {
        if (error != null) {
          reject(error);
          return;
        }
        resolve(value);
      });
    });
}

export const types = {
  isArrayBufferView(value: unknown): value is ArrayBufferView<ArrayBufferLike> {
    return ArrayBuffer.isView(value);
  },
  isDate(value: unknown): value is Date {
    return value instanceof Date;
  },
  isMap(value: unknown): value is Map<unknown, unknown> {
    return value instanceof Map;
  },
  isNativeError(value: unknown): value is Error {
    return value instanceof Error;
  },
  isPromise(value: unknown): value is Promise<unknown> {
    return value instanceof Promise;
  },
  isRegExp(value: unknown): value is RegExp {
    return value instanceof RegExp;
  },
  isSet(value: unknown): value is Set<unknown> {
    return value instanceof Set;
  },
  isUint8Array(value: unknown): value is Uint8Array {
    return value instanceof Uint8Array;
  },
};

const api = { inspect, promisify, types, TextEncoder, TextDecoder };


export default api;
