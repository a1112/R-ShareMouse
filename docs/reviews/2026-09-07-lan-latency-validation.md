# 本机授权、局域网端测与延迟优化 · 2026-09-07

## 结论

代码已从 `4b186dc` 快进同步到 `origin/main` 的 `3a152a6`。本机已部署 Release 应用和同包 daemon，系统输入监控、辅助功能均实测为 true，输入后端 Healthy。真实 Mac → Windows 注入与文件传输通过。**当前无线网络未达到稳定 5 ms 内；实体键鼠跨屏、滚轮、返回边缘和文件夹拖拽尚未完成手动验收。**

本轮将目标定义为应用 daemon 发起请求到收到认证对端 ACK 的 **RTT P95 < 5000 µs，且零失败样本**。这不是单向估算，也不包括显示器呈现或操作系统完成派发后的响应。即使此门槛通过，也不能代表每次操作的最大延迟低于 5 ms。

## 已完成的修改

- 便携 RDev 捕获路径的默认鼠标节流从 5 ms 改为零。短于 5 ms 的手势不再在进入队列之前被整体丢弃；已有有界语义队列仍负责负载下的合并。显式指定非零节流的调用方保持原行为。本项修复不代表当前 Mac 原生捕获路径的网络 RTT 降低。
- 延迟请求返回可选 `probe_sequence`，ACK 附带发送端单调时钟测得的 `raw_round_trip_us`。旧客户端仍可读取原有结果，测试脚本只接受目标、方向和请求序号全部匹配的 ACK。
- 本地事件订阅先注册接收器再发送初始快照，避免客户端收到快照立即发起探测时漏收 ACK。
- 增加 `scripts/perf/measure-lan-latency.py`，保存每个原始样本、失败、P50/P95/P99 和门槛结论。超时、无连接、旧 daemon 或超预算均不判为通过。
- 应用包补齐同目录 Release daemon，避免 GUI 找到工作区旧 Debug daemon。最终包为仓库根目录的 `R-ShareMouse.app`；应用包和证据均未加入 Git。

本地开发包为 ad-hoc 签名，更新会改变代码身份；本次对准确的 `com.rsharemouse.desktop` 记录重新授权，并在最终构建后复查。系统日志确认该 GUI 是 daemon 的权限责任主体，只授权 daemon 文件不足以解决问题。权限操作期间误移除的 MacTool-Debug-UX 记录已立即恢复，并再次核实为开启。

## 实机证据

Mac 与已连接 Windows 对端通信；用户确认对端已更新并解锁，但本轮没有取得 Windows 终端的构建 SHA。Mac 无线状态为 2.4 GHz、802.11n、20 MHz、72 Mbps，信号约 -26 dBm。

| 检查 | 结果 |
| --- | --- |
| 最终本机权限 | `input_monitoring=true`、`accessibility=true`、`ready=true` |
| 最终本机输入后端 | Healthy，局域网发现 1 台、连接 1 台 |
| 本机键盘 Shift / 鼠标注入 | 两项 Success |
| 最终远端 Shift 注入 | 2/2 成功；现有整数毫秒诊断平均 4 ms、最大 5 ms。两次样本不构成稳定性验收。 |
| Mac → Windows 文件传输 | 62 字节测试文件，终态 Completed；未删除源文件。这里只验证应用文件传输协议，不作为 Finder → Explorer 拖放证明。 |
| 最终精确 RTT | 200/200 响应；最小 4.360 ms，平均 8.781 ms，P50 6.560 ms，P95 13.080 ms，P99 39.961 ms，最大 247.693 ms；23/200 小于 5 ms；门槛失败。 |
| 网络 ICMP 对照 | 100 次发送、99 次响应，1% 丢包；最小/平均/最大 3.141/6.876/176.029 ms。与最终应用探测不是同一时间窗。 |
| 实体输入证据 | 最终 doctor 仍为 `physical_events=0`，无实体远端事件；不能判定双机物理控制验收完成。 |

尝试将 Quinn ACK 最大等待协商为 1 ms：100 次微秒级探测 P50/P95 为 6.470/27.152 ms；恢复默认后的对照为 6.351/26.238 ms，两组均 100/100 响应。无线网络波动很大，该对照不能建立因果收益，故**未保留 ACK 调参**。[Quinn 官方 API](https://docs.rs/quinn/latest/quinn/struct.TransportConfig.html#method.ack_frequency_config)说明该设置只控制支持扩展的对端 ACK 频率，不能作为端到端 RTT 保证。

## 验证和复测

- 输入/网络测试：330 通过，1 项原有 LAN discovery 忽略。
- 增加微秒时间和结果兼容性后，core/daemon：576 通过。
- CLI/daemon 的前一轮回归：315 通过（与上项有重复，不应相加作为唯一测试数）。
- Python 测试 4 通过，覆盖匹配、P95 门槛、失败样本、超大 IPC 帧。
- 最新前端构建通过，前端测试 257 项通过。
- Release daemon 构建、Windows daemon 交叉检查、格式和差异检查通过；现有编译告警仍保留。

在项目根目录执行，先用 `target/release/rshare devices --detailed` 获取已连接目标 UUID：

```sh
python3 scripts/perf/measure-lan-latency.py \
  --device-id TARGET_UUID --samples 200 --budget-ms 5 \
  --output target/lan-latency.json
```

脚本的成功退出要求零失败且 P95 严格小于预算。原始数据在 `target/lan-validation/2026-09-07/`，主要文件是 `final-lan-latency.json`、`final-permissions.json`、`final-doctor.log`、`final-local-validation.json`、`final-file-transfer.json`、`final-endpoint-events.json` 和两组 `ack-*-microseconds*.json`。启动尚未连上对端的失败记录也保留。

下一步需要同一有线链路或 5 GHz 网络作为对照，并重新运行相同测试；网络改变不预先视为达标。还需用实体鼠标和键盘完成 Mac → Windows → Mac、滚轮、文件夹拖拽，再收集双方事件和实际落盘结果。未执行锁屏/睡眠恢复验收，也未改动驱动或放宽安全检查。
