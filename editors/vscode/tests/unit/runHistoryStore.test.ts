import { describe, it, expect } from "vitest";
import type { Memento } from "vscode";
import {
  RunHistoryStore,
  applyHistoryFilter,
  historyFilterPredicate,
  trimWithPinned,
  type RunHistoryEntry,
} from "../../src/views/RunHistoryView";
import type { Report } from "../../src/util/schemaGuards";

/** Tiny in-memory stand-in for vscode.Memento. */
class FakeMemento implements Memento {
  private readonly store = new Map<string, unknown>();
  keys(): readonly string[] {
    return Array.from(this.store.keys());
  }
  get<T>(key: string, defaultValue?: T): T | undefined {
    return (this.store.get(key) as T | undefined) ?? defaultValue;
  }
  async update(key: string, value: unknown): Promise<void> {
    if (value === undefined) this.store.delete(key);
    else this.store.set(key, value);
  }
}

function makeEntry(overrides: Partial<RunHistoryEntry> = {}): RunHistoryEntry {
  return {
    id: overrides.id ?? Math.random().toString(36).slice(2),
    timestamp: Date.now(),
    label: "1/1 steps",
    environment: null,
    tags: [],
    status: "PASSED",
    passed: 1,
    failed: 0,
    total: 1,
    durationMs: 100,
    files: [],
    selectors: [],
    dryRun: false,
    pinned: false,
    ...overrides,
  };
}

describe("historyFilterPredicate", () => {
  it("all matches every entry", () => {
    expect(historyFilterPredicate(makeEntry(), { kind: "all" })).toBe(true);
    expect(historyFilterPredicate(makeEntry({ status: "FAILED" }), { kind: "all" })).toBe(true);
  });

  it("passed matches only PASSED status", () => {
    expect(historyFilterPredicate(makeEntry({ status: "PASSED" }), { kind: "passed" })).toBe(true);
    expect(historyFilterPredicate(makeEntry({ status: "FAILED" }), { kind: "passed" })).toBe(false);
    expect(historyFilterPredicate(makeEntry({ status: "ERRORED" }), { kind: "passed" })).toBe(false);
  });

  it("failed matches FAILED and ERRORED", () => {
    expect(historyFilterPredicate(makeEntry({ status: "FAILED" }), { kind: "failed" })).toBe(true);
    expect(historyFilterPredicate(makeEntry({ status: "ERRORED" }), { kind: "failed" })).toBe(true);
    expect(historyFilterPredicate(makeEntry({ status: "PASSED" }), { kind: "failed" })).toBe(false);
    expect(historyFilterPredicate(makeEntry({ status: "CANCELLED" }), { kind: "failed" })).toBe(false);
  });

  it("env matches by exact environment name", () => {
    const e = makeEntry({ environment: "staging" });
    expect(historyFilterPredicate(e, { kind: "env", value: "staging" })).toBe(true);
    expect(historyFilterPredicate(e, { kind: "env", value: "production" })).toBe(false);
    const noEnv = makeEntry({ environment: null });
    expect(historyFilterPredicate(noEnv, { kind: "env", value: "" })).toBe(true);
  });

  it("tag matches when the entry's tag list contains the requested tag", () => {
    const e = makeEntry({ tags: ["smoke", "slow"] });
    expect(historyFilterPredicate(e, { kind: "tag", value: "smoke" })).toBe(true);
    expect(historyFilterPredicate(e, { kind: "tag", value: "fast" })).toBe(false);
  });

  it("tag with no value matches any tagged entry", () => {
    expect(historyFilterPredicate(makeEntry({ tags: ["smoke"] }), { kind: "tag" })).toBe(true);
    expect(historyFilterPredicate(makeEntry({ tags: [] }), { kind: "tag" })).toBe(false);
  });
});

describe("applyHistoryFilter", () => {
  it("filters a list down to the matching entries", () => {
    const entries = [
      makeEntry({ id: "a", status: "PASSED" }),
      makeEntry({ id: "b", status: "FAILED" }),
      makeEntry({ id: "c", status: "PASSED" }),
    ];
    const passed = applyHistoryFilter(entries, { kind: "passed" });
    expect(passed.map((e) => e.id)).toEqual(["a", "c"]);
  });
});

describe("trimWithPinned", () => {
  it("drops the oldest unpinned entries first", () => {
    const entries = [
      makeEntry({ id: "e5" }),
      makeEntry({ id: "e4" }),
      makeEntry({ id: "e3" }),
      makeEntry({ id: "e2" }),
      makeEntry({ id: "e1" }),
    ];
    const trimmed = trimWithPinned(entries, 3);
    expect(trimmed.map((e) => e.id)).toEqual(["e5", "e4", "e3"]);
  });

  it("never evicts pinned entries", () => {
    const entries = [
      makeEntry({ id: "new1" }),
      makeEntry({ id: "new2" }),
      makeEntry({ id: "new3" }),
      makeEntry({ id: "old-pinned", pinned: true }),
    ];
    const trimmed = trimWithPinned(entries, 2);
    const ids = trimmed.map((e) => e.id);
    expect(ids).toContain("old-pinned");
    expect(ids).toContain("new1");
    expect(ids).toContain("new2");
    expect(ids).not.toContain("new3");
  });
});

