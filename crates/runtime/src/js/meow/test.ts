// meow:test — native hermetic test runner API
// test(name, fn) registers a test; expect(val) builds assertions.
// __meowTestRunAll() stores JSON results via the op seam.

const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

interface TestEntry {
  name: string;
  fn: () => void;
}

interface TestResult {
  name: string;
  passed: boolean;
  error: string | null;
  stack: string | null;
}

const tests: TestEntry[] = [];

export function test(name: string, fn: () => void): void {
  tests.push({ name, fn });
}

export function expect<T>(actual: T): Expect<T> {
  return new ExpectImpl(actual, false);
}

// eslint-disable-next-line @typescript-eslint/no-unused-vars
export interface Expect<T> {
  toBe(expected: T): void;
  toEqual(expected: T): void;
  toBeNull(): void;
  toBeDefined(): void;
  toBeTruthy(): void;
  toBeFalsy(): void;
  toThrow(): void;
  readonly not: Expect<T>;
}

class ExpectImpl<T> {
  private actual: T;
  private isNot: boolean;

  constructor(actual: T, isNot: boolean) {
    this.actual = actual;
    this.isNot = isNot;
  }

  get not(): Expect<T> {
    return new ExpectImpl(this.actual, !this.isNot) as unknown as Expect<T>;
  }

  toBe(expected: T): void {
    const pass = Object.is(this.actual, expected);
    if (this.isNot ? pass : !pass) {
      throw new Error(
        `expect(${formatValue(this.actual)}).${this.isNot ? "not." : ""}toBe(${formatValue(expected)})`,
      );
    }
  }

  toEqual(expected: T): void {
    const pass = deepEqual(this.actual, expected);
    if (this.isNot ? pass : !pass) {
      throw new Error(
        `expect(${formatValue(this.actual)}).${this.isNot ? "not." : ""}toEqual(${formatValue(expected)})`,
      );
    }
  }

  toBeNull(): void {
    this.toBe(null as unknown as T);
  }

  toBeDefined(): void {
    if (this.isNot ? this.actual !== undefined : this.actual === undefined) {
      throw new Error(
        `expected value to ${this.isNot ? "not " : ""}be defined`,
      );
    }
  }

  toBeTruthy(): void {
    const pass = !!this.actual;
    if (this.isNot ? pass : !pass) {
      throw new Error(
        `expected value to ${this.isNot ? "not " : ""}be truthy`,
      );
    }
  }

  toBeFalsy(): void {
    const pass = !this.actual;
    if (this.isNot ? pass : !pass) {
      throw new Error(
        `expected value to ${this.isNot ? "not " : ""}be falsy`,
      );
    }
  }

  toThrow(): void {
    const fn = this.actual as unknown as () => void;
    let threw = false;
    try {
      fn();
    } catch {
      threw = true;
    }
    if (this.isNot ? threw : !threw) {
      throw new Error(
        `expected function to ${this.isNot ? "not " : ""}throw`,
      );
    }
  }
}

function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a === null || b === null) return false;
  if (a === undefined || b === undefined) return false;
  if (typeof a !== typeof b) return false;
  if (typeof a !== "object") return Object.is(a, b);
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return false;
    return a.every((val, i) => deepEqual(val, b[i]));
  }
  const aObj = a as Record<string, unknown>;
  const bObj = b as Record<string, unknown>;
  const aKeys = Object.keys(aObj);
  const bKeys = Object.keys(bObj);
  if (aKeys.length !== bKeys.length) return false;
  return aKeys.every((key) =>
    Object.prototype.hasOwnProperty.call(bObj, key) && deepEqual(aObj[key], bObj[key]),
  );
}

function formatValue(val: unknown): string {
  if (val === null) return "null";
  if (val === undefined) return "undefined";
  if (typeof val === "string") return `"${val}"`;
  if (typeof val === "number" || typeof val === "boolean") return String(val);
  try {
    return JSON.stringify(val);
  } catch {
    return String(val);
  }
}

// Runner — called by Rust after module evaluation.
// Runs all tests, collects results, stores JSON via op.
(globalThis as Record<string, unknown>).__meowTestRunAll = function __meowTestRunAll(): void {
  const results: TestResult[] = [];
  for (const { name, fn } of tests) {
    try {
      fn();
      results.push({ name, passed: true, error: null, stack: null });
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      const stack = err instanceof Error ? err.stack || null : null;
      results.push({ name, passed: false, error: message, stack });
    }
  }
  const json = JSON.stringify(results);
  ((rawOps as Record<string, unknown>).op_test_store_results as (s: string) => void)(json);
};
