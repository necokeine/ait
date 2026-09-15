import type { OpenDialogOptions } from "electron";
import type { SettingsResponse } from "./types.js";

export const defaultWorkdirSetting = "projects.default_workdir";

/** Resolve the core's empty host-default path at the trusted desktop boundary. */
export function desktopSettings(settings: SettingsResponse, documents: string): SettingsResponse {
  return {
    ...settings,
    schema: {
      ...settings.schema,
      definitions: settings.schema.definitions.map((definition) => definition.id === defaultWorkdirSetting
        && definition.defaultValue === "" ? { ...definition, defaultValue: documents } : definition),
    },
    values: {
      ...settings.values,
      [defaultWorkdirSetting]: settings.values[defaultWorkdirSetting] || documents,
    },
  };
}

export function directoryDialogOptions(defaultPath: string): OpenDialogOptions {
  return {
    title: "Choose a Project directory",
    defaultPath,
    properties: ["openDirectory", "createDirectory"],
  };
}
