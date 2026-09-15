import test from "node:test";
import assert from "node:assert/strict";
import { audioMetric, audioDeviceState, canGrantAudio, audioErrorMessage } from "./network-audio-model.mjs";
test("unmeasured latency stays unknown, including zero and malformed values", () => {
  assert.equal(audioMetric(undefined), "未测量");
  assert.equal(audioMetric(NaN), "未测量");
  assert.equal(audioMetric(0), "0.00 ms");
  assert.equal(audioMetric(3), "3.00 ms");
});
test("pending OS registration is distinct from registered and disconnected", () => {
  assert.equal(audioDeviceState({status:"Pending"}), "等待系统注册");
  assert.equal(audioDeviceState({status:"Offline"}), "离线 · 静音");
});
test("UI never offers re-export for virtual endpoints", () => {
  const peer="12345678-1234-1234-1234-123456789012";
  assert.equal(canGrantAudio(peer,{id:"mic",virtual_device:false}),true);
  assert.equal(canGrantAudio(peer,{id:"mic",virtual_device:true}),false);
  assert.equal(canGrantAudio("not-a-peer",{id:"mic"}),false);
});

test("unavailable backend messages explain the next user action", () => {
  assert.equal(audioErrorMessage("Error: 命令 network_audio 需要 Tauri bridge 或 daemon 网络网关"), "请在更新后的桌面应用中管理网络音频设备。");
  assert.equal(audioErrorMessage("unknown variant NetworkAudio"), "正在运行的后台服务版本较旧，请更新并重启服务。");
  assert.equal(audioErrorMessage("permission denied"), "permission denied");
});
