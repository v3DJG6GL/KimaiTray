// @vitest-environment jsdom

import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSettings } from "../types";

const mocks = vi.hoisted(() => ({
  getCurrentWindow: vi.fn(),
  scaleFactor: vi.fn(),
  onResized: vi.fn(),
  resizeCleanup: vi.fn(),
  resizeListener: undefined as ((event: { payload: { width: number; height: number } }) => void) | undefined,
  loadSettings: vi.fn(),
  onSettingsChange: vi.fn(),
  patchSettings: vi.fn(),
  settingsCleanup: vi.fn(),
  settingsListener: undefined as ((settings: AppSettings) => void) | undefined,
  setPopupCornerRadius: vi.fn(),
  setPopupSize: vi.fn(),
  setPopupZoom: vi.fn(),
  setPopupVibrancy: vi.fn(),
  setDisplayMode: vi.fn(),
  setTrayIconSize: vi.fn(),
  setTrayIconShape: vi.fn(),
  addMediaListener: vi.fn(),
  removeMediaListener: vi.fn(),
  mediaListener: undefined as ((event: { matches: boolean }) => void) | undefined,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: mocks.getCurrentWindow,
}));
vi.mock("../settings/service", () => ({
  loadSettings: mocks.loadSettings,
  onSettingsChange: mocks.onSettingsChange,
  patchSettings: mocks.patchSettings,
}));
vi.mock("../api/trayApi", () => ({
  setPopupCornerRadius: mocks.setPopupCornerRadius,
  setPopupSize: mocks.setPopupSize,
  setPopupZoom: mocks.setPopupZoom,
  setPopupVibrancy: mocks.setPopupVibrancy,
  setDisplayMode: mocks.setDisplayMode,
  setTrayIconSize: mocks.setTrayIconSize,
  setTrayIconShape: mocks.setTrayIconShape,
}));

import { useAppearance } from "./useAppearance";

function settings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    accentStyle: "blue",
    reduceVisualEffects: false,
    uiSize: "default",
    roundedPopupCorners: true,
    theme: "light",
    popupLayout: "classic",
    displayMode: "tray",
    popupWidth: 360,
    popupHeight: 640,
    trayIconSize: "medium",
    trayIconShape: "dot",
    ...overrides,
  } as AppSettings;
}

