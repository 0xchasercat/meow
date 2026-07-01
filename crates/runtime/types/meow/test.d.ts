// GENERATED — do not edit (run: meow types)
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
export declare function test(name: string, fn: () => void): void;
export declare function expect<T>(actual: T): Expect<T>;
