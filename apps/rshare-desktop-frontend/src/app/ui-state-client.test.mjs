import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  UiStateClient,
  createTauriUiStateConnector,
  createWebSocketUiStateConnector,
} from "./ui-state-client.mjs";

const BOOT_A = "00000000-0000-0000-0000-000000000001";
const BOOT_B = "00000000-0000-0000-0000-000000000002";

function snapshot(bootId, revision, marker = revision) {
  return {
    type: "snapshot",
    payload: {
      protocol_version: 1,
      boot_id: bootId,
      revision,
      marker,
      status: {},
      devices: [],
      layout: {},
      capabilities: {},
      display_inventory: {},
      dynamic_state: {},
      active_sessions: {},
    },
  };
}

function pointerDelta(bootId, revision, x = revision) {
  return {
    type: "delta",
    payload: {
      boot_id: bootId,
      revision,
      change: {
        type: "pointer",
        payload: { x, y: 0, observed_at_ms: revision },
      },
    },
  };
}

function extendedMouseButtonDelta(bootId, revision, button = { Other: 7 }) {
  return {
    type: "delta",
    payload: {
      boot_id: bootId,
      revision,
      change: {
        type: "key_button",
        payload: {
          type: "mouse_button",
          button,
          state: "Pressed",
          observed_at_ms: revision,
        },
      },
    },
  };
}

function heartbeat(bootId, revision) {
  return {
    type: "heartbeat",
    payload: { boot_id: bootId, revision, sent_at_ms: Date.now() },
  };
}

function resyncRequired(bootId, revision) {
  return {
    type: "resync_required",
    payload: {
      boot_id: bootId,
      current_revision: revision,
      reason: "revision_gap",
    },
  };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

function wait(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(predicate, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() >= deadline) {
      assert.fail("condition was not met before timeout");
    }
    await wait(5);
  }
}

function scriptedTransport(envelopes, { label = "transport" } = {}) {
  return async ({ cursor, onEnvelope, onDisconnect }) => {
    let closed = false;
    let resyncRequests = 0;
    const timer = setTimeout(() => {
      if (closed) return;
      for (const envelope of envelopes) {
        onEnvelope(structuredClone(envelope));
      }
    }, 0);
    return {
      label,
      cursor,
      close() {
        closed = true;
        clearTimeout(timer);
      },
      requestFullResync() {
        resyncRequests += 1;
      },
      resyncRequestCount() {
        return resyncRequests;
      },
      disconnect(error = new Error(`${label} disconnected`)) {
        if (!closed) onDisconnect(error);
      },
    };
  };
}

function fakeTauriTransport(envelopes) {
  let listener = null;
  let nextStreamId = 0;
  let initialSequenceSent = false;
  const reservations = [];
  const starts = [];
  const stops = [];
  const invoke = async (command, args = {}) => {
    if (command === "reserve_ui_state_stream") {
      const streamId = ++nextStreamId;
      reservations.push(streamId);
      return streamId;
    }
    if (command === "start_ui_state_stream") {
      starts.push({ streamId: args.streamId, cursor: args.cursor });
      if (!initialSequenceSent) {
        initialSequenceSent = true;
        setTimeout(() => {
          for (const envelope of envelopes) {
            listener?.({ payload: structuredClone(envelope) });
          }
        }, 0);
      }
      return undefined;
    }
    if (command === "stop_ui_state_stream") {
      stops.push(args.streamId);
      return undefined;
    }
    throw new Error(`unexpected command ${command}`);
  };
  const listen = async (eventName, handler) => {
    assert.equal(eventName, "rshare://ui-state");
    listener = handler;
    return () => {
      if (listener === handler) listener = null;
    };
  };
  return {
    connect: createTauriUiStateConnector({ invoke, listen }),
    reservations,
    starts,
    stops,
  };
}

