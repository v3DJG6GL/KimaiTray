import { describe, expect, it } from "vitest";
import { dailyIdleStats } from "./idleStats";

describe("daily idle statistics", () => {
  it("splits time at local midnight and includes empty days", () => {
    const start = new Date(2026, 8, 9, 23, 50).getTime() / 1000;
    const end = new Date(2026, 8, 10, 0, 20).getTime() / 1000;
    const days = dailyIdleStats([{ startedAt: start, endedAt: end }], 3, new Date(2026, 8, 10, 12));
    expect(days.map((day) => day.seconds)).toEqual([0, 600, 1200]);
  });

  it("uses today's date by default", () => {
    expect(dailyIdleStats([], 1)[0].date.getDate()).toBe(new Date().getDate());
  });
});
