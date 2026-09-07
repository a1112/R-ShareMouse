import assert from "node:assert/strict";
import { createServer, request as httpRequest } from "node:http";
import { test } from "node:test";
import { daemonBridgeGuard } from "./dev-bridge-security.mjs";

test("daemon bridge rejects cross-origin requests before dispatching any action", async () => {
  let dispatched = 0;
  const server = createServer((request, response) => daemonBridgeGuard(request, response, () => {
    dispatched += 1;
    response.end("local action");
  }));
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  const send = (path, method, headers = {}) => new Promise((resolve, reject) => {
    const request = httpRequest({ host: "127.0.0.1", port, path, method, headers }, (response) => {
      response.resume();
      response.on("end", () => resolve(response.statusCode));
    });
    request.on("error", reject);
    request.end(method === "POST" ? '"Status"' : undefined);
  });
  try {
    for (const [path, method] of [
      ["/__rshare/ipc", "POST"], ["/__rshare/service", "POST"],
      ["/__rshare/display-capture", "POST"], ["/__rshare/logs", "GET"],
      ["/__rshare/logs", "DELETE"],
    ]) {
      for (const origin of ["https://untrusted.example", "null", "http://localhost:9999"]) {
        assert.equal(await send(path, method, { origin, "content-type": "application/json" }), 403);
      }
    }
    assert.equal(await send("/__rshare/ipc", "POST", { "content-type": "text/plain" }), 403);
    assert.equal(await send("/__rshare/logs", "GET", { "sec-fetch-site": "cross-site" }), 403);
    assert.equal(await send("/__rshare/logs", "GET", { host: `untrusted.example:${port}` }), 403);
    assert.equal(dispatched, 0);
    assert.equal(await send("/__rshare/ipc", "POST", {
      origin: `http://127.0.0.1:${port}`, "content-type": "application/json; charset=utf-8",
    }), 200);
    assert.equal(await send("/__rshare/logs", "GET"), 200);
    assert.equal(dispatched, 2);
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});

test("daemon bridge only accepts local socket peers and well-formed local authorities", () => {
  const check = (remoteAddress, host, origin) => {
    let allowed = false;
    daemonBridgeGuard({ socket: { remoteAddress }, method: "GET", headers: { host, origin } },
      { setHeader() {}, end() {} }, () => { allowed = true; });
    return allowed;
  };
  assert.equal(check("::1", "[::1]:5176", "http://[::1]:5176"), true);
  assert.equal(check("::ffff:127.0.0.1", "localhost:5176", "http://localhost:5176"), true);
  for (const address of ["192.168.1.2", "::ffff:192.168.1.2", undefined]) {
    assert.equal(check(address, "localhost:5176"), false);
  }
  for (const host of [undefined, "localhost:5176/path", "user@localhost:5176", "localhost.evil:5176"]) {
    assert.equal(check("127.0.0.1", host), false);
  }
});