function fakeWebSocketTransport(envelopes) {
  const instances = [];
  let initialSequenceSent = false;
  class FakeWebSocket {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSING = 2;
    static CLOSED = 3;

    constructor(url) {
      this.url = url;
      this.readyState = FakeWebSocket.CONNECTING;
      this.listeners = new Map();
      this.sent = [];
      instances.push(this);
      setTimeout(() => {
        this.readyState = FakeWebSocket.OPEN;
        this.emit("open", {});
      }, 0);
    }

    addEventListener(type, listener) {
      const listeners = this.listeners.get(type) ?? [];
      listeners.push(listener);
      this.listeners.set(type, listeners);
    }

    removeEventListener(type, listener) {
      this.listeners.set(
        type,
        (this.listeners.get(type) ?? []).filter((item) => item !== listener),
      );
    }

    emit(type, event) {
      for (const listener of this.listeners.get(type) ?? []) {
        listener(event);
      }
    }

    send(data) {
      this.sent.push(JSON.parse(data));
      const command = this.sent.at(-1);
      if (command.type === "subscribe" && !initialSequenceSent) {
        initialSequenceSent = true;
        setTimeout(() => {
          for (const envelope of envelopes) {
            this.emit("message", {
              data: JSON.stringify(envelope),
            });
          }
        }, 0);
      }
    }

    close(code = 1000) {
      if (this.readyState === FakeWebSocket.CLOSED) return;
      this.readyState = FakeWebSocket.CLOSED;
      this.emit("close", { code, reason: "" });
    }
  }
  return {
    connect: createWebSocketUiStateConnector({
      WebSocket: FakeWebSocket,
      url: "ws://127.0.0.1/ui-state",
    }),
    instances,
  };
}

async function collectWithTransport(connect, envelopes) {
  const delivered = [];
  const statuses = [];
  const client = new UiStateClient({
    connect,
    onEnvelope: (envelope) => delivered.push(envelope),
    onStatus: (status) => statuses.push(status.state),
    heartbeatTimeoutMs: 1000,
  });
  await client.start();
  await waitFor(() => delivered.length === envelopes.length);
  const result = {
    delivered,
    statuses,
    revision: client.currentRevision(),
  };
  await client.stop();
  return result;
}

test("tauri and websocket transports expose identical envelope behavior", async () => {
  const envelopes = [
    snapshot(BOOT_A, 1),
    pointerDelta(BOOT_A, 2),
    heartbeat(BOOT_A, 2),
  ];

  const tauriTransport = fakeTauriTransport(envelopes);
  const websocketTransport = fakeWebSocketTransport(envelopes);
  const tauri = await collectWithTransport(tauriTransport.connect, envelopes);
  const websocket = await collectWithTransport(websocketTransport.connect, envelopes);

  assert.deepEqual(tauri, websocket);
  assert.equal(tauri.revision, 2);
  assert.deepEqual(tauriTransport.stops, [1]);
  assert.deepEqual(websocketTransport.instances[0].sent[0], {
    type: "subscribe",
    cursor: null,
  });
});

test("real Tauri and WebSocket adapters expose equivalent gap and boot handling", async () => {
  const envelopes = [
    snapshot(BOOT_A, 1),
    pointerDelta(BOOT_A, 3),
    heartbeat(BOOT_B, 3),
  ];
  const run = async (transport) => {
    const delivered = [];
    const client = new UiStateClient({
      connect: transport.connect,
      onEnvelope: (envelope) => delivered.push(envelope.type),
      onStatus: () => {},
      heartbeatTimeoutMs: 1000,
    });
    await client.start();
    await waitFor(() => delivered.includes("heartbeat"));
    const result = { delivered, revision: client.currentRevision() };
    await client.stop();
    return result;
  };

  const tauriTransport = fakeTauriTransport(envelopes);
  const websocketTransport = fakeWebSocketTransport(envelopes);
  assert.deepEqual(
    await run(tauriTransport),
    await run(websocketTransport),
  );
  assert.equal(tauriTransport.starts.length, 2);
  assert.equal(
    websocketTransport.instances[0].sent.filter((item) => item.type === "resync")
      .length,
    1,
  );
});

test("Tauri adapter closes only its latest owner stream id after resync", async () => {
  const transport = fakeTauriTransport([]);
  const connection = await transport.connect({
    cursor: null,
    onEnvelope: () => {},
    onDisconnect: () => {},
  });

  await connection.requestFullResync();
  await connection.close();

  assert.deepEqual(
    transport.starts.map((item) => item.streamId),
    [1, 2],
  );
  assert.deepEqual(transport.stops, [2]);
});

