# 项目审查与修复记录 · 2026-09-05

审查基线为 `main` / `b248d9bf7ff844c890a4ba72d8da8d031c262a1c`，开始时已有 18 个文件的未提交修改。此次修复叠加在该工作区上，原始差异已保存。覆盖 Rust 工作区、输入路由与注入、QUIC/IPC、状态发布、桌面前端、资产导入、CLI、平台驱动契约、依赖和 CI 配置。

本轮修复已构建并验证，但项目不能标记为全部验收通过：实验性 macOS 虚拟显示仍缺少 Rust 接入，完整测试中有 1 项契约失败；真实双机键鼠验证也未完成。

## 已修复的问题

| 优先级 | 问题与触发条件 | 修复与证据 |
| --- | --- | --- |
| P1 | 进入远端后移动并点击，可靠点击帧仍携带本机边缘坐标及进入时的旧序号，注入端可能把光标移回错误位置。 | 点击坐标改为目标桌面跟踪坐标，随鼠标移动更新序号。两个路由回归用例修复前失败；新增生产路由到注入 actor 的集成测试，分别覆盖数据报正常到达及全部丢失。 |
| P1 | Vite 开发桥接接口直接处理跨来源 POST，可触发本机服务控制或 IPC 命令。 | 所有 `/__rshare` 路由先校验回环连接、Host、Origin、Fetch Metadata；POST 必须为 JSON。外部 Origin 的只读 Status 复现从 HTTP 200 变为 403，正常同源请求通过。 |
| P2 | 本机注入成功被算作远端通过；已断开的远端历史事件、过期 active 标记也可能误导验收状态。 | 注入结果必须属于当前已连接远端；失败结果优先于历史成功。后端验收优先消费 daemon 推送的 Input capability，没有该数据时要求明确的 Healthy 状态。新增断开、目标不符、降级与恢复回归。 |
| P2 | ZIP 资产导入没有解压大小或条目限制，逐文件完整读入内存；资产 ID 可包含子目录。 | 限制压缩包 64 MiB、单文件 64 MiB、总解压量 256 MiB、1024 项和 manifest 1 MiB；限制实际解压字节数并流式写盘；拒绝符号链接及嵌套/隐藏存储 ID。导入失败清理及原资产保留测试通过。 |
| P2 | 前端锁定依赖扫描有 8 项问题，其中 1 项 critical、6 项 high。 | Vite 6.3.5 → 6.4.3，Playwright 1.55.0 → 1.55.1，并更新兼容范围内的传递依赖。重新 `npm ci`、构建、测试后审计为 0。Vite 修补范围参见[官方安全公告](https://github.com/vitejs/vite/security/advisories/GHSA-fx2h-pf6j-xcff)。 |
| P2 | CLI 声明了未使用的 `atty 0.2.14`，引入停止维护及潜在未对齐读取提示。 | 删除该直接依赖及失去引用的旧 `hermit-abi` 锁定条目；CLI 24 项测试通过。Rust 审计中的 unmaintained / unsound 提示分别从 18 / 2 降为 17 / 1。 |
| P2 | `::1` 等 IPv6 绑定值被拼成没有方括号的地址，daemon 无法正确解析。 | 统一通过 `Config::bind_address()` 构造 `SocketAddr`，覆盖 IPv4、带/不带方括号的 IPv6 和非法绑定值。该修复不代表 IPv6 局域网发现已实机验收。 |
| P2 | QUIC CLI 功能测试要求共享开发机上的性能测量必须稳定，合法的“不稳定且保留证据”结果导致测试失败。 | 保留生产性能门槛；测试同时检查稳定成功，以及完整重试后不稳定必须返回错误、保留两个完整批次和准确原因。没有放宽时延、丢包或稳定性门槛。 |
| P2 | Linux CI 缺少 Tauri/X11/ALSA/udev 原生依赖，安全检查的 Rust 版本落后于当前锁定依赖；文档仍描述旧 GUI crate 和换行 JSON IPC。 | 增加共用 Linux 构建准备 action，构建嵌入前端，调整工具链版本并校正文档。原生依赖依据[Tauri 构建前置条件](https://v2.tauri.app/start/prerequisites/)。已检查 YAML 语法；尚未在远端 Linux runner 执行。 |
| P3 | 浅色主题验收标签沿用深色主题浅色文字，难以辨认；WebSocket 初始订阅函数的无效循环使默认 Clippy 失败。 | 增加两套状态文字色并检查实际页面；移除不发生循环的代码，保留关闭连接时的错误语义。 |

## 验证结果

本机环境：macOS / Apple Silicon，Rust 1.94.1，Node 26.4.0。CI 使用仓库 `.node-version` 固定的 Node 24.15.0；本轮未在该 Node 版本执行。

| 检查 | 结果 |
| --- | --- |
| `cargo test --workspace --locked --no-fail-fast` | **1100 通过，1 失败，1 忽略**，已运行全部测试目标和文档测试。忽略项为原有 `test_lan_discovery`。 |
| `cargo build --workspace --release --locked` | 通过，生成 `target/release/rshare-gui`、`rshare-daemon`、`rshare`、`rshare-perf`。 |
| `cargo clippy --workspace --all-targets --all-features --locked` | 通过，仍有既有编译/Clippy 告警；严格 `-D warnings` 基线未通过，不应宣称零告警。 |
| `cargo fmt --all -- --check`、`git diff --check` | 通过。 |
| 前端 `npm ci` / `npm test` / `npm run build` | 通过，**253 项测试**。生产 JS 约 610 kB，仍有大于 500 kB 的分包提示。 |
| Playwright 非固定性能机功能场景 | **1 项通过**，覆盖持续指针/离散输入更新且不触发备用轮询。使用已安装的 Chrome 和隔离测试配置；专用 Chromium 下载失败，未冒充固定浏览器性能验收。 |
| `npm audit` | **0 项漏洞**，包括开发依赖。 |
| `cargo audit --no-fetch --no-yanked` | 漏洞分类为 0；另有 **17 项 unmaintained、1 项 unsound** 提示。先前已同步数据库，最近提交为 2026-09-02；yanked 查询因网络超时未完成。 |
| macOS 虚拟显示 C++ SDK 语法和静态分析 | `scripts/driver/check-macos-vdisplay.sh` 通过。没有执行驱动加载。 |
| 实际界面 / CLI | 查看原生应用，并用最新前端连接当前 daemon 检查布局、设备页与验收标签。新构建的 CLI doctor 确认 IPC 在线、远端为 0、输入后端 `PermissionDenied` 降级，界面没有将其标记为双机通过。 |

最后删除未使用的 CLI 依赖后，另行通过 `cargo test -p rshare-cli --locked`（24 项）和 `cargo build -p rshare-cli --release --locked`；对应日志为 `cli-dependency-tests.log`、`cli-dependency-release.log`。

当前原生应用和 daemon 为审查开始前已经运行的进程；新代码的原生 Release 二进制已生成，最新界面代码通过浏览器桥接验证。此记录不代表已替换运行中的原生程序。

## 尚未关闭的缺口

1. **macOS 虚拟显示接入缺失。** `macos_virtual_display_driver_package_declares_framebuffer_user_client` 在缺少 `RSHARE_MACOS_VDISPLAY_SERVICE_CLASS` 的断言处失败。工作区没有对应 `IOServiceOpen` / `IOConnectCallStructMethod` 桥接、macOS 显示 EDID 识别常量和 CLI `driver-status` 命令。保留该契约及失败日志，驱动 README 已明确当前状态。完成该功能还需要接口实现、签名/驱动环境和显示拓扑实机验证。
2. **真实输入验收条件不满足。** 本机当前输入权限被拒绝、没有连接远端。单元测试、模拟注入 actor、QUIC loopback 和浏览器功能场景不证明 macOS/Windows 双机操作、锁屏、睡眠恢复或驱动输入已通过。
3. **Rust 依赖及严格 lint 债务。** `glib 0.18.5` 对应 `RUSTSEC-2024-0429`，受 Tauri 的 GTK 平台依赖链约束；需要上游依赖迁移及跨平台验证。未使用的 `atty` 已移除。普通 Clippy 通过不等于严格检查通过。

## 复查入口与本地证据

- 输入路由：`crates/rshare-core/src/input_router.rs`；端到端回归：`apps/rshare-daemon/tests/input_pipeline_integration.rs`。
- 开发接口边界：`apps/rshare-desktop-frontend/src/app/dev-bridge-security.mjs` 和对应测试。
- 验收状态：`apps/rshare-desktop-frontend/src/app/desktop-model.mjs` 及设备页调用。
- 资产解压：`apps/rshare-desktop/src-tauri/src/hardware_assets.rs`。
- 所有本轮日志、审查前补丁及审计结果保存在仓库忽略目录 `target/project-review/2026-09-05/`。主要文件为 `rust-all-targets.log`、`release.log`、`clippy-final.log`、`frontend-final.log`、`playwright-chrome.log`、`npm-audit-final.json`、`cargo-audit-final.json`、`live-doctor.log` 和 `before.patch`。

本轮未提交或推送。工作区仍包含审查前的修改；本报告只将表中列出的修复计为本轮成果。
