import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { deleteStartupDatabase } from "../src/startup-recovery.js";

test("deleting the selected catalog removes only its SQLite files and is retryable", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ait-startup-reset-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const database = join(root, "ait-development.sqlite3");
  for (const suffix of ["", "-wal", "-shm", "-journal", ".lock"]) await writeFile(database + suffix, "old");
  await writeFile(join(root, "ait.sqlite3"), "production");
  await mkdir(join(root, "project/.ait"), { recursive: true });
  await writeFile(join(root, "project/.ait/project.sqlite3"), "project history");
  await deleteStartupDatabase(database);
  await deleteStartupDatabase(database);
  assert.deepEqual((await readdir(root)).sort(), ["ait-development.sqlite3.lock", "ait.sqlite3", "project"]);
  assert.equal(await readFile(join(root, "ait.sqlite3"), "utf8"), "production");
  assert.equal(await readFile(join(root, "project/.ait/project.sqlite3"), "utf8"), "project history");
});

test("preflight rejects directories and links before deleting any database file", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ait-startup-reset-invalid-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const database = join(root, "catalog.sqlite3");
  await writeFile(database, "old");
  await writeFile(database + "-wal", "pending");
  await mkdir(database + "-shm");
  await assert.rejects(deleteStartupDatabase(database), /non-file/);
  assert.equal(await readFile(database, "utf8"), "old");
  assert.equal(await readFile(database + "-wal", "utf8"), "pending");
  await rm(database + "-shm", { recursive: true });
  await symlink(database, database + "-shm");
  await assert.rejects(deleteStartupDatabase(database), /non-file/);
  assert.equal(await readFile(database, "utf8"), "old");
});
