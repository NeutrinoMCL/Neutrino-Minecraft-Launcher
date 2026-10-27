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
|------------------|------------------------------|
| 语言             | Rust 2024                    |
| GUI              | Slint 1.18                   |
| 下载             | ureq（阻塞式 HTTP + rustls） |
| 序列化 / 校验    | serde_json / sha1            |
| 解压             | zip（natives）               |
| 构建             | Cargo                        |
| CI               | GitHub Actions（Windows）    |
| 当前主要构建目标 | Windows MSVC                 |

## Microsoft 登录

微软账号登录使用 OAuth 2.0 设备码流程，Scope 为 `XboxLive.signin offline_access`。

- 登录入口：账户 → 微软账号。
- 启动登录后会自动打开浏览器，并在界面显示设备码。
- 登录成功后会读取 Minecraft 档案（UUID / 玩家名），把 Microsoft 账号写入账号列表并设为当前账号。
- Minecraft access token、Microsoft refresh token 和过期时间保存在 `%APPDATA%/Neutrino/account.toml`；
  启动游戏前如果 token 过期会自动刷新，刷新失败时会提示重新登录。
- 用 Microsoft 账号启动时使用 `userType=msa` 和真实 UUID / access token，可以进入正版服务器；
  离线角色仍然使用原来的离线 UUID。
- `account.toml` 里包含认证 token，请勿分享；删除账号会同时删除对应 token。旧版本的
  `accounts.json` 会在首次启动时自动迁移为 `account.toml`，迁移成功后删除旧文件。
