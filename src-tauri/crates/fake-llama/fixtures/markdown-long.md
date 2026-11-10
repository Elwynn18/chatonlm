Here is a complete TypeScript module that queues downloads, retries the failed ones with an exponential backoff and keeps a short history of the tasks:

```typescript
// Small task queue with retries, a typed event emitter and an LRU cache.

export type TaskState = "pending" | "running" | "done" | "failed" | "cancelled";

export interface TaskOptions {
  /** Number of extra attempts before giving up. */
  retries?: number;
  /** First delay between two attempts, in milliseconds. */
  backoffMs?: number;
  signal?: AbortSignal;
}

export interface TaskRecord<T> {
  id: number;
  label: string;
  state: TaskState;
  attempts: number;
  result?: T;
  error?: Error;
  startedAt?: number;
  finishedAt?: number;
}

type Listener<E> = (event: E) => void;

export class Emitter<Events extends Record<string, unknown>> {
  private listeners = new Map<keyof Events, Set<Listener<never>>>();

  on<K extends keyof Events>(name: K, listener: Listener<Events[K]>): () => void {
    const set = this.listeners.get(name) ?? new Set<Listener<never>>();
    this.listeners.set(name, set);
    set.add(listener as Listener<never>);
    return () => set.delete(listener as Listener<never>);
  }

  emit<K extends keyof Events>(name: K, event: Events[K]): void {
    for (const listener of this.listeners.get(name) ?? []) {
      (listener as Listener<Events[K]>)(event);
    }
  }
}

export class LruCache<K, V> {
  private readonly entries = new Map<K, V>();

  constructor(private readonly capacity: number) {
    if (!Number.isInteger(capacity) || capacity < 1) {
      throw new RangeError(`capacity must be a positive integer, got ${capacity}`);
    }
  }

  get size(): number {
    return this.entries.size;
  }

  get(key: K): V | undefined {
    if (!this.entries.has(key)) return undefined;
    const value = this.entries.get(key) as V;
    this.entries.delete(key);
    this.entries.set(key, value);
    return value;
  }

  set(key: K, value: V): this {
    this.entries.delete(key);
    this.entries.set(key, value);
    while (this.entries.size > this.capacity) {
      const oldest = this.entries.keys().next().value as K;
      this.entries.delete(oldest);
    }
    return this;
  }
}

const sleep = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener(
      "abort",
      () => {
        clearTimeout(timer);
        reject(new DOMException("Aborted", "AbortError"));
      },
      { once: true },
    );
  });

interface QueueEvents extends Record<string, unknown> {
  start: TaskRecord<unknown>;
  settle: TaskRecord<unknown>;
  idle: { completed: number; failed: number };
}

export class TaskQueue extends Emitter<QueueEvents> {
  private nextId = 1;
  private running = 0;
  private completed = 0;
  private failed = 0;
  private readonly waiting: Array<() => void> = [];
  readonly history = new LruCache<number, TaskRecord<unknown>>(100);

  constructor(private readonly concurrency = 4) {
    super();
  }

  async run<T>(
    label: string,
    task: (attempt: number, signal?: AbortSignal) => Promise<T>,
    { retries = 2, backoffMs = 250, signal }: TaskOptions = {},
  ): Promise<T> {
    const record: TaskRecord<T> = { id: this.nextId++, label, state: "pending", attempts: 0 };
    this.history.set(record.id, record);
    await this.acquire();
    record.state = "running";
    record.startedAt = performance.now();
    this.emit("start", record);
    try {
      for (;;) {
        record.attempts += 1;
        try {
          const result = await task(record.attempts, signal);
          record.result = result;
          record.state = "done";
          this.completed += 1;
          return result;
        } catch (error) {
          if (signal?.aborted) {
            record.state = "cancelled";
            throw error;
          }
          if (record.attempts > retries) {
            record.state = "failed";
            record.error = error instanceof Error ? error : new Error(String(error));
            this.failed += 1;
            throw record.error;
          }
          await sleep(backoffMs * 2 ** (record.attempts - 1), signal);
        }
      }
    } finally {
      record.finishedAt = performance.now();
      this.emit("settle", record);
      this.release();
    }
  }

  private acquire(): Promise<void> {
    if (this.running < this.concurrency) {
      this.running += 1;
      return Promise.resolve();
    }
    return new Promise((resolve) => this.waiting.push(resolve));
  }

  private release(): void {
    const next = this.waiting.shift();
    if (next) {
      next();
      return;
    }
    this.running -= 1;
    if (this.running === 0) {
      this.emit("idle", { completed: this.completed, failed: this.failed });
    }
  }
}

const SIZE_PATTERN = /^(\d+(?:\.\d+)?)\s*(B|KB|MB|GB|TB)$/i;
const UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** Parses "1.5 GB" into a number of bytes (base 1024). */
export function parseSize(text: string): number {
  const match = SIZE_PATTERN.exec(text.trim());
  if (!match) {
    throw new SyntaxError(`not a size: "${text}"`);
  }
  const [, amount, unit] = match;
  const power = UNITS.indexOf(unit.toUpperCase() as (typeof UNITS)[number]);
  return Math.round(Number(amount) * 1024 ** power);
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)} s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes} min ${Math.round(seconds % 60)} s`;
}

export async function* pullAll(
  queue: TaskQueue,
  names: readonly string[],
  fetchLayer: (name: string, attempt: number) => Promise<number>,
): AsyncGenerator<{ name: string; bytes: number }, void, undefined> {
  const pending = names.map((name) =>
    queue
      .run(`pull ${name}`, (attempt) => fetchLayer(name, attempt), { retries: 3 })
      .then((bytes) => ({ name, bytes })),
  );
  for (const result of pending) {
    yield await result;
  }
}

async function fakeLayer(name: string, attempt: number): Promise<number> {
  if (attempt === 1 && name.startsWith("gemma")) {
    throw new Error(`transient error for ${name}`);
  }
  await sleep(10);
  return parseSize(SIZES.get(name) ?? "0 B");
}

const SIZES = new Map<string, string>([
  ["qwen3:0.6b", "523 MB"],
  ["qwen3.5:0.8b", "1.0 GB"],
  ["gemma3:4b", "3.3 GB"],
]);

async function main(): Promise<void> {
  const queue = new TaskQueue(2);
  const stop = queue.on("idle", ({ completed, failed }) => {
    console.log(`queue idle: ${completed} done, ${failed} failed`);
  });
  const started = performance.now();
  let total = 0;
  for await (const { name, bytes } of pullAll(queue, [...SIZES.keys()], fakeLayer)) {
    total += bytes;
    console.log(`${name.padEnd(14)} ${(bytes / 1024 ** 2).toFixed(0).padStart(6)} MiB`);
  }
  const elapsed = formatDuration(performance.now() - started);
  console.log(`total ${(total / 1024 ** 3).toFixed(2)} GiB in ${elapsed}`);
  stop();
}

main().catch((error: unknown) => {
  console.error(error);
  process.exitCode = 1;
});
```

The queue never runs more than `concurrency` tasks at once, and `pullAll` yields the results in the order of the names, not in the order in which the downloads finish.
