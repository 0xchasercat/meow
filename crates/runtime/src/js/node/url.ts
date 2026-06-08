import path from "node:path";

const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

interface ProcessInfo {
  readonly platform: string;
}

interface NativeUrlOps {
  op_node_process_info(): ProcessInfo;
}

const processInfo = (rawOps as unknown as NativeUrlOps).op_node_process_info();
const isWindows = processInfo.platform === "win32";
export const URL = globalThis.URL;
export const URLSearchParams = globalThis.URLSearchParams;


function toFileUrl(value: string | URL): URL {
  return value instanceof URL ? value : new URL(value);
}

function encodePathSegment(text: string): string {
  return encodeURIComponent(text).replace(/[!'()*]/g, (char) =>
    `%${char.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

export function fileURLToPath(value: string | URL): string {
  const url = toFileUrl(value);
  if (url.protocol !== "file:") {
    throw new TypeError(`Expected a file: URL, got ${url.protocol}`);
  }
  const pathname = decodeURIComponent(url.pathname);
  if (!isWindows) {
    if (url.hostname.length > 0) {
      return `//${url.hostname}${pathname}`;
    }
    return pathname;
  }
  if (url.hostname.length > 0) {
    return `\\\\${url.hostname}${pathname.replace(/\//g, "\\")}`;
  }
  const drivePath = pathname.startsWith("/") && /^[a-zA-Z]:/.test(pathname.slice(1))
    ? pathname.slice(1)
    : pathname;
  return drivePath.replace(/\//g, "\\");
}

export function pathToFileURL(pathname: string): URL {
  if (isWindows) {
    const normalized = path.win32.resolve(pathname).replace(/\\/g, "/");
    if (normalized.startsWith("//")) {
      const parts = normalized.slice(2).split("/");
      const host = parts.shift() ?? "";
      return new URL(`file://${host}/${parts.map(encodePathSegment).join("/")}`);
    }
    return new URL(`file:///${normalized.split("/").map(encodePathSegment).join("/")}`);
  }
  const resolved = path.posix.resolve(pathname);
  return new URL(`file://${resolved.split("/").map(encodePathSegment).join("/")}`);
}

const api = { URL, URLSearchParams, fileURLToPath, pathToFileURL };


export default api;
