# 三平台网络音频设备：设计与实现记录

## 交付状态

**开发中的组件集合，尚未完成可用的端到端功能。** 本文不能作为三平台音频共享已完成的证明。daemon 的网络音频状态保持未就绪，现有控制握手仍声明 `separate_media_quic_version = 0`。启用配置不会假装建立媒体会话或注册系统设备。

## 已确认的产品约束

- macOS 13+（arm64/x86_64）、Windows 10/11 x64、Linux x86_64 PipeWire 1.x；系统音频接口加 Windows ASIO。
- 无压缩 PCM24/Float32，48/96 kHz，每对端最多 8 路输入和 8 路输出，共用活动采样率。
- 有线千兆和合格低延迟音频硬件上的单向端到端 P95 ≤10 ms；默认 1 ms 网络分帧、3 ms 接收缓冲，可设 2–20 ms。
- 导出端逐对端、逐端点授权。配对不意味着麦克风授权。导入端自动注册已授权端点；应用打开设备后才开始采集。
- 设备 UID 由远端 ID、端点持久 ID 和方向确定。临时断线保留设备，静音并丢弃过期数据；撤销授权停止流并撤销注册。
- 虚拟端点不再导出；不自动改变系统默认设备。不提供降噪、回声消除、自动增益和多远端采样级同步。
- ASIO 采用独立 GPLv3 组件和版本化 IPC；**该组件本轮未实现，未引入任何 ASIO SDK**。
- 开发验证版允许 Windows 测试签名；不自动重启机器或修改 Secure Boot。

## 代码分层

| 层 | 当前实现 | 边界 |
|---|---|---|
| `rshare-core::network_audio` | 端点、授权、配置、设备、会话、错误及媒体控制契约 | 与旧 PCM16 协议隔离 |
| `rshare-audio::registry` | 稳定身份、授权过滤、目录一致性、断线状态、注册确认、采样率与声道预算 | 纯策略；不会调用 OS |
| `rshare-audio::media` | 二进制分片、按 MTU 编码、有界乱序重组、代次与格式校验 | 音频线程之外执行分配 |
| `rshare-audio::engine` | PCM 转换、预分配播放缓冲、32 tap/256 phase 重采样、PI 漂移控制、静音恢复 | 尚未连接物理音频回调 |
| `rshare-audio::bridge` | 版本化 SPSC 共享内存 ABI、POSIX 映射、macOS XPC 映射客户端 | 一写一读，调用方负责角色所有权 |
| `rshare-net::media_transport` | 独立 QUIC、双向证书校验、控制 token/代次绑定、有界数据量及撤销连接 | 尚未接到 daemon 的连接事件与能力广告 |
| daemon/IPC/CLI/GUI | 状态查询、配置持久化、端点授权、撤销、错误展示 | `Open` 明确返回后端不可用 |
| macOS | 原生 UID 枚举、自有 HAL 插件、XPC 内存 broker、通用二进制构建和安装卸载脚本 | 仅组件构建和直接 vtable 测试通过；未加载进系统 HAL |
| Linux | PipeWire 目录解析与独立原生流桥接源码、构建/安装/卸载脚本 | 未在 Linux 编译或运行；未接入 daemon |
| Windows | 通用 Rust 音频代码能进行 GNU 目标检查 | WaveRT、ASIO 驱动和原生音频桥接均未实现 |

## 媒体与驱动接口

- 计划默认独立媒体端口 UDP 27438，ALPN `rshare-audio/1`。只有控制连接确认对端身份后才允许创建 `Binding`；token 和代次必须在两端控制平面中同步。
- 音频数据报使用 72 字节头：magic、版本、头长、流 UUID、代次、序号、采样位置、采样率、声道数、编码、采样帧数、分片序号/总数、偏移、总长度、分片长度、保留字段。多字节头字段为大端；PCM 数据为小端。
- 单包表示 1 ms 音频；重组最多同时保留 20 帧，20 ms 到期，拒绝重叠分片、错误长度、重复/旧代次数据。可靠流只用于媒体控制，音频数据不重传。
- `RShareAudioRing` ABI 1：128 字节头、4096 帧最大容量、最多 8 声道 Float32；单生产者/消费者原子游标。离线读静音；满环丢弃新数据。
- macOS HAL 使用自定义 `rsad` property 接受注册/移除请求，并通过 `AudioServerPlugIn_MachServices` 声明的 XPC 服务取得共享内存。直接文件映射仅用于 vtable 测试，不能冒充宿主沙箱验收。
- macOS 当前每个已创建端点固定一种采样率；运行中重设采样率、完整客户端 reset 与重启恢复仍须实现和验收。
- 本地 IPC 新增 `NetworkAudio(AudioCommand)` / `NetworkAudio(AudioSnapshot)`；快照带不同来源的诊断值，因此 `DaemonResponse` 不再实现 `Eq`，仍实现 `PartialEq`。
- 配置文件与 `layout.json` 同目录，名为 `network-audio.json`；缺失时默认禁用。使用临时文件、同步和替换持久化；非法配置只禁用音频并报告错误。
- `Configure` 不允许修改授权列表；必须使用 `Grant`/`Revoke`。`Grant` 还要求现有信任存储中有操作者明确批准的证书。
- 旧音频流新增 `buffer_depth_ms`；`latency_ms` 不再由队列深度填充。真实单向延迟没有测量时保持未知。