test("aborted delayed Tauri listen cannot reserve or replace a newer stream", async () => {
  const firstListen = deferred();
  let listenCount = 0;
  const calls = [];
  let nextId = 0;
  const listen = async () => {
    listenCount += 1;
    return listenCount === 1 ? firstListen.promise : () => {};
  };
  const invoke = async (command, args = {}) => {
    calls.push({ command, args });
    if (command === "reserve_ui_state_stream") return ++nextId;
    return undefined;
  };
  const connect = createTauriUiStateConnector({ invoke, listen });
  const oldController = new AbortController();
  const old = connect({
    cursor: null,
    signal: oldController.signal,
    onEnvelope: () => {},
    onDisconnect: () => {},
  });
  await Promise.resolve();
  oldController.abort();

  const active = await connect({
    cursor: null,
    signal: new AbortController().signal,
    onEnvelope: () => {},
    onDisconnect: () => {},
  });
  firstListen.resolve(() => {});
  await assert.rejects(old, { name: "AbortError" });

  assert.deepEqual(
    calls.filter((call) => call.command === "reserve_ui_state_stream"),
    [{ command: "reserve_ui_state_stream", args: {} }],
  );
  assert.deepEqual(
    calls.filter((call) => call.command === "start_ui_state_stream"),
    [
      {
        command: "start_ui_state_stream",
        args: { cursor: null, streamId: 1 },
      },
    ],
  );
  await active.close();
});

test("a stale delayed Tauri start rejection cannot clear the newer owner", async () => {
  const firstStart = deferred();
  const calls = [];
  let nextId = 0;
  const invoke = async (command, args = {}) => {
    calls.push({ command, args });
    if (command === "reserve_ui_state_stream") return ++nextId;
    if (command === "start_ui_state_stream" && args.streamId === 1) {
      return firstStart.promise;
    }
    return undefined;
  };
  const connect = createTauriUiStateConnector({
    invoke,
    listen: async () => () => {},
  });
  const oldController = new AbortController();
  const old = connect({
    cursor: null,
    signal: oldController.signal,
    onEnvelope: () => {},
    onDisconnect: () => {},
  });
  await waitFor(
    () =>
      calls.some(
        (call) =>
          call.command === "start_ui_state_stream" && call.args.streamId === 1,
      ),
  );
  oldController.abort();

  const active = await connect({
    cursor: { boot_id: BOOT_A, revision: 4 },
    signal: new AbortController().signal,
    onEnvelope: () => {},
    onDisconnect: () => {},
  });
  firstStart.reject(new Error("stale reservation"));
  await assert.rejects(old, /stale reservation/);

  assert.deepEqual(
    calls.filter((call) => call.command === "start_ui_state_stream"),
    [
      {
        command: "start_ui_state_stream",
        args: { cursor: null, streamId: 1 },
      },
      {
        command: "start_ui_state_stream",
        args: {
          cursor: { boot_id: BOOT_A, revision: 4 },
          streamId: 2,
        },
      },
    ],
  );
  assert.deepEqual(
    calls.filter((call) => call.command === "stop_ui_state_stream"),
    [],
  );
  await active.close();
  assert.deepEqual(
    calls.filter((call) => call.command === "stop_ui_state_stream"),
    [
      {
        command: "stop_ui_state_stream",
        args: { streamId: 2 },
      },
    ],
  );
});

test("an aborted Tauri start that already succeeded stops only its reservation", async () => {
  const firstStart = deferred();
  const calls = [];
  let nextId = 0;
  const invoke = async (command, args = {}) => {
    calls.push({ command, args });
    if (command === "reserve_ui_state_stream") return ++nextId;
    if (command === "start_ui_state_stream") return firstStart.promise;
    return undefined;
  };
  const connect = createTauriUiStateConnector({
    invoke,
    listen: async () => () => {},
  });
  const controller = new AbortController();
  const pending = connect({
    cursor: null,
    signal: controller.signal,
    onEnvelope: () => {},
    onDisconnect: () => {},
  });
  await waitFor(
    () =>
      calls.some((call) => call.command === "start_ui_state_stream"),
  );
  controller.abort();
  firstStart.resolve();
  await assert.rejects(pending, { name: "AbortError" });
  assert.deepEqual(
    calls.filter((call) => call.command === "stop_ui_state_stream"),
    [
      {
        command: "stop_ui_state_stream",
        args: { streamId: 1 },
      },
    ],
  );
});

