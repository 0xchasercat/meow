export interface AssertionErrorOptions {
  message?: string;
  actual?: unknown;
  expected?: unknown;
  operator?: string;
}

type NodeAssertionError = Error & {
  code: string;
  actual?: unknown;
  expected?: unknown;
  operator?: string;
  generatedMessage: boolean;
};

export class AssertionError extends Error implements NodeAssertionError {
  code = "ERR_ASSERTION";
  actual?: unknown;
  expected?: unknown;
  operator?: string;
  generatedMessage: boolean;

  constructor(options: AssertionErrorOptions = {}) {
    super(options.message ?? "Assertion failed");
    this.name = "AssertionError";
    this.actual = options.actual;
    this.expected = options.expected;
    this.operator = options.operator;
    this.generatedMessage = options.message === undefined;
  }
}

export function fail(message?: string): never {
  throw new AssertionError({ message });
}

export function ok(value: unknown, message?: string): asserts value {
  if (!value) fail(message);
}

export function equal(actual: unknown, expected: unknown, message?: string): void {
  if (actual != expected) {
    throw new AssertionError({ actual, expected, message, operator: "==" });
  }
}

export function notEqual(actual: unknown, expected: unknown, message?: string): void {
  if (actual == expected) {
    throw new AssertionError({ actual, expected, message, operator: "!=" });
  }
}

export function strictEqual(actual: unknown, expected: unknown, message?: string): void {
  if (actual !== expected) {
    throw new AssertionError({ actual, expected, message, operator: "===" });
  }
}

export function notStrictEqual(actual: unknown, expected: unknown, message?: string): void {
  if (actual === expected) {
    throw new AssertionError({ actual, expected, message, operator: "!==" });
  }
}

export function deepStrictEqual(actual: unknown, expected: unknown, message?: string): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new AssertionError({ actual, expected, message, operator: "deepStrictEqual" });
  }
}

export function throws(fn: () => unknown, expected?: RegExp | ((err: unknown) => boolean), message?: string): void {
  try {
    fn();
  } catch (err) {
    if (expected instanceof RegExp && !expected.test(String((err as Error).message ?? err))) {
      throw new AssertionError({ actual: err, expected, message, operator: "throws" });
    }
    if (typeof expected === "function" && !expected(err)) {
      throw new AssertionError({ actual: err, expected, message, operator: "throws" });
    }
    return;
  }
  throw new AssertionError({ message: message ?? "Missing expected exception", operator: "throws" });
}

export function doesNotThrow(fn: () => unknown, message?: string): void {
  try {
    fn();
  } catch (err) {
    throw new AssertionError({ actual: err, message, operator: "doesNotThrow" });
  }
}

export const ifError = (value: unknown): void => {
  if (value !== null && value !== undefined) throw value;
};

const assert = Object.assign(ok, {
  AssertionError,
  fail,
  ok,
  equal,
  notEqual,
  strictEqual,
  notStrictEqual,
  deepStrictEqual,
  throws,
  doesNotThrow,
  ifError,
});

export default assert;
