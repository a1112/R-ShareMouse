# macOS 输入权限与开发包签名

权限界面读取运行中 daemon 的 CoreGraphics 预检结果，表示当前进程能否监听及发送键鼠事件，不是系统设置中两项开关的直接读数。辅助功能授权可同时提供监听和发送能力；因此只授予辅助功能后，输入监听显示可用、系统“输入监控”列表仍为空，可以是正常现象，不需要为了填满列表重复授权。

当前完整共享使用可拦截事件的 event tap，需要辅助功能；独立输入监控只提供监听能力，不能替代辅助功能完成远端输入注入和本机输入抑制。依据：[Apple DTS 权限说明](https://developer.apple.com/forums/thread/828052)、[WWDC19 macOS Security](https://developer.apple.com/videos/play/wwdc2019/701/)。

每次重新构建后用同一个 Apple Development 或 Developer ID 身份签名应用包，包括同包 daemon。不要用 ad-hoc 签名做需要持久保留输入权限的开发包。推荐从仓库根目录运行统一打包命令，退出旧应用和 daemon 后安装到固定路径：

```sh
APPLE_SIGNING_IDENTITY='Apple Development: YOUR NAME (TEAMID)' \
  bash scripts/package-macos-desktop.sh --install
open R-ShareMouse.app
```

该命令会构建前端、daemon、CLI 和 Tauri 应用，以相同证书签名 app 与同包二进制，然后将上一版应用移到废纸篓。未签名或 ad-hoc 的开发包不能作为日常更新包。另一台 Mac 可以使用自己的证书，但它之后的更新必须保持同一证书身份、bundle ID 和安装路径。若要两台机器共用同一发布包，应使用共同的发布证书。

首次切换到固定签名时，在“系统设置 → 隐私与安全性 → 辅助功能”中给**当前** `R-ShareMouse.app` 授权，然后在应用中重启服务，或完全退出应用和 daemon 后重新打开。授权前启动的 daemon 可能仍返回旧的权限结果，后台故障状态也会保留到重启；不能要求旧进程先报告就绪才允许重启。

最后点“重新检测”，确认监听与注入能力都可用，且输入后端为健康状态。如果重启后仍未生效，再核对授权的应用路径和固定签名；系统中的旧授权记录不能代替当前二进制的实际能力。无需重置所有应用的权限，也不要仅因为输入监控列表为空而删除已有授权。

只读检查示例：

```sh
codesign -dv --verbose=4 /path/to/R-ShareMouse.app
codesign -dv --verbose=4 /path/to/R-ShareMouse.app/Contents/MacOS/rshare-daemon
codesign --verify --deep --strict /path/to/R-ShareMouse.app
```

签名证书身份、应用 bundle ID、安装路径或二进制位置变化后，都应重新核验实际权限。CLI 或终端进程的授权状态不代表 GUI 启动的 daemon 已获授权。