test("real Tauri and WebSocket adapters both reconnect after first-envelope timeout", async () => {
  const run = async (transport, attemptCount) => {
    const states = [];
    const client = new UiStateClient({
      connect: transport.connect,
      onEnvelope: () => {},
      onStatus: (status) => states.push(status.state),
      heartbeatTimeoutMs: 25,
    });
    try {
      await client.start();
      await waitFor(() => states.includes("stale"), 150);
      await waitFor(() => attemptCount() >= 2, 300);
      return states.filter((state) =>
        ["stale", "reconnecting", "connected"].includes(state),
      );
    } finally {
      await client.stop();
    }
  };

  const tauriTransport = fakeTauriTransport([]);
  const websocketTransport = fakeWebSocketTransport([]);
  const tauriStates = await run(
    tauriTransport,
    () => tauriTransport.starts.length,
  );
  const websocketStates = await run(
    websocketTransport,
    () => websocketTransport.instances.length,
  );
  assert.ok(tauriStates.includes("stale"));
  assert.ok(websocketStates.includes("stale"));
  assert.ok(tauriStates.includes("reconnecting"));
  assert.ok(websocketStates.includes("reconnecting"));
});

test("revision gaps never deliver the invalid delta and request one full resync", async () => {
  let handle;
  const delivered = [];
  const connect = async (options) => {
    handle = await scriptedTransport([])(options);
    return handle;
  };
  const client = new UiStateClient({
    connect,
    onEnvelope: (envelope) => delivered.push(envelope),
    onStatus: () => {},
  });

  await client.start();
  client.handleEnvelope(snapshot(BOOT_A, 4));
  client.handleEnvelope(pointerDelta(BOOT_A, 6));
  client.handleEnvelope(pointerDelta(BOOT_A, 7));

  assert.deepEqual(delivered.map((value) => value.type), ["snapshot"]);
  assert.equal(client.currentRevision(), 4);
  assert.equal(handle.resyncRequestCount(), 1);

  client.handleEnvelope(snapshot(BOOT_A, 7));
  client.handleEnvelope(pointerDelta(BOOT_A, 8));
  assert.deepEqual(delivered.map((value) => value.type), [
    "snapshot",
    "snapshot",
    "delta",
  ]);
  assert.equal(client.currentRevision(), 8);
  await client.stop();
});

test("boot changes and explicit resync notices coalesce into one full resync", async () => {
  let handle;
  const connect = async (options) => {
    handle = await scriptedTransport([])(options);
    return handle;
  };
  const client = new UiStateClient({
    connect,
    onEnvelope: () => {},
    onStatus: () => {},
  });

  await client.start();
  client.handleEnvelope(snapshot(BOOT_A, 2));
  client.handleEnvelope(pointerDelta(BOOT_B, 3));
  client.handleEnvelope(resyncRequired(BOOT_B, 3));
  client.handleEnvelope(heartbeat(BOOT_B, 3));

  assert.equal(client.currentRevision(), 2);
  assert.equal(handle.resyncRequestCount(), 1);
  await client.stop();
});

test("malformed envelopes do not deliver, advance, or keep the stream healthy", async () => {
  const statuses = [];
  const delivered = [];
  const client = new UiStateClient({
    connect: scriptedTransport([]),
    onEnvelope: (envelope) => delivered.push(envelope),
    onStatus: (status) => statuses.push(status),
    heartbeatTimeoutMs: 45,
  });
  const malformed = [
    {
      ...snapshot(BOOT_A, 2),
      payload: { ...snapshot(BOOT_A, 2).payload, protocol_version: 2 },
    },
    {
      ...snapshot(BOOT_A, 2),
      payload: { ...snapshot(BOOT_A, 2).payload, dynamic_state: undefined },
    },
    {
      type: "delta",
      payload: { boot_id: BOOT_A, revision: 2, change: { type: "pointer" } },
    },
    {
      type: "heartbeat",
      payload: { boot_id: BOOT_A, revision: 1 },
    },
    {
      type: "resync_required",
      payload: {
        boot_id: BOOT_A,
        current_revision: 1,
        reason: "invented_reason",
      },
    },
  ];

  try {
    await client.start();
    client.handleEnvelope(snapshot(BOOT_A, 1));
    for (const envelope of malformed) {
      await wait(12);
      client.handleEnvelope(envelope);
    }
    await waitFor(() => statuses.some((status) => status.state === "stale"), 150);
    assert.equal(client.currentRevision(), 1);
    assert.deepEqual(delivered.map((envelope) => envelope.type), ["snapshot"]);
  } finally {
    await client.stop();
  }
});

