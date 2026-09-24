# 2026-09-21 整改实施与验收记录

依据用户提供的《R-ShareMouse 项目整改报告与 USB 驱动级转发设计》实施。
报告审查的是 `f838f29`，本次工作基于 `83ee176`，目录为 `H:/project/R-ShareMouse`。
已 fetch 当前 origin；工作分支 `codex/fix-public-ci` 与同名远端一致。
原有资源监视器工作区修改予以保留。未提交、推送、安装或加载新驱动。

**当前交付是权限与旧 USB 主机端的整改批次，不是报告全部里程碑完成。**
尤其没有实现接收端 UDE 虚拟 USB 总线，不能把下面的 descriptor probe 当作系统 USB 枚举。

## 已改变的行为

### 本地控制通道

- Windows 使用按账户 SID 和登录会话区分的命名管道。DACL 仅允许该账户和 SYSTEM，拒绝远程管道客户端；两端查询对方进程身份并验证账户、会话相同。
- Unix 使用 `/tmp/rshare-ipc-<uid>/daemon-<port>.sock`，检查目录所有者、0700 权限及双方 peer credentials；进程锁保护活跃 socket，拒绝符号链接，回收崩溃后遗留 socket。
- daemon、CLI 和桌面客户端已迁移。生产 daemon 的旧 TCP / WebSocket 控制入口停止监听。客户端与 daemon 需一起升级。
- 同时最多 32 个本地客户端，首个请求超时 5 秒。尚未实现按敏感操作分组的 capability scope，也没有跨提权边界的服务 broker。
- 性能工具中的 TCP 是独立的临时端口测试夹具，只衡量帧协议 handler；报告明确标注，不代表命名管道/Unix socket 的性能。

### 实验性 USB 主机端

- 本地明确授权后才向指定且人工批准证书的 peer 公布设备。导出标识为随机 UUID，网络端不能用本机设备路径作为 claim 的回退入口。
- 租约检查 peer、认证连接代际、租约 ID、导出标识及过期时间；传输 ID 不得重复或倒退。
- claim 必须独占；第一版授权仅接收已确认当前配置的单配置、单接口、alt 0、USB2 vendor-class 设备，端点仅允许 Bulk/Interrupt。
- 描述符解析失败时不再接收部分配置树；校验长度、接口/端点数量、重复端点、方向、请求载荷和超时。旧控制路径仅允许读取 device descriptor。
- Windows CreateFileW 添加 FILE_FLAG_OVERLAPPED；真实接口号替代固定接口 0。
- 原生同步 I/O 放入有界 blocking worker，退出网络消息处理主循环。实际仍有一个 runtime 锁，不是每设备 actor，也不是原生异步完成实现。
- RAII 资源记账覆盖工作线程和发送阶段：单请求载荷 1 MiB；租约 32 请求/4 MiB、peer 128 请求/16 MiB、全局 512 请求/64 MiB；全局执行 worker 最多 16 个。按多份同时驻留的缓冲区保守计费。解码前的网络缓冲与动态 wire credit 尚未接通。
- 断线、主动撤销和到期按租约所有者关闭原生句柄；旧连接的工作和完成会再次检查连接代际。同步 I/O 执行期间清理仍可能等待该调用完成。
- pending completion 先验证发送者、代际、设备、长度，再移除等待项；错误发送者无法吃掉合法等待项。
- 精确取消、reset、hotplug 能力不再宣称支持；取消不会再退化为中止设备全部端点。热插拔和独立 device/configuration generation 仍待实现。

### Windows HID 与 CI

- filter/vhid 控制设备 ACL 收紧为 SYSTEM。**服务 SID/broker 尚未完成：新驱动不能直接由普通或管理员桌面进程打开，不能作为可直接升级安装的完整驱动发行版。** 本次未改变已安装驱动。
- VHF 鼠标报告保留 32 位有符号位移和滚轮。七键以上发送 ErrorRollOver，内部保留全部按键状态，释放后恢复正确六键报告。
- VHF 控制文件独占，file cleanup 释放该拥有者的全部按键/按钮；注入校验精确长度及保留字段。仍缺少 watchdog、版本化写入 ABI 与内核采集抑制。
- Linux 测试 helper 先复制路径元数据再消费 event，修复报告描述的 E0505 模式；未整包克隆输入事件。
- Linux workspace tests 从性能 job 拆出，CI 起始创建 manifest，结束记录 success/failure/skipped 并始终尝试上传；HID C 行为测试独立执行。

## 实验性 USB 操作

两端应先升级到同一版本，启用已有 experimental USB 配置并人工批准对端证书。
以下命令不会安装接收端 USB 总线；`probe` 只做 device descriptor 控制传输。

```text
# 导出端：选择 list 返回的 UUID，而不是原生 Windows 设备路径
rshare usb list
rshare usb authorize <peer-uuid> <export-key> --lifetime-secs 300

# 另一端：使用导出端公布的 key
rshare usb list
rshare usb probe <exporter-peer-uuid> <export-key>

# 导出端：撤销授权并关闭租约
rshare usb revoke <peer-uuid> <export-key>
```

授权不持久化，最长 3600 秒；断线后需要重新授权。未授权设备不会公布。
同一账户内的进程仍属于同一个本地信任边界。

