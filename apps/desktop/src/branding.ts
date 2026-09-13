import type { App } from "electron";
import { mkdirSync } from "node:fs";

export function configureDesktopIdentity(app: Pick<App, "getPath" | "setPath" | "setName">): void {
  // Resolve the existing profile before changing Electron's display name, so
  // upgrades keep the same catalog, settings, and Chromium session storage.
  const userData = app.getPath("userData");
  const sessionData = app.getPath("sessionData");
  app.setName("Ait");
  for (const [name, path] of [["userData", userData], ["sessionData", sessionData]] as const) {
    mkdirSync(path, { recursive: true });
    app.setPath(name, path);
  }
}
