type BufferInstance = Uint8Array & {
  toString(encoding?: string, start?: number, end?: number): string;
  equals(other: Uint8Array): boolean;
};

export interface BufferConstructor {
  from(
    value: string | ArrayBuffer | ArrayBufferView<ArrayBufferLike> | readonly number[],
    encoding?: string,
  ): BufferInstance;
  alloc(
    size: number,
    fill?: number | string | ArrayBuffer | ArrayBufferView<ArrayBufferLike> | readonly number[],
    encoding?: string,
  ): BufferInstance;
  isBuffer(value: unknown): value is BufferInstance;
  byteLength(value: string | ArrayBuffer | ArrayBufferView<ArrayBufferLike>, encoding?: string): number;
  new (value: number | ArrayLike<number> | ArrayBufferLike): BufferInstance;
}

type NodeError = Error & { code?: string };

function strictWebError(message: string): never {
  const error = new Error(message) as NodeError;
  error.code = "ERR_STRICT_WEB_WITHDRAWN";
  throw error;
}

function deniedBuffer(): BufferConstructor {
  function denied(): never {
    return strictWebError("strict-web mode withdraws Buffer and node:buffer");
  }
  return Object.freeze({
    from: denied,
    alloc: denied,
    isBuffer() {
      return false;
    },
    byteLength: denied,
  }) as unknown as BufferConstructor;
}

const BufferValue = (globalThis as unknown as { Buffer?: BufferConstructor }).Buffer ?? deniedBuffer();

export const Buffer = BufferValue;
export default { Buffer };
