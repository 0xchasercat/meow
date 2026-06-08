const jsonCache = new Map<string, unknown>();
const proxyCache = new WeakMap<object, object>();

function isTrackable(value: unknown): value is object {
  return (typeof value === "object" && value !== null) || typeof value === "function";
}

export function syncNamedExports(
  value: unknown,
  names: readonly string[],
  setNamed: (name: string, value: unknown) => void,
): void {
  if (!isTrackable(value)) {
    for (const name of names) {
      setNamed(name, undefined);
    }
    return;
  }
  for (const name of names) {
    setNamed(name, Reflect.get(value, name));
  }
}

export function createNamedExportsProxy(
  names: readonly string[],
  setNamed: (name: string, value: unknown) => void,
  seed?: unknown,
): unknown {
  const target = seed === undefined ? {} : seed;
  if (!isTrackable(target)) {
    return target;
  }
  const cached = proxyCache.get(target);
  if (cached !== undefined) {
    syncNamedExports(cached, names, setNamed);
    return cached;
  }
  const proxy = new Proxy(target, {
    set(target, prop, value, receiver) {
      const ok = Reflect.set(target, prop, value, receiver);
      if (ok && typeof prop === "string") {
        setNamed(prop, value);
      }
      return ok;
    },
    defineProperty(target, prop, descriptor) {
      const ok = Reflect.defineProperty(target, prop, descriptor);
      if (ok && typeof prop === "string") {
        setNamed(prop, Reflect.get(target, prop));
      }
      return ok;
    },
    deleteProperty(target, prop) {
      const ok = Reflect.deleteProperty(target, prop);
      if (ok && typeof prop === "string") {
        setNamed(prop, undefined);
      }
      return ok;
    },
  });
  proxyCache.set(target, proxy);
  syncNamedExports(proxy, names, setNamed);
  return proxy;
}

export function requireFromNamespace(namespaceValue: unknown): unknown {
  if (
    namespaceValue !== null &&
    typeof namespaceValue === "object" &&
    "__meow_cjs_exports__" in namespaceValue
  ) {
    return (namespaceValue as { __meow_cjs_exports__: unknown }).__meow_cjs_exports__;
  }
  return namespaceValue;
}

export function requireJson(key: string, source: string): unknown {
  if (!jsonCache.has(key)) {
    jsonCache.set(key, JSON.parse(source));
  }
  return jsonCache.get(key);
}

export function dynamicRequire(specifier: string, referrer: string): never {
  throw new Error(
    `meow: CommonJS require(${JSON.stringify(specifier)}) from ${referrer} was not pre-walked; use a string-literal require or legacy mode (LOAD-005).`,
  );
}

export interface CjsModuleRecord {
  exports: unknown;
  filename: string;
  id: string;
  path: string;
  loaded: boolean;
  parent: unknown;
  children: unknown[];
  require(specifier: unknown): unknown;
}

export function createCjsModule(
  initialExports: unknown,
  requireFn: (specifier: unknown) => unknown,
  filename: string,
  dirname: string,
  setDefault: (value: unknown) => unknown,
  syncNamedFrom: (value: unknown) => void,
): {
  module: CjsModuleRecord;
  exports: unknown;
  start(): boolean;
  finish(): void;
  abort(): void;
  current(): unknown;
} {
  let current = initialExports;
  let executing = false;
  const moduleValue = {
    filename,
    id: filename,
    path: dirname,
    loaded: false,
    parent: undefined,
    children: [],
    require: requireFn,
  } as CjsModuleRecord;
  Object.defineProperty(moduleValue, "exports", {
    enumerable: true,
    configurable: true,
    get() {
      return current;
    },
    set(next: unknown) {
      current = setDefault(next);
      syncNamedFrom(current);
    },
  });
  return {
    module: moduleValue,
    exports: initialExports,
    start() {
      if (moduleValue.loaded || executing) {
        return false;
      }
      executing = true;
      return true;
    },
    finish() {
      executing = false;
      moduleValue.loaded = true;
      syncNamedFrom(current);
    },
    abort() {
      executing = false;
    },
    current() {
      return current;
    },
  };
}
