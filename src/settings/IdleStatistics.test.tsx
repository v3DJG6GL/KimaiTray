// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({ getIdleStats: vi.fn() }));
vi.mock("../api/idleApi", () => api);
vi.mock("react-i18next", () => ({ useTranslation: () => ({
  t: (key: string, values?: Record<string, unknown>) => values ? `${key} ${JSON.stringify(values)}` : key,
  i18n: { language: "en" },
}) }));
import IdleStatistics from "./IdleStatistics";

async function show() {
  await act(async () => { fireEvent.click(screen.getByText("idle.statsShow")); });
}

beforeEach(() => { vi.useFakeTimers(); api.getIdleStats.mockReset(); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe("idle statistics view", () => {
  it("loads on demand, displays empty history and stops refreshing when hidden", async () => {
    api.getIdleStats.mockResolvedValue([]);
    render(<IdleStatistics retentionDays={30} />);
    expect(api.getIdleStats).not.toHaveBeenCalled();
    await show();
    expect(screen.getByText("idle.statsEmpty")).toBeTruthy();
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
    expect(api.getIdleStats).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByText("idle.statsHide"));
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
    expect(api.getIdleStats).toHaveBeenCalledTimes(2);
  });

  it("shows loading and recovers from a failed read", async () => {
    let reject!: (error: Error) => void;
    api.getIdleStats.mockReturnValueOnce(new Promise((_, fail) => { reject = fail; }));
    render(<IdleStatistics retentionDays={1} />);
    await show();
    expect(screen.getByRole("status").textContent).toBe("idle.statsLoading");
    await act(async () => reject(new Error("unavailable")));
    expect(screen.getByRole("alert").textContent).toBe("idle.statsError");
    api.getIdleStats.mockResolvedValue([]);
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("renders totals, chart and paginated history, then reloads retention", async () => {
    const now = Math.floor(Date.now() / 1000);
    api.getIdleStats.mockResolvedValue(Array.from({ length: 21 }, (_, i) => ({ startedAt: now - 4200 + i * 200, endedAt: now - 4100 + i * 200 })));
    const { rerender } = render(<IdleStatistics retentionDays={30} />);
    await show();
    expect(screen.getAllByRole("row")).toHaveLength(21);
    expect(screen.getByRole("img")).toBeTruthy();
    expect(screen.getByText(/idle.statsSummary/).textContent).toContain('"count":21');
    fireEvent.click(screen.getByText("idle.statsNext"));
    expect(screen.getAllByRole("row")).toHaveLength(2);
    fireEvent.click(screen.getByText("idle.statsPrevious"));
    expect(screen.getAllByRole("row")).toHaveLength(21);
    api.getIdleStats.mockResolvedValue([{ startedAt: now - 60, endedAt: now }]);
    await act(async () => { rerender(<IdleStatistics retentionDays={1} />); });
    expect(screen.getAllByRole("row")).toHaveLength(2);
    expect(screen.queryByText("idle.statsNext")).toBeNull();
  });

  it.each([false, true])("ignores a pending response after unmount (failure: %s)", async (failure) => {
    let resolve!: (value: unknown) => void;
    let reject!: (error: Error) => void;
    api.getIdleStats.mockReturnValue(new Promise((ok, fail) => { resolve = ok; reject = fail; }));
    const { unmount } = render(<IdleStatistics retentionDays={30} />);
    await show();
    unmount();
    await act(async () => { if (failure) reject(new Error("late")); else resolve([]); });
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
    expect(api.getIdleStats).toHaveBeenCalledTimes(1);
  });
});
