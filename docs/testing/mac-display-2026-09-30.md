# Mac 原生附屏实现与测试 — 2026-09-30

## 实现范围

- daemon 创建并持有 macOS 独立虚拟显示器，通过 ScreenCaptureKit 采集，
  VideoToolbox 编码 H.264；iOS 原生解码、等比显示、横屏和全屏。
- USB：直接使用 Apple usbmuxd，仅接纳 USB 设备，连接手机 loopback 27438。
  六位临时配对码；不会使用开发调试隧道或自动切换到无线。
- 局域网 / Wi-Fi：TCP 27439，TLS 1.2 PSK AES-128-GCM，每次接收生成新的
  256 位随机密钥，用户复制配对链接。地址限定私有 IPv4。蜂窝接口禁用。
- Mac 桌面提供 USB / 局域网切换、设备/链接输入、720p/1080p、开始/停止和
  daemon 状态。CLI 支持 `display usb` 与 `display lan --link`。
- 有界帧队列、序号与计数校验、错误配对拒绝、单接收会话、后台关闭、锁屏
  停止、socket 取消和虚拟屏清理。出现清理错误时保留失败状态，不声称已移除。
- 原生 Mac 路径暂不包含触摸回传、音频、Android 接收器、自动发现或自动重连。

## 环境及构建

- 主机：Apple Silicon，macOS 27.0 (26A428)。
- 手机：iPhone 16 Pro Max，iOS 27.0.1；最新开发签名应用已安装。
- Mac release daemon、CLI、GUI 及 React 前端构建通过。更新本地
  `R-ShareMouse.app`，保持原 Apple Development 签名，深度严格签名验证通过。
- 原生代码使用实验性 CGVirtualDisplay 私有 API，不能据此宣称所有 macOS 版本兼容。
- 未提交或推送；保留本工作区原有改动。`target/` 证据为本地构建产物。

## 已通过

| 验证 | 结果 | 本地证据（相对仓库根目录） |
| --- | --- | --- |
| 实际系统虚拟屏创建、枚举、删除、再次创建 | 通过 | `target/mac-usb-display/mac-display-no-transaction.log` |
| ScreenCaptureKit 采集真实虚拟屏并输出 H.264 SPS/PPS 与帧 | 通过；与上一项共 2 个实屏集成测试 | 同上 |
| 原生 Mac VideoToolbox 编码器输出格式 | 通过，生成像素 | `native-encoder-test.log`（均在同一证据目录） |
| USB 协议原生 iOS 解码/显示、横屏、全屏、后台停止 | 118 帧确认；2 个未认证连接拒绝 | `usb-regression-final.log` |
| TLS 局域网协议原生 iOS 解码/显示及同样 UI 生命周期 | 139 帧确认；错误 TLS 密钥及 2 个未认证连接拒绝 | `lan-receiver-tests.log` |
| Rust core 协议与 LAN 链接校验 | 4 个通过 | `core-final-tests.log` |
| Rust platform 单元测试 | 68 个通过 | `platform-final-tests.log` |
| daemon 单元测试 | 232 个通过 | `daemon-final-tests.log` |
| CLI / 桌面 Rust 单元测试 | 27 / 52 个通过 | `cli-final-tests.log`、`desktop-final-tests.log` |
| 前端测试 | 283 个通过 | `frontend-final-tests.log` |
| iOS 单元测试 | 10 个通过 | `ios-unit-final.log` |
| Mac 实际 GUI | USB 与加密局域网选择、配对输入、缺权限错误显示正常 | 原生窗口 AX 检查 |

118/139 帧两次 UI 回归使用原生 Mac 编码器生成的绿色视频，接收端是独立
iPhone 模拟器；它们证明协议、加密与原生渲染兼容，**不代表物理 USB 或
Wi-Fi 出画面验收**。测试调用真实 VideoToolbox 与显示层，未模拟“已连接”状态。

实屏测试修复了一个 macOS 生命周期问题：创建屏幕后冗余提交系统显示排列
会导致释放对象后屏幕仍存在。现在使用唯一显示器标识和系统默认排列，不在
创建流程调用显示配置事务，实际采集与移除回归均通过。运行中手动更改模式或
排列仍存在系统兼容性风险，发生回收失败时需重启 daemon。

## 真机验收状态：尚未完成

- 手机 USB 接口在 IOKit 中可见，但 Apple `usbmuxd` 的 ListDevices 和 Listen
  均未返回设备；`idevice_id -l` 也为空。开发工具当前连接类型为 localNetwork，
  不能把它算作 USB。尝试重启 USB 服务时 `sudo -n` 返回需要密码，未修改
  系统权限或信任记录。用户可执行 `sudo launchctl kickstart -k system/com.apple.usbmuxd`
  后重新插线、解锁并检查 `rshare display usb devices`。
- iPhone 真机 TLS 配对成功并触发了虚拟屏创建，但手机随后锁屏/离开接收页面；
  该次零视频帧，不能记为无线显示通过。
- 使用正式签名 daemon 再次连到原生接收器后，系统明确返回
  `Allow Screen Recording for R-ShareMouse in System Settings, then restart the app`。
  实屏集成测试进程具备的采集权限不等于正式应用已授权。已打开系统屏幕录制
  设置，等待用户开启 R-ShareMouse / rshare-daemon 权限。首次开始也会请求系统
  授权，不修改 TCC 数据库。
- 真机最终验收仍需：窗口跨入新增屏幕后的手机画面、实际无线网卡连接、
  USB 数据线传输、拔线/后台/锁屏回收、重新连接及持续运行。未测量端到端
  画面延迟或稳定帧率，不将 1080p/60 fps 配置值写成实测性能。

## 重现

```sh
cargo test -p rshare-platform --test macos_virtual_display --locked -- --ignored --test-threads=1
cargo test -p rshare-core usb_display --locked
cargo test -p rshare-daemon --bin rshare-daemon --locked

RSHARE_ENCODER_FIXTURE="$PWD/target/mac-usb-display/native-encoder-frame.bin" \
  cargo test -p rshare-platform --lib native_encoder_emits --locked
RSHARE_IOS_SIMULATOR=SIMULATOR_UUID \
RSHARE_NATIVE_ENCODER_FIXTURE="$PWD/target/mac-usb-display/native-encoder-frame.bin" \
  python3 scripts/test-ios-usb-receiver.py
RSHARE_IOS_SIMULATOR=SIMULATOR_UUID RSHARE_DISPLAY_TEST_TRANSPORT=lan \
RSHARE_NATIVE_ENCODER_FIXTURE="$PWD/target/mac-usb-display/native-encoder-frame.bin" \
  python3 scripts/test-ios-usb-receiver.py
```

实屏集成测试会短暂创建和删除真实系统显示器，只在解锁桌面显式运行。
不要公开配对链接、`*-private.json`、包含配对密钥的等待页截图或原始设备日志。
原生 UI 测试结果包位于同一证据目录的 `native-usb-receiver-*.xcresult`。

实现参考：[ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit/capturing-screen-content-in-macos)、
[Apple TLS PSK API](https://developer.apple.com/documentation/security/sec_protocol_options_add_pre_shared_key(_:_:_:))、
[libusbmuxd](https://github.com/libimobiledevice/libusbmuxd)、
[Chromium 虚拟屏实现](https://chromium.googlesource.com/chromium/src/+/HEAD/ui/display/mac/test/virtual_display_util_mac.mm)。
显示配置后无法移除的相关实测记录也见
[go-macos/virtualdisplay](https://pkg.go.dev/github.com/go-macos/virtualdisplay)；本仓库另进行了上述真实系统回归。