## 尚需实现，不能跳过

### Windows 现有 PCM16 转发路径的增量实现

现有 `StartAudioCapture` / `StartAudioForwarding` 路径增加 WASAPI loopback：选择默认或指定的扬声器端点，使用该端点的输出格式建立 CPAL 输入流。仅 Windows 支持该入口的系统声音采集；麦克风入口继续使用输入端点。指定端点不存在、离线、来源类型不符或名称重复时返回错误，不回退到默认麦克风。

采集队列最多保留 5 个 20 ms 帧，回调使用非阻塞入队；发送前按单调时钟丢弃超过 100 ms 的帧。发送计数只统计成功提交的帧。接收端校验格式、完整帧长度和会话内格式一致性；输出设备不支持相同采样率/声道数时明确报错，等待后续重采样接入。

这部分仍使用原有 PCM16 协议、20 ms 分帧与活动控制目标，不实现新网络音频虚拟设备、PCM24/Float32、8 入/8 出或 ≤10 ms 指标，也没有消除原有发送路径对网络管理锁的依赖。

验证命令：

```powershell
cargo test -p rshare-daemon -p rshare-audio -p rshare-net --locked --no-fail-fast
# 需要实际 Windows 扬声器端点，仅检查 loopback 流能否打开：
cargo test -p rshare-daemon windows_loopback_capture_opens_default_render_endpoint --locked -- --ignored
```

本次 Windows 验证：上述三包测试 524 通过、2 默认忽略；单独运行 loopback 硬件检查 1 通过，实际创建并启动本机默认输出端点的采集流。日志为 `target/audio-forwarding-tests.log` 和 `target/audio-loopback-hardware-test.log`。硬件检查未验证音频样本、跨机播放或物理延迟。

双机验证仍需启用现有音频采集/转发设置、建立 RemoteActive 目标，从系统声音端点启动转发，并在接收端确认播放；应分别测试指定端点、拔出设备、网络阻塞和停止转发。

### 新网络音频设备路径

1. daemon 将 authenticated control generation、媒体端口和 token 协商接入媒体连接；完成连接重置、撤销与关闭的整条生命周期。
2. 双向媒体控制处理器：授权目录发布、自动注册/移除、应用 open/close 通知、授权复核、多路会话、重连恢复与端点热插拔。
3. 将平台物理输入输出、虚拟端点共享环、网络收发和重采样连接为常驻实时工作线程；避免在键鼠锁或 Tokio 控制线程内执行音频工作。
4. 完成 macOS XPC 真正宿主加载和音频回调验证、采样率配置变化、broker/daemon/音频宿主重启恢复及长期设备资源回收。
5. Linux 原生构建与自动进程生命周期管理，物理端点路由、JACK 兼容性及失败状态反馈。
6. Windows WaveRT 动态端点驱动、IOCTL、安装卸载/测试签名，以及独立 GPLv3 ASIO 驱动、ASIO 物理硬件宿主与控制面板。
7. GUI 的完整路由/通道映射编辑及真实会话诊断推送；当前面板主要提供配置、授权和状态。
8. 三平台实机组合、真实第三方应用、物理延迟、键鼠竞争负载及 8 小时稳定性验收。

## 平台依据

- [Apple: Audio Server Plug-In sandbox and Mach services](https://developer.apple.com/library/archive/qa/qa1811/_index.html)
- [Microsoft: SYSVAD](https://learn.microsoft.com/en-us/samples/microsoft/windows-driver-samples/sysvad-virtual-audio-device-driver-sample/)
- [PipeWire streams](https://docs.pipewire.org/page_streams.html)
- [Steinberg ASIO GPLv3 distribution](https://www.steinberg.net/developers/asiosdk-open/)
