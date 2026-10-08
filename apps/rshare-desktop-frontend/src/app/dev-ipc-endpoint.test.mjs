import test from "node:test";
import assert from "node:assert/strict";
import { resolveDaemonIpcEndpoint } from "./dev-ipc-endpoint.mjs";

test("macOS and Linux use the daemon's per-effective-user Unix socket", () => {
  for (const platform of ["darwin", "linux"]) {
    assert.deepEqual(resolveDaemonIpcEndpoint({ platform, uid: 501, env: {} }), { path: "/tmp/rshare-ipc-501/daemon-27435.sock" });
  }
});

test("explicit IPC path preserves the caller's Windows per-user/session endpoint", () => {
  const path = String.raw`\\.\pipe\RShareMouse-S-1-5-21-test-2-27435`;
  assert.deepEqual(resolveDaemonIpcEndpoint({ platform: "win32", env: { RSHARE_DAEMON_IPC_PATH: path } }), { path });
  assert.throws(() => resolveDaemonIpcEndpoint({ platform: "win32", env: {} }), /RSHARE_DAEMON_IPC_PATH/);
});

test("TCP is enabled only by an explicit test port and always stays on loopback", () => {
  assert.deepEqual(resolveDaemonIpcEndpoint({ platform: "darwin", uid: 501, env: { RSHARE_DAEMON_IPC_PORT: "32510" } }), { host: "127.0.0.1", port: 32510 });
  assert.throws(() => resolveDaemonIpcEndpoint({ platform: "darwin", uid: 501, env: { RSHARE_DAEMON_IPC_PORT: "wrong" } }), /port/);
  assert.throws(() => resolveDaemonIpcEndpoint({ platform: "darwin", uid: 501, env: { RSHARE_DAEMON_IPC_PORT: "0" } }), /port/);
});

test("Unix endpoints require a valid effective uid", () => {
  assert.throws(() => resolveDaemonIpcEndpoint({ platform: "darwin", env: {} }), /uid/);
});