describe("appearance synchronization", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    document.documentElement.className = "";
    document.documentElement.dataset.window = "tray-popup";
    mocks.resizeListener = undefined;
    mocks.settingsListener = undefined;
    mocks.mediaListener = undefined;
    mocks.getCurrentWindow.mockReturnValue({
      scaleFactor: mocks.scaleFactor,
      onResized: mocks.onResized,
    });
    mocks.scaleFactor.mockResolvedValue(2);
    mocks.onResized.mockImplementation((listener) => {
      mocks.resizeListener = listener;
      return Promise.resolve(mocks.resizeCleanup);
    });
    mocks.loadSettings.mockResolvedValue(settings());
    mocks.onSettingsChange.mockImplementation((listener) => {
      mocks.settingsListener = listener;
      return Promise.resolve(mocks.settingsCleanup);
    });
    mocks.patchSettings.mockResolvedValue(undefined);
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({
        matches: true,
        addEventListener: (_event: string, listener: (event: { matches: boolean }) => void) => {
          mocks.mediaListener = listener;
          mocks.addMediaListener();
        },
        removeEventListener: mocks.removeMediaListener,
      })),
    });
  });

  afterEach(() => vi.useRealTimers());

  it("applies tray settings, persists bounded resize and reacts to transparent detached mode", async () => {
    const { unmount } = renderHook(() => useAppearance());

    await waitFor(() => expect(mocks.setPopupSize).toHaveBeenCalledOnce());
    expect(mocks.setPopupSize).toHaveBeenCalledWith(360, 640, 1);
    expect(mocks.setPopupCornerRadius).toHaveBeenCalledWith(10);
    expect(mocks.setPopupVibrancy).toHaveBeenCalledWith(false);
    expect(mocks.setDisplayMode).toHaveBeenCalledWith("tray");
    expect(mocks.setTrayIconSize).toHaveBeenCalledWith("medium");
    expect(mocks.setTrayIconShape).toHaveBeenCalledWith("dot");
    expect(document.documentElement.dataset).toMatchObject({
      accent: "blue",
      reduceMotion: "false",
      uiSize: "default",
      roundedPopup: "true",
      theme: "light",
      layout: "classic",
      displayMode: "tray",
    });
    expect(document.documentElement.classList.contains("dark")).toBe(false);

    act(() => mocks.resizeListener?.({ payload: { width: 720, height: 100 } }));
    await waitFor(
      () => expect(mocks.patchSettings).toHaveBeenCalledWith({ popupWidth: 360, popupHeight: 320 }),
      { timeout: 1_000 },
    );

    act(() => mocks.resizeListener?.({ payload: { width: 720, height: 9_999 } }));
    await waitFor(
      () => expect(mocks.patchSettings).toHaveBeenCalledWith({ popupWidth: 360, popupHeight: 1_200 }),
      { timeout: 1_000 },
    );

    const detached = settings({
      accentStyle: "purple",
      reduceVisualEffects: true,
      uiSize: "scale130",
      roundedPopupCorners: false,
      theme: "transparent",
      popupLayout: "focus",
      displayMode: "detached",
      trayIconSize: "large",
      trayIconShape: "square",
    });
    act(() => mocks.settingsListener?.(detached));

    expect(mocks.setPopupZoom).toHaveBeenCalledWith(1.3);
    expect(mocks.setPopupCornerRadius).toHaveBeenLastCalledWith(0);
    expect(mocks.setPopupVibrancy).toHaveBeenLastCalledWith(true);
    expect(mocks.setDisplayMode).toHaveBeenLastCalledWith("detached");
    expect(mocks.setTrayIconSize).toHaveBeenLastCalledWith("large");
    expect(mocks.setTrayIconShape).toHaveBeenLastCalledWith("square");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(mocks.addMediaListener).toHaveBeenCalledOnce();

    act(() => mocks.mediaListener?.({ matches: false }));
    expect(document.documentElement.classList.contains("dark")).toBe(false);

    const zoomCalls = mocks.setPopupZoom.mock.calls.length;
    act(() => mocks.settingsListener?.(detached));
    expect(mocks.setPopupZoom).toHaveBeenCalledTimes(zoomCalls);
    expect(mocks.removeMediaListener).toHaveBeenCalledOnce();

    unmount();
    await waitFor(() => expect(mocks.settingsCleanup).toHaveBeenCalledOnce());
    expect(mocks.resizeCleanup).toHaveBeenCalledOnce();
    expect(mocks.removeMediaListener).toHaveBeenCalledTimes(2);
  });

  it("applies dark appearance outside the tray without native resize tracking", async () => {
    document.documentElement.dataset.window = "settings";
    mocks.loadSettings.mockResolvedValue(settings({
      theme: "dark",
      popupWidth: 0,
      popupHeight: 0,
      displayMode: undefined,
      trayIconSize: undefined,
      trayIconShape: undefined,
      uiSize: "small",
    }));
    const { unmount } = renderHook(() => useAppearance());
    await waitFor(() => expect(document.documentElement.classList.contains("dark")).toBe(true));
    expect(mocks.scaleFactor).not.toHaveBeenCalled();
    expect(mocks.setPopupSize).toHaveBeenCalledWith(306, 544, 0.85);
    expect(mocks.setPopupVibrancy).not.toHaveBeenCalled();
    unmount();
    await waitFor(() => expect(mocks.settingsCleanup).toHaveBeenCalled());
  });

  it("ignores irrelevant resize events and debounces the latest height", async () => {
    vi.useFakeTimers();
    const { unmount } = renderHook(() => useAppearance());
    await act(async () => Promise.resolve());
    act(() => mocks.resizeListener?.({ payload: { width: 720, height: 1_280 } }));
    expect(mocks.patchSettings).not.toHaveBeenCalled();

    act(() => mocks.settingsListener?.(settings({ popupHeight: 600 })));
    act(() => mocks.resizeListener?.({ payload: { width: 720, height: 1_200 } }));
    expect(mocks.patchSettings).not.toHaveBeenCalled();
    mocks.patchSettings.mockRejectedValueOnce(new Error("disk"));
    act(() => mocks.resizeListener?.({ payload: { width: 720, height: 1_400 } }));
    act(() => mocks.resizeListener?.({ payload: { width: 720, height: 1_600 } }));
    await act(async () => vi.advanceTimersByTime(250));
    expect(mocks.patchSettings).toHaveBeenCalledTimes(1);
    expect(mocks.patchSettings).toHaveBeenCalledWith({ popupWidth: 360, popupHeight: 800 });

    act(() => mocks.settingsListener?.(settings({ displayMode: "detached" })));
    act(() => mocks.resizeListener?.({ payload: { width: 720, height: 2_000 } }));
    expect(mocks.patchSettings).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("restores width and persists horizontal resizing with display and UI scaling", async () => {
    vi.useFakeTimers();
    mocks.loadSettings.mockResolvedValue(settings({ popupWidth: 500, uiSize: "scale130" }));
    const { unmount } = renderHook(() => useAppearance());
    await act(async () => Promise.resolve());
    expect(mocks.setPopupSize).toHaveBeenLastCalledWith(650, 832, 1.3);

    act(() => mocks.resizeListener?.({ payload: { width: 1560, height: 1664 } }));
    await act(async () => vi.advanceTimersByTime(250));
    expect(mocks.patchSettings).toHaveBeenLastCalledWith({ popupWidth: 600, popupHeight: 640 });

    // A subsequent height change must not reset the user-selected width.
    act(() => mocks.settingsListener?.(settings({ popupWidth: 600, popupHeight: 700, uiSize: "scale130" })));
    expect(mocks.setPopupSize).toHaveBeenLastCalledWith(780, 910, 1.3);
    unmount();
  });

  it.each([0, Number.NaN])("ignores invalid display scale %s", async (scale) => {
    vi.useFakeTimers();
    mocks.scaleFactor.mockResolvedValue(scale);
    const { unmount } = renderHook(() => useAppearance());
    await act(async () => Promise.resolve());
    act(() => mocks.resizeListener?.({ payload: { width: 1400, height: 1600 } }));
    await act(async () => vi.advanceTimersByTime(250));
    expect(mocks.patchSettings).not.toHaveBeenCalled();
    unmount();
  });

  it("does not persist duplicate resize events after dimensions settle", async () => {
    vi.useFakeTimers();
    const { unmount } = renderHook(() => useAppearance());
    await act(async () => Promise.resolve());
    act(() => mocks.resizeListener?.({ payload: { width: 1400, height: 1600 } }));
    await act(async () => vi.advanceTimersByTime(250));
    expect(mocks.patchSettings).toHaveBeenCalledExactlyOnceWith({ popupWidth: 700, popupHeight: 800 });
    act(() => mocks.resizeListener?.({ payload: { width: 1400, height: 1600 } }));
    await act(async () => vi.advanceTimersByTime(250));
    expect(mocks.patchSettings).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("bounds horizontal resizing and saves the last complete size", async () => {
    vi.useFakeTimers();
    const { unmount } = renderHook(() => useAppearance());
    await act(async () => Promise.resolve());
    act(() => mocks.resizeListener?.({ payload: { width: 100, height: 1280 } }));
    await act(async () => vi.advanceTimersByTime(250));
    expect(mocks.patchSettings).toHaveBeenLastCalledWith({ popupWidth: 300, popupHeight: 640 });

    act(() => mocks.resizeListener?.({ payload: { width: 9999, height: 1280 } }));
    act(() => mocks.resizeListener?.({ payload: { width: 9999, height: 1600 } }));
    await act(async () => vi.advanceTimersByTime(250));
    expect(mocks.patchSettings).toHaveBeenLastCalledWith({ popupWidth: 1000, popupHeight: 800 });
    expect(mocks.patchSettings).toHaveBeenCalledTimes(2);
    unmount();
  });

  it.each([
    [2, 1, "default", 700, 640, 700, 640],
    [1, 2, "default", 1400, 1280, 700, 640],
    [2, 1.5, "scale130", 1365, 1248, 700, 640],
  ] as const)("uses current DPI after %s → %s with UI scale %s", async (
    initialDpi, nextDpi, uiSize, width, height, popupWidth, popupHeight,
  ) => {
    vi.useFakeTimers();
    mocks.scaleFactor.mockResolvedValue(initialDpi);
    mocks.loadSettings.mockResolvedValue(settings({ uiSize }));
    const { unmount } = renderHook(() => useAppearance());
    await act(async () => Promise.resolve());
    // Resize once on the initial display, then switch monitors without remounting.
    act(() => mocks.resizeListener?.({ payload: { width: 1000, height: 1280 } }));
    await act(async () => vi.advanceTimersByTime(250));
    mocks.patchSettings.mockClear();
    mocks.scaleFactor.mockResolvedValue(nextDpi);
    act(() => mocks.resizeListener?.({ payload: { width, height } }));
    await act(async () => vi.advanceTimersByTime(250));
    expect(mocks.patchSettings).toHaveBeenCalledExactlyOnceWith({ popupWidth, popupHeight });
    unmount();
  });

  it("ignores stale DPI responses and responses after unmount", async () => {
    vi.useFakeTimers();
    const { unmount } = renderHook(() => useAppearance());
    await act(async () => Promise.resolve());
    let resolveScale!: (scale: number) => void;
    mocks.scaleFactor.mockReturnValueOnce(new Promise((resolve) => { resolveScale = resolve; }));
    act(() => mocks.resizeListener?.({ payload: { width: 1000, height: 1280 } }));
    await act(async () => vi.advanceTimersByTime(250));
    act(() => mocks.resizeListener?.({ payload: { width: 1400, height: 1280 } }));
    await act(async () => resolveScale(2));
    expect(mocks.patchSettings).not.toHaveBeenCalled();
    await act(async () => vi.advanceTimersByTime(250));
    expect(mocks.patchSettings).toHaveBeenCalledExactlyOnceWith({ popupWidth: 700, popupHeight: 640 });

    mocks.scaleFactor.mockReturnValueOnce(new Promise((resolve) => { resolveScale = resolve; }));
    act(() => mocks.resizeListener?.({ payload: { width: 1600, height: 1280 } }));
    await act(async () => vi.advanceTimersByTime(250));
    unmount();
    await act(async () => resolveScale(2));
    expect(mocks.patchSettings).toHaveBeenCalledTimes(1);
  });

  it("cleans up listeners that resolve after unmount and ignores late settings", async () => {
    let resolveResize!: (cleanup: () => void) => void;
    mocks.onResized.mockReturnValueOnce(new Promise((resolve) => { resolveResize = resolve; }));
    let resolveSettings!: (value: AppSettings) => void;
    mocks.loadSettings.mockReturnValueOnce(new Promise((resolve) => { resolveSettings = resolve; }));
    const { unmount } = renderHook(() => useAppearance());
    await act(async () => Promise.resolve());
    unmount();
    await act(async () => {
      resolveResize(mocks.resizeCleanup);
      resolveSettings(settings({ accentStyle: "red" }));
    });
    expect(mocks.resizeCleanup).toHaveBeenCalledOnce();
    expect(document.documentElement.dataset.accent).not.toBe("red");
  });
});
