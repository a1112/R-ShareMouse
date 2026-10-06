import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import TestRenderer, {act} from "react-test-renderer";
import {useInputPulse} from "./use-input-pulse.mjs";

test("wheel feedback expires and only a new event restarts the pulse", (t) => {
  t.mock.timers.enable({apis: ["setTimeout"]});
  let active;
  function Probe({sequence}) { active = useInputPulse(sequence); return null; }
  let renderer;
  act(() => { renderer = TestRenderer.create(React.createElement(Probe, {sequence: null})); });
  assert.equal(active, false);
  act(() => renderer.update(React.createElement(Probe, {sequence: 1})));
  assert.equal(active, true);
  act(() => t.mock.timers.tick(180));
  assert.equal(active, false);
  act(() => renderer.update(React.createElement(Probe, {sequence: 1})));
  assert.equal(active, false);
  act(() => renderer.update(React.createElement(Probe, {sequence: 2})));
  assert.equal(active, true);
  act(() => renderer.unmount());
});
