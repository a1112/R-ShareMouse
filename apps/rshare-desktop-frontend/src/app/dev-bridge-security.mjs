import { isIP } from "node:net";

function isLoopbackAddress(address) {
  if (typeof address !== "string") return false;
  if (address === "::1") return true;
  const ipv4 = address.startsWith("::ffff:") ? address.slice(7) : address;
  return isIP(ipv4) === 4 && ipv4.startsWith("127.");
}

function isLocalBridgeRequest(request) {
  if (!isLoopbackAddress(request.socket?.remoteAddress)) return false;
  if (request.headers["sec-fetch-site"] === "cross-site") return false;

  const host = request.headers.host;
  if (typeof host !== "string") return false;
  try {
    const protocol = request.socket.encrypted ? "https:" : "http:";
    const target = new URL(`${protocol}//${host}`);
    if (target.host !== host || target.pathname !== "/" || target.search || target.hash) {
      return false;
    }
    const hostname = target.hostname.replace(/^\[|\]$/g, "");
    if (hostname !== "localhost" && !isLoopbackAddress(hostname)) return false;

    // Browsers supply Origin for writes; native local tools may omit it.
    // Comparing the entire origin also rejects null, credentials and paths.
    const origin = request.headers.origin;
    if (origin !== undefined && origin !== target.origin) return false;
    if (request.method === "POST") {
      const contentType = request.headers["content-type"];
      if (typeof contentType !== "string"
          || contentType.split(";", 1)[0].trim().toLowerCase() !== "application/json") {
        return false;
      }
    }
    return true;
  } catch {
    return false;
  }
}

// Mounted before every /__rshare handler, including logs and service control.
// Vite's response CORS policy alone does not prevent cross-origin side effects.
export function daemonBridgeGuard(request, response, next) {
  if (isLocalBridgeRequest(request)) {
    next();
    return;
  }
  response.statusCode = 403;
  response.setHeader("Content-Type", "application/json; charset=utf-8");
  response.end(JSON.stringify({ error: "Daemon bridge requires a same-origin loopback request" }));
}
