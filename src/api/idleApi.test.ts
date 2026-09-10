import { describe, expect, it, vi } from "vitest";

const core = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => core);

import { getIdleSeconds, getIdleStats } from "./idleApi";

describe("idle API", () => {
  it("reads persisted idle periods", async () => {
    const periods = [{ startedAt: 100, endedAt: 200 }];
    core.invoke.mockResolvedValue(periods);
    await expect(getIdleStats()).resolves.toEqual(periods);
    expect(core.invoke).toHaveBeenCalledWith("get_idle_stats");
  });
  it("returns the duration reported by the native idle detector", async () => {
    core.invoke.mockResolvedValue(42);

    await expect(getIdleSeconds()).resolves.toBe(42);
    expect(core.invoke).toHaveBeenCalledWith("get_idle_seconds");
  });
});
