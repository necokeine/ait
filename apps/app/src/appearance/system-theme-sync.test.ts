// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { darkTheme, lightTheme } from "@/styles/theme";
import { subscribeToSystemTheme } from "./system-theme-sync.web";

const { runtime } = vi.hoisted(() => ({
  runtime: {
    themeName: "light" as "light" | "dark" | undefined,
    hasAdaptiveThemes: true,
    updateTheme: vi.fn(),
  },
}));

vi.mock("react-native-unistyles", () => ({ UnistylesRuntime: runtime }));

let media: MediaQueryList;
let unsubscribe: (() => void) | undefined;

beforeEach(() => {
  runtime.themeName = "light";
  runtime.hasAdaptiveThemes = true;
  runtime.updateTheme.mockClear();
  media = Object.assign(new window.EventTarget(), {
    matches: false,
    media: "(prefers-color-scheme: dark)",
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
  });
  vi.spyOn(window, "matchMedia").mockReturnValue(media);
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
  unsubscribe = subscribeToSystemTheme();
});

afterEach(() => {
  unsubscribe?.();
  vi.restoreAllMocks();
});

describe("system theme synchronization", () => {
  it("publishes the current palette for both directions of an automatic switch", () => {
    runtime.themeName = "dark";
    media.dispatchEvent(new Event("change"));
    expect(runtime.updateTheme).toHaveBeenLastCalledWith("dark", expect.any(Function));
    expect(runtime.updateTheme.mock.calls[0][1](darkTheme)).toBe(darkTheme);

    runtime.themeName = "light";
    media.dispatchEvent(new Event("change"));
    expect(runtime.updateTheme).toHaveBeenLastCalledWith("light", expect.any(Function));
    expect(runtime.updateTheme.mock.calls[1][1](lightTheme)).toBe(lightTheme);
  });

  it.each(["focus", "pageshow", "visibilitychange"])(
    "recovers a missed media notification on %s without refreshing unchanged palettes",
    (event) => {
      const target = event === "visibilitychange" ? document : window;
      target.dispatchEvent(new Event(event));
      expect(runtime.updateTheme).not.toHaveBeenCalled();

      runtime.themeName = "dark";
      target.dispatchEvent(new Event(event));
      target.dispatchEvent(new Event(event));
      expect(runtime.updateTheme).toHaveBeenCalledTimes(1);
      expect(runtime.updateTheme).toHaveBeenCalledWith("dark", expect.any(Function));
    },
  );

  it("defers hidden-page updates until the page becomes visible", () => {
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    runtime.themeName = "dark";
    media.dispatchEvent(new Event("change"));
    window.dispatchEvent(new Event("focus"));
    expect(runtime.updateTheme).not.toHaveBeenCalled();

    vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    expect(runtime.updateTheme).toHaveBeenCalledWith("dark", expect.any(Function));
  });

  it("does not override a fixed theme or an unavailable theme", () => {
    runtime.hasAdaptiveThemes = false;
    runtime.themeName = "dark";
    media.dispatchEvent(new Event("change"));
    expect(runtime.updateTheme).not.toHaveBeenCalled();

    runtime.hasAdaptiveThemes = true;
    runtime.themeName = undefined;
    window.dispatchEvent(new Event("focus"));
    expect(runtime.updateTheme).not.toHaveBeenCalled();
  });

  it("removes every listener when unsubscribed", () => {
    unsubscribe?.();
    runtime.themeName = "dark";
    media.dispatchEvent(new Event("change"));
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("focus"));
    window.dispatchEvent(new Event("pageshow"));
    expect(runtime.updateTheme).not.toHaveBeenCalled();
  });
});
