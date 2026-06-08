type Listener = (...args: unknown[]) => void;

interface ListenerEntry {
  readonly once: boolean;
  readonly listener: Listener;
}

export class EventEmitter {
  #events = new Map<string | symbol, ListenerEntry[]>();

  addListener(eventName: string | symbol, listener: Listener): this {
    return this.on(eventName, listener);
  }

  on(eventName: string | symbol, listener: Listener): this {
    const existing = this.#events.get(eventName);
    const next: ListenerEntry = { once: false, listener };
    if (existing === undefined) {
      this.#events.set(eventName, [next]);
    } else {
      existing.push(next);
    }
    return this;
  }

  once(eventName: string | symbol, listener: Listener): this {
    const existing = this.#events.get(eventName);
    const next: ListenerEntry = { once: true, listener };
    if (existing === undefined) {
      this.#events.set(eventName, [next]);
    } else {
      existing.push(next);
    }
    return this;
  }

  off(eventName: string | symbol, listener: Listener): this {
    return this.removeListener(eventName, listener);
  }

  removeListener(eventName: string | symbol, listener: Listener): this {
    const existing = this.#events.get(eventName);
    if (existing === undefined) {
      return this;
    }
    const filtered = existing.filter((entry) => entry.listener !== listener);
    if (filtered.length === 0) {
      this.#events.delete(eventName);
    } else {
      this.#events.set(eventName, filtered);
    }
    return this;
  }

  removeAllListeners(eventName?: string | symbol): this {
    if (eventName === undefined) {
      this.#events.clear();
      return this;
    }
    this.#events.delete(eventName);
    return this;
  }

  emit(eventName: string | symbol, ...args: unknown[]): boolean {
    const existing = this.#events.get(eventName);
    if (existing === undefined || existing.length === 0) {
      return false;
    }
    const keep: ListenerEntry[] = [];
    for (const entry of [...existing]) {
      entry.listener(...args);
      if (!entry.once) {
        keep.push(entry);
      }
    }
    if (keep.length === 0) {
      this.#events.delete(eventName);
    } else {
      this.#events.set(eventName, keep);
    }
    return true;
  }

  listenerCount(eventName: string | symbol): number {
    return this.#events.get(eventName)?.length ?? 0;
  }

  listeners(eventName: string | symbol): Listener[] {
    const existing = this.#events.get(eventName);
    if (existing === undefined) {
      return [];
    }
    return existing.map((entry) => entry.listener);
  }
}

const api = { EventEmitter };
export default api;
