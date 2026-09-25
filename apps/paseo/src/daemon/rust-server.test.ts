import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { RustServerManager, resolveDesktopServerHome } from "./rust-server";

const managers: RustServerManager[] = [];
const homes: string[] = [];
function create(binary: string, home = mkdtempSync(path.join(tmpdir(), "ait-desktop-test-"))) {
  homes.push(home);
  const manager = new RustServerManager({ binary, home, timeoutMs: 15000 });
  managers.push(manager);
  return manager;
}
afterEach(async () => {
  for (const manager of managers.splice(0)) await manager.stop();
  for (const home of homes.splice(0)) rmSync(home, { recursive: true, force: true });
});

it("reports spawn failure and allows stop after a failed start", async () => {
  const manager = create("/missing/ait-server");
  await expect(manager.start()).rejects.toThrow();
  expect(manager.status()).toMatchObject({ status: "errored", ownedByDesktop: false });
  await expect(manager.stop()).resolves.toMatchObject({ status: "stopped" });
});

describe.skipIf(!process.env.AIT_SERVER_BIN)("real Rust child lifecycle", () => {
  it("serializes startup, authenticates readiness, rotates credentials and cleans up on restart/stop", async () => {
    const manager = create(process.env.AIT_SERVER_BIN!);
    const [a, b] = await Promise.all([manager.start(), manager.start()]);
    expect(a).toEqual(b);
    expect(a.status).toBe("running");
    expect(a.serverId).not.toBe("");
    const token = manager.authorization(`ws://${a.listen}/v1/ws`)!;
    expect(token).toHaveLength(64);
    expect(manager.authorization(`ws://localhost:${a.listen!.split(":").at(-1)}/v1/ws`)).toBe(
      token,
    );
    expect(manager.authorization(`ws://${a.listen}/v1/ws?token=x`)).toBeUndefined();
    expect(manager.authorization("ws://127.0.0.1:1/v1/ws")).toBeUndefined();
    expect(JSON.stringify(a)).not.toContain(token);
    expect(readFileSync(path.join(a.home, "daemon.log"), "utf8")).not.toContain(token);
    await expect(manager.stop({ pid: a.pid! + 1, startedAt: a.startedAt! })).rejects.toThrow(
      "changed",
    );
    expect(manager.status().status).toBe("running");
    const restarted = await manager.restart();
    expect(restarted.serverId).toBe(a.serverId);
    expect(restarted.listen).toBe(a.listen);
    expect(restarted.pid).not.toBe(a.pid);
    expect(manager.authorization(`ws://${restarted.listen}/v1/ws`)).not.toBe(token);
    expect(() => process.kill(a.pid!, 0)).toThrow();
    await manager.stop();
    expect(() => process.kill(restarted.pid!, 0)).toThrow();
    expect(manager.authorization(`ws://${restarted.listen}/v1/ws`)).toBeUndefined();
  }, 40000);

  it("does not adopt or stop another process using the same data directory", async () => {
    const first = create(process.env.AIT_SERVER_BIN!);
    const running = await first.start();
    const second = create(process.env.AIT_SERVER_BIN!, running.home);
    await expect(second.start()).rejects.toThrow();
    await second.stop();
    expect(() => process.kill(running.pid!, 0)).not.toThrow();
    expect(first.status().status).toBe("running");
  }, 30000);
});

it("keeps Rust desktop data separate from the legacy Paseo home", () => {
  expect(resolveDesktopServerHome({ PASEO_HOME: "/legacy-paseo" })).not.toBe("/legacy-paseo");
  expect(
    resolveDesktopServerHome({ AIT_SERVER_DATA_DIR: "/rust-desktop", PASEO_HOME: "/legacy-paseo" }),
  ).toBe("/rust-desktop");
});