test("Rust extended mouse button objects are accepted only for one u8 Other value", async () => {
  const delivered = [];
  const client = new UiStateClient({
    connect: scriptedTransport([]),
    onEnvelope: (envelope) => delivered.push(envelope),
    onStatus: () => {},
    heartbeatTimeoutMs: 1000,
  });

  try {
    await client.start();
    client.handleEnvelope(snapshot(BOOT_A, 1));
    client.handleEnvelope(extendedMouseButtonDelta(BOOT_A, 2, { Other: 7 }));
    client.handleEnvelope(
      extendedMouseButtonDelta(BOOT_A, 3, { Other: 7, extra: true }),
    );
    client.handleEnvelope(extendedMouseButtonDelta(BOOT_A, 3, { Other: 256 }));

    assert.equal(client.currentRevision(), 2);
    assert.deepEqual(delivered.map((envelope) => envelope.type), [
      "snapshot",
      "delta",
    ]);
  } finally {
    await client.stop();
  }
});

test("any envelope refreshes activity and stale status retains the last snapshot", async () => {
  const statuses = [];
  let connectAttempts = 0;
  const client = new UiStateClient({
    connect: async (options) => {
      connectAttempts += 1;
      return scriptedTransport([])(options);
    },
    onEnvelope: () => {},
    onStatus: (status) => statuses.push(status),
    heartbeatTimeoutMs: 45,
  });

  try {
    await client.start();
    client.handleEnvelope(snapshot(BOOT_A, 9, "retained"));
    await wait(30);
    client.handleEnvelope(heartbeat(BOOT_A, 9));
    await wait(30);
    assert.notEqual(statuses.at(-1)?.state, "stale");
    await waitFor(() => statuses.some((status) => status.state === "stale"));
    const stale = statuses.findLast((status) => status.state === "stale");
    assert.equal(stale.snapshot.marker, "retained");
    await waitFor(() => connectAttempts >= 2);
    assert.equal(client.currentRevision(), 9);
  } finally {
    await client.stop();
  }
});

test("a connected stream with no first envelope times out and reconnects", async () => {
  const statuses = [];
  let attempts = 0;
  const client = new UiStateClient({
    connect: async () => {
      attempts += 1;
      return { close() {} };
    },
    onEnvelope: () => {},
    onStatus: (status) => statuses.push(status),
    heartbeatTimeoutMs: 30,
  });

  try {
    await client.start();
    await waitFor(() => statuses.some((status) => status.state === "stale"), 200);
    await waitFor(() => attempts >= 2, 300);
  } finally {
    await client.stop();
  }
});

test("a connect promise that never resolves is timed out and superseded", async () => {
  const statuses = [];
  let attempts = 0;
  const client = new UiStateClient({
    connect: () => {
      attempts += 1;
      return new Promise(() => {});
    },
    onEnvelope: () => {},
    onStatus: (status) => statuses.push(status),
    heartbeatTimeoutMs: 25,
  });

  try {
    const start = client.start();
    await waitFor(() => statuses.some((status) => status.state === "stale"), 200);
    await waitFor(() => attempts >= 2, 300);
    await start;
  } finally {
    await client.stop();
  }
});

test("a late resync rejection from connection A cannot close reconnected B", async () => {
  const resync = deferred();
  const connections = [];
  const delivered = [];
  const connect = async (options) => {
    const index = connections.length;
    const connection = {
      options,
      closes: 0,
      close() {
        this.closes += 1;
      },
      requestFullResync:
        index === 0 ? () => resync.promise : () => Promise.resolve(),
    };
    connections.push(connection);
    return connection;
  };
  const client = new UiStateClient({
    connect,
    onEnvelope: (envelope) => delivered.push(envelope),
    onStatus: () => {},
    heartbeatTimeoutMs: 1000,
  });

  try {
    await client.start();
    connections[0].options.onEnvelope(snapshot(BOOT_A, 1));
    connections[0].options.onEnvelope(pointerDelta(BOOT_A, 3));
    connections[0].options.onDisconnect(new Error("A closed"));
    await waitFor(() => connections.length >= 2);
    connections[1].options.onEnvelope(snapshot(BOOT_B, 1));

    resync.reject(new Error("late A resync failure"));
    await wait(20);
    assert.equal(connections[1].closes, 0);
    connections[1].options.onEnvelope(pointerDelta(BOOT_B, 2));
    assert.equal(client.currentRevision(), 2);
    assert.deepEqual(delivered.slice(-2).map((item) => item.type), [
      "snapshot",
      "delta",
    ]);
  } finally {
    resync.promise.catch(() => {});
    await client.stop();
  }
});

