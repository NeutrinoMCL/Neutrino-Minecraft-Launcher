# Neutrino Minecraft Launcher

> 使用 Rust + Slint 构建的开源 Minecraft 启动器。

[![CI](https://img.shields.io/github/actions/workflow/status/NeutrinoMCL/Neutrino-Minecraft-Launcher/ci.yml?branch=main&label=CI&style=for-the-badge)](https://github.com/NeutrinoMCL/Neutrino-Minecraft-Launcher/actions/workflows/ci.yml)
[![Stars](https://img.shields.io/github/stars/NeutrinoMCL/Neutrino-Minecraft-Launcher?style=for-the-badge)](https://github.com/NeutrinoMCL/Neutrino-Minecraft-Launcher/stargazers)
[![Downloads](https://img.shields.io/github/downloads/NeutrinoMCL/Neutrino-Minecraft-Launcher/total?label=downloads&style=for-the-badge)](https://github.com/NeutrinoMCL/Neutrino-Minecraft-Launcher/releases)
[![Rust](https://img.shields.io/badge/Rust-2024-000000?logo=rust&logoColor=white&style=for-the-badge)](https://www.rust-lang.org/)
[![Slint](https://img.shields.io/badge/Slint-1.18-2379F4?style=for-the-badge)](https://slint.dev/)

> [!IMPORTANT]
> 项目处于早期开发阶段。当前已实现基础导航壳、壁纸界面、**命令行版本安装器**（`neutrino-install`），
> 以及 GUI「实例」页的原版 / Fabric / Forge 实例创建与 Modrinth 内容浏览下载；
> Minecraft 启动（离线账号）已可用，Microsoft 设备码登录流程已接入。

## 简介

Neutrino Minecraft Launcher（中微子 Minecraft 启动器）的目标是提供一个轻量、易用的桌面 Minecraft 启动器。项目使用 Rust 编写核心逻辑，使用 Slint 构建声明式用户界面。

当前版本：`0.1.0`（开发中）

## 功能状态

- [x] Rust + Slint 桌面应用骨架
- [x] Slint UI 骨架、顶部导航与壁纸背景
- [x] 设置页、外观主题切换（跟随系统 / 深色 / 浅色）、图片 / 纯色壁纸
- [x] 配置文件 `conf.toml`（主题 / 背景壁纸 / 纯色调色板，设置页修改后自动写回）
- [x] 实例列表（扫描 `versions` 目录）与详情标签页
- [x] 实例内容扫描（模组、资源包、光影）
- [x] 离线角色添加、选择与本地保存
- [x] 离线角色皮肤（Steve / Alex / 自定义）
- [x] Java 运行时检测与打开安装目录
- [x] 版本安装核心库（版本清单、依赖规则、并发下载、SHA-1 校验、natives 解压）
- [x] 命令行安装器 `neutrino-install`（支持 `--list` / `--dry-run` / 镜像地址 / 线程数）
- [x] 安装功能接入 GUI（「实例」页：创建实例弹窗、版本列表、安装进度、安装结果提示）
- [x] 创建实例时选择模组加载器（Fabric / Forge）与加载器版本，或跳过只装原版
- [x] 自定义实例名，实例目录为自包含的 `versions/<实例名>/`
- [x] 「发现」页四类别：模组 / 光影包 / 材质包 / 整合包（Modrinth / CurseForge 双下载源，支持按游戏版本、模组加载器、分类筛选和排序，点击项目选择版本和加载器下载）
- [x] GitHub Actions CI（格式检查、Clippy、测试）
- [x] 行为准则与安全策略
- [x] Microsoft 设备码登录流程（需配置客户端 ID）
- [x] Microsoft 账号在线启动（token 持久化、启动前自动刷新、`userType=msa`）
- [x] Minecraft 启动（参数拼装、JVM 参数、进程管理；当前支持离线账号 + 原版 / Fabric / Forge 实例）
- [x] 离线账户登录
- [ ] 整合包一键安装（`.mrpack` 解析与版本生成）
- [ ] 下载源与镜像配置（核心库已预留地址参数）
- [ ] 自动更新与发布流程

## 技术栈

| 类别             | 技术                         |
| ---------------- | ---------------------------- |
| 语言             | Rust 2024                    |
| GUI              | Slint 1.18                   |
| 下载             | ureq（阻塞式 HTTP + rustls） |
| 序列化 / 校验    | serde_json / sha1            |
| 解压             | zip（natives）               |
| 构建             | Cargo                        |
| 当前主要构建目标 | Windows MSVC                 |

## 配置文件

外观设置保存在 `conf.toml`。启动时按以下顺序查找第一个存在的文件：

1. 环境变量 `NEUTRINO_CONFIG` 指定的路径；
2. 当前工作目录下的 `conf.toml`（开发 / 便携模式）；
3. `%APPDATA%/Neutrino/conf.toml`。

```toml
[appearance.theme]
colormode = "light" # light / dark / system

[appearance.background]
mode = "image" # image / color
choice = "%built-in:Jokull"
wallpapers = ["C:\\Pictures\\night.png"]
random = false
color = "#1E3A8A"
```

- `colormode`：`light` / `dark` / `system`，未知值按 `system` 处理。
- `mode`：`image` 使用图片 / 内置壁纸，`color` 使用纯色背景。
- `choice`：`%built-in:<名称>` 使用内置壁纸（见 `ui/images/backgrounds/`），`%file:<绝对路径>` 使用自定义图片。
- `wallpapers`：设置页里添加的全部自定义壁纸（绝对路径）；列表下方可以逐张选择或删除。
- `random`：为 `true` 时每次启动从 `wallpapers` 里随机挑一张，`choice` 作为关闭随机后的回退。
- `color`：纯色背景的 `#RRGGBB` 颜色；设置页提供预设调色板和自定义 HEX 输入。
- 在设置页切换主题、切换图片 / 纯色、添加 / 选择 / 删除壁纸、选色、开关随机都会立即写回同一份配置文件。

## 版本安装（命令行）

GUI 的「实例」页 →「添加实例」已经可以通过向导创建原版 / Fabric / Forge 实例；
`neutrino-install` 是同一套安装核心的命令行入口（只装原版），适合脚本化或排错时使用。
两者都把依赖库和资源文件装进 `.minecraft`（或你指定的目录），装完启动器的「实例」页就能看到它。

```sh
# 查看可安装的版本（release / snapshot / all）
cargo run --bin neutrino-install -- --list
cargo run --bin neutrino-install -- --list snapshot

# 预演：只统计要下多少，不写盘
cargo run --bin neutrino-install -- 1.20.4 --dry-run

# 正式安装（含 assets）
cargo run --bin neutrino-install -- 1.20.4

# 只装最小可启动集合，装到指定目录、16 线程
cargo run --bin neutrino-install -- 1.20.4 -d D:/mctest --no-assets --threads 16
```

常用参数：

| 参数                   | 说明                                   |
| ---------------------- | -------------------------------------- |
| `-d, --dir <路径>`     | 游戏目录，默认取官方 `.minecraft`      |
| `-t, --threads <数量>` | 并发下载线程数，默认 8                 |
| `--retries <次数>`     | 单文件失败重试次数，默认 3             |
| `--manifest <URL>`     | 自定义版本清单地址（换镜像用）         |
| `--no-assets`          | 不下载资源文件（assets 通常几百 MB）   |
| `--virtual-assets`     | 为 1.6 及更早版本建立 virtual 资源目录 |
| `--dry-run`            | 只统计下载量，不写盘                   |

安装逻辑做了这些事：

1. 拉取 `version_manifest_v2.json` 并定位目标版本；
2. 拉取版本 JSON，按 `rules` 过滤出当前平台（Windows / macOS / Linux，x86_64 / arm64）需要的依赖库；
3. 下载客户端 jar、全部依赖库、资源索引与资源文件，**每个文件都用 SHA-1 校验**；
4. 1.18 及更早版本的 natives 会自动解压到 `versions/<id>/natives/`；
5. 已存在且校验通过的文件直接跳过，所以重复执行是幂等的，断点续传天然可用；
6. 结束时对比该版本要求的 Java 大版本与本机检测结果，给出提示。

## 下载

目前尚未发布可下载版本。首个 Release 发布后，顶部下载量徽章会自动更新。

## 已知问题

- **GUI 启动即崩溃（已规避）**：Slint 1.18.1 的 `accessibility` 特性在部分 Windows 环境下
  会导致窗口创建时 panic，表现为双击 exe 一闪就退。当前已关闭该特性，代价是暂时没有读屏软件（UIA）支持。
  相关开发注意事项见 [CONTRIBUTING.md](CONTRIBUTING.md#windows-开发注意事项)。
- 安装入口位于「实例」页，同一时间只允许一个安装 / 下载任务，暂不支持取消。
- 整合包（`.mrpack`）目前只能浏览，一键安装需要解析覆盖文件并生成版本 JSON，尚未实现。

## 参与贡献

欢迎提交 Issue 和 Pull Request！开发环境、代码约定、提交规范和 PR 流程请阅读
[CONTRIBUTING.md](CONTRIBUTING.md)，参与讨论前请遵守[行为准则](CODE_OF_CONDUCT.md)。

安全问题请勿公开提交 Issue，请按 [SECURITY.md](SECURITY.md) 中的方式报告。

## 许可证

本项目采用 [GPL-3.0](LICENSE) 许可证。

> NOT AN OFFICIAL MINECRAFT SERVICE. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT.