- 如果登录时出现 **403 / Invalid app registration**：Microsoft/Xbox/XSTS 已经通过，但 Minecraft Services
  只允许白名单内的 Client ID 调用 `login_with_xbox`。需要先为你的 Entra 应用申请 Minecraft 第三方授权
  （见 [Minecraft 官方说明](https://help.minecraft.net/hc/en-us/articles/16254801392141)），
  或使用已经获批的 Client ID。这个 403 不是代码或 Entra 权限配置问题。
- 本地开发与 CI 的客户端 ID 配置见 [CONTRIBUTING.md](CONTRIBUTING.md#microsoft-登录配置可选)。

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
|------------------------|----------------------------------------|
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

## 实例页与发现页（GUI）

「实例」页左侧底部有「创建新实例」入口，点击后弹出创建窗口：

1. **选择创建方式**：先选「安装新实例」；「导入整合包」入口暂时只提示即将支持。
2. **选择版本**：弹窗内展示 Mojang 版本清单，支持正式版 / 快照 / 愚人节 / 全部筛选；点击任意版本行直接进入加载器选择，版本行不再带下载按钮。
3. **选择加载器**：Fabric、Forge，或「跳过」只装原版。
4. **选择加载器版本**：Fabric 会标出稳定版，Forge 展示该 Minecraft 版本对应的全部 Forge 版本。
5. **输入实例名**：默认 `<版本>-<加载器>`（跳过加载器时就是版本号），保存前会校验非法字符和重名。

创建完成后会生成自包含的 `versions/<实例名>/`（合并后的版本 JSON + 客户端 jar），
加载器依赖库和资源文件放在共享的 `libraries/`、`assets/`。Forge 安装会调用官方 installer，
需要本机已检测到 Java；Fabric 直接读取 Fabric Meta 的 loader profile，不需要启动 Java。

启动时 `--gameDir` 和工作目录都指向 `versions/<实例名>/`，所以 `mods/`、`saves/`、`config/`、
`logs/` 都是按实例隔离的；`libraries/`、`assets/` 仍是所有实例共享的运行时文件。

创建/切换加载器时会写入 `versions/<实例名>/neutrino.json` 记录原版版本、加载器类型和加载器版本。
实例详情「模组」页顶部会显示当前加载器；点击后可以拉取该 Minecraft 版本对应的加载器版本并切换，
切换只重写实例的版本 JSON 和依赖，不会删除 `mods/`、存档或配置。旧实例没有 `neutrino.json` 时，
会从版本 JSON 的 Fabric / Forge 依赖坐标推断。

「发现」页左侧分四个类别，右上角可以切换下载源、游戏版本、模组加载器和分类：

| 类别   | 数据来源                                                            | 下载位置                 |
|--------|---------------------------------------------------------------------|--------------------------|
| 模组   | Modrinth（project_type = mod）/ CurseForge（classId = 6）           | `mods/`                  |
| 光影包 | Modrinth（project_type = shader）/ CurseForge（classId = 6552）     | `shaderpacks/`           |
| 材质包 | Modrinth（project_type = resourcepack）/ CurseForge（classId = 12） | `resourcepacks/`         |
| 整合包 | Modrinth（project_type = modpack）/ CurseForge（classId = 4471）    | 暂仅浏览，一键安装待实现 |

- **搜索**：顶部的搜索框按 Enter 或点「搜索」会带上关键词重新请求列表；Modrinth 使用
  `query`，CurseForge 使用 `searchFilter`。清空输入或点「清除」会取消搜索。
- **项目标签**：项目名称后会用蓝色小标签显示客户端 / 服务端支持情况、所有分类和已识别的
  模组加载器；列表和项目详情都会显示。
- **游戏版本**：来自 Mojang 版本清单（只列正式版）。
- **模组加载器**：按下载源和类别动态提供。模组 / 整合包通常是 Fabric、Forge、NeoForge、Quilt、
  LiteLoader；光影是 Iris、OptiFine、Canvas；资源包不显示加载器筛选。
- **分类**：Modrinth 读取 `/tag/category`，CurseForge 读取对应 classId 的分类列表。
- **排序**：右上角下拉框，Modrinth 支持相关度 / 下载量 / 关注量 / 发布日期 / 更新日期；
  CurseForge 支持热度 / 名称 / 最近更新 / 创建日期 / 下载量。CurseForge 没有单独的
  created 排序字段，「创建日期」使用最接近的 ReleasedDate。

CurseForge 的加载器筛选必须配合游戏版本使用；没有选择游戏版本时加载器下拉框会禁用并提示先选版本。
这些搜索、筛选和排序都会进入请求，左侧摘要会显示当前生效的条件；单次最多加载 50 个项目。

点击任意项目会进入版本选择页：文件按游戏版本分组，组标题显示该组的文件数，点击标题可折叠 /
展开；版本名后面用蓝色标签显示加载器——模组是模组加载器，光影是 Iris / OptiFine / Canvas 等
光影加载器，资源包不显示。版本名下方只保留文件名和大小，不再重复游戏版本；点每行右侧的
「下载」按当前类别落到对应目录（项目列表本身不再放下载按钮）。兼容游戏版本太多的光影 /
资源包会归入「多版本」组。下载前同样走 SHA-1 校验；CurseForge 只允许下载 `downloadUrl`
不为空的文件。同一时间只允许一个下载任务。

实例详情的模组 / 资源包 / 光影面板右侧有一个下载按钮：它不会跳转页面，而是在弹窗里打开
发现页的搜索列表；打开时会自动按实例的游戏版本和加载器筛选，点进项目后版本按游戏版本从新到旧
分组，顶部会显示与实例最匹配的「推荐版本」和下载按钮。下载会直接写入当前实例对应的
`mods/`、`resourcepacks/` 或 `shaderpacks/` 目录，关闭弹窗后实例内容会自动刷新。直接在
「发现」页下载仍然落到默认 `.minecraft`。

旁边的更新按钮会检查已安装内容的新版本。下载弹窗会把来源记录到实例根目录的
`neutrino-content.json`（项目 ID、版本 ID、下载源）；更新检查只针对这些记录，向
Modrinth / CurseForge 查询与当前游戏版本、加载器兼容的最新正式版。手动放进文件夹的文件
不会被记录，也就不会出现在更新列表里。更新时下载新文件并替换旧文件，原本被禁用的内容
会继续保持禁用。

CurseForge 需要 API Key：复制 `.env.example` 为 `.env`，把 `CURSEFORGE_API_KEY`
设置成在 [CurseForge for Studios](https://console.curseforge.com/) 申请的 Key
（也兼容 `NEUTRINO_CURSEFORGE_API_KEY`）。Key 会按原文读取，包含 `$` 也不需要转义。
未配置时仍可使用 Modrinth；切到 CurseForge 会在列表上方提示缺少 Key。

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