## RSM 问题台账

“源码实现”不等于已经通过驱动、跨系统或硬件验收。

| ID | 本次状态 | 剩余工作 |
|---|---|---|
| 001 | 所有权问题源码修复 | 在 Linux 完整 daemon 测试目标上运行验证 |
| 002 | ACL 收紧 | 服务 SID、broker、跨权限集成与安装回滚 |
| 003 | OS 身份本地 IPC 已迁移 | capability scope、跨账户攻击测试、Unix 实机运行 |
| 004 | peer/连接/lease/key 绑定，移除路径回退 | 独立设备和配置代际、完整热插拔事件 |
| 005 | OVERLAPPED 打开标志修复 | WinUSB 实机初始化验收 |
| 006 | 原生 I/O 移出网络事件循环 | 每设备 actor、每端点调度、原生异步完成 |
| 007 | 移除虚假精确取消，明确 unsupported | 每请求取消状态机和 CancelAck/完成竞态 |
| 008 | 分层资源预算及 RAII 回收 | wire credit、解码前预算和实际压力验收 |
| 009 | 先核验后移除，加入代际 | 新协议的完整 RequestKey |
| 010 | 断线/撤销/超时租约回收 | 原生 pending I/O 取消、热拔插统一终结 |
| 011 | 32 位鼠标报告与 C 行为测试 | WDK 构建及真实 Windows HID 栈验收 |
| 012 | ErrorRollOver 与完整按键状态恢复 | Windows 多键硬件验收；未新增 NKRO |
| 013 | 文件独占及 cleanup 释放 | watchdog、owner/epoch 契约和进程被杀验收 |
| 014 | 未实现 | fail-open 采集抑制状态机 |
| 015 | 实际接口号、严格单接口授权 | 复合接口句柄、alt/config barrier |
| 016 | 未实施动态测量 | DPC/锁持有时间基线，再决定队列调整 |
| 017 | 拆分 job、早期 manifest、逐项结果 | 远端 CI 实际运行 |
| 018 | USB 分派、授权、预算、本地传输抽取 | 继续拆分 IPC/音频/布局等编排 |
| 019 | 旧 ABI 精确长度/字段验证 | size/version/owner/epoch 新写入 ABI |
| 020 | 未实现 | MSRV 与固定工具链分开验证、依赖审计、SBOM、发布门禁 |

## 新 USB 架构仍未交付的内容

报告的独立版本化 USB wire 协议、export/import service、Windows UDE/UdeCx 接收驱动、
可验证的 loopback fixture、Linux USB/IP 互操作、完整配置/端点屏障、精确异步取消、
恶意载荷/fuzz、拥塞/断线/热拔插/长时压力实验均未完成。
旧 WinUSB 主机端的限制不构成这些里程碑的替代实现。

## 验证证据

本机工具链：Windows x64，rustc 1.94.1。构建产物位于 `D:/codex-build/rshare-rectification`。

| 验证 | 结果 | 本机证据 |
|---|---|---|
| core/platform/daemon Rust 回归 | core 单测 169、daemon main 225 通过；详见下面的基线失败 | `D:/codex-build/rshare-rectification-tests.log` |
| 命名管道真实收发、双向身份核验、单 listener | 通过 | `D:/codex-build/rshare-local-transport.log` |
| core 含测试的 Linux 目标交叉检查 | 通过，编译了 Unix 分支；没有运行 Linux 测试 | `D:/codex-build/rshare-linux-core-check.log` |
| USB 授权/描述符/请求校验专项 | 8 项通过 | `D:/codex-build/rshare-usb-tests.log` |
| Windows 驱动包契约 | 6 项通过 | `D:/codex-build/rshare-driver-package.log` |
| 桌面 Rust 编译检查 | 通过 | `D:/codex-build/rshare-desktop-check.log` |
| 最终 daemon/CLI/perf 编译检查 | 通过 | `D:/codex-build/rshare-final-check.log` |
| HID C 行为测试，MSVC /W4 /WX | 通过；±300、i32 极值、多键 overflow/recovery | `D:/codex-build/rshare-report-tests.exe` |
| CI manifest 人为失败/跳过场景 | 正确记录 failed 与 skipped | `D:/codex-build/rshare-evidence-test/manifest.json` |

全量 Rust 回归最初失败于两个测试目标；更新旧六键数组断言后 Windows 驱动包目标已通过。
仍有既有失败：`macos_virtual_display_driver_package_declares_framebuffer_user_client` 要求
`virtual_display.rs` 包含 `RSHARE_MACOS_VDISPLAY_SERVICE_CLASS` 等 IOKit 实现，当前基线文件没有这些代码。
本次没有删掉断言或新增假字符串来消除失败，不能声称整个 workspace 已全绿。

当前没有 WDK 驱动构建/Driver Verifier/签名/安装/物理 USB 枚举的验收证据。
WSL Ubuntu 已存在，但未配置本地 Linux cargo/gcc 工具链；交叉检查不能代替运行测试。
macOS 尚未接入当前 Codex 主机，也未提供可用 SSH 连接信息；没有跨机音频/USB 的实机测试结果。
