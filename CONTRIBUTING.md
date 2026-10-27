# 为 Neutrino Minecraft Launcher 做贡献

感谢你愿意花时间参与这个项目！无论是修一个错别字、报告一个 Bug，还是提交新功能，
每一份贡献都会让 Neutrino MCL 变得更好 :sparkles:。

开始之前，请先阅读 [行为准则](CODE_OF_CONDUCT.md)。参与本项目的所有交流都适用这份准则。

> [!IMPORTANT]
> 项目目前处于早期开发阶段，接口和界面都还可能变化。动手写代码前，建议先开 Issue 确认方向，
> 避免辛苦写完的改动因为设计不一致而无法合并。

## 新贡献者指南

第一次参与？可以从这些步骤开始：

1. 在 [Issues](https://github.com/NeutrinoMCL/Neutrino-Minecraft-Launcher/issues)
   和 [Pull Requests](https://github.com/NeutrinoMCL/Neutrino-Minecraft-Launcher/pulls)
   中搜索是否已有相同问题或改动。
2. 小改动（错别字、文档、明显的 Bug 修复）可以直接提交 PR。
3. 较大的改动（新功能、重构、依赖升级、界面改版）请先开 Issue 描述动机和方案，
   等维护者确认后再动手。
4. 如果你是来报告安全问题的，**不要**公开提 Issue，请按 [SECURITY.md](SECURITY.md) 的方式私下报告。
5. Fork 仓库、创建分支，然后按照下面的开发流程开始工作。

## 我们欢迎的贡献

- Bug 修复和崩溃、卡死、下载失败等稳定性问题
- 文档修正：错别字、失效链接、过时说明
- 测试补充：核心逻辑的单元测试，尤其是安装、路径、规则求值等模块
- 性能与体验改进：下载速度、内存占用、UI 响应
- 新功能：整合包（`.mrpack`）一键安装、下载镜像配置、自动更新等路线图上的内容
- 其他能让启动器更易用、更可靠的改动

## 需要先讨论的改动

以下改动不是不能做，但请先开 Issue 对齐方案：

- 大规模重构、目录结构调整、重命名公共 API
- 更换 GUI 框架、网络库、异步运行时等基础技术选型
- 添加体积较大的依赖，或引入新的系统级权限
- 与项目定位无关的功能（例如内置服务器管理、账号交易等）
- 纯格式化、纯重排代码但没有任何行为变化的 PR（会干扰 `git blame`）

## 开发环境

### 环境要求

- Rust stable **1.88+**（项目使用 edition 2024 与 `let` chains；当前用 1.99.1 验证）
- Windows：MSVC 工具链与 Visual Studio Build Tools（勾选“使用 C++ 的桌面开发”）
- 其他平台尚未验证
- 可选：[Slint Viewer](https://slint.dev/)（`cargo install slint-viewer`），用于单独预览界面

### Slint 资源

项目 UI 使用 Slint 1.18。开始改界面前，建议先浏览：

- [Slint 官方文档](https://docs.slint.dev/latest/docs/slint/)
- [Slint 中文文档](https://slintcn.neutriny.dev/)（非官方，由 [Neutriny](https://github.com/Neutriny) 维护）
- 仓库内的 `slint` skill（`.agents/skills/slint/`），包含常见布局与编译期陷阱说明

### 获取代码

```sh
git clone https://github.com/NeutrinoMCL/Neutrino-Minecraft-Launcher.git
cd Neutrino-Minecraft-Launcher
```

建议从 `main` 新建分支，分支名使用 `feat/`、`fix/`、`docs/`、`chore/` 等前缀，
例如 `feat/modpack-install`、`fix/download-retry`。

### Microsoft 登录配置（可选）

只有在调试微软账号登录时才需要配置客户端 ID。复制 `.env.example` 为 `.env`：

```powershell
Copy-Item .env.example .env
```

然后填写：

```text
NEUTRINO_MSA_CLIENT_ID=<你的 Azure 应用客户端 ID>
```

也可以在终端里临时设置：

```powershell
$env:NEUTRINO_MSA_CLIENT_ID = "<你的 Azure 应用客户端 ID>"
```

- `.env` 已被 `.gitignore` 忽略，`.env.example` 可以提交。
- client ID 是公开标识，不是密钥；不要把 `client_secret` 放进桌面客户端。
- CI 从仓库变量 `NEUTRINO_MSA_CLIENT_ID` 读取该值（Settings → Secrets and variables → Actions → Variables）。

## 常用命令

```sh
# 运行应用
cargo run

# 发布构建
cargo build --release

# 只预览 Slint UI（不启动完整应用）
slint-viewer ui/app-window.slint

# 检查单个 .slint 文件
slint-viewer --check ui/app-window.slint

# 命令行安装器：查看可用版本 / 预演安装
cargo run --bin neutrino-install -- --list
cargo run --bin neutrino-install -- 1.20.4 --dry-run
```

命令行安装器的完整参数和安装流程见 [README](README.md#版本安装命令行)。

## 提交前检查

提交 PR 前，请在本地运行与 CI 完全相同的三项检查：

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

需要自动格式化时运行：

```sh
cargo fmt --all
```

CI（`.github/workflows/ci.yml`）会在 `windows-latest` 上对 `main` 的推送和所有 PR 执行同样的检查，
所以本地通过后再推送可以省下一轮等待。

## 代码风格

- 使用 `rustfmt` 默认配置；提交代码前务必运行 `cargo fmt --all`。
- Clippy 警告视为错误，不要用 `#[allow(...)]` 掩盖问题，除非有充分理由并在代码里说明。
- `.editorconfig` 约定：UTF-8、LF 换行、文件末尾保留换行、去除行尾空白；
  Rust 使用 4 空格缩进，Slint 及其他文件使用 2 空格。
- 标识符、类型、模块名使用英文；面向用户的文档和界面文案使用中文。
- 新增逻辑尽量附带内联 `#[cfg(test)]` 单元测试（项目没有 `tests/` 目录）。
- 改动 UI 时请保持现有组件拆分方式，不要在 `app-window.slint` 里堆叠大段视图代码。

## 架构约定

- `src/lib.rs` 是 GUI 与 CLI 共用的库，`core`（安装、网络、路径、启动）、`java`、`instances`
  都在这里；**核心逻辑必须保持与 UI 无关**。
- `core/install.rs` 是唯一写入游戏目录的地方：所有文件都走 `.part` → 校验 → rename，
  已通过校验的文件直接跳过，保证重复执行幂等。新增写盘逻辑请复用这条路径。
- `core/paths.rs` 统一负责 `.minecraft` 目录布局，不要在别处手拼 `versions/...`。
- `src/desktop.rs`（`InstallController`）是 GUI 与核心库之间唯一的桥梁，负责版本列表缓存、
  筛选、后台安装 / 下载线程。Slint 属性只能在 UI 线程写，后台结果必须通过
  `Weak::upgrade_in_event_loop` 回到 UI 线程，进度回调则捕获 `Arc<Mutex<_>>` 的控制器。
- `src/bin/neutrino-install.rs` 是同一套安装核心的 CLI 入口，保持与 GUI 行为一致。
- `build.rs` 只编译 `ui/app-window.slint`；新增的 `.slint` 文件必须被它直接或间接 import，
  或者显式加入 `build.rs`，否则不会被编译。
- `slint` 与 `slint-build` 版本必须一致（当前固定 1.18.1）。
- 新增 `AppState` 属性时保持 `in`（只由 Rust 设置）；视图内的临时状态放在视图组件上。
- 项目结构：

```text
.
├── src/main.rs                  # GUI 入口（Slint）
├── src/lib.rs                   # 共享库入口（GUI 与 CLI 共用）
├── src/core/                    # 安装核心
│   ├── manifest.rs              # 版本清单 / 版本 JSON / 规则求值 / Maven 坐标
│   ├── modrinth.rs              # Modrinth API 封装（搜索 / 版本列表 / 文件选择）
│   ├── net.rs                   # HTTP 下载、重试、SHA-1 校验、并发
│   ├── install.rs               # 安装主流程、实例合并与报告
│   ├── loader.rs                # Fabric / Forge 版本查询与加载器安装
│   ├── launch.rs                # 启动参数拼装与进程管理
│   ├── natives.rs               # 旧版本 natives 解压
│   ├── paths.rs                 # .minecraft 目录布局
│   ├── progress.rs              # 进度上报（节流 + 可跨线程）
│   ├── hash.rs                  # SHA-1 工具
│   └── error.rs                 # 统一错误类型
├── src/bin/neutrino-install.rs  # 命令行安装器
├── src/desktop.rs               # 实例页控制器：版本列表 / 创建向导 / Modrinth 下载 / 安装线程编排（GUI 专用）
├── src/accounts.rs              # 离线角色与皮肤保存、切换
├── src/skins.rs                 # 自定义皮肤头像裁剪
├── src/image_util.rs            # 图像加载与转换工具
├── src/java.rs                  # Java 运行时检测
├── src/instances.rs             # versions 目录实例扫描与模组 / 资源包 / 光影扫描
├── src/preview3d.rs             # wgpu 3D 预览（皮肤 / 原理图），纹理导入 Slint
├── src/preview3d.wgsl           # 预览用着色器
├── src/msa.rs                   # Microsoft 设备码登录
├── ui/app-window.slint          # 主窗口入口
├── ui/shell/                    # AppShell / TopBar / NavItem
├── ui/views/                    # 启动页 / 实例页 / 账户页 / 发现页 / 设置页
├── ui/models/                   # Page、JavaInfo、VersionInfo 等共享类型
├── ui/state/                    # AppState / Actions / MsaState
├── ui/theme/                    # 主题偏好
├── ui/fonts/                    # 字体资源
├── ui/images/                   # 图标与壁纸
├── build.rs                     # 编译 Slint UI
├── .github/workflows/ci.yml     # CI 工作流
├── AGENTS.md                    # AI 协作说明
├── CONTRIBUTING.md              # 贡献指南
├── SECURITY.md                  # 安全政策
└── CODE_OF_CONDUCT.md           # 行为准则
```

## Commit 规范

提交信息使用 [Conventional Commits](https://www.conventionalcommits.org/) 前缀：

```text
feat: 添加整合包一键安装
fix: 修复 Forge 实例启动参数缺失
docs: 补充 Windows 构建说明
refactor: 拆分实例扫描逻辑
chore: 升级 Slint 到 1.18.1
```

- 一个提交只做一件事，不要把无关的格式化或重构混进功能提交。
- 提交信息用一句话说清“做了什么”，需要时在正文补充动机和影响范围。
- 合并前请整理提交历史，必要时 squash。

## Pull Request

`main` 分支受保护，所有改动都必须通过 PR 合并。

1. 从最新 `main` 创建分支并完成改动。
2. 推送分支后向 `main` 发起 PR，并填写 PR 模板（改动内容、动机、验证方式、备注）。
3. 如果 PR 对应某个 Issue，请在描述中关联（例如 `Closes #123`）。
4. 确保 CI 的格式检查、Clippy 和测试全部通过。
5. UI 改动请附上截图或录屏，并在本地用 `cargo run` 或 `slint-viewer` 实际验证。
6. 准备合并时再请求 review；仍在开发中的 PR 请使用 Draft。
7. PR 需要至少 1 个维护者 approval 才能合并；review 意见处理完后请重新请求 review。

写 PR 时请注意：

- 保持改动聚焦，方便 review；大改动可以拆成多个 PR。
- 不要提交构建产物、`.env` 或本地游戏目录；编辑器配置只保留团队共享的设置（参见 `.gitignore`）。
- 允许维护者编辑你的 PR 分支（GitHub 上的 “Allow edits by maintainers”），方便处理小问题。
- 如果 review 后长时间没有进展，可以在 PR 里礼貌地提醒。

## PR 合并之后

恭喜，你的改动已经进入 `main` :tada:。

- 可以删除已合并的本地分支，并同步最新的 `main`。
- 如果发现后续问题或想继续改进，欢迎再开 Issue 或 PR。
- 你的贡献会随下一次 Release 与所有用户见面。

## Windows 开发注意事项

- **不要给 `slint` 加回 `accessibility` 特性**。`Cargo.toml` 使用 `default-features = false`；
  在部分 Windows 11 + Slint 1.18.1 + accesskit_windows 0.34 环境下，窗口创建时 `SetPropW`
  会返回 `ERROR_NOT_ENOUGH_MEMORY` 导致启动即崩溃。要恢复读屏支持，先确认上游已修复。
- `.cargo/config.toml` 为 MSVC 目标设置了 8 MB 栈空间，请保留该设置。
- `.delta/` 是忽略的临时 worktree / clone 目录，搜索和编辑时请跳过。
- 从脚本里启动 GUI 验证时，用 `tasklist` 确认进程存活，而不是看 shell 是否返回；
  `(./app.exe &)` 在 shell 退出后可能把进程一并带走。

## 许可证

本项目使用 [GPL-3.0](LICENSE)。提交 PR 即表示你同意将自己的贡献按同一许可证发布，
并确认你有权提交这些代码。
