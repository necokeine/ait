import { UnistylesRuntime } from "react-native-unistyles";

/** Reconcile retained React theme snapshots with the browser's current system palette. */
export function subscribeToSystemTheme(): () => void {
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  let lastThemeName = UnistylesRuntime.themeName;

  const synchronize = () => {
    if (document.visibilityState === "hidden" || !UnistylesRuntime.hasAdaptiveThemes) return;
    const themeName = UnistylesRuntime.themeName;
    if (!themeName || themeName === lastThemeName) return;
    lastThemeName = themeName;

    // CSS media queries can switch while a retained React tree misses the theme notification.
    // Re-publish the current palette so withUnistyles and appearance boundaries catch up.
    // A normal theme notification has already supplied this same object, so React bails out.
    UnistylesRuntime.updateTheme(themeName, (theme) => theme);
  };

  media.addEventListener("change", synchronize);
  document.addEventListener("visibilitychange", synchronize);
  window.addEventListener("focus", synchronize);
  window.addEventListener("pageshow", synchronize);

  return () => {
    media.removeEventListener("change", synchronize);
    document.removeEventListener("visibilitychange", synchronize);
    window.removeEventListener("focus", synchronize);
    window.removeEventListener("pageshow", synchronize);
  };
}
