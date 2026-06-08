const rawOps = (globalThis as unknown as {
  Deno: { core: { ops: Record<string, unknown> } };
}).Deno.core.ops;

interface NativeUiOps {
  op_ui_purr(body: string): void;
  op_ui_hiss(body: string): void;
  op_ui_pounce(body: string): void;
}

const ops = rawOps as unknown as NativeUiOps;

export function purr(body: string): void {
  ops.op_ui_purr(body);
}

export function hiss(body: string): void {
  ops.op_ui_hiss(body);
}

export function pounce(body: string): void {
  ops.op_ui_pounce(body);
}

export const ui = { purr, hiss, pounce };
