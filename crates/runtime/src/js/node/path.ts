const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

interface ProcessInfo {
  readonly cwd: string;
  readonly platform: string;
}

interface NativePathOps {
  op_node_process_info(): ProcessInfo;
}

export interface ParsedPath {
  root: string;
  dir: string;
  base: string;
  ext: string;
  name: string;
}

interface PathModule {
  readonly sep: string;
  readonly delimiter: string;
  basename(path: string, suffix?: string): string;
  dirname(path: string): string;
  extname(path: string): string;
  format(pathObject: Partial<ParsedPath>): string;
  isAbsolute(path: string): boolean;
  join(...paths: string[]): string;
  normalize(path: string): string;
  parse(path: string): ParsedPath;
  relative(from: string, to: string): string;
  resolve(...paths: string[]): string;
  toNamespacedPath(path: string): string;
}

const pathInfo = (rawOps as unknown as NativePathOps).op_node_process_info();

function normalizeSlashes(path: string, separator: string): string {
  if (separator === "/") {
    return path.replace(/\\+/g, "/");
  }
  return path.replace(/\/+?/g, "\\");
}

function isWinAbsolute(path: string): boolean {
  return /^[a-zA-Z]:[\\/]/.test(path) || path.startsWith("\\\\");
}

function winRoot(path: string): string {
  if (path.startsWith("\\\\")) {
    const normalized = path.replace(/\//g, "\\");
    const match = /^\\\\[^\\]+\\[^\\]+/.exec(normalized);
    return match?.[0] ?? "\\\\";
  }
  const match = /^[a-zA-Z]:[\\/]/.exec(path);
  return match?.[0].replace(/\//g, "\\") ?? "";
}

function posixRoot(path: string): string {
  return path.startsWith("/") ? "/" : "";
}

function splitSegments(path: string, separator: string, root: string): string[] {
  const normalized = separator === "/" ? path.replace(/\\/g, "/") : path.replace(/\//g, "\\");
  const rest = root.length > 0 ? normalized.slice(root.length) : normalized;
  return rest.split(separator).filter((segment) => segment.length > 0);
}

function normalizePath(path: string, separator: string): string {
  if (path.length === 0) {
    return ".";
  }
  const normalized = normalizeSlashes(path, separator);
  const root = separator === "/" ? posixRoot(normalized) : winRoot(normalized);
  const segments = splitSegments(normalized, separator, root);
  const out: string[] = [];
  for (const segment of segments) {
    if (segment === ".") {
      continue;
    }
    if (segment === "..") {
      if (out.length > 0 && out[out.length - 1] !== "..") {
        out.pop();
      } else if (root.length === 0) {
        out.push("..");
      }
      continue;
    }
    out.push(segment);
  }
  let result = `${root}${out.join(separator)}`;
  const hadTrailing = normalized.endsWith(separator);
  if (result.length === 0) {
    result = root.length > 0 ? root : ".";
  }
  if (hadTrailing && result !== root && result !== ".") {
    result += separator;
  }
  return result;
}

function basenameImpl(path: string, separator: string, suffix?: string): string {
  const normalized = normalizePath(path, separator);
  const trimmed = normalized.endsWith(separator) && normalized !== separator
    ? normalized.slice(0, -1)
    : normalized;
  const last = trimmed.slice(trimmed.lastIndexOf(separator) + 1);
  if (suffix !== undefined && last.endsWith(suffix)) {
    return last.slice(0, last.length - suffix.length);
  }
  return last;
}

function dirnameImpl(path: string, separator: string): string {
  const normalized = normalizePath(path, separator);
  const root = separator === "/" ? posixRoot(normalized) : winRoot(normalized);
  if (normalized === root) {
    return root || ".";
  }
  const trimmed = normalized.endsWith(separator) ? normalized.slice(0, -1) : normalized;
  const index = trimmed.lastIndexOf(separator);
  if (index < root.length) {
    return root || ".";
  }
  if (index < 0) {
    return ".";
  }
  return trimmed.slice(0, index) || root || separator;
}

function extnameImpl(path: string, separator: string): string {
  const base = basenameImpl(path, separator);
  const index = base.lastIndexOf(".");
  if (index <= 0) {
    return "";
  }
  return base.slice(index);
}

function parseImpl(path: string, separator: string): ParsedPath {
  const normalized = normalizePath(path, separator);
  const root = separator === "/" ? posixRoot(normalized) : winRoot(normalized);
  const dir = dirnameImpl(normalized, separator);
  const base = basenameImpl(normalized, separator);
  const ext = extnameImpl(base, separator);
  const name = ext.length === 0 ? base : base.slice(0, base.length - ext.length);
  return { root, dir, base, ext, name };
}

function formatImpl(pathObject: Partial<ParsedPath>, separator: string): string {
  const root = pathObject.root ?? "";
  const dir = pathObject.dir ?? "";
  const base = pathObject.base ?? `${pathObject.name ?? ""}${pathObject.ext ?? ""}`;
  if (dir.length === 0) {
    return root.length > 0 ? `${root}${base}` : base;
  }
  return dir.endsWith(separator) ? `${dir}${base}` : `${dir}${separator}${base}`;
}

function relativeImpl(from: string, to: string, separator: string): string {
  const fromNormalized = normalizePath(from, separator);
  const toNormalized = normalizePath(to, separator);
  if (fromNormalized === toNormalized) {
    return "";
  }
  const fromRoot = separator === "/" ? posixRoot(fromNormalized) : winRoot(fromNormalized);
  const toRoot = separator === "/" ? posixRoot(toNormalized) : winRoot(toNormalized);
  if (fromRoot.toLowerCase() !== toRoot.toLowerCase()) {
    return toNormalized;
  }
  const fromSegments = splitSegments(fromNormalized, separator, fromRoot);
  const toSegments = splitSegments(toNormalized, separator, toRoot);
  let common = 0;
  while (
    common < fromSegments.length &&
    common < toSegments.length &&
    fromSegments[common]?.toLowerCase() === toSegments[common]?.toLowerCase()
  ) {
    common += 1;
  }
  const up = new Array(fromSegments.length - common).fill("..");
  const down = toSegments.slice(common);
  const result = [...up, ...down].join(separator);
  return result.length === 0 ? "." : result;
}

function resolveImpl(cwd: string, separator: string, paths: string[]): string {
  let resolved = "";
  for (let index = paths.length - 1; index >= -1; index -= 1) {
    const piece = index >= 0 ? paths[index] ?? "" : cwd;
    if (piece.length === 0) {
      continue;
    }
    resolved = resolved.length === 0 ? piece : `${piece}${separator}${resolved}`;
    if (separator === "/" ? resolved.startsWith("/") : isWinAbsolute(resolved)) {
      break;
    }
  }
  return normalizePath(resolved, separator);
}

function joinImpl(separator: string, paths: string[]): string {
  const filtered = paths.filter((value) => value.length > 0);
  if (filtered.length === 0) {
    return ".";
  }
  return normalizePath(filtered.join(separator), separator);
}

function createPathModule(separator: string, delimiter: string, cwd: string): PathModule {
  return {
    sep: separator,
    delimiter,
    basename(path, suffix) {
      return basenameImpl(path, separator, suffix);
    },
    dirname(path) {
      return dirnameImpl(path, separator);
    },
    extname(path) {
      return extnameImpl(path, separator);
    },
    format(pathObject) {
      return formatImpl(pathObject, separator);
    },
    isAbsolute(path) {
      return separator === "/" ? path.startsWith("/") : isWinAbsolute(path);
    },
    join(...paths) {
      return joinImpl(separator, paths);
    },
    normalize(path) {
      return normalizePath(path, separator);
    },
    parse(path) {
      return parseImpl(path, separator);
    },
    relative(from, to) {
      return relativeImpl(from, to, separator);
    },
    resolve(...paths) {
      return resolveImpl(cwd, separator, paths);
    },
    toNamespacedPath(path) {
      return path;
    },
  };
}

export const posix = createPathModule("/", ":", pathInfo.cwd.replace(/\\/g, "/"));
export const win32 = createPathModule("\\", ";", pathInfo.cwd.replace(/\//g, "\\"));
const active = pathInfo.platform === "win32" ? win32 : posix;

export const sep = active.sep;
export const delimiter = active.delimiter;
export const basename = active.basename.bind(active);
export const dirname = active.dirname.bind(active);
export const extname = active.extname.bind(active);
export const format = active.format.bind(active);
export const isAbsolute = active.isAbsolute.bind(active);
export const join = active.join.bind(active);
export const normalize = active.normalize.bind(active);
export const parse = active.parse.bind(active);
export const relative = active.relative.bind(active);
export const resolve = active.resolve.bind(active);
export const toNamespacedPath = active.toNamespacedPath.bind(active);

export default active;
