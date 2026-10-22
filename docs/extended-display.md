# 扩展显示器（实验功能）

Mac 主机可创建系统可见的独立扩展屏，通过 USB 或加密局域网传给原生 iOS
接收器。Windows 仍使用 IDD 驱动和浏览器 WebRTC 接收，多指输入注入为
Windows 原生触摸事件。

## Mac → iPhone / iPad（原生 USB、局域网 / Wi-Fi）

1. 安装新版本 Mac 和 iOS 应用，保持 Mac 解锁，并为 R-ShareMouse 开启系统
   屏幕录制权限。iOS 构建安装方法见 `apps/rshare-ios/README.md`。
2. 手机打开“扩展屏 → 连接 Mac 附屏”，选择连接方式，点击开启接收。
   等待与播放期间保持屏幕唤醒，进入后台或停止时恢复系统休眠设置。
3. Mac 设置 → 移动端控制 → Mac 扩展屏，选择同一种连接方式：
   - **USB 数据线**：信任此 Mac，选择 USB 设备并输入手机六位配对码；无需热点。
   - **局域网 / Wi-Fi**：两端在同一局域网，允许手机访问本地网络，复制手机的
     `rshare-display://` 配对链接到 Mac。链接含临时密钥，请勿公开；停止后失效。
4. 选择 720p 或 1080p，开始附屏。收到手机解码并提交显示的确认后，桌面才显示
   播放状态。可将窗口移入系统新增的屏幕；位置使用 macOS 默认排列。当前避免
   运行中更改屏幕模式或排列，以保证关闭时释放。原生接收支持等比缩放、
   横竖屏、全屏和退出全屏。
5. 停止、手机进入后台、链路断开或 Mac 锁屏都会结束会话并移除本次创建的屏幕。
   重新连接需要再次开启接收。一次只允许一个原生附屏会话。

Mac 使用实验性 `CGVirtualDisplay` 创建独立屏幕，ScreenCaptureKit 采集该屏，
VideoToolbox 编码 H.264；iOS 原生解码显示。视频队列有界，接收端逐帧确认。
目标上限 1080p/60 fps，实际帧率和延迟取决于设备与连接，尚无性能承诺。
Mac 私有显示 API 可能随系统更新变化；不需要内核驱动或修改 SIP。

USB 使用 Apple `usbmuxd`，仅选择 `ConnectionType=USB` 的设备，连接手机本地
27438 端口；不会自动切换到 Wi-Fi 或开发调试隧道。局域网使用 TCP 27439 和
TLS 1.2 PSK AES-128-GCM，每次接收生成新的 256 位随机密钥。当前配对链接仅
接受私有 IPv4 地址，不提供公网中继、自动发现或原生 Mac 触摸回传。

CLI（daemon 必须运行）：

```sh
rshare display usb devices
rshare display usb start --device DEVICE_UDID --code PHONE_CODE --width 1280 --height 720 --fps 30
rshare display lan --link 'PHONE_PAIRING_LINK' --width 1920 --height 1080 --fps 60
rshare display usb status  # 两种原生连接共用会话状态与停止命令
rshare display usb stop
```

硬件已接入但 USB 设备列表为空时，先检查手机解锁、信任、数据线和 Mac USB
枚举。Apple USB 服务异常与 Wi-Fi 开发工具可连接是两件事；不能把无线连接
记为 USB 验收通过。最新测试记录：`testing/mac-display-2026-09-30.md`。

## Windows → 浏览器接收

1. 主机安装并验证仓库中的 Windows IDD 虚拟显示驱动，见
   `drivers/windows/README.md`。主机必须运行在交互式用户桌面。
2. 在 `%APPDATA%\rshare\config.toml` 中启用：
   ```toml
   [features]
   mobile_gateway_enabled = true
   ```
   重启新构建的 daemon。此入口复用现有实验性 HTTP 网关，只在可信局域网使用；
   配对和信令不是 HTTPS，WebRTC 视频使用其加密传输。
3. 桌面设置 → 移动端控制 → 扩展显示器，复制本机发送链接，在主机
   **Edge / Chrome** 打开（链接必须使用 `localhost`）。
4. 点击“创建 1080p 扩展屏”。若已有虚拟显示器，先在显示设置中处理，
   会话不会接管或删除已有屏幕。未安装驱动时会显示错误。
5. 复制接收链接，在其他电脑或 iPad 浏览器打开。也可通过现有移动控制页面
   中的“用作扩展显示器”进入；需要有效的现有配对令牌。
   iPhone / iPad 可在原生应用中粘贴接收链接，直接进入“扩展屏”；已配对时
   点击“扩展屏”标签即可。Android 使用支持 WebRTC 的系统浏览器接收。
6. 主机点击“选择扩展屏并分享”，在浏览器选择器中选择刚创建的**完整显示器**。
   浏览器要求用户手动选择屏幕，不能由网页静默授权。
7. 确认主机预览确实来自该扩展屏，然后勾选启用 Windows 多点触摸。
   接收端支持最多十指；视频黑边不接受按下，拖动会限制在屏幕范围内。
8. 可把 Windows 窗口拖到扩展屏，接收端点击“播放 / 全屏”。保持主机页面运行。
   关闭主机页会释放触点并删除该会话创建的屏幕。

## 手机与平板接收体验

- iOS 点击“播放 / 全屏”会隐藏应用导航和标签栏，横竖屏保持等比画面；
  “退出全屏”返回控制页。原生容器保留网页触摸目标。
