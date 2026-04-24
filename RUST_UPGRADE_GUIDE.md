# R-ShareMouse 依赖升级指南

## 📋 升级概览

### 日期
2025-04-24

### 升级的包

| 包名 | 原版本 | 新版本 | 破坏性变更 |
|------|--------|--------|------------|
| tokio | 1.40 | 1.52 | 🟢 否 |
| thiserror | 1.0 | 2.0 | 🟡 是 |
| bytes | 1.7 | 1.10 | 🟢 否 |
| enigo | 0.2 | 0.6 | 🔴 是 |
| arboard | 3.4 | 4.0 | 🟡 是 |
| config | 0.14 | 0.15 | 🟢 否 |
| uuid | 1.10 | 1.15 | 🟢 否 |
| hostname | 0.4 | 0.5 | 🟢 否 |
| egui | 0.29 | 0.30 | 🟡 是 |
| eframe | 0.29 | 0.30 | 🟡 是 |

---

## 🔴 重要：需要代码适配的升级

### 1. thiserror 1.0 → 2.0

**主要变更：**
- `#[error]` 宏现在需要更明确的格式字符串
- 某些 trait 的实现要求略有变化

**迁移示例：**
```rust
// 旧代码
#[derive(Error, Debug)]
pub enum MyError {
    #[error("Failed: {0}")]
    Failed(String),
}

// 可能需要更新为
#[derive(Error, Debug)]
pub enum MyError {
    #[error("Failed: {0}")]
    Failed(String),
}
```

### 2. enigo 0.2 → 0.6

**重大变更：**
- API 完全重新设计
- 特性系统变更
- 平台特定实现方式变化

**迁移指南：**
```rust
// 旧代码 (enigo 0.2)
use enigo::{Enigo, MouseControllable, KeyboardControllable};
let mut enigo = Enigo::new(&enigo::Settings::default()).unwrap();
enigo.mouse_move_to(100, 100);
enigo.key_click(enigo::Key::Space);

// 新代码 (enigo 0.6)
use enigo::{Enigo, Mouse, Keyboard};
let mut enigo = Enigo::new(&enigo::Settings::default()).unwrap();
enigo.move_mouse(100, 100, enigo::Coordinate::Abs);
enigo.text("hello");
```

**建议：** 查看 [enigo 更新日志](https://github.com/enigo-rs/enigo/blob/master/CHANGELOG.md)

### 3. egui/eframe 0.29 → 0.30

**主要变更：**
- 新的 Atoms 布局原语
- 增强的弹出支持
- 更好的 SVG 支持

**需要检查：**
- 自定义 UI 代码是否兼容
- 布局逻辑是否需要调整

---

## 🚀 升级步骤

### 1. 更新依赖
```bash
cd R-ShareMouse
cargo update
```

### 2. 检查编译错误
```bash
# 检查是否有编译错误
cargo check --workspace

# 如果有错误，先修复 enigo 相关的代码
```

### 3. 运行测试
```bash
cargo test --workspace
```

### 4. 构建所有应用
```bash
cargo build --release
```

---

## ⚠️ 已知问题和解决方案

### enigo 0.6 API 变更

问题：输入处理代码可能需要大量修改

**解决方案：**
1. 暂时保持 enigo 0.2，其他依赖先升级
2. 或创建适配层来桥接 API 差异
3. 或查看项目是否有使用 enigo 的高级特性

### thiserror 2.0

问题：某些错误类型可能需要调整

**解决方案：**
```bash
# 运行 clippy 获取建议
cargo clippy --workspace
```

---

## 🧪 测试清单

- [ ] `cargo check --workspace` 通过
- [ ] `cargo test --workspace` 通过
- [ ] `cargo clippy --workspace` 无错误
- [ ] `cargo build --release` 成功
- [ ] 输入捕获功能正常
- [ ] 输入注入功能正常
- [ ] GUI 启动和显示正常

---

## 🔙 回滚方案

如果升级后出现问题：

```bash
# 回滚 Cargo.toml
git checkout HEAD~1 Cargo.toml

# 清理并重新构建
cargo clean
cargo build
```

---

## 📚 参考链接

- [thiserror 2.0 迁移指南](https://docs.rs/thiserror/latest/thiserror/)
- [enigo 更新日志](https://github.com/enigo-rs/enigo/blob/master/CHANGELOG.md)
- [egui CHANGELOG](https://github.com/emilk/egui/blob/master/CHANGELOG.md)
- [tokio 1.52 发布说明](https://github.com/tokio-rs/tokio/releases/tag/tokio-1.52.0)
