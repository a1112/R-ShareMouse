import { expect, test } from "@playwright/test";
import { createRequire } from "node:module";
import os from "node:os";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const require = createRequire(import.meta.url);
const PLAYWRIGHT_VERSION = require("@playwright/test/package.json").version;
const BOOT_ID = "00000000-0000-0000-0000-000000000020";

const SNAPSHOT = {
  protocol_version: 1,
  boot_id: BOOT_ID,
  revision: 0,
  status: {
    device_id: "local-perf",
    device_name: "Performance runner",
    hostname: "fixed-runner",
    bind_address: "127.0.0.1:27431",
    discovery_port: 27432,
    healthy: true,
    latency_feedback: { latency_ms: 1 },
  },
  devices: [],
  layout: {
    version: 1,
    nodes: [{ device_id: "local-perf", name: "Performance runner" }],
    links: [],
  },
  capabilities: {
    local_device_id: "local-perf",
    generated_at_ms: 1,
    devices: [],
  },
  display_inventory: { displays: [] },
  dynamic_state: {
    pointer: { x: 0, y: 0, observed_at_ms: 0 },
    gamepads: [],
    pressed_keys: [],
    pressed_mouse_buttons: [],
    pressed_gamepad_buttons: [],
    diagnostics: { latency_ms: 1 },
  },
  active_sessions: { control: null, media_sessions: [] },
};

function daemonResponse(request) {
  if (request === "Status") return { Status: SNAPSHOT.status };
  if (request === "Devices") return { Devices: SNAPSHOT.devices };
  if (request === "GetLayout") return { Layout: SNAPSHOT.layout };
  if (request === "LocalControls") {
    return {
      LocalControls: {
        sequence: 0,
        keyboard: { detected: false, pressed_keys: [], event_count: 0 },
        mouse: {
          detected: false,
          x: 0,
          y: 0,
          pressed_buttons: [],
          event_count: 0,
          move_count: 0,
        },
        keyboard_devices: [],
        mouse_devices: [],
        gamepads: [],
        recent_events: [],
        display: { display_count: 0, displays: [] },
      },
    };
  }
  if (request === "MobileAccess") {
    return {
      MobileAccess: {
        enabled: false,
        bind_address: "unavailable",
        page_url: null,
        token: null,
      },
    };
  }
  if (request && typeof request === "object" && "EndpointEvents" in request) {
    return { EndpointEvents: [] };
  }
  return "Ack";
}

