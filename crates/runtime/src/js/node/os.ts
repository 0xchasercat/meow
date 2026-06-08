const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

type NodeError = Error & { code?: string };

interface ProcessInfo {
  readonly enabled: boolean;
  readonly platform: string;
  readonly arch: string;
}

interface NativeOsOps {
  op_node_process_info(): ProcessInfo;
  op_hermetic_env_entries(): Array<[string, string]>;
}

interface CpuInfo {
  readonly model: string;
  readonly speed: number;
  readonly times: {
    readonly user: number;
    readonly nice: number;
    readonly sys: number;
    readonly idle: number;
    readonly irq: number;
  };
}

const ops = rawOps as unknown as NativeOsOps;
const processInfo = ops.op_node_process_info();
const env = Object.fromEntries(ops.op_hermetic_env_entries()) as Record<string, string>;

function strictWebError(message: string): never {
  const error = new Error(message) as NodeError;
  error.code = "ERR_STRICT_WEB_WITHDRAWN";
  throw error;
}

export function platform(): string {
  return processInfo.platform;
}

export function arch(): string {
  return processInfo.arch;
}

export const EOL = processInfo.platform === "win32" ? "\r\n" : "\n";

export function homedir(): string {
  if (!processInfo.enabled) {
    strictWebError("strict-web mode withdraws os.homedir()");
  }
  if (processInfo.platform === "win32") {
    return env.USERPROFILE ?? env.HOMEDRIVE + env.HOMEPATH;
  }
  return env.HOME ?? "/";
}

export function tmpdir(): string {
  if (!processInfo.enabled) {
    strictWebError("strict-web mode withdraws os.tmpdir()");
  }
  if (processInfo.platform === "win32") {
    return env.TEMP ?? env.TMP ?? `${homedir()}\\AppData\\Local\\Temp`;
  }
  return env.TMPDIR ?? env.TMP ?? env.TEMP ?? "/tmp";
}

export function cpus(): CpuInfo[] {
  return [];
}

const api = { platform, arch, EOL, homedir, tmpdir, cpus };
export default api;
