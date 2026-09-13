import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { configureDesktopIdentity } from "../src/branding.js";

test("renaming the app preserves an existing profile and separate Chromium session path", async () => {
  const directory = await mkdtemp(join(tmpdir(), "ait-branding-"));
  try {
    let name = "@ait/desktop";
    const userData = join(directory, name);
    const sessionData = join(directory, "custom-session");
    const overrides = new Map<string, string>();
    configureDesktopIdentity({
      getPath: (key) => overrides.get(key)
        ?? (key === "sessionData" ? sessionData : join(directory, name)),
      setName: (value) => { name = value; },
      setPath: (key, value) => {
        assert.ok(existsSync(value), "Electron requires an existing directory");
        overrides.set(key, value);
      },
    });
    assert.equal(name, "Ait");
    assert.equal(overrides.get("userData"), userData);
    assert.equal(overrides.get("sessionData"), sessionData);
    assert.ok(!existsSync(join(directory, "Ait")), "renaming must not create a fresh profile");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