async function installHarness(page) {
  await page.addInitScript(({ snapshot }) => {
    window.__rsharePerfEnableStoreAccess = true;
    const listenersFor = () => new Map();
    const sockets = [];

    class PerformanceWebSocket {
      static CONNECTING = 0;
      static OPEN = 1;
      static CLOSING = 2;
      static CLOSED = 3;

      constructor(url) {
        this.url = String(url);
        this.readyState = PerformanceWebSocket.CONNECTING;
        this.listeners = listenersFor();
        sockets.push(this);
        queueMicrotask(() => {
          if (this.readyState !== PerformanceWebSocket.CONNECTING) return;
          this.readyState = PerformanceWebSocket.OPEN;
          this.dispatch("open", new Event("open"));
        });
      }

      addEventListener(type, listener, options) {
        const entries = this.listeners.get(type) ?? [];
        entries.push({ listener, once: Boolean(options?.once) });
        this.listeners.set(type, entries);
      }

      removeEventListener(type, listener) {
        const entries = this.listeners.get(type) ?? [];
        this.listeners.set(
          type,
          entries.filter((entry) => entry.listener !== listener),
        );
      }

      dispatch(type, event) {
        const entries = [...(this.listeners.get(type) ?? [])];
        for (const entry of entries) {
          entry.listener.call(this, event);
          if (entry.once) this.removeEventListener(type, entry.listener);
        }
        this[`on${type}`]?.call(this, event);
      }

      send(data) {
        if (!this.url.endsWith("/ui-state")) return;
        let request;
        try {
          request = JSON.parse(String(data));
        } catch {
          return;
        }
        if (request.type === "subscribe") {
          queueMicrotask(() => {
            this.dispatch(
              "message",
              new MessageEvent("message", {
                data: JSON.stringify({ type: "snapshot", payload: snapshot }),
              }),
            );
          });
        }
      }

      close(code = 1000, reason = "") {
        if (this.readyState === PerformanceWebSocket.CLOSED) return;
        this.readyState = PerformanceWebSocket.CLOSED;
        this.dispatch("close", new CloseEvent("close", { code, reason }));
      }
    }

    window.WebSocket = PerformanceWebSocket;
    window.__rsharePerfEmit = (envelope) => {
      for (const socket of sockets) {
        if (
          socket.url.endsWith("/ui-state") &&
          socket.readyState === PerformanceWebSocket.OPEN
        ) {
          socket.dispatch(
            "message",
            new MessageEvent("message", { data: JSON.stringify(envelope) }),
          );
        }
      }
    };

    const counters = {
      active: false,
      reactCommits: 0,
      profiledTopologyCommits: 0,
      profiledTopologyMounts: 0,
    };
    window.__rshareReactPerf = counters;
    window.__rsharePerfRecordTopologyCommit = () => {
      if (counters.active) counters.profiledTopologyCommits += 1;
      else counters.profiledTopologyMounts += 1;
    };
    let rendererId = 0;
    const renderers = new Map();
    window.__REACT_DEVTOOLS_GLOBAL_HOOK__ = {
      supportsFiber: true,
      renderers,
      inject(renderer) {
        rendererId += 1;
        renderers.set(rendererId, renderer);
        return rendererId;
      },
      onCommitFiberRoot() {
        if (counters.active) counters.reactCommits += 1;
      },
      onCommitFiberUnmount() {},
    };
  }, { snapshot: SNAPSHOT });

  let countRequests = false;
  let dashboardOrEndpointRequests = 0;
  page.on("request", (request) => {
    if (!countRequests || !request.url().includes("/__rshare/ipc")) return;
    const body = request.postData() ?? "";
    if (
      body.includes('"Status"') ||
      body.includes('"Devices"') ||
      body.includes('"GetLayout"') ||
      body.includes('"EndpointEvents"')
    ) {
      dashboardOrEndpointRequests += 1;
    }
  });

  await page.route("**/__rshare/ipc", async (route) => {
    let request = null;
    try {
      request = JSON.parse(route.request().postData() ?? "null");
    } catch {
      // The app reports malformed bridge requests; the performance fixture stays deterministic.
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(daemonResponse(request)),
    });
  });

  await page.goto("/");
  await page.waitForFunction(
    (bootId) =>
      window.__rsharePerfStoreAccess?.store.getState().bootId === bootId,
    BOOT_ID,
  );
  await page.getByRole("button", { name: "设备", exact: true }).click();

  await page.evaluate(async () => {
    const {
      store: uiStateStore,
      selectInputVisuals,
      selectTopologyProjection,
    } = window.__rsharePerfStoreAccess;
      const report = {
      active: false,
      paintSamples: [],
      paintFramePending: false,
      lastPointerObservedAt: null,
      inputCommits: 0,
      discreteApplied: 0,
      discretePaintSamples: [],
      lastDiscretePaintSequence: 0,
      lastDiscreteSequence: 0,
      nextDiscreteSequence: 0,
      finalDiscreteState: null,
      longTasks: 0,
      topologyStatusActive: false,
      topologyStatusSamples: [],
    };
    window.__rshareUiPerf = report;
    window.__rsharePerfRecordInputPaint = (observedAt) => {
      if (
        !report.active ||
        !Number.isFinite(observedAt) ||
        observedAt === report.lastPointerObservedAt
      ) {
        return;
      }
      report.lastPointerObservedAt = observedAt;
      // The production probe calls this from a React passive effect. Because
      // the external-store update commits inside the UI RAF callback, Chrome
      // paints that committed tree before running the passive-effect task.
      report.paintSamples.push(Math.max(0, performance.now() - observedAt));
    };
    window.__rsharePerfRecordDiscretePaint = (observedAt, sequence) => {
      if (
        !report.active ||
        !Number.isFinite(observedAt) ||
        !Number.isInteger(sequence) ||
        sequence <= report.lastDiscretePaintSequence
      ) {
        return;
      }
      report.lastDiscretePaintSequence = sequence;
      const latency = Math.max(0, performance.now() - observedAt);
      report.discretePaintSamples.push(latency);
      report.paintSamples.push(latency);
    };
    window.__rsharePerfRecordTopologyStatusPaint = (observedAt) => {
      if (!report.topologyStatusActive || !Number.isFinite(observedAt)) return;
      report.topologyStatusSamples.push(
        Math.max(0, performance.now() - observedAt),
      );
    };
    window.__rshareUiPerfUnsubscribeInput = uiStateStore.subscribe(
      selectInputVisuals,
      (input) => {
        if (!report.active) return;
        report.inputCommits += 1;
        const transition = input.lastDiscreteTransition;
        const sequence = Number(transition?.perf_sequence);
        if (
          Number.isInteger(sequence) &&
          sequence > report.lastDiscreteSequence
        ) {
          report.lastDiscreteSequence = sequence;
          report.discreteApplied += 1;
          report.finalDiscreteState = transition.state;
        }
      },
    );
    if (
      typeof PerformanceObserver === "function" &&
      PerformanceObserver.supportedEntryTypes?.includes("longtask")
    ) {
      window.__rshareLongTaskObserver = new PerformanceObserver((list) => {
        if (!report.active) return;
        report.longTasks += list
          .getEntries()
          .filter((entry) => entry.duration > 50).length;
      });
      window.__rshareLongTaskObserver.observe({ entryTypes: ["longtask"] });
    }
    await new Promise((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(resolve)),
    );
  });

  return {
    startRequestCounting() {
      countRequests = true;
    },
    stopRequestCounting() {
      countRequests = false;
    },
    requestCount() {
      return dashboardOrEndpointRequests;
    },
  };
}

