# 网络音频验证记录 — 2026-09-15

## 判定

**尚未达到方案的完成条件。** 当前实现了可测试的通用组件、macOS 插件组件、Linux 桥接源码及管理界面。尚不能通过系统设备完成真实的跨机录音/播放。Windows WaveRT/ASIO 和 daemon 端到端编排属于未实现，不只是“待硬件验证”。

## 已执行

| 检查 | 结果与范围 |
|---|---|
| `cargo test -p rshare-audio -p rshare-net --locked` | 220 通过。含实际 loopback QUIC，48/96 kHz、8 声道 PCM24/Float32 编解码/重采样链路；非物理端到端测试 |
| `cargo test --workspace --locked --no-fail-fast` | 当次 1139 通过、2 失败，见下方；该次之后增加的音频测试已在定向套件执行 |
| 前端 `npm test` | 261 通过 |
| 前端 `npm run build` | 通过；仍有现有大 chunk 提示 |
| `cargo build --workspace --release --locked` | 通过，有已有平台代码未使用项警告 |
| `cargo check -p rshare-audio --target x86_64-pc-windows-gnu --locked` | 通过；只证明通用 Rust 代码可检查，不能证明 Windows 驱动存在或可用 |
| `sh scripts/driver/build-macos-audio.sh` | arm64/x86_64 HAL 插件与 XPC broker 均编译并完成开发用临时签名 |
| `sh scripts/driver/test-macos-audio.sh` | ASan/UBSan vtable 测试通过：注册、属性、客户端计数、PCM 读取、静音、移除、文件权限。测试使用专门文件映射分支，未覆盖生产 XPC 宿主加载 |
| `cargo run -p rshare-audio --example inspect --locked` | 实际 Core Audio 原生枚举成功，内置麦克风和扬声器有稳定 UID；RShare HAL 插件状态为未加载 |
| `python3 -m unittest discover -s scripts/perf -p test_measure_audio_latency.py` | 2 通过；验证共同录制时钟的脉冲延迟计算、缺失脉冲和超限失败 |
| 安装卸载脚本 `sh -n` | 通过；未执行系统安装或音频服务重启 |

浏览器本地预览已检查音频页面布局及“未测量”延迟展示；浏览器不具备新的 Tauri 命令，明确提示需要更新后的桌面应用。此检查不替代新 daemon 的桌面会话验证。

## 全量测试失败

1. `rshare-platform --test macos_vdisplay_driver_package` 中 `macos_virtual_display_driver_package_declares_framebuffer_user_client` 失败。测试要求 `virtual_display.rs` 包含 `RSHARE_MACOS_VDISPLAY_SERVICE_CLASS` 常量；检查 HEAD 版本确认原有源码同样缺少该字符串。本轮未修改相关实现或测试。
2. `rshare-perf` 的 `quic::tests::stall_recovery_uses_sequence_timeline_not_whole_run_p99` 在全量运行中失败；单独使用相同测试名重跑通过。保留该次计时失败记录，不宣称全工作区测试全部通过。

## 构建产物

- `target/audio-driver/macos/RShareAudio.driver`
- `target/audio-driver/macos/org.rshare.audio.broker`
- `target/audio-driver/macos/driver-test`
- 常规 workspace Release 产物，以及前端 `dist/`。

运行中的用户 daemon 没有被替换。旧进程不认识新的 `NetworkAudio` IPC 命令；诊断新接口需运行新构建的 daemon，但即使新版本也会如实报告媒体编排尚未接入。

## 后续实机验收

- 三平台两两组合双向，再做各平台同系统双机；每个端点必须被第三方应用真正选中并录放。
- 两端 48/96 kHz、8 入/8 出分别运行 30 分钟。合格千兆有线环境下单向 P95 ≤10 ms，启动后无欠载；另测 8 小时漂移。
- 叠加文件传输，键鼠可靠事件不丢失，键鼠 P95 相对匹配基线增量 ≤1 ms。
- 热插拔、端点忙、权限撤销、断线、睡眠唤醒、服务崩溃、卸载均需实测。
- 使用共同 ADC 时钟同时录下参考与接收脉冲，可运行：

```sh
python3 scripts/perf/measure-audio-latency.py recording.wav \
  --reference-channel 0 --received-channel 1 --minimum-pulses 100 \
  --max-latency-ms 10 --output audio-latency.json
```

脚本测的是两路物理观察点间延迟，需记录接线并校准声卡通道差异。不能用 RTT/2、缓冲长度或两台机器的墙上时钟差代替。

## 未执行/未完成

Linux 桥接编译与运行、Windows WaveRT/ASIO 实现及测试、生产 XPC/HAL 加载、实际音频路由、物理延迟、键鼠负载对比、8 小时稳定性、正式签名/公证。CI 工作流已添加，但尚未推送或取得 CI 结果。