describe("RunHistoryStore", () => {
  it("persists added entries and surfaces them in LIFO order", async () => {
    const store = new RunHistoryStore(new FakeMemento());
    await store.add(makeEntry({ id: "a" }));
    await store.add(makeEntry({ id: "b" }));
    expect(store.all().map((e) => e.id)).toEqual(["b", "a"]);
  });

  it("evicts the oldest unpinned entry when over cap but keeps pinned entries", async () => {
    const store = new RunHistoryStore(new FakeMemento());
    // Seed 20 unpinned entries at the cap, plus one pinned.
    for (let i = 0; i < 20; i++) {
      await store.add(makeEntry({ id: `u${i}` }));
    }
    await store.add(makeEntry({ id: "pinned", pinned: true }));
    // Push one more — should evict the oldest unpinned (u0), keep pinned.
    await store.add(makeEntry({ id: "fresh" }));
    const ids = store.all().map((e) => e.id);
    expect(ids).toContain("pinned");
    expect(ids).toContain("fresh");
    expect(ids).not.toContain("u0");
    // Pinned entries appear before unpinned in the listing order.
    expect(ids[0]).toBe("pinned");
  });

  it("pin() sets pinned=true on an existing entry", async () => {
    const store = new RunHistoryStore(new FakeMemento());
    await store.add(makeEntry({ id: "x" }));
    await store.pin("x");
    expect(store.findById("x")?.pinned).toBe(true);
  });

  it("unpin() removes the pinned flag and restores capped eviction", async () => {
    const store = new RunHistoryStore(new FakeMemento());
    // Fill with 20 unpinned + 1 pinned
    for (let i = 0; i < 20; i++) {
      await store.add(makeEntry({ id: `u${i}` }));
    }
    await store.add(makeEntry({ id: "later" }));
    await store.pin("later");
    // Now unpin it — the store should not grow beyond 20 unpinned.
    await store.unpin("later");
    const ids = store.all().map((e) => e.id);
    // `later` is still present (it was the most recent), but the
    // oldest unpinned (u0) should be evicted since now we have 21
    // unpinned and the cap is 20.
    expect(ids).toContain("later");
    expect(ids).not.toContain("u0");
  });

  it("clear() keeps pinned entries and drops the rest", async () => {
    const store = new RunHistoryStore(new FakeMemento());
    await store.add(makeEntry({ id: "a", pinned: true }));
    await store.add(makeEntry({ id: "b" }));
    await store.add(makeEntry({ id: "c" }));
    await store.clear();
    const remaining = store.all().map((e) => e.id);
    expect(remaining).toEqual(["a"]);
  });

  it("gracefully normalizes legacy entries missing the new fields", () => {
    const memento = new FakeMemento();
    // Simulate a pre-NAZ-276 persisted entry shape.
    memento.update("tarn.runHistory", [
      {
        id: "legacy",
        timestamp: 1,
        label: "1/1",
        environment: null,
        tags: [],
        status: "PASSED",
        passed: 1,
        failed: 0,
        total: 1,
        durationMs: 1,
        files: [],
        dryRun: false,
      },
    ]);
    const store = new RunHistoryStore(memento);
    const loaded = store.all();
    expect(loaded).toHaveLength(1);
    expect(loaded[0].pinned).toBe(false);
    expect(loaded[0].selectors).toEqual([]);
  });
});

describe("RunHistoryStore.entryFromReport", () => {
  it("copies files and selectors onto the entry and defaults pinned to false", () => {
    const report = {
      schema_version: 1,
      version: "1",
      duration_ms: 10,
      files: [],
      summary: {
        files: 1,
        tests: 1,
        steps: { total: 2, passed: 1, failed: 1 },
        status: "FAILED",
      },
    } as unknown as Report;
    const entry = RunHistoryStore.entryFromReport(report, {
      environment: "staging",
      tags: ["smoke"],
      files: ["tests/a.tarn.yaml"],
      selectors: ["tests/a.tarn.yaml::login"],
      dryRun: false,
    });
    expect(entry.environment).toBe("staging");
    expect(entry.tags).toEqual(["smoke"]);
    expect(entry.files).toEqual(["tests/a.tarn.yaml"]);
    expect(entry.selectors).toEqual(["tests/a.tarn.yaml::login"]);
    expect(entry.pinned).toBe(false);
    expect(entry.status).toBe("FAILED");
  });
});