async function runStreamScenario(page, {
  durationMs,
  pointerHz,
  discreteTransitions,
}) {
  return page.evaluate(
    async ({ bootId, durationMs, pointerHz, discreteTransitions }) => {
      const perf = window.__rshareUiPerf;
      const reactPerf = window.__rshareReactPerf;
      perf.paintSamples.length = 0;
      perf.paintFramePending = false;
      perf.lastPointerObservedAt = null;
      perf.inputCommits = 0;
      perf.discreteApplied = 0;
      perf.discretePaintSamples.length = 0;
      const discreteSequenceBase = perf.nextDiscreteSequence;
      perf.lastDiscretePaintSequence = discreteSequenceBase;
      perf.lastDiscreteSequence = discreteSequenceBase;
      perf.finalDiscreteState = null;
      perf.longTasks = 0;
      reactPerf.reactCommits = 0;
      reactPerf.profiledTopologyCommits = 0;
      perf.active = true;
      reactPerf.active = true;

      const totalPointerDeltas = Math.round((durationMs * pointerHz) / 1000);
      const tickMs = 10;
      const ticks = Math.ceil(durationMs / tickMs);
      let pointerDeltasSent = 0;
      let discreteTransitionsSent = 0;
      let revision = window.__rsharePerfStoreAccess.store.currentRevision();
      const startedAt = performance.now();

      for (let tick = 0; tick < ticks; tick += 1) {
        const target = startedAt + tick * tickMs;
        const delay = target - performance.now();
        if (delay > 0) {
          await new Promise((resolve) => setTimeout(resolve, delay));
        }

        const pointerTarget = Math.floor(
          ((tick + 1) * totalPointerDeltas) / ticks,
        );
        while (pointerDeltasSent < pointerTarget) {
          pointerDeltasSent += 1;
          revision += 1;
          window.__rsharePerfEmit({
            type: "delta",
            payload: {
              boot_id: bootId,
              revision,
              change: {
                type: "pointer",
                payload: {
                  x: pointerDeltasSent,
                  y: pointerDeltasSent % 900,
                  observed_at_ms: Math.floor(performance.now()),
                },
              },
            },
          });
        }
        const discreteTarget = Math.floor(
          ((tick + 1) * discreteTransitions) / ticks,
        );
        while (discreteTransitionsSent < discreteTarget) {
          revision += 1;
          window.__rsharePerfEmit({
            type: "delta",
            payload: {
              boot_id: bootId,
              revision,
              change: {
                type: "key_button",
                payload: {
                  type: "key",
                  key_code: 42,
                  state:
                    discreteTransitionsSent % 2 === 0 ? "Pressed" : "Released",
                  perf_sequence:
                    discreteSequenceBase + discreteTransitionsSent + 1,
                  observed_at_ms: Math.floor(performance.now()),
                },
              },
            },
          });
          discreteTransitionsSent += 1;
        }
      }

      const remaining = startedAt + durationMs - performance.now();
      if (remaining > 0) {
        await new Promise((resolve) => setTimeout(resolve, remaining));
      }
      await new Promise((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(resolve)),
      );
      const pendingLongTasks =
        window.__rshareLongTaskObserver?.takeRecords() ?? [];
      perf.longTasks += pendingLongTasks.filter(
        (entry) => entry.duration > 50,
      ).length;
      perf.active = false;
      reactPerf.active = false;
      perf.nextDiscreteSequence =
        discreteSequenceBase + discreteTransitionsSent;

      const samples = [...perf.paintSamples].sort((left, right) => left - right);
      const percentile = (fraction) => {
        if (!samples.length) return null;
        const index = Math.min(
          samples.length - 1,
          Math.max(0, Math.ceil((samples.length - 1) * fraction)),
        );
        return samples[index];
      };

      return {
        duration_ms: performance.now() - startedAt,
        pointer_deltas_sent: pointerDeltasSent,
        discrete_transitions_sent: discreteTransitionsSent,
        discrete_transitions_applied: perf.discreteApplied,
        discrete_paint_sample_count: perf.discretePaintSamples.length,
        discrete_paint_samples_ms: [...perf.discretePaintSamples],
        final_discrete_state: perf.finalDiscreteState,
        paint_sample_count: samples.length,
        paint_p50_ms: percentile(0.5),
        paint_p95_ms: percentile(0.95),
        paint_p99_ms: percentile(0.99),
        paint_max_ms: samples.at(-1) ?? null,
        paint_samples_ms: samples,
        react_commits_during_pointer_flood: reactPerf.reactCommits,
        input_commits_during_pointer_flood: perf.inputCommits,
        topology_commits_during_pointer_flood:
          reactPerf.profiledTopologyCommits,
      topology_commit_probe_mounts: reactPerf.profiledTopologyMounts,
        long_tasks_over_50ms: perf.longTasks,
      };
    },
    { bootId: BOOT_ID, durationMs, pointerHz, discreteTransitions },
  );
}

