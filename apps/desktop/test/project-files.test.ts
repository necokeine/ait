import assert from "node:assert/strict";
import { mkdir, mkdtemp, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { resolveProjectPath, vscodeFileUrl } from "../src/project-files.js";

test("resolves existing paths inside the Project and rejects traversal through symlinks", async () => {
  const temporary = await mkdtemp(join(tmpdir(), "ait-project-files-"));
  try {
    const project = join(temporary, "project");
    const outside = join(temporary, "outside.txt");
    await mkdir(join(project, "src"), { recursive: true });
    await writeFile(join(project, "src", "main.rs"), "fn main() {}\n");
    await writeFile(outside, "private\n");
    await symlink(outside, join(project, "outside-link.txt"));

    assert.equal(await resolveProjectPath(project, "src/main.rs"), await realpath(join(project, "src", "main.rs")));
    await assert.rejects(resolveProjectPath(project, "../outside.txt"), /outside the current Project/);
    await assert.rejects(resolveProjectPath(project, "outside-link.txt"), /outside the current Project/);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});

test("builds an encoded VS Code file URL with a line and column", () => {
  const url = vscodeFileUrl("/tmp/project/my file.rs", 12, 4);
  assert.equal(url, "vscode://file/tmp/project/my%20file.rs:12:4");
});