- iOS 在附屏画面实际播放时防止自动休眠；停止、切换标签、后台和错误时恢复。
  浏览器在支持并允许 Screen Wake Lock 时防休眠，HTTP 环境可能无法启用。
- 网络异常最多自动重试 5 次；也可点“重新连接”。页面保留内存中的配对凭证，
  重连不需要把令牌重新放回地址栏。主动“断开”后不会自动恢复。
- 后台/锁屏时释放所有触点并暂停接收，回到前台重新连接。主机仍保留本次
  创建的虚拟屏，接收端重连会同步屏幕信息，但必须由主机重新确认触摸授权。
- 此 WebRTC 页面使用 Windows 驱动路径；Mac 原生附屏按上文操作。

## 低延迟与有线传输

- 视频直接走 WebRTC，不经过截图轮询、JSON 像素数组或公共中继服务器。
- 优先协商 H.264，目标上限 1920×1080、60 fps、12 Mbps；实际编码器、
  硬件加速、帧率和延迟取决于浏览器、GPU、设备和网络，不承诺达到上限。
- 视频与控制分离；移动触点按动画帧合并，按下/抬起立即发送，队列有界。
- 统计显示协商编码、实际帧率/码率、ICE 选中连接的地址和网络 RTT。
  RTT **不是** glass-to-glass 画面延迟；地址被浏览器隐藏时不能证明走哪张网卡。
- 支持可达的 USB 网络共享、USB 以太网等 IP 链路。把接收链接中的主机 IP
  改为该网卡地址；必要时断开 Wi-Fi，确认视频没有改走无线候选地址。
- 浏览器的 USB 热点方式依赖
  支持热点的蜂窝版 iPad、运营商配置及 Windows Apple 驱动。Wi-Fi 版可通过
  兼容的 USB-C/Lightning 以太网适配器接入局域网。
- 普通数据线直连使用上文原生接收器。2026-09-29 浏览器路径的历史测试记录
  见 `testing/usb-display-2026-09-29.md`；不能用其开发调试隧道替代 USB 验收。

## 触摸边界

使用 Windows synthetic pointer API 的触摸类型，支持应用的滚动、缩放等
多点触摸行为；不会伪装为物理 HID 触摸屏，也不会承诺设置页显示硬件触控能力。
Apple Pencil 在网页中按普通触点处理，不提供笔倾斜、悬停或掌压拒绝。
无法注入安全桌面、UAC 或高权限窗口是 Windows 权限边界。

主机显式确认捕获屏幕后才能启用触摸；daemon 使用该屏幕的真实系统坐标。
断线、输入超时、显示布局变化会取消触点。修改屏幕分辨率或位置后重新开启会话。
当前只允许一个发送页和一个接收端。接收端断开后主机页面保留扩展屏，允许重新
接入，但需要重新启用触摸；关闭主机页或 daemon 才移除屏幕。

## 已执行自动验证与待验收

自动测试覆盖触摸坐标/序列/多指释放、负显示坐标、排他显示器租约、信令
权限/限流、HTTP 认证、连接代次撤销及关闭清理。浏览器联调使用真实 Edge
WebRTC 编码解码和生成画面，模拟 daemon 信令与系统捕获；不代表驱动/iPad
真机通过。运行命令：

```powershell
cargo test -p rshare-platform --test remote_touch --locked
cargo test -p rshare-daemon --bin rshare-daemon extended_display --locked
npm test --prefix apps/rshare-desktop-frontend
npm run build --prefix apps/rshare-desktop-frontend
$env:RSHARE_BROWSER_CHANNEL = 'msedge'
node apps/rshare-desktop-frontend/tests/extended-display-browser.mjs
```

iOS 原生接收回归（macOS、Xcode、已启动模拟器和 Chrome）：

```sh
RSHARE_IOS_SIMULATOR=SIMULATOR_UUID \
  node apps/rshare-desktop-frontend/tests/mobile-display-ios.mjs
```

该脚本启动真实 Chrome 编码器和本机 WebSocket 测试服务，运行 WKWebView
视频解码、原生全屏、横屏布局、手动重连和前后台恢复测试；检查全屏截图的
测试画面占比，结果及截图保存在 `target/mobile-display-validation/`。
输入为生成画面，系统屏幕和信令服务为测试替身，不能替代 Windows 驱动、
真实手机/平板及物理触摸验收。当前测试记录见
`docs/testing/mobile-display-2026-09-29.md`。

仍需真机验收：系统第二屏枚举、窗口跨屏、iPad Safari 播放/全屏、双指
缩放/滚动、旋转/锁屏/拔线时触点释放、关闭主机后屏幕移除、Wi-Fi 与指定
USB 网络链路的实际选路及画面延迟 p50/p95。浏览器不提供 WebRTC 或屏幕
捕获 API 时明确报错；不把未测试的 iPadOS 版本列为已兼容。

References:
- https://developer.apple.com/documentation/screencapturekit/capturing-screen-content-in-macos
- https://developer.apple.com/documentation/videotoolbox
- https://developer.apple.com/documentation/security/sec_protocol_options_add_pre_shared_key(_:_:_:)
- https://developer.mozilla.org/en-US/docs/Web/API/MediaDevices/getDisplayMedia
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-injectsyntheticpointerinput
- https://support.apple.com/en-gb/111785
- https://github.com/libimobiledevice/libusbmuxd
