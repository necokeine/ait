import { lstat, unlink } from "node:fs/promises";

/** Removes only the selected desktop catalog and SQLite sidecars, without a backup. */
export async function deleteStartupDatabase(databasePath: string): Promise<void> {
  // Keep the catalog until its sidecars are gone, so a failed deletion can be retried.
  const paths = ["-wal", "-shm", "-journal", ""].map((suffix) => databasePath + suffix);
  for (const path of paths) {
    try {
      if (!(await lstat(path)).isFile()) throw new Error(`Refusing to delete a non-file database path: ${path}`);
    } catch (error) {
      if (!isMissing(error)) throw error;
    }
  }
  for (const path of paths) {
    try {
      await unlink(path);
    } catch (error) {
      if (!isMissing(error)) throw error;
    }
  }
}

function isMissing(error: unknown): boolean {
  return error instanceof Error && "code" in error && error.code === "ENOENT";
}

/** Only an owned daemon's startup failure can authorize desktop database recovery. */
export class LegacyDatabaseStartupError extends Error {}
