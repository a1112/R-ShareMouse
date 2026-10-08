// Matches crates/rshare-core/src/local_transport.rs. Windows callers supply
// their actual SID/session pipe path; this proxy must not invent an identity.
export function resolveDaemonIpcEndpoint({ platform, uid, env = {} }) {
  if (env.RSHARE_DAEMON_IPC_PATH) return { path: env.RSHARE_DAEMON_IPC_PATH };
  // Explicit override for transport tests, never the production default.
  if (env.RSHARE_DAEMON_IPC_PORT !== undefined) {
    const port = Number(env.RSHARE_DAEMON_IPC_PORT);
    if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("invalid daemon test TCP port");
    return { host: "127.0.0.1", port };
  }
  if (platform === "win32") throw new Error("Windows dev gateway requires the daemon's per-user/session pipe in RSHARE_DAEMON_IPC_PATH");
  if (!Number.isInteger(uid) || uid < 0) throw new Error("daemon Unix IPC requires an effective uid");
  return { path: `/tmp/rshare-ipc-${uid}/daemon-27435.sock` };
}