test("an old asynchronous stop cannot mark a concurrently restarted client stopped", async () => {
  const closeGate = deferred();
  const connections = [];
  const statuses = [];
  const client = new UiStateClient({
    connect: async (options) => {
      const index = connections.length;
      const connection = {
        options,
        close: index === 0 ? () => closeGate.promise : () => {},
      };
      connections.push(connection);
      return connection;
    },
    onEnvelope: () => {},
    onStatus: (status) => statuses.push(status.state),
    heartbeatTimeoutMs: 1000,
  });

  try {
    await client.start();
    await waitFor(() => connections.length === 1);
    const oldStop = client.stop();
    await client.start();
    await waitFor(() => connections.length === 2);
    connections[1].options.onEnvelope(snapshot(BOOT_B, 1));
    closeGate.resolve();
    await oldStop;

    assert.equal(client.currentRevision(), 1);
    assert.equal(statuses.at(-1), "healthy");
  } finally {
    closeGate.resolve();
    await client.stop();
  }
});

test("reconnect delays are 100, 250, 500, then capped at 1000 ms", async () => {
  const observedDelays = [];
  let attempts = 0;
  const client = new UiStateClient({
    connect: async () => {
      attempts += 1;
      throw new Error("offline");
    },
    onEnvelope: () => {},
    onStatus: (status) => {
      if (status.state === "reconnecting") {
        observedDelays.push(status.retryDelayMs);
      }
    },
  });

  await client.start();
  await waitFor(() => observedDelays.length >= 4, 2500);
  assert.deepEqual(observedDelays.slice(0, 4), [100, 250, 500, 1000]);
  await client.stop();
});

test("fallback polling waits one second, stays single-flight, and stops on a snapshot", async () => {
  let fallbackCalls = 0;
  let releasePoll;
  const firstPoll = new Promise((resolve) => {
    releasePoll = resolve;
  });
  const statuses = [];
  const client = new UiStateClient({
    connect: () => new Promise(() => {}),
    onEnvelope: () => {},
    onStatus: (status) => {
      statuses.push(status.state);
      if (status.state === "fallback_poll") {
        fallbackCalls += 1;
        return firstPoll;
      }
    },
    heartbeatTimeoutMs: 25,
  });

  try {
    await client.start();
    await wait(900);
    assert.equal(fallbackCalls, 0);
    await waitFor(() => fallbackCalls === 1, 400);
    await wait(250);
    assert.equal(fallbackCalls, 1);

    client.handleEnvelope(snapshot(BOOT_A, 1));
    releasePoll();
    await wait(10);
    assert.equal(fallbackCalls, 1);
    assert.equal(statuses.at(-1), "healthy");
  } finally {
    releasePoll();
    await client.stop();
  }
});

test("App wires Tauri and browser streams through the same UiStateClient", async () => {
  const source = await readFile(new URL("./App.tsx", import.meta.url), "utf8");

  assert.match(source, /createTauriUiStateConnector/);
  assert.match(source, /createWebSocketUiStateConnector/);
  assert.match(source, /listenTauriEventTransport/);
  assert.match(source, /WebSocket:\s*window\.WebSocket/);
  assert.match(source, /new UiStateClient\(\{/);
  assert.doesNotMatch(
    source,
    /catch \(refreshError\) \{\s*setPayload\(EMPTY_PAYLOAD\)/,
    "fallback failures must not erase the last streamed snapshot",
  );
  assert.doesNotMatch(
    source,
    /setInterval\(\(\) => \{\s*refreshDashboard\(\)/,
    "healthy push must not race an unconditional dashboard poll",
  );
});

test("Vite proxies /ui-state to the daemon websocket without fabricating state", async () => {
  const source = await readFile(
    new URL("../../vite.config.ts", import.meta.url),
    "utf8",
  );

  assert.match(source, /['"]\/ui-state['"]\s*:\s*\{/);
  assert.match(source, /target:\s*['"]ws:\/\/127\.0\.0\.1:27436['"]/);
  assert.match(source, /ws:\s*true/);
  assert.doesNotMatch(source, /start_ui_state_stream[\s\S]*return undefined/);
});