async function runTopologyStatusScenario(page, transitions) {
  return page.evaluate(async ({ bootId, transitions }) => {
    const uiStateStore = window.__rsharePerfStoreAccess.store;
    const perf = window.__rshareUiPerf;
    perf.topologyStatusSamples.length = 0;
    perf.topologyStatusActive = true;
    let revision = uiStateStore.currentRevision();

    for (let index = 0; index < transitions; index += 1) {
      revision += 1;
      const observedAt = Math.floor(performance.now());
      const type = index % 2 === 0 ? "topology" : "status";
      const payload =
        type === "topology"
          ? {
              version: index + 2,
              nodes: [{ device_id: "local-perf", name: "Performance runner" }],
              links: [],
              observed_at_ms: observedAt,
            }
          : {
              device_id: "local-perf",
              device_name: "Performance runner",
              hostname: "fixed-runner",
              bind_address: "127.0.0.1:27431",
              discovery_port: 27432,
              healthy: true,
              latency_feedback: { latency_ms: 1 },
              observed_at_ms: observedAt,
            };
      window.__rsharePerfEmit({
        type: "delta",
        payload: {
          boot_id: bootId,
          revision,
          change: { type, payload },
        },
      });
      await new Promise((resolve) => requestAnimationFrame(resolve));
    }

    await new Promise((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(resolve)),
    );
    perf.topologyStatusActive = false;
    const samples = [...perf.topologyStatusSamples].sort(
      (left, right) => left - right,
    );
    const percentile = (fraction) => {
      if (!samples.length) return null;
      const index = Math.min(
        samples.length - 1,
        Math.max(0, Math.ceil((samples.length - 1) * fraction)),
      );
      return samples[index];
    };
    return {
      topology_status_updates_sent: transitions,
      topology_status_sample_count: samples.length,
      topology_status_p50_ms: percentile(0.5),
      topology_status_p95_ms: percentile(0.95),
      topology_status_p99_ms: percentile(0.99),
      topology_status_max_ms: samples.at(-1) ?? null,
      topology_status_samples_ms: samples,
    };
  }, { bootId: BOOT_ID, transitions });
}

async function cleanupHarness(page) {
  await page.evaluate(() => {
    window.__rshareUiPerf.active = false;
    window.__rshareUiPerf.topologyStatusActive = false;
    window.__rshareReactPerf.active = false;
    window.__rshareLongTaskObserver?.disconnect();
    window.__rshareUiPerfUnsubscribeTopology?.();
    window.__rshareUiPerfUnsubscribeInput?.();
    delete window.__rsharePerfRecordInputPaint;
    delete window.__rsharePerfRecordDiscretePaint;
    delete window.__rsharePerfRecordTopologyStatusPaint;
  });
}

