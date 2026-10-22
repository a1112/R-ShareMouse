# 移动端附屏实现与验证（2026-09-29）

## 范围

在既有 Windows 系统扩展屏 / WebRTC 发送功能上，补齐 iPhone、iPad 原生接收
体验和通用移动浏览器的恢复逻辑。保留工作区原有 iOS 客户端及移动输入传输改动。
Windows 仍需安装虚拟显示驱动，并由用户在本机浏览器选择要分享的完整显示器。

本轮没有实现 macOS 系统虚拟扩展屏、Android 原生接收应用或普通 iPad USB
数据线直连。Android 的入口是支持 WebRTC 的系统浏览器。

## 实现

- 粘贴 `/display?t=...` 链接直接进入 iOS 附屏；重连保留所选标签。
- iOS 全屏隐藏导航、标签和状态栏，横竖屏等比显示，保留触摸及退出按钮。
  原生应用由 SwiftUI 管理容器，不再叠加 WebKit 的全屏窗口。
- 实际播放时启用 iOS 防休眠；停止、后台、离开标签或故障时释放。
  浏览器采用可用的 Screen Wake Lock，拒绝授权不会阻止播放。
- 网络中断最多重试五次；提供手动重连。凭证只在页面内存中保留，不写回地址栏。
  主动断开不自动重连，后台停止连接，前台恢复；按下中的触点在暂停时释放。
- 收集可能提前到达的 ICE 候选，忽略旧连接的异步结果。
- daemon 向新接收端同步当前屏幕信息，重连撤销触摸授权。最后一次空触点释放
  可以安全地与撤销并发；接收端心跳刷新桌面的移动连接状态。

## 自动验证

| 验证 | 结果与边界 |
| --- | --- |
| 前端 Node 测试 | 282 项通过，含防休眠异步释放、坐标与触点测试 |
| daemon 附屏测试 | 8 项通过，含真实 TCP/WebSocket 中继、权限、元数据、清理及心跳 |
| 移动网关回归 | 80 项通过；与附屏测试存在重叠，不作为独立总数累加 |
| 原生触摸状态测试 | 3 项通过；未执行 Windows 物理触摸注入 |
| 浏览器集成 | Chrome 真实 WebRTC 编码/解码、手机尺寸双指、旋转、持有触点暂停释放、自动/手动重连、五次重试上限通过 |
| iOS 单元与连接测试 | 6 项单元测试和 2 项连接/离线 UI 测试通过 |
| iPhone / iPad 接收测试 | 两台专用模拟器均通过真实 Chrome 编码 → WKWebView 解码；全屏、布局、退出、重连、前后台恢复与截图覆盖检查 |
| 构建 | 前端生产构建、daemon release 构建及 iOS 模拟器构建通过 |

现有 Rust unused-code 和前端包体积提示仍存在。没有声称整个 workspace 测试全绿。

测试输入是生成的视频，屏幕创建及信令服务为测试替身；实际生产网关的认证、
信令与心跳另由 Rust TCP/WebSocket 测试覆盖。所有图像都标明 WebRTC 测试画面。
XCTest 使用整屏截图保留正确的旋转信息；iPad 顶部标签和 iPhone 底部标签分别
由可访问性按钮验证。共享模拟器曾被另一应用切换前台，最终使用专用模拟器复验。

## 复现与本地证据

```sh
npm test --prefix apps/rshare-desktop-frontend
npm run build --prefix apps/rshare-desktop-frontend
cargo test -p rshare-daemon --bin rshare-daemon extended_display --locked
cargo test -p rshare-daemon --bin rshare-daemon mobile_gateway --locked
cargo test -p rshare-platform --test remote_touch --locked
cargo build -p rshare-daemon --release --locked
RSHARE_BROWSER_CHANNEL=chrome node apps/rshare-desktop-frontend/tests/extended-display-browser.mjs
RSHARE_IOS_SIMULATOR=SIMULATOR_UUID node apps/rshare-desktop-frontend/tests/mobile-display-ios.mjs
```

请选择没有被其他任务使用的已启动模拟器。原生视频测试脚本启动仅监听 loopback
的测试服务，并检查初次连接、手动重连、前台恢复至少三次接收连接。
结果及截图位于 `target/mobile-display-validation/`，普通 iOS 回归结果位于
`target/mobile-display-ios/Logs/Test/`；这些目录是本地证据，不提交配对凭证或日志。

本轮最终原生回执：

- iPhone：`target/mobile-display-validation/ios-1790675914291.xcresult`
- iPad：`target/mobile-display-validation/ios-1790676067185.xcresult`
- 单元/连接回归：`target/mobile-display-ios/Logs/Test/Test-RShareMouse-2026.09.29_18-00-01-+0800.xcresult`
- 视频截图：同名 `ios-*-attachments/` 下按 `manifest.json` 查找。

## 真机验收待办

尚未在 Windows 驱动主机与实体手机/平板之间验收系统第二屏枚举、窗口拖入、
触摸实际注入、锁屏/拔线、屏幕回收、持续帧率、指定网卡选路及端到端显示延迟。
模拟器通过不代表上述验收完成。使用步骤见 `docs/extended-display.md`。
