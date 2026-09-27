# 扩展显示器（实验功能）

Windows 主机创建系统可见的第二屏，另一台电脑、手机或 iPad 使用浏览器
接收画面。接收端的多指输入注入为 Windows 原生触摸事件。

## 使用

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
   Android 当前使用系统浏览器接收，现有 APK 的 WebView 导航仍仅用于移动控制。
6. 主机点击“选择扩展屏并分享”，在浏览器选择器中选择刚创建的**完整显示器**。
   浏览器要求用户手动选择屏幕，不能由网页静默授权。
7. 确认主机预览确实来自该扩展屏，然后勾选启用 Windows 多点触摸。
   接收端支持最多十指；视频黑边不接受按下，拖动会限制在屏幕范围内。
8. 可把 Windows 窗口拖到扩展屏，接收端点击“播放 / 全屏”。保持主机页面运行。
   关闭主机页会释放触点并删除该会话创建的屏幕。

## 低延迟与有线传输

- 视频直接走 WebRTC，不经过截图轮询、JSON 像素数组或公共中继服务器。
- 优先协商 H.264，目标上限 1920×1080、60 fps、12 Mbps；实际编码器、
  硬件加速、帧率和延迟取决于浏览器、GPU、设备和网络，不承诺达到上限。
- 视频与控制分离；移动触点按动画帧合并，按下/抬起立即发送，队列有界。
- 统计显示协商编码、实际帧率/码率、ICE 选中连接的地址和网络 RTT。
  RTT **不是** glass-to-glass 画面延迟；地址被浏览器隐藏时不能证明走哪张网卡。
- 支持可达的 USB 网络共享、USB 以太网等 IP 链路。把接收链接中的主机 IP
  改为该网卡地址；必要时断开 Wi-Fi，确认视频没有改走无线候选地址。
- **iPad 仅插普通 USB 数据线的直接传输尚未实现。** USB 热点方式依赖
  支持热点的蜂窝版 iPad、运营商配置及 Windows Apple 驱动。Wi-Fi 版可通过
  兼容的 USB-C/Lightning 以太网适配器接入局域网。
- 真正 cable-only iPad USB 路径需要原生 iPad 接收程序和 Windows usbmux
  桥接；当前 Windows 工作区不能完成 iOS 签名、安装和真机验证。此实现
  没有把“接上数据线”显示为 USB 就绪。

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

仍需真机验收：系统第二屏枚举、窗口跨屏、iPad Safari 播放/全屏、双指
缩放/滚动、旋转/锁屏/拔线时触点释放、关闭主机后屏幕移除、Wi-Fi 与指定
USB 网络链路的实际选路及画面延迟 p50/p95。浏览器不提供 WebRTC 或屏幕
捕获 API 时明确报错；不把未测试的 iPadOS 版本列为已兼容。

References:
- https://developer.mozilla.org/en-US/docs/Web/API/MediaDevices/getDisplayMedia
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-injectsyntheticpointerinput
- https://support.apple.com/en-gb/111785
- https://github.com/libimobiledevice/libusbmuxd