async function recordReport(testInfo, metrics) {
  const report = {
    batch_id: process.env.RSHARE_PERF_BATCH_ID ?? null,
    batch_attempt: Number(process.env.RSHARE_PERF_BATCH_ATTEMPT ?? 1),
    run_index: Number(process.env.RSHARE_PERF_RUN_INDEX ?? 1),
    scenario: "desktop-ui-state",
    ...metrics,
  };
  const configuredOutput = process.env.RSHARE_PERF_OUTPUT;
  const outputPath = configuredOutput
    ? path.resolve(configuredOutput)
    : testInfo.outputPath("ui-state-report.json");
  await mkdir(path.dirname(outputPath), { recursive: true });
  await writeFile(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
  await testInfo.attach("ui-state-report", {
    body: Buffer.from(JSON.stringify(report, null, 2)),
    contentType: "application/json",
  });
  return report;
}

async function executeScenario(page, browser, testInfo, options) {
  const harness = await installHarness(page);
  try {
    if ((options.warmupDurationMs ?? 0) > 0) {
      await runStreamScenario(page, {
        durationMs: options.warmupDurationMs,
        pointerHz: options.pointerHz,
        discreteTransitions: 20,
      });
      await runTopologyStatusScenario(page, 20);
    }
    harness.startRequestCounting();
    const pointerMetrics = await runStreamScenario(page, options);
    const topologyStatusMetrics = await runTopologyStatusScenario(
      page,
      options.topologyStatusTransitions,
    );
    const graphics = await page.evaluate(() => {
      const canvas = document.createElement("canvas");
      const gl = canvas.getContext("webgl");
      if (!gl) return { vendor: "unavailable", renderer: "unavailable" };
      const extension = gl.getExtension("WEBGL_debug_renderer_info");
      return {
        vendor: extension
          ? String(gl.getParameter(extension.UNMASKED_VENDOR_WEBGL))
          : String(gl.getParameter(gl.VENDOR)),
        renderer: extension
          ? String(gl.getParameter(extension.UNMASKED_RENDERER_WEBGL))
          : String(gl.getParameter(gl.RENDERER)),
      };
    });
    harness.stopRequestCounting();
    return recordReport(testInfo, {
      ...pointerMetrics,
      ...topologyStatusMetrics,
      dashboard_or_endpoint_polls_while_healthy: harness.requestCount(),
      environment: {
        node_version: process.version,
        node_options: process.env.NODE_OPTIONS ?? "",
        os_version: os.version(),
        os_release: os.release(),
        playwright_version: PLAYWRIGHT_VERSION,
        browser_name: browser.browserType().name(),
        browser_version: browser.version(),
        headless: true,
        viewport: page.viewportSize(),
        graphics,
      },
    });
  } finally {
    harness.stopRequestCounting();
    await cleanupHarness(page);
  }
}

test("healthy UI stream processes pointer and discrete input without fallback polls", async ({
  page,
  browser,
}, testInfo) => {
  const report = await executeScenario(page, browser, testInfo, {
    durationMs: 1_000,
    pointerHz: 1_000,
    discreteTransitions: 10,
    topologyStatusTransitions: 10,
  });

  expect(report.pointer_deltas_sent).toBe(1_000);
  expect(report.discrete_transitions_sent).toBe(10);
  expect(report.discrete_transitions_applied).toBe(10);
  expect(report.discrete_paint_sample_count).toBe(10);
  expect(report.final_discrete_state).toBe("Released");
  expect(report.paint_sample_count).toBeGreaterThanOrEqual(30);
  expect(report.topology_commit_probe_mounts).toBeGreaterThan(0);
  expect(report.react_commits_during_pointer_flood).toBeGreaterThan(0);
  expect(report.topology_commits_during_pointer_flood).toBe(0);
  expect(report.topology_status_sample_count).toBe(10);
  expect(report.dashboard_or_endpoint_polls_while_healthy).toBe(0);
});

test("1000 Hz pointer flood and discrete transitions meet the UI gate @fixed-runner", async ({
  page,
  browser,
}, testInfo) => {
  const report = await executeScenario(page, browser, testInfo, {
    durationMs: 30_000,
    warmupDurationMs: 2_000,
    pointerHz: 1_000,
    discreteTransitions: 300,
    topologyStatusTransitions: 300,
  });

  expect(report.pointer_deltas_sent).toBe(30_000);
  expect(report.discrete_transitions_sent).toBe(300);
  expect(report.discrete_transitions_applied).toBe(300);
  expect(report.discrete_paint_sample_count).toBe(300);
  expect(report.final_discrete_state).toBe("Released");
  expect(report.paint_sample_count).toBeGreaterThanOrEqual(900);
  expect(report.topology_commit_probe_mounts).toBeGreaterThan(0);
  expect(report.paint_p95_ms).toBeLessThanOrEqual(16.7);
  expect(report.paint_p99_ms).toBeLessThanOrEqual(33);
  expect(report.topology_commits_during_pointer_flood).toBe(0);
  expect(report.topology_status_sample_count).toBe(300);
  expect(report.topology_status_p95_ms).toBeLessThanOrEqual(50);
  expect(report.topology_status_p99_ms).toBeLessThanOrEqual(100);
  expect(report.long_tasks_over_50ms).toBe(0);
  expect(report.dashboard_or_endpoint_polls_while_healthy).toBe(0);
});
