const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

type NodeError = Error & { code?: string };

type LookupCallback = (error: NodeError | null, address: string | LookupAddress[] | null, family?: number) => void;

export type LookupFamily = 4 | 6 | 0;

interface NativeDnsOps {
  op_node_dns_lookup(hostname: string, family: number, all: boolean): Promise<LookupAddress[]>;
}

export interface LookupAddress {
  readonly address: string;
  readonly family: number;
}

export interface LookupOptions {
  readonly family?: LookupFamily;
  readonly all?: boolean;
}

const ops = rawOps as unknown as NativeDnsOps;

function ensureCallback(callback: unknown): LookupCallback {
  if (typeof callback !== "function") {
    throw new TypeError("dns.lookup() requires a callback");
  }
  return callback as LookupCallback;
}

function normalizeNodeError(error: unknown): NodeError {
  const nodeError = error instanceof Error ? error as NodeError : new Error(String(error)) as NodeError;
  const message = String(nodeError.message ?? error);
  const marker = message.indexOf(":");
  if (marker > 0) {
    const code = message.slice(0, marker);
    if (/^[A-Z][A-Z0-9_]+$/.test(code)) {
      nodeError.code ??= code;
    }
  }
  nodeError.code ??= "ENOTFOUND";
  return nodeError;
}

export function lookup(hostname: string, callback: LookupCallback): void;
export function lookup(hostname: string, options: number, callback: LookupCallback): void;
export function lookup(hostname: string, options: LookupOptions, callback: LookupCallback): void;
export function lookup(
  hostname: string,
  optionsOrCallback?: number | LookupOptions | LookupCallback,
  callback?: LookupCallback,
): void {
  const resolvedCallback = typeof optionsOrCallback === "function"
    ? optionsOrCallback
    : ensureCallback(callback);
  let family = 0;
  let all = false;
  if (typeof optionsOrCallback !== "function" && optionsOrCallback !== undefined) {
    if (typeof optionsOrCallback === "number") {
      family = optionsOrCallback;
    } else if (typeof optionsOrCallback === "object" && optionsOrCallback !== null) {
      family = optionsOrCallback.family ?? 0;
      all = optionsOrCallback.all ?? false;
    } else {
      throw new TypeError("The options argument must be an object, a number, or a callback");
    }
  }
  ops.op_node_dns_lookup(String(hostname), family, all).then(
    (values) => {
      if (all) {
        resolvedCallback(null, values);
        return;
      }
      const first = values[0];
      if (first === undefined) {
        const error = new Error(`ENOTFOUND: getaddrinfo ENOTFOUND ${hostname}`) as NodeError;
        error.code = "ENOTFOUND";
        resolvedCallback(error, null);
        return;
      }
      resolvedCallback(null, first.address, first.family);
    },
    (error) => {
      resolvedCallback(normalizeNodeError(error), null);
    },
  );
}

const api = {
  lookup,
};

export default api;
