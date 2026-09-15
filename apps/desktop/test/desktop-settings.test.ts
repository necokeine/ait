import assert from "node:assert/strict";
import test from "node:test";
import { defaultWorkdirSetting, desktopSettings, directoryDialogOptions } from "../src/desktop-settings.js";
import type { SettingsResponse } from "../src/types.js";

function coreSettings(path: string, revision = 1): SettingsResponse {
  return {
    schema: { revision: 3, definitions: [{
      id: defaultWorkdirSetting, category: "projects", label: "Default work directory",
      description: "Directory offered when creating a Project.", kind: { type: "path" },
      defaultValue: "", restartRequired: false,
    }] },
    values: { [defaultWorkdirSetting]: path, "interface.theme": "dark" },
    revision,
  };
}

test("new and legacy empty settings resolve to host Documents without mutating core state or revision", () => {
  for (const revision of [1, 12]) {
    const core = coreSettings("", revision);
    const original = structuredClone(core);
    const result = desktopSettings(core, "/Users/test/文档");
    assert.equal(result.values[defaultWorkdirSetting], "/Users/test/文档");
    assert.equal(result.schema.definitions[0]?.defaultValue, "/Users/test/文档");
    assert.equal(result.values["interface.theme"], "dark");
    assert.equal(result.revision, revision);
    assert.deepEqual(core, original);
  }
});

test("saved directories are preserved and reset resolves the host default again", () => {
  for (const selected of ["/Volumes/Work/项目 & notes", "C:\\Users\\Test User\\Projects"]) {
    const saved = desktopSettings(coreSettings(selected, 13), "/host/Documents");
    assert.equal(saved.values[defaultWorkdirSetting], selected);
    assert.equal(saved.revision, 13);
    assert.equal(desktopSettings(saved, "/host/Documents").values[defaultWorkdirSetting], selected);
    const reset = desktopSettings(coreSettings("", 14), "/host/Documents");
    assert.equal(reset.values[defaultWorkdirSetting], "/host/Documents");
    assert.equal(reset.revision, 14);
  }
});

test("native picker starts at the supplied path and accepts only directories", () => {
  const options = directoryDialogOptions("/Volumes/Work/项目 & notes");
  assert.equal(options.defaultPath, "/Volumes/Work/项目 & notes");
  assert.ok(options.properties?.includes("openDirectory"));
  assert.ok(!options.properties?.includes("openFile"));
  assert.ok(!options.properties?.includes("multiSelections"));
});
