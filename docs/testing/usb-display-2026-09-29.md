# USB 到 iPhone 扩展显示实测（2026-09-29）

**验收结果：未通过。当前 Mac + 普通 USB 数据线不能把这部手机作为系统扩展屏。**

设备为 iPhone 16 Pro Max / iOS 27.0.1。`devicectl` 确认 physical、paired、
Developer Mode Enabled、Transport Type wired；当前 iOS Debug 应用已重新构建、
覆盖安装并成功启动。安装成功不是视频或扩展屏成功。

## 实测结果

| 层级 | 结果 | 证据 |
| --- | --- | --- |
| USB 连接、信任配对、应用安装/启动 | 通过 | `device-details.json`、`install.json`、构建日志 |
| 普通 iPhone USB 网络共享 | 当前未建立 | en9 inactive，DHCP 无 IP |
| Apple 开发调试隧道 | 页面和 WebSocket 信令通过 | utun4；手机 `fdf6:8806:6a48::1`，Mac `fdf6:8806:6a48::2`；收到 offer/answer |
| 排除其他网络后的 WebRTC 视频 | 未通过 | 没有获准 USB ICE 候选或选中连接；真机 videoWidth/videoHeight 均为 0 |
| Mac 系统第二屏 | 未通过 | daemon 创建请求返回 Unsupported，`virtual display creation is not implemented on this platform` |
| 黑屏时播放提示 | 已修复并真机复验 | `presentation.playing=false`；截图不再显示“画面播放中” |

视频探针使用当前接收页面和真实 iPhone WKWebView，主机 Chrome 提供生成画面，
信令由临时测试服务转发。**没有创建真实虚拟屏，也没有捕获 Mac 桌面。**
测试服务只监听 loopback 和开发隧道地址，使用随机临时令牌；信令中的 ICE
候选只允许指定隧道两端，过滤主机的 `192.168.2.16` 和手机的 mDNS 候选，
防止无线回退被误认成 USB。25 秒观察中没有解码画面；并未关闭用户的 Wi-Fi。

Apple 调试隧道的 HTTP 可达性不能替代普通数据线视频协议。现有 WebRTC
候选不包含此次调试隧道；需要补齐原生 USB 视频传输/主机桥接，同时实现
Mac 系统虚拟显示器，才能继续窗口跨屏与延迟验收。

## 本次修复与回归

真机暴露了 WebKit 在尚未收到视频帧时允许 `play()` 返回、`paused=false`
的问题。网页原先据此通知原生层“正在播放”。现在同时检查视频数据就绪、
非零画面尺寸和 PeerConnection 已连接；原生注入脚本的初始状态也检查画面。
首帧、尺寸变化、等待、断线会重新同步播放与防休眠状态。

- 先增加首帧状态测试并观察失败，修复后对应 8 项测试通过。
- 完整前端测试 283/283 通过。
- `cargo build -p rshare-daemon --release --locked` 通过（原有 11 项警告）；
  更新后的构建产物在 `target/release/rshare-daemon`，未替换运行中的 bundle。
- Chrome 真实 WebRTC 编解码、原生状态事件、触摸、重连及断线回归通过；
  系统捕获/信令仍为测试替身。使用已安装 Chrome，未安装额外浏览器。
  首轮退出码为 0；加入播放状态断言后的重跑输出 PASS，但本机 Node 26 /
  Chrome 测试进程在退出清理时挂起，已单独结束残留测试进程。断言通过
  与自动清理异常分别记录，不能把后者记成完整的正常退出。
- iOS 真机 Debug 构建、覆盖安装、USB 探针复测通过执行，探针结果如上，
  **不把探针正常结束记成 USB 视频通过**。

运行中的桌面 bundle/daemon 未替换；虚拟显示器 Unsupported 是对运行中
服务的实际请求结果，也与当前平台源码一致。测试产生的 Unsupported
显示器记录已删除，列表恢复为空；临时服务器/浏览器已关闭，手机回到
正常启动状态，保留原有配对信息。

## 复现与原始证据

先安装当前 Debug iOS 应用，并用 `devicectl device info details` 确认 wired；
通过 `ifconfig` 与 `route -n get -inet6 <手机隧道地址>` 确定两端地址。

```sh
RSHARE_IOS_DEVICE=DEVICE_UDID \
RSHARE_USB_HOST=MAC_TUNNEL_IPV6 \
RSHARE_USB_PEER=PHONE_TUNNEL_IPV6 \
  node apps/rshare-desktop-frontend/tests/mobile-display-usb.mjs

RSHARE_BROWSER_CHANNEL=chrome \
  node apps/rshare-desktop-frontend/tests/extended-display-browser.mjs
```

本机原始证据保存在 `target/usb-display-validation/`（不提交设备标识或原始截图）：

- `usb-probe.json`：请求来源、信令类型、候选过滤结果、双方实际视频状态。
- `phone-usb-probe.png`：修复后真机等待画面，未显示播放标记。
- `virtual-display-result.txt`：运行中 daemon 的 Unsupported 结果。
- `device-details.json`、`iphone-usb-network.txt`、`install.json`：设备/网络/安装结果。
- `ios-build.log`、`daemon-build.log`、`frontend-tests.log`、`browser-regression.log`：构建与回归结果。
