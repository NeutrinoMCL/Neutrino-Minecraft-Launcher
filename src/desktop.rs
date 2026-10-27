//! “发现”页的版本列表与安装流程（GUI 专用，CLI 不依赖）。
//!
//! 线程模型：
//! - Slint 的属性只能在 UI 线程写，所以后台线程一律通过
//!   `Weak::upgrade_in_event_loop` 把结果投递回 UI 线程；
//! - `core::Progress` 的回调可能来自多个下载线程，因此这里只捕获
//!   `InstallController`（内部全是 `Arc<Mutex<_>>`），满足 `Send + Sync`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use neutrino_minecraft_launcher::core::{
    DEFAULT_MANIFEST_URL, Downloader, GamePaths, InstallReport, InstanceInstallOptions, LoaderKind,
    Progress, VersionManifest, VersionSummary, curseforge, default_game_dir, install_instance,
    loader,
    modrinth::{self, ContentKind},
};
use neutrino_minecraft_launcher::instances;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{
    AppState, AppWindow, ContentUpdateInfo, InstanceInfo, LoaderInfo, ProjectInfo,
    ProjectVersionGroup, ProjectVersionInfo, VersionInfo,
};

/// 发现页游戏版本下拉框里表示“不筛选”的选项。
const ALL_GAME_VERSIONS: &str = "全部版本";
const ALL_LOADERS: &str = "全部加载器";
const ALL_CATEGORIES: &str = "全部分类";

/// 筛选下拉框里的一项：UI 显示 `label`，请求 API 用 `key`。
#[derive(Debug, Clone)]
struct FilterChoice {
    label: String,
    key: String,
}

fn all_choice(label: &str) -> FilterChoice {
    FilterChoice {
        label: label.to_string(),
        key: String::new(),
    }
}

fn labels(options: &[FilterChoice]) -> Vec<slint::SharedString> {
    options
        .iter()
        .map(|option| option.label.clone().into())
        .collect()
}

/// 加载器选项按下载源和内容类别变化；第一项永远是“全部加载器”。
fn loader_choices(source: DiscoverSource, kind: ContentKind) -> Vec<FilterChoice> {
    let entries: &[(&str, &str)] = match (source, kind) {
        (DiscoverSource::Modrinth, ContentKind::Mod) => &[
            ("Fabric", "fabric"),
            ("Forge", "forge"),
            ("NeoForge", "neoforge"),
            ("Quilt", "quilt"),
            ("LiteLoader", "liteloader"),
        ],
        (DiscoverSource::Modrinth, ContentKind::Shader) => &[
            ("Iris", "iris"),
            ("OptiFine", "optifine"),
            ("Canvas", "canvas"),
        ],
        (DiscoverSource::Modrinth, ContentKind::ModPack) => &[
            ("Fabric", "fabric"),
            ("Forge", "forge"),
            ("NeoForge", "neoforge"),
            ("Quilt", "quilt"),
        ],
        (DiscoverSource::CurseForge, ContentKind::Mod) => &[
            ("Forge", "forge"),
            ("Fabric", "fabric"),
            ("NeoForge", "neoforge"),
            ("Quilt", "quilt"),
            ("LiteLoader", "liteloader"),
        ],
        (DiscoverSource::CurseForge, ContentKind::ModPack) => &[
            ("Forge", "forge"),
            ("Fabric", "fabric"),
            ("NeoForge", "neoforge"),
            ("Quilt", "quilt"),
        ],
        _ => &[],
    };

    let mut choices = vec![all_choice(ALL_LOADERS)];
    choices.extend(entries.iter().map(|(label, key)| FilterChoice {
        label: (*label).to_string(),
        key: (*key).to_string(),
    }));
    choices
}

/// 排序选项按下载源变化；第一项是默认排序。
fn sort_choices(source: DiscoverSource) -> Vec<FilterChoice> {
    let entries: &[(&str, &str)] = match source {
        DiscoverSource::Modrinth => &[
            ("相关度", "relevance"),
            ("下载量", "downloads"),
            ("关注量", "follows"),
            ("发布日期", "newest"),
            ("更新日期", "updated"),
        ],
        DiscoverSource::CurseForge => &[
            ("热度", "2"),
            ("名称", "4"),
            ("最近更新", "3"),
            // CurseForge 没有单独的 created 排序，ReleasedDate 是最接近的字段。
            ("创建日期", "11"),
            ("下载量", "6"),
        ],
    };

    entries
        .iter()
        .map(|(label, key)| FilterChoice {
            label: (*label).to_string(),
            key: (*key).to_string(),
        })
        .collect()
}

fn default_sort(source: DiscoverSource) -> FilterChoice {
    sort_choices(source)
        .into_iter()
        .next()
        .unwrap_or_else(|| all_choice("相关度"))
}

/// Modrinth 的分类是 slug，转成可读一点的标题。
fn pretty_category(name: &str) -> String {
    name.split('-')
        .map(capitalize)
        .collect::<Vec<_>>()
        .join(" ")
}

/// 首字母大写，其余保持原样。
fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// 分类显示名：常见分类翻成中文，专有名词保留原文。
fn category_label(source: DiscoverSource, name: &str) -> String {
    match source {
        DiscoverSource::Modrinth => modrinth_category_label(name),
        DiscoverSource::CurseForge => curseforge_category_label(name),
    }
}

fn modrinth_category_label(slug: &str) -> String {
    let label = match slug {
        "adventure" => "冒险",
        "atmosphere" => "氛围",
        "audio" => "音频",
        "blocks" => "方块",
        "bloom" => "泛光",
        "cartoon" => "卡通",
        "challenging" => "挑战",
        "colored-lighting" => "彩色光照",
        "combat" => "战斗",
        "core-shaders" => "核心光影",
        "cursed" => "诅咒",
        "decoration" => "装饰",
        "economy" => "经济",
        "entities" => "实体",
        "environment" => "环境",
        "equipment" => "装备",
        "fantasy" => "奇幻",
        "foliage" => "植被",
        "food" => "食物",
        "fonts" => "字体",
        "game-mechanics" => "游戏机制",
        "gui" => "界面",
        "high" => "高配",
        "items" => "物品",
        "kitchen-sink" => "大杂烩整合包",
        "library" => "前置库",
        "lightweight" => "轻量",
        "locale" => "语言",
        "low" => "低配",
        "magic" => "魔法",
        "management" => "管理",
        "medium" => "中配",
        "minigame" => "小游戏",
        "mobs" => "生物",
        "models" => "模型",
        "modded" => "模组兼容",
        "multiplayer" => "多人",
        "optimization" => "优化",
        "path-tracing" => "路径追踪",
        "pbr" => "PBR",
        "potato" => "低配（土豆）",
        "quests" => "任务",
        "realistic" => "写实",
        "reflections" => "反射",
        "screenshot" => "截图",
        "semi-realistic" => "半写实",
        "shadows" => "阴影",
        "simplistic" => "简约",
        "social" => "社交",
        "storage" => "存储",
        "technology" => "科技",
        "themed" => "主题",
        "transportation" => "交通",
        "tweaks" => "微调",
        "utility" => "实用",
        "vanilla-like" => "原版风格",
        "worldgen" => "世界生成",
        _ => return pretty_category(slug),
    };

    label.to_string()
}

fn curseforge_category_label(name: &str) -> String {
    let label = match name {
        "Armor, Tools, and Weapons" => "装备与武器",
        "Blood Magic" => "血魔法",
        "Create" => "机械动力",
        "Integrated Dynamics" => "集成动力",
        "Dimensions" => "维度",
        "Server Utility" => "服务器实用",
        "Map and Information" => "地图与信息",
        "Addons" => "扩展",
        "Galacticraft" => "星系",
        "Bug Fixes" => "错误修复",
        "Applied Energistics 2" => "应用能源 2",
        "Biomes" => "生物群系",
        "API and Library" => "API 与前置",
        "Food" => "食物",
        "Tinker's Construct" => "匠魂",
        "Industrial Craft" => "工业时代",
        "Farming" => "农业",
        "Mobs" => "生物",
        "Education" => "教育",
        "Buildcraft" => "建筑",
        "Technology" => "科技",
        "Storage" => "存储",
        "Automation" => "自动化",
        "Utility & QoL" => "实用与体验",
        "Structures" => "结构",
        "Thermal Expansion" => "热力膨胀",
        "Energy, Fluid, and Item Transport" => "能量与物流",
        "Redstone" => "红石",
        "Forestry" => "林业",
        "Miscellaneous" => "杂项",
        "Genetics" => "基因",
        "World Gen" => "世界生成",
        "Performance" => "性能",
        "Twilight Forest" => "暮色森林",
        "Cosmetic" => "装饰",
        "Thaumcraft" => "神秘时代",
        "Twitch Integration" => "Twitch 集成",
        "Ores and Resources" => "矿物与资源",
        "Player Transport" => "玩家移动",
        "Energy" => "能源",
        "Adventure and RPG" => "冒险与 RPG",
        "Processing" => "处理",
        "Magic" => "魔法",
        "Skyblock" => "空岛",
        "CreativeMode" => "创造模式",
        "Refined Storage" => "精致存储",
        "Farmer's Delight" => "农夫乐事",
        "Horror" => "恐怖",
        "Photo Realistic" => "照片写实",
        "512x and Higher" => "512x 及以上",
        "Traditional" => "传统",
        "Font Packs" => "字体包",
        "Mod Support" => "模组支持",
        "Medieval" => "中世纪",
        "Data Packs" => "数据包",
        "Animated" => "动画",
        "Modern" => "现代",
        "Steampunk" => "蒸汽朋克",
        "Realistic" => "写实",
        "Fantasy" => "奇幻",
        "Vanilla" => "原版",
        "Extra Large" => "超大",
        "Small / Light" => "轻量",
        "Combat / PvP" => "战斗 / PvP",
        "Sci-Fi" => "科幻",
        "FTB Official Pack" => "FTB 官方整合包",
        "Quests" => "任务",
        "Tech" => "科技",
        "Map Based" => "地图",
        "Multiplayer" => "多人",
        "Mini Game" => "小游戏",
        "Vanilla+" => "原版增强",
        "Hardcore" => "硬核",
        "Exploration" => "探索",
        "Expert" => "专家",
        _ => return name.to_string(),
    };

    label.to_string()
}

/// CurseForge API Key。启动时从 `.env` 读原始值（见 [`init_curseforge_key`]）。
static CURSEFORGE_KEY: OnceLock<Option<String>> = OnceLock::new();

/// 启动时调用一次，记录 CurseForge API Key。
///
/// 不能只用 `dotenvy` 读环境变量：CurseForge 的 Key 形如 `$2a$10$...`，
/// `dotenvy` 会把 `$2a` / `$10` 当成变量展开，把 Key 截断。
/// 所以这里优先按原样读 `.env` 文件；外部环境变量优先于 `.env`。
pub fn init_curseforge_key(external: Option<&str>, env_path: Option<&Path>) {
    let key = external
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .or_else(|| env_path.and_then(read_curseforge_key));

    let _ = CURSEFORGE_KEY.set(key);
}

fn read_curseforge_key(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let key = key.strip_prefix("export ").map(str::trim).unwrap_or(key);
        if !curseforge::API_KEY_ENVS.contains(&key) {
            continue;
        }

        let value = unquote_env_value(value.trim());
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    None
}

fn unquote_env_value(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        return &value[1..value.len() - 1];
    }

    value.split(" #").next().unwrap_or(value).trim()
}

/// 发现页的下载源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoverSource {
    Modrinth,
    CurseForge,
}

impl DiscoverSource {
    fn parse(key: &str) -> Self {
        if key.eq_ignore_ascii_case("curseforge") {
            Self::CurseForge
        } else {
            Self::Modrinth
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Modrinth => "modrinth",
            Self::CurseForge => "curseforge",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Modrinth => "Modrinth",
            Self::CurseForge => "CurseForge",
        }
    }
}

/// 版本列表的筛选条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    Release,
    Snapshot,
    Fools,
    All,
}

impl Filter {
    fn parse(key: &str) -> Self {
        match key {
            "snapshot" => Self::Snapshot,
            "fools" => Self::Fools,
            "all" => Self::All,
            _ => Self::Release,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Release => "正式版",
            Self::Snapshot => "快照版",
            Self::Fools => "愚人节版",
            Self::All => "版本",
        }
    }

    fn matches(self, version: &VersionSummary) -> bool {
        match self {
            Self::Release => version.kind == "release",
            Self::Snapshot => version.kind == "snapshot",
            Self::Fools => is_april_fools(&version.id),
            Self::All => true,
        }
    }
}

/// Mojang 清单里历年愚人节版本的固定 ID。
///
/// 这些版本在清单里仍然标成 `snapshot`，只能按 ID 识别；以后新增的愚人节版本
/// 需要补进这个列表。
fn is_april_fools(id: &str) -> bool {
    const APRIL_FOOLS: [&str; 9] = [
        "15w14a",
        "1.RV-Pre1",
        "3D Shareware v1.34",
        "20w14infinite",
        "22w13oneblockatatime",
        "23w13a_or_b",
        "24w14potato",
        "25w14craftmine",
        "26w14a",
    ];

    APRIL_FOOLS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(id))
}

/// 当前打开项目的来源信息，用于把下载的文件记录到实例的更新清单里。
#[derive(Debug, Clone)]
struct ProjectRef {
    id: String,
    source: DiscoverSource,
}

/// 从实例下载弹窗打开项目时的目标环境，用来算推荐版本。
#[derive(Debug, Clone)]
struct DownloadContext {
    game_version: String,
    /// 加载器显示名，例如 Fabric / Forge
    loader: String,
}

/// 发现页的控制器，可在多个回调之间共享。
#[derive(Clone)]
pub struct InstallController {
    ui: Arc<Mutex<slint::Weak<AppWindow>>>,
    manifest: Arc<Mutex<Option<VersionManifest>>>,
    filter: Arc<Mutex<Filter>>,
    /// 当前发现页类别；None 表示"原版游戏"
    content: Arc<Mutex<Option<ContentKind>>>,
    /// 当前下载源
    source: Arc<Mutex<DiscoverSource>>,
    /// 当前搜索关键词；空字符串表示不搜索
    query: Arc<Mutex<String>>,
    /// 当前游戏版本筛选；None 表示“全部版本”
    game_version: Arc<Mutex<Option<String>>>,
    /// 当前模组加载器筛选
    loader_filter: Arc<Mutex<Option<FilterChoice>>>,
    /// 当前分类筛选
    category_filter: Arc<Mutex<Option<FilterChoice>>>,
    /// 当前排序（始终有一项）
    sort_filter: Arc<Mutex<FilterChoice>>,
    /// 当前可选的加载器 / 分类 / 排序（UI 只读 label，控制器按 label 找 key）
    loader_options: Arc<Mutex<Vec<FilterChoice>>>,
    category_options: Arc<Mutex<Vec<FilterChoice>>>,
    sort_options: Arc<Mutex<Vec<FilterChoice>>>,
    /// 发现页列表请求的代号，用来丢弃过期结果
    projects_generation: Arc<AtomicU64>,
    /// 分类选项请求的代号
    filters_generation: Arc<AtomicU64>,
    /// 当前打开项目的版本列表（点击下载时按 key 查找）
    versions: Arc<Mutex<Vec<VersionChoice>>>,
    /// 当前打开的项目；下载到实例时用来记录来源
    project_ref: Arc<Mutex<Option<ProjectRef>>>,
    /// 项目详情请求的代号
    project_generation: Arc<AtomicU64>,
    /// 当前下载目标实例；`None` 表示下载到默认 `.minecraft`
    download_target: Arc<Mutex<Option<PathBuf>>>,
    /// 实例内容更新检查的代号
    update_generation: Arc<AtomicU64>,
    /// 检查到的可更新项
    update_candidates: Arc<Mutex<Vec<UpdateCandidate>>>,
    /// 是否有更新任务正在执行
    updating_content: Arc<AtomicBool>,
    /// 实例详情里“切换加载器版本”列表请求的代号
    instance_loader_versions_generation: Arc<AtomicU64>,
}

impl InstallController {
    /// 绑定窗口。
    pub fn new(ui: slint::Weak<AppWindow>) -> Self {
        Self {
            ui: Arc::new(Mutex::new(ui)),
            manifest: Arc::new(Mutex::new(None)),
            filter: Arc::new(Mutex::new(Filter::Release)),
            content: Arc::new(Mutex::new(None)),
            source: Arc::new(Mutex::new(DiscoverSource::Modrinth)),
            query: Arc::new(Mutex::new(String::new())),
            game_version: Arc::new(Mutex::new(None)),
            loader_filter: Arc::new(Mutex::new(None)),
            category_filter: Arc::new(Mutex::new(None)),
            sort_filter: Arc::new(Mutex::new(default_sort(DiscoverSource::Modrinth))),
            loader_options: Arc::new(Mutex::new(vec![FilterChoice {
                label: ALL_LOADERS.to_string(),
                key: String::new(),
            }])),
            category_options: Arc::new(Mutex::new(vec![FilterChoice {
                label: ALL_CATEGORIES.to_string(),
                key: String::new(),
            }])),
            sort_options: Arc::new(Mutex::new(sort_choices(DiscoverSource::Modrinth))),
            projects_generation: Arc::new(AtomicU64::new(0)),
            filters_generation: Arc::new(AtomicU64::new(0)),
            versions: Arc::new(Mutex::new(Vec::new())),
            project_ref: Arc::new(Mutex::new(None)),
            project_generation: Arc::new(AtomicU64::new(0)),
            download_target: Arc::new(Mutex::new(None)),
            update_generation: Arc::new(AtomicU64::new(0)),
            update_candidates: Arc::new(Mutex::new(Vec::new())),
            updating_content: Arc::new(AtomicBool::new(false)),
            instance_loader_versions_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 从网络拉取版本清单（后台线程），成功后重建列表。
    pub fn refresh_versions(&self) {
        // 回调都在 UI 线程触发，这里的 upgrade 是安全的
        if self.loading() {
            return;
        }

        self.on_ui(|ui| {
            let state = ui.global::<AppState>();
            state.set_versions_loading(true);
            state.set_versions_error("".into());
        });

        let this = self.clone();
        std::thread::spawn(move || {
            let downloader = Downloader::new(Progress::silent());
            match downloader.get_json::<VersionManifest>(DEFAULT_MANIFEST_URL, "版本清单") {
                Ok(manifest) => {
                    if let Ok(mut cache) = this.manifest.lock() {
                        *cache = Some(manifest);
                    }
                    this.rebuild_from_cache();
                }
                Err(err) => {
                    this.on_ui(move |ui| {
                        let state = ui.global::<AppState>();
                        state.set_versions_loading(false);
                        state.set_versions_error(format!("获取版本列表失败：{err}").into());
                    });
                }
            }
        });
    }

    /// 切换筛选条件（只用缓存重建，不联网）。
    pub fn apply_filter(&self, key: &str) {
        if let Ok(mut filter) = self.filter.lock() {
            *filter = Filter::parse(key);
        }
        self.rebuild_from_cache();
    }

    /// 切换发现页类别；"vanilla" 走版本列表，其余走项目列表。
    pub fn open_category(&self, key: &str) {
        self.close_project();

        let kind = ContentKind::parse(key);
        if let Ok(mut content) = self.content.lock() {
            *content = kind;
        }

        let category = if kind.is_some() {
            key.to_string()
        } else {
            "vanilla".to_string()
        };

        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_category(category.into());
            state.set_download_message("".into());
            state.set_projects_error("".into());
            state.set_install_message("".into());
        });

        self.refresh_filter_options();
        match kind {
            Some(kind) => self.load_projects(kind),
            None => self.rebuild_from_cache(),
        }
    }

    /// 从实例下载弹窗打开内容列表：按实例游戏版本 / 加载器自动筛选。
    pub fn open_instance_download(&self, kind_key: &str, game_version: &str, loader_name: &str) {
        let Some(kind) = ContentKind::parse(kind_key) else {
            return;
        };

        self.close_project();

        if let Ok(mut content) = self.content.lock() {
            *content = Some(kind);
        }
        let game_version = game_version.trim().to_string();
        if let Ok(mut current) = self.game_version.lock() {
            *current = (!game_version.is_empty()).then(|| game_version.clone());
        }
        if let Ok(mut filter) = self.category_filter.lock() {
            *filter = None;
        }
        if let Ok(mut query) = self.query.lock() {
            query.clear();
        }

        let source = self
            .source
            .lock()
            .map(|guard| *guard)
            .unwrap_or(DiscoverSource::Modrinth);
        let loader = loader_choices(source, kind)
            .into_iter()
            .find(|choice| choice.label.eq_ignore_ascii_case(loader_name.trim()))
            .filter(|choice| !choice.key.is_empty());

        let category = kind.project_type().to_string();
        let selected_game_version = game_version.clone();
        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_category(category.into());
            state.set_discover_query("".into());
            state.set_selected_game_version(selected_game_version.into());
            state.set_download_message("".into());
            state.set_projects_error("".into());
            state.set_install_message("".into());
        });

        // refresh_filter_options 会清空加载器 / 分类筛选，所以要在它之后再设置。
        self.refresh_filter_options();
        if let Ok(mut filter) = self.loader_filter.lock() {
            *filter = loader;
        }
        self.load_projects(kind);
    }

    /// 设置搜索关键词并重新加载当前类别。
    pub fn set_query(&self, query: &str) {
        let query = query.trim().to_string();
        let changed = {
            let Ok(mut current) = self.query.lock() else {
                return;
            };
            let changed = *current != query;
            *current = query.clone();
            changed
        };

        self.close_project();
        self.on_ui(move |ui| {
            ui.global::<AppState>().set_discover_query(query.into());
        });

        if changed {
            self.reload_projects();
        }
    }

    /// 切换下载源（Modrinth / CurseForge），并重新加载当前类别。
    pub fn choose_source(&self, key: &str) {
        let source = DiscoverSource::parse(key);
        let changed = {
            let Ok(mut current) = self.source.lock() else {
                return;
            };
            let changed = *current != source;
            *current = source;
            changed
        };
        if !changed {
            return;
        }

        self.close_project();
        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_discover_source(source.key().into());
            state.set_download_message("".into());
            state.set_projects_error("".into());
        });
        self.refresh_sort_options(source);
        self.refresh_filter_options();
        self.reload_projects();
    }

    /// 选择游戏版本筛选；空字符串或“全部版本”表示不筛选。
    pub fn choose_game_version(&self, version: &str) {
        let version = version.trim();
        let next = if version.is_empty() || version == ALL_GAME_VERSIONS {
            None
        } else {
            Some(version.to_string())
        };

        let changed = {
            let Ok(mut current) = self.game_version.lock() else {
                return;
            };
            let changed = *current != next;
            *current = next.clone();
            changed
        };
        if !changed {
            return;
        }

        // CurseForge 的 modLoaderType 必须配合 gameVersion 使用；
        // 取消版本筛选时把加载器筛选一并清掉。
        let source = self
            .source
            .lock()
            .map(|guard| *guard)
            .unwrap_or(DiscoverSource::Modrinth);
        if source == DiscoverSource::CurseForge
            && next.is_none()
            && let Ok(mut filter) = self.loader_filter.lock()
        {
            *filter = None;
        }

        self.close_project();
        let selected = next.unwrap_or_default();
        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_selected_game_version(selected.into());
            state.set_download_message("".into());
            state.set_projects_error("".into());
        });
        self.reload_projects();
    }

    /// 选择模组加载器筛选；“全部加载器”表示不筛选。
    pub fn choose_loader_filter(&self, label: &str) {
        let source = self
            .source
            .lock()
            .map(|guard| *guard)
            .unwrap_or(DiscoverSource::Modrinth);

        // CurseForge 没有游戏版本时 API 会忽略 modLoaderType，UI 也会禁用，
        // 这里再兜底一次。
        if source == DiscoverSource::CurseForge
            && self
                .game_version
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
                .is_none()
        {
            return;
        }

        let choice = self
            .loader_options
            .lock()
            .ok()
            .and_then(|options| options.iter().find(|option| option.label == label).cloned());
        let Some(choice) = choice else {
            return;
        };

        let next = (!choice.key.is_empty()).then_some(choice);
        let changed = {
            let Ok(mut current) = self.loader_filter.lock() else {
                return;
            };
            let changed = current.as_ref().map(|choice| &choice.key)
                != next.as_ref().map(|choice| &choice.key);
            *current = next;
            changed
        };
        if changed {
            self.reload_projects();
        }
    }

    /// 选择分类筛选；“全部分类”表示不筛选。
    pub fn choose_category_filter(&self, label: &str) {
        let choice = self
            .category_options
            .lock()
            .ok()
            .and_then(|options| options.iter().find(|option| option.label == label).cloned());
        let Some(choice) = choice else {
            return;
        };

        let next = (!choice.key.is_empty()).then_some(choice);
        let changed = {
            let Ok(mut current) = self.category_filter.lock() else {
                return;
            };
            let changed = current.as_ref().map(|choice| &choice.key)
                != next.as_ref().map(|choice| &choice.key);
            *current = next;
            changed
        };
        if changed {
            self.reload_projects();
        }
    }

    /// 选择排序方式；每个下载源有自己的一组选项。
    pub fn choose_sort(&self, label: &str) {
        let choice = self
            .sort_options
            .lock()
            .ok()
            .and_then(|options| options.iter().find(|option| option.label == label).cloned());
        let Some(choice) = choice else {
            return;
        };

        let changed = {
            let Ok(mut current) = self.sort_filter.lock() else {
                return;
            };
            let changed = current.key != choice.key;
            *current = choice;
            changed
        };
        if changed {
            self.reload_projects();
        }
    }

    /// 切换下载源后重建排序选项，并回到该源的默认排序。
    fn refresh_sort_options(&self, source: DiscoverSource) {
        let options = sort_choices(source);
        let default = options
            .first()
            .cloned()
            .unwrap_or_else(|| all_choice("相关度"));
        let labels = labels(&options);

        if let Ok(mut current) = self.sort_options.lock() {
            *current = options;
        }
        if let Ok(mut current) = self.sort_filter.lock() {
            *current = default;
        }

        self.on_ui(move |ui| {
            ui.global::<AppState>()
                .set_sort_options(ModelRc::new(VecModel::from(labels)));
        });
    }

    /// 切换类别 / 下载源后重建筛选选项：加载器是静态表，分类走 API。
    fn refresh_filter_options(&self) {
        if let Ok(mut filter) = self.loader_filter.lock() {
            *filter = None;
        }
        if let Ok(mut filter) = self.category_filter.lock() {
            *filter = None;
        }

        let source = self
            .source
            .lock()
            .map(|guard| *guard)
            .unwrap_or(DiscoverSource::Modrinth);
        let kind = self.content.lock().ok().and_then(|guard| *guard);

        let loader_options = kind
            .map(|kind| loader_choices(source, kind))
            .unwrap_or_else(|| vec![all_choice(ALL_LOADERS)]);
        let loader_labels = labels(&loader_options);
        if let Ok(mut options) = self.loader_options.lock() {
            *options = loader_options;
        }

        let category_options = vec![all_choice(ALL_CATEGORIES)];
        let category_labels = labels(&category_options);
        if let Ok(mut options) = self.category_options.lock() {
            *options = category_options;
        }

        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_loader_options(ModelRc::new(VecModel::from(loader_labels)));
            state.set_category_options(ModelRc::new(VecModel::from(category_labels)));
        });

        let Some(kind) = kind else {
            return;
        };

        let generation = self.filters_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let this = self.clone();
        std::thread::spawn(move || {
            let downloader = Downloader::new(Progress::silent());
            let result = match source {
                DiscoverSource::Modrinth => modrinth::categories(&downloader)
                    .map(|categories| {
                        categories
                            .into_iter()
                            .filter(|category| category.project_type == kind.project_type())
                            .map(|category| FilterChoice {
                                label: category_label(DiscoverSource::Modrinth, &category.name),
                                key: category.name,
                            })
                            .collect::<Vec<_>>()
                    })
                    .map_err(|err| err.to_string()),
                DiscoverSource::CurseForge => curseforge_api_key().and_then(|api_key| {
                    curseforge::categories(&downloader, kind, &api_key)
                        .map(|categories| {
                            categories
                                .into_iter()
                                .filter(|category| {
                                    !category.is_class.unwrap_or(false) && !category.name.is_empty()
                                })
                                .map(|category| FilterChoice {
                                    label: category_label(
                                        DiscoverSource::CurseForge,
                                        &category.name,
                                    ),
                                    key: category.id.to_string(),
                                })
                                .collect::<Vec<_>>()
                        })
                        .map_err(|err| err.to_string())
                }),
            };

            let categories = match result {
                Ok(mut categories) => {
                    categories.sort_by(|left, right| left.label.cmp(&right.label));
                    categories
                }
                Err(err) => {
                    eprintln!("获取分类列表失败：{err}");
                    Vec::new()
                }
            };

            let mut options = vec![all_choice(ALL_CATEGORIES)];
            options.extend(categories);
            let labels = labels(&options);

            let guard = this.clone();
            this.on_ui(move |ui| {
                if guard.filters_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if let Ok(mut current) = guard.category_options.lock() {
                    *current = options;
                }
                ui.global::<AppState>()
                    .set_category_options(ModelRc::new(VecModel::from(labels)));
            });
        });
    }

    /// 按当前类别 / 下载源 / 游戏版本重新拉取列表。
    fn reload_projects(&self) {
        let Some(kind) = self.content.lock().ok().and_then(|guard| *guard) else {
            return;
        };
        self.load_projects(kind);
    }

    /// 从当前下载源拉取某个类别的项目列表。
    fn load_projects(&self, kind: ContentKind) {
        let source = self
            .source
            .lock()
            .map(|guard| *guard)
            .unwrap_or(DiscoverSource::Modrinth);
        let game_version = self
            .game_version
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let loader = self
            .loader_filter
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let category = self
            .category_filter
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let sort = self
            .sort_filter
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| default_sort(source));
        let query = self
            .query
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let generation = self.projects_generation.fetch_add(1, Ordering::SeqCst) + 1;

        let query_for_ui = query.clone();
        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_projects_loading(true);
            state.set_projects_summary("".into());
            state.set_discover_query(query_for_ui.into());
            // 先清空，避免短暂看到上一个类别的数据
            state.set_projects(ModelRc::new(VecModel::from(Vec::<ProjectInfo>::new())));
        });

        let this = self.clone();
        std::thread::spawn(move || {
            let downloader = Downloader::new(Progress::silent());
            // CurseForge 的 modLoaderType 必须配合 gameVersion；没有版本时不要传。
            let loader_key = if source == DiscoverSource::CurseForge && game_version.is_none() {
                None
            } else {
                loader.as_ref().map(|choice| choice.key.as_str())
            };
            let category_key = category.as_ref().map(|choice| choice.key.as_str());
            let query = query.trim();
            let result = search_projects(
                &downloader,
                source,
                kind,
                (!query.is_empty()).then_some(query),
                game_version.as_deref(),
                loader_key,
                category_key,
                &sort.key,
                50,
            );

            match result {
                Ok(response) => {
                    let icon_bytes = load_project_icon_bytes(source, &response.hits);
                    let hits = response.hits;

                    let mut filters = Vec::new();
                    if !query.is_empty() {
                        filters.push(format!("搜索「{query}」"));
                    }
                    if let Some(category) = &category {
                        filters.push(category.label.clone());
                    }
                    if let Some(loader) = &loader {
                        filters.push(loader.label.clone());
                    }
                    if let Some(version) = &game_version {
                        filters.push(version.clone());
                    }
                    let suffix = if filters.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", filters.join(" · "))
                    };
                    let summary = format!(
                        "{} 共 {} 个{}{suffix}",
                        source.name(),
                        short_number(response.total),
                        content_label(kind)
                    );

                    // 解码放在后台线程（`image` 在 debug 下很慢）；`slint::Image`
                    // 不是 Send，所以只在 UI 线程把 RGBA 拷成图片。
                    let icons = icon_bytes
                        .into_iter()
                        .map(|bytes| bytes.and_then(|bytes| crate::image_util::decode_rgba(&bytes)))
                        .collect::<Vec<_>>();

                    let guard = this.clone();
                    this.on_ui(move |ui| {
                        if guard.projects_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }

                        let items: Vec<ProjectInfo> = hits
                            .iter()
                            .zip(icons)
                            .map(|(hit, icon)| {
                                let icon = icon.map(|image| crate::image_util::from_rgba(&image));
                                to_project_info(hit, icon)
                            })
                            .collect();

                        let state = ui.global::<AppState>();
                        state.set_projects(ModelRc::new(VecModel::from(items)));
                        state.set_projects_summary(summary.into());
                        state.set_projects_loading(false);
                        state.set_projects_error("".into());
                        state.set_discover_source(source.key().into());
                    });
                }
                Err(err) => {
                    let guard = this.clone();
                    this.on_ui(move |ui| {
                        if guard.projects_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }

                        let state = ui.global::<AppState>();
                        state.set_projects_loading(false);
                        state.set_projects_error(
                            format!("获取{}失败：{err}", content_label(kind)).into(),
                        );
                    });
                }
            }
        });
    }

    /// 打开项目详情，加载该项目的所有版本和模组加载器。
    pub fn open_project(&self, project_id: &str) {
        let Some(kind) = self.content.lock().ok().and_then(|guard| *guard) else {
            return;
        };
        let Some(ui) = self.upgrade() else {
            return;
        };

        let project = {
            let state = ui.global::<AppState>();
            let projects = state.get_projects();
            projects
                .iter()
                .find(|project| project.project_id.as_str() == project_id)
                .map(|project| {
                    (
                        project.title,
                        project.initial,
                        project.subtitle,
                        project.description,
                        project.icon,
                        project.has_icon,
                        project.tags,
                    )
                })
        };
        let Some((title, initial, subtitle, description, icon, has_icon, tags)) = project else {
            return;
        };

        let source = self
            .source
            .lock()
            .map(|guard| *guard)
            .unwrap_or(DiscoverSource::Modrinth);
        let project_id = project_id.to_string();
        if let Ok(mut current) = self.project_ref.lock() {
            *current = Some(ProjectRef {
                id: project_id.clone(),
                source,
            });
        }
        let generation = self.project_generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut versions) = self.versions.lock() {
            versions.clear();
        }

        // 只有从实例下载弹窗打开时才带推荐上下文；普通发现页不显示推荐版本。
        let context = self
            .download_target
            .lock()
            .map(|target| target.is_some())
            .unwrap_or(false)
            .then(|| {
                let state = ui.global::<AppState>();
                DownloadContext {
                    game_version: state.get_instance_game_version().to_string(),
                    loader: state.get_instance_loader_name().to_string(),
                }
            });
        let game_rank = self
            .manifest
            .lock()
            .ok()
            .and_then(|manifest| {
                manifest.as_ref().map(|manifest| {
                    manifest
                        .versions
                        .iter()
                        .enumerate()
                        .map(|(index, version)| (version.id.clone(), index))
                        .collect::<HashMap<_, _>>()
                })
            })
            .unwrap_or_default();

        {
            let state = ui.global::<AppState>();
            state.set_project_open(true);
            state.set_project_title(title);
            state.set_project_initial(initial);
            state.set_project_subtitle(subtitle);
            state.set_project_description(description);
            state.set_project_icon(icon);
            state.set_project_has_icon(has_icon);
            state.set_project_tags(tags);
            reset_project_results(&state);
            state.set_project_versions_loading(true);
            state.set_download_message("".into());
        }

        let this = self.clone();
        std::thread::spawn(move || {
            let downloader = Downloader::new(Progress::silent());
            let result = load_versions(
                &downloader,
                source,
                kind,
                &project_id,
                context.as_ref(),
                &game_rank,
            );

            match result {
                Ok((groups, choices, recommended)) => {
                    let guard = this.clone();
                    this.on_ui(move |ui| {
                        if guard.project_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }

                        if let Ok(mut cache) = guard.versions.lock() {
                            *cache = choices;
                        }

                        let state = ui.global::<AppState>();
                        state.set_project_version_groups(version_group_model(groups));
                        let recommended = recommended.map(entry_to_info);
                        state.set_has_recommended_version(recommended.is_some());
                        if let Some(recommended) = recommended {
                            state.set_recommended_version(recommended);
                        }
                        state.set_project_versions_loading(false);
                        state.set_project_versions_error("".into());
                    });
                }
                Err(err) => {
                    let guard = this.clone();
                    this.on_ui(move |ui| {
                        if guard.project_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }

                        let state = ui.global::<AppState>();
                        state.set_has_recommended_version(false);
                        state.set_project_versions_loading(false);
                        state.set_project_versions_error(format!("获取版本列表失败：{err}").into());
                    });
                }
            }
        });
    }

    /// 关闭项目详情，回到项目列表。
    pub fn close_project(&self) {
        self.project_generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut versions) = self.versions.lock() {
            versions.clear();
        }
        if let Ok(mut current) = self.project_ref.lock() {
            *current = None;
        }

        self.on_ui(|ui| {
            let state = ui.global::<AppState>();
            state.set_project_open(false);
            state.set_project_title("".into());
            state.set_project_subtitle("".into());
            state.set_project_description("".into());
            state.set_project_initial("".into());
            state.set_project_icon(slint::Image::default());
            state.set_project_has_icon(false);
            state.set_project_tags(ModelRc::new(VecModel::from(
                Vec::<slint::SharedString>::new(),
            )));
            reset_project_results(&state);
        });
    }

    /// 设置下载目标实例；空字符串表示恢复下载到默认 `.minecraft`。
    pub fn set_download_target(&self, path: &str) {
        let path = path.trim();
        if let Ok(mut target) = self.download_target.lock() {
            *target = if path.is_empty() {
                None
            } else {
                Some(PathBuf::from(path))
            };
        }
    }

    /// 下载项目详情里选中的版本。
    pub fn download_version(&self, key: &str) {
        let Some(kind) = self.content.lock().ok().and_then(|guard| *guard) else {
            return;
        };

        if !kind.supports_install() {
            self.on_ui(|ui| {
                let state = ui.global::<AppState>();
                state.set_download_ok(false);
                state.set_download_message("整合包一键安装还没做，暂时只能浏览".into());
            });
            return;
        }

        // 同一时间只允许一个下载任务
        if self
            .with_ui(|ui| ui.global::<AppState>().get_downloading_version())
            .is_some_and(|key| !key.is_empty())
        {
            return;
        }

        let choice = self
            .versions
            .lock()
            .ok()
            .and_then(|cache| cache.iter().find(|choice| choice.key == key).cloned());
        let Some(choice) = choice else {
            return;
        };

        let target = self
            .download_target
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let in_instance = target.is_some();
        let tracked = target.as_ref().and_then(|_| {
            self.project_ref
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
                .map(|project| instances::TrackedContent {
                    source: project.source.key().to_string(),
                    project_id: project.id,
                    version_id: choice.version_id.clone(),
                    version: choice.version_number.clone(),
                    file_name: choice.file_name.clone(),
                })
        });

        let key = key.to_string();
        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_downloading_version(key.into());
            state.set_download_message("".into());
        });

        let this = self.clone();
        std::thread::spawn(move || {
            let (ok, message) = match download_choice(&choice, kind, target.as_deref()) {
                Ok(dest) => {
                    if let (Some(instance), Some(tracked)) = (target.as_deref(), tracked) {
                        let mut metadata = instances::read_content_metadata(instance);
                        metadata.track(kind.target_dir(), &choice.file_name, tracked);
                        if let Err(err) = instances::write_content_metadata(instance, &metadata) {
                            eprintln!("记录内容来源失败：{err}");
                        }
                    }

                    let target_dir = kind.target_dir();
                    let location = if in_instance {
                        format!("当前实例的 {target_dir} 目录")
                    } else {
                        target_dir.to_string()
                    };
                    (
                        true,
                        format!(
                            "已下载 {} 到 {location}",
                            dest.file_name().unwrap_or_default().to_string_lossy(),
                        ),
                    )
                }
                Err(err) => (false, err),
            };

            this.on_ui(move |ui| {
                let state = ui.global::<AppState>();
                state.set_downloading_version("".into());
                state.set_download_ok(ok);
                state.set_download_message(message.into());
            });
        });
    }

    /// 检查通过启动器下载到实例的内容是否有新版本。
    pub fn check_content_updates(&self, instance: &str, folder: &str, kind_key: &str) {
        let Some(kind) = ContentKind::parse(kind_key) else {
            return;
        };
        if !kind.supports_install() {
            return;
        }

        let instance = PathBuf::from(instance);
        if instance.as_os_str().is_empty() {
            return;
        }
        let folder = folder.to_string();

        let generation = self.update_generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut candidates) = self.update_candidates.lock() {
            candidates.clear();
        }

        self.on_ui(|ui| {
            let state = ui.global::<AppState>();
            state.set_content_updates(ModelRc::new(
                VecModel::from(Vec::<ContentUpdateInfo>::new()),
            ));
            state.set_update_checking(true);
            state.set_update_summary("正在检查…".into());
            state.set_update_error("".into());
            state.set_update_message("".into());
            state.set_update_ok(false);
            state.set_updating_content_key("".into());
        });

        let this = self.clone();
        std::thread::spawn(move || {
            let loader_info = instances::detect_instance_loader(&instance.to_string_lossy());
            let metadata = instances::read_content_metadata(&instance);
            let tracked = metadata.entries.get(&folder).cloned().unwrap_or_default();

            if tracked.is_empty() {
                this.finish_update_check(
                    generation,
                    0,
                    0,
                    String::new(),
                    "没有记录到下载来源；只有通过启动器下载的内容才能检查更新。",
                );
                return;
            }

            let downloader = Downloader::new(Progress::silent());
            let directory = instance.join(&folder);
            let total = tracked.len();
            let mut found = 0usize;
            let mut failed = 0usize;
            let mut first_error = String::new();

            for (index, (old_name, item)) in tracked.into_iter().enumerate() {
                if this.is_update_stale(generation) {
                    return;
                }
                this.publish_update_progress(generation, index + 1, total);

                let disabled = !directory.join(&old_name).exists()
                    && directory.join(format!("{old_name}.disabled")).exists();
                if !disabled && !directory.join(&old_name).exists() {
                    continue;
                }

                let source = DiscoverSource::parse(&item.source);
                let result = match source {
                    DiscoverSource::Modrinth => find_modrinth_update(
                        &downloader,
                        &item.project_id,
                        &loader_info.game_version,
                        &loader_info.loader,
                        kind,
                    ),
                    DiscoverSource::CurseForge => find_curseforge_update(
                        &downloader,
                        &item.project_id,
                        &loader_info.game_version,
                        &loader_info.loader,
                        kind,
                    ),
                };

                match result {
                    Ok(Some(new_version)) if new_version.version_id != item.version_id => {
                        let candidate = UpdateCandidate {
                            key: format!("{folder}|{old_name}"),
                            instance: instance.clone(),
                            folder: folder.clone(),
                            old_name,
                            current_version: if item.version.is_empty() {
                                item.version_id.clone()
                            } else {
                                item.version.clone()
                            },
                            source,
                            project_id: item.project_id,
                            new_version,
                        };
                        if let Ok(mut candidates) = this.update_candidates.lock() {
                            candidates.push(candidate);
                        }
                        found += 1;
                        let items = this
                            .update_candidates
                            .lock()
                            .map(|candidates| {
                                candidates.iter().map(update_info).collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        this.publish_update_items(generation, items, found);
                    }
                    Ok(_) => {}
                    Err(err) => {
                        failed += 1;
                        if first_error.is_empty() {
                            first_error = err;
                        }
                    }
                }
            }

            this.finish_update_check(generation, found, failed, first_error, "没有可更新的内容。");
        });
    }

    /// 下载并替换一条可更新内容。
    pub fn apply_content_update(&self, key: &str) {
        if self.updating_content.swap(true, Ordering::SeqCst) {
            return;
        }

        let candidate = self
            .update_candidates
            .lock()
            .ok()
            .and_then(|candidates| candidates.iter().find(|item| item.key == key).cloned());
        let Some(candidate) = candidate else {
            self.updating_content.store(false, Ordering::SeqCst);
            return;
        };

        let key = key.to_string();
        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_updating_content_key(key.into());
            state.set_update_message("".into());
            state.set_update_ok(false);
        });

        let this = self.clone();
        std::thread::spawn(move || {
            let (ok, message) = match replace_content(&candidate) {
                Ok(_) => (
                    true,
                    format!(
                        "已更新 {} → {}",
                        candidate.old_name, candidate.new_version.version
                    ),
                ),
                Err(err) => (false, err),
            };

            let items = if ok {
                if let Ok(mut candidates) = this.update_candidates.lock() {
                    candidates.retain(|item| item.key != candidate.key);
                }
                Some(
                    this.update_candidates
                        .lock()
                        .map(|candidates| candidates.iter().map(update_info).collect::<Vec<_>>())
                        .unwrap_or_default(),
                )
            } else {
                None
            };
            let remaining = items.as_ref().map_or(0, Vec::len);
            this.updating_content.store(false, Ordering::SeqCst);

            this.on_ui(move |ui| {
                let state = ui.global::<AppState>();
                state.set_updating_content_key("".into());
                state.set_update_ok(ok);
                state.set_update_message(message.into());
                if let Some(items) = items {
                    state.set_content_updates(ModelRc::new(VecModel::from(items)));
                    state.set_update_summary(
                        if remaining == 0 {
                            "全部内容已是最新".to_string()
                        } else {
                            format!("还有 {remaining} 个可更新项")
                        }
                        .into(),
                    );
                }
            });
        });
    }

    fn is_update_stale(&self, generation: u64) -> bool {
        self.update_generation.load(Ordering::SeqCst) != generation
    }

    fn publish_update_progress(&self, generation: u64, checked: usize, total: usize) {
        let this = self.clone();
        let summary = format!("正在检查 {checked}/{total}…");
        self.on_ui(move |ui| {
            if this.is_update_stale(generation) {
                return;
            }
            ui.global::<AppState>().set_update_summary(summary.into());
        });
    }

    fn publish_update_items(&self, generation: u64, items: Vec<ContentUpdateInfo>, found: usize) {
        let this = self.clone();
        let summary = format!("发现 {found} 个可更新项");
        self.on_ui(move |ui| {
            if this.is_update_stale(generation) {
                return;
            }
            let state = ui.global::<AppState>();
            state.set_content_updates(ModelRc::new(VecModel::from(items)));
            state.set_update_summary(summary.into());
        });
    }

    fn finish_update_check(
        &self,
        generation: u64,
        found: usize,
        failed: usize,
        first_error: String,
        empty_message: &'static str,
    ) {
        let this = self.clone();
        self.on_ui(move |ui| {
            if this.is_update_stale(generation) {
                return;
            }
            let state = ui.global::<AppState>();
            state.set_update_checking(false);
            state.set_update_summary(
                if found > 0 {
                    format!("发现 {found} 个可更新项")
                } else {
                    empty_message.to_string()
                }
                .into(),
            );
            if failed > 0 {
                state.set_update_error(format!("{failed} 项检查失败：{first_error}").into());
            } else {
                state.set_update_error("".into());
            }
        });
    }

    /// 重置“添加实例”向导：先回到创建方式选择。
    pub fn reset_add_instance(&self) {
        self.on_ui(|ui| {
            let state = ui.global::<AppState>();
            state.set_add_step("source".into());
            state.set_pending_version("".into());
            state.set_pending_loader("".into());
            state.set_pending_loader_version("".into());
            state.set_instance_name("".into());
            state.set_loader_versions(ModelRc::new(VecModel::from(Vec::<LoaderInfo>::new())));
            state.set_loader_versions_loading(false);
            state.set_loader_versions_error("".into());
            state.set_install_message("".into());
        });
    }

    /// 从创建方式选择进入“安装新实例”的版本列表。
    pub fn start_new_instance(&self) {
        self.on_ui(|ui| {
            let state = ui.global::<AppState>();
            state.set_add_step("versions".into());
            state.set_install_message("".into());
        });
    }

    /// 选中一个原版版本，进入加载器选择。
    pub fn choose_version(&self, version: &str) {
        let version = version.to_string();
        let name = loader::default_instance_name(&version, None);

        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_add_step("loader".into());
            state.set_pending_version(version.into());
            state.set_pending_loader("".into());
            state.set_pending_loader_version("".into());
            state.set_instance_name(name.into());
            state.set_install_message("".into());
        });
    }

    /// 选择加载器；空字符串表示跳过。
    pub fn choose_loader(&self, key: &str) {
        let Some(kind) = LoaderKind::parse(key) else {
            let version = self
                .with_ui(|ui| ui.global::<AppState>().get_pending_version())
                .unwrap_or_default();
            let name = loader::default_instance_name(&version, None);

            self.on_ui(move |ui| {
                let state = ui.global::<AppState>();
                state.set_pending_loader("".into());
                state.set_pending_loader_version("".into());
                state.set_instance_name(name.into());
                state.set_add_step("name".into());
                state.set_install_message("".into());
            });
            return;
        };

        let version = self
            .with_ui(|ui| ui.global::<AppState>().get_pending_version())
            .unwrap_or_default();
        let default_name = loader::default_instance_name(&version, Some(kind));

        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_pending_loader(kind.key().into());
            state.set_pending_loader_version("".into());
            state.set_instance_name(default_name.into());
            state.set_add_step("loader-version".into());
            state.set_loader_versions_loading(true);
            state.set_loader_versions_error("".into());
            state.set_loader_versions(ModelRc::new(VecModel::from(Vec::<LoaderInfo>::new())));
            state.set_install_message("".into());
        });

        let this = self.clone();
        std::thread::spawn(move || {
            let downloader = Downloader::new(Progress::silent());
            let result = loader::loader_versions(&downloader, kind, &version);

            this.on_ui(move |ui| {
                let state = ui.global::<AppState>();
                // 用户可能已经返回并换了加载器/版本，丢弃过期结果。
                if state.get_pending_loader() != kind.key()
                    || state.get_pending_version() != version
                {
                    return;
                }
                state.set_loader_versions_loading(false);
                match result {
                    Ok(versions) => {
                        let items = loader_version_items(kind, versions);
                        state.set_loader_versions(ModelRc::new(VecModel::from(items)));
                    }
                    Err(err) => {
                        state.set_loader_versions_error(
                            format!("获取 {} 版本失败：{err}", kind.name()).into(),
                        );
                    }
                }
            });
        });
    }

    /// 选中加载器版本，进入实例命名。
    pub fn choose_loader_version(&self, version: &str) {
        let version = version.to_string();
        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_pending_loader_version(version.into());
            state.set_add_step("name".into());
            state.set_install_message("".into());
        });
    }

    /// 实例详情：读取当前实例加载器可切换的版本。
    pub fn refresh_instance_loader_versions(&self, game_version: &str, loader_key: &str) {
        let Some(kind) = LoaderKind::parse(loader_key) else {
            self.on_ui(|ui| {
                let state = ui.global::<AppState>();
                state.set_loader_versions_loading(false);
                state.set_loader_versions_error("当前实例的加载器暂不支持版本切换".into());
                state.set_loader_versions(ModelRc::new(VecModel::from(Vec::<LoaderInfo>::new())));
            });
            return;
        };

        if game_version.trim().is_empty() {
            self.on_ui(|ui| {
                let state = ui.global::<AppState>();
                state.set_loader_versions_loading(false);
                state.set_loader_versions_error(
                    "无法确定这个实例的 Minecraft 版本，不能切换加载器".into(),
                );
                state.set_loader_versions(ModelRc::new(VecModel::from(Vec::<LoaderInfo>::new())));
            });
            return;
        }

        let generation = self
            .instance_loader_versions_generation
            .fetch_add(1, Ordering::SeqCst)
            + 1;

        self.on_ui(|ui| {
            let state = ui.global::<AppState>();
            state.set_loader_versions_loading(true);
            state.set_loader_versions_error("".into());
            state.set_loader_versions(ModelRc::new(VecModel::from(Vec::<LoaderInfo>::new())));
            state.set_loader_switch_message("".into());
            state.set_loader_switch_ok(false);
        });

        let this = self.clone();
        let game_version = game_version.to_string();
        std::thread::spawn(move || {
            let downloader = Downloader::new(Progress::silent());
            let result = loader::loader_versions(&downloader, kind, &game_version);

            let versions_generation = this.instance_loader_versions_generation.clone();
            this.on_ui(move |ui| {
                if versions_generation.load(Ordering::SeqCst) != generation {
                    return;
                }

                let state = ui.global::<AppState>();
                state.set_loader_versions_loading(false);
                match result {
                    Ok(versions) => {
                        let items = loader_version_items(kind, versions);
                        state.set_loader_versions(ModelRc::new(VecModel::from(items)));
                    }
                    Err(err) => {
                        state.set_loader_versions_error(
                            format!("获取 {} 版本失败：{err}", kind.name()).into(),
                        );
                    }
                }
            });
        });
    }

    /// 实例详情：把现有实例切换到指定的加载器版本。
    pub fn switch_instance_loader(
        &self,
        instance: &str,
        game_version: &str,
        loader_key: &str,
        loader_version: &str,
    ) {
        let Some(kind) = LoaderKind::parse(loader_key) else {
            return;
        };
        if instance.trim().is_empty()
            || game_version.trim().is_empty()
            || loader_version.trim().is_empty()
        {
            return;
        }

        let Some(ui) = self.upgrade() else {
            return;
        };
        let state = ui.global::<AppState>();
        if state.get_installing() || state.get_switching_loader() {
            return;
        }

        state.set_installing(true);
        state.set_switching_loader(true);
        state.set_installing_version(instance.into());
        state.set_install_progress(0.0);
        state.set_install_progress_text("0%".into());
        state.set_install_stage("准备中…".into());
        state.set_loader_switch_message("".into());
        state.set_loader_switch_ok(false);
        drop(state);
        drop(ui);

        let this = self.clone();
        let instance = instance.to_string();
        let loader_version = loader_version.to_string();
        let mut options = InstanceInstallOptions::new(
            default_game_dir(),
            instance.clone(),
            game_version.to_string(),
        );
        options.loader = Some(kind);
        options.loader_version = Some(loader_version.clone());

        std::thread::spawn(move || {
            let progress = install_progress(&this);

            let outcome = install_instance(&options, &progress);
            this.finish_loader_switch(&instance, kind, &loader_version, outcome);
        });
    }

    fn finish_loader_switch(
        &self,
        instance: &str,
        kind: LoaderKind,
        loader_version: &str,
        outcome: Result<InstallReport, impl std::fmt::Display>,
    ) {
        let (ok, message) = match &outcome {
            Ok(_) => (
                true,
                format!(
                    "已将实例「{instance}」切换到 {} {loader_version}",
                    kind.name()
                ),
            ),
            Err(err) => (false, format!("切换加载器失败：{err}")),
        };

        let instance_items = to_instance_infos(&default_game_dir().display().to_string());
        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_installing(false);
            state.set_switching_loader(false);
            state.set_install_stage("".into());
            state.set_install_progress(if ok { 1.0 } else { 0.0 });
            state.set_install_progress_text(if ok { "100%" } else { "" }.into());
            state.set_loader_switch_ok(ok);
            state.set_loader_switch_message(message.into());
            state.set_instances(ModelRc::new(VecModel::from(instance_items)));
        });
    }

    /// 向导返回上一步。
    pub fn go_back(&self) {
        let Some((step, loader)) = self.with_ui(|ui| {
            let state = ui.global::<AppState>();
            (
                state.get_add_step().to_string(),
                state.get_pending_loader().to_string(),
            )
        }) else {
            return;
        };

        let next = match step.as_str() {
            "name" if loader.is_empty() => "loader",
            "name" => "loader-version",
            "loader-version" => "loader",
            "loader" => "versions",
            _ => "source",
        };

        self.on_ui(move |ui| ui.global::<AppState>().set_add_step(next.into()));
    }

    /// 按当前向导状态创建实例。
    pub fn create_instance(&self, instance_name: &str) {
        let Some(ui) = self.upgrade() else {
            return;
        };

        let state = ui.global::<AppState>();
        if state.get_installing() {
            return;
        }

        let version = state.get_pending_version().to_string();
        let loader_key = state.get_pending_loader().to_string();
        let loader_version = state.get_pending_loader_version().to_string();
        let instance = instance_name.trim().to_string();
        let loader_kind = LoaderKind::parse(&loader_key);

        if let Some(message) =
            validate_instance(&version, &instance, loader_kind.is_some(), &loader_version)
        {
            state.set_install_ok(false);
            state.set_install_message(message.into());
            return;
        }

        state.set_installing(true);
        state.set_installing_version(version.clone().into());
        state.set_install_progress(0.0);
        state.set_install_progress_text("0%".into());
        state.set_install_stage("准备中…".into());
        state.set_install_message("".into());
        drop(state);
        drop(ui);

        let this = self.clone();
        let mut options =
            InstanceInstallOptions::new(default_game_dir(), instance.clone(), version.clone());
        options.loader = loader_kind;
        options.loader_version = loader_kind.map(|_| loader_version);

        std::thread::spawn(move || {
            let progress = install_progress(&this);

            let outcome = install_instance(&options, &progress);
            this.finish_instance_install(&instance, outcome);
        });
    }

    fn finish_instance_install(
        &self,
        instance: &str,
        outcome: Result<InstallReport, impl std::fmt::Display>,
    ) {
        let (ok, message) = match &outcome {
            Ok(report) => (
                true,
                format!(
                    "实例「{instance}」创建完成：{} 个依赖库，{} 个资源文件，用时 {:.1}s",
                    report.library_count,
                    report.asset_count,
                    report.elapsed.as_secs_f64()
                ),
            ),
            Err(err) => (false, format!("实例「{instance}」创建失败：{err}")),
        };

        // 安装会往 versions/ 里写目录，顺手把实例列表刷新掉
        let game_dir = default_game_dir();
        let instance_items = to_instance_infos(&game_dir.display().to_string());
        let created_name = instance.to_string();

        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_installing(false);
            state.set_install_stage("".into());
            state.set_install_message(message.into());
            state.set_install_ok(ok);

            if ok {
                state.set_install_progress(1.0);
                state.set_install_progress_text("100%".into());
                state.set_add_step("done".into());
                state.set_instance_name(created_name.into());
            } else {
                state.set_install_progress(0.0);
                state.set_install_progress_text("".into());
            }

            state.set_instances(ModelRc::new(VecModel::from(instance_items)));
        });
    }

    fn rebuild_from_cache(&self) {
        let filter = self
            .filter
            .lock()
            .map(|guard| *guard)
            .unwrap_or(Filter::Release);
        let Some(manifest) = self.manifest.lock().ok().and_then(|guard| guard.clone()) else {
            return;
        };

        let items = build_version_items(&manifest, filter);
        let summary = format!("共 {} 个{}", items.len(), filter.label());
        let game_versions = game_version_choices(&manifest);
        let latest_release = manifest.latest.release;
        let latest_snapshot = manifest.latest.snapshot;

        self.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_versions(ModelRc::new(VecModel::from(items)));
            state.set_versions_summary(summary.into());
            state.set_latest_release(latest_release.into());
            state.set_latest_snapshot(latest_snapshot.into());
            let game_versions = game_versions
                .into_iter()
                .map(Into::into)
                .collect::<Vec<slint::SharedString>>();
            state.set_game_versions(ModelRc::new(VecModel::from(game_versions)));
            state.set_versions_loading(false);
            state.set_versions_error("".into());
        });
    }

    fn loading(&self) -> bool {
        self.with_ui(|ui| ui.global::<AppState>().get_versions_loading())
            .unwrap_or(true)
    }

    fn upgrade(&self) -> Option<AppWindow> {
        self.ui.lock().ok().and_then(|guard| guard.upgrade())
    }

    fn with_ui<R>(&self, f: impl FnOnce(&AppWindow) -> R) -> Option<R> {
        let ui = self.upgrade()?;
        Some(f(&ui))
    }

    /// 把一段操作投递到 UI 线程执行。
    fn on_ui(&self, f: impl FnOnce(AppWindow) + Send + 'static) {
        let weak = match self.ui.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let _ = weak.upgrade_in_event_loop(f);
    }
}

/// 把加载器版本列表转成 UI 行；新建实例和切换加载器共用。
fn loader_version_items(kind: LoaderKind, versions: Vec<loader::LoaderVersion>) -> Vec<LoaderInfo> {
    versions
        .into_iter()
        .map(|item| LoaderInfo {
            kind: kind.key().into(),
            name: kind.name().into(),
            version: item.version.into(),
            stable: item.stable,
        })
        .collect()
}

/// 安装 / 切换加载器共用的进度回调：把 core 的进度快照投递到 UI 线程。
fn install_progress(this: &InstallController) -> Progress {
    let this = this.clone();
    Progress::new(move |snapshot| {
        let ratio = snapshot.ratio() as f32;
        let text = format!("{:.0}%", ratio * 100.0);
        let stage = snapshot.stage.clone();
        this.on_ui(move |ui| {
            let state = ui.global::<AppState>();
            state.set_install_progress(ratio);
            state.set_install_progress_text(text.into());
            state.set_install_stage(stage.into());
        });
    })
}

/// 校验实例名；返回错误提示（`None` 表示通过）。
fn validate_instance(
    version: &str,
    instance: &str,
    has_loader: bool,
    loader_version: &str,
) -> Option<String> {
    if version.is_empty() {
        return Some("没有选择 Minecraft 版本".to_string());
    }
    if instance.is_empty() {
        return Some("实例名不能为空".to_string());
    }
    if instance.chars().count() > 64 {
        return Some("实例名不能超过 64 个字符".to_string());
    }
    if instance.chars().any(|ch| {
        matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || ch.is_control()
    }) {
        return Some("实例名不能包含 / \\ : * ? \" < > | 等字符".to_string());
    }
    if instance.starts_with('.') || instance.ends_with('.') || instance.ends_with(' ') {
        return Some("实例名不能以点开头/结尾，也不能以空格结尾".to_string());
    }

    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED
        .iter()
        .any(|name| instance.eq_ignore_ascii_case(name))
    {
        return Some("这个名称是 Windows 保留名".to_string());
    }

    if has_loader && instance == version {
        return Some("带加载器的实例名不能和原版版本号相同".to_string());
    }
    if has_loader && loader_version.is_empty() {
        return Some("没有选择加载器版本".to_string());
    }

    let json = GamePaths::new(default_game_dir()).version_json(instance);
    if json.is_file() {
        return Some(format!("实例「{instance}」已存在"));
    }

    None
}

fn build_version_items(manifest: &VersionManifest, filter: Filter) -> Vec<VersionInfo> {
    manifest
        .versions
        .iter()
        .filter(|version| filter.matches(version))
        .map(|version| {
            let is_fools = is_april_fools(&version.id);
            VersionInfo {
                version_id: version.id.clone().into(),
                kind: version.kind.clone().into(),
                kind_label: if is_fools {
                    "愚人节"
                } else {
                    kind_label(&version.kind)
                }
                .into(),
                is_fools,
                release_date: short_date(&version.release_time).into(),
            }
        })
        .collect()
}

/// 发现页的游戏版本下拉框：第一项“全部版本”，后面是正式版（新的在前）。
fn game_version_choices(manifest: &VersionManifest) -> Vec<String> {
    let mut choices = vec![ALL_GAME_VERSIONS.to_string()];
    choices.extend(
        manifest
            .versions
            .iter()
            .filter(|version| version.kind == "release")
            .map(|version| version.id.clone()),
    );
    choices
}

/// 清空项目详情的版本列表、推荐版本和下载状态；打开 / 关闭项目时共用。
fn reset_project_results(state: &AppState) {
    state.set_project_version_groups(ModelRc::new(VecModel::from(
        Vec::<ProjectVersionGroup>::new(),
    )));
    state.set_has_recommended_version(false);
    state.set_recommended_version(ProjectVersionInfo::default());
    state.set_project_versions_loading(false);
    state.set_project_versions_error("".into());
    state.set_downloading_version("".into());
}

fn to_instance_infos(game_dir: &str) -> Vec<InstanceInfo> {
    instances::detect_instances(game_dir)
        .into_iter()
        .map(|instance| InstanceInfo {
            name: instance.name.into(),
            path: instance.path.into(),
        })
        .collect()
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "release" => "正式版",
        "snapshot" => "快照",
        "old_beta" => "Beta",
        "old_alpha" => "Alpha",
        _ => "其他",
    }
}

/// `2026-09-15T11:23:02+00:00` -> `2026-09-15`
fn short_date(release_time: &str) -> String {
    release_time.chars().take(10).collect()
}

/// 项目详情里的一行可下载文件。
#[derive(Debug, Clone)]
struct VersionChoice {
    key: String,
    /// Modrinth version id / CurseForge file id
    version_id: String,
    /// 版本号，用于记录已下载内容的版本
    version_number: String,
    url: String,
    file_name: String,
    sha1: Option<String>,
    size: u64,
}

/// 从下载源查到的较新版本。
#[derive(Debug, Clone)]
struct NewVersion {
    version_id: String,
    version: String,
    url: String,
    file_name: String,
    sha1: Option<String>,
    size: u64,
}

/// 一条可更新的实例内容。
#[derive(Debug, Clone)]
struct UpdateCandidate {
    /// UI 里的唯一键：目录 + 旧文件名
    key: String,
    instance: PathBuf,
    folder: String,
    /// 启用时的文件名（不含 `.disabled`）
    old_name: String,
    current_version: String,
    source: DiscoverSource,
    project_id: String,
    new_version: NewVersion,
}

/// 项目详情里的一个版本行，以及它要出现的游戏版本分组。
///
/// 这里只放纯数据（所有字段都能跨线程），Slint 的 `ModelRc` 在 UI 线程再组装。
#[derive(Debug, Clone)]
struct VersionEntry {
    key: String,
    name: String,
    game_versions: String,
    /// 原始游戏版本列表，用于推荐版本匹配
    supported_versions: Vec<String>,
    loaders: Vec<String>,
    channel: String,
    /// 是否为正式版
    release: bool,
    file_name: String,
    size: String,
    downloadable: bool,
    groups: Vec<String>,
    /// 是否被选为当前实例的推荐版本
    recommended: bool,
}

/// 按游戏版本分组后的版本行（同样保持 `Send`）。
#[derive(Debug, Clone)]
struct VersionGroup {
    game_version: String,
    versions: Vec<VersionEntry>,
}

/// 加载项目的全部版本，转换成按游戏版本分组的数据 + 下载时用的文件信息。
/// 加载项目版本后的结果：分组、下载缓存、推荐版本。
type LoadedProjectVersions = (Vec<VersionGroup>, Vec<VersionChoice>, Option<VersionEntry>);

fn load_versions(
    downloader: &Downloader,
    source: DiscoverSource,
    kind: ContentKind,
    project_id: &str,
    context: Option<&DownloadContext>,
    game_rank: &HashMap<String, usize>,
) -> Result<LoadedProjectVersions, String> {
    let (mut entries, choices) = match source {
        DiscoverSource::Modrinth => load_modrinth_versions(downloader, kind, project_id)?,
        DiscoverSource::CurseForge => load_curseforge_versions(downloader, kind, project_id)?,
    };
    let recommended = mark_recommended(&mut entries, context, kind);
    Ok((
        group_project_versions(entries, game_rank),
        choices,
        recommended,
    ))
}

/// 在兼容当前实例的版本里挑最新正式版作为推荐；没有正式版时退回最新的兼容版本。
fn mark_recommended(
    entries: &mut [VersionEntry],
    context: Option<&DownloadContext>,
    kind: ContentKind,
) -> Option<VersionEntry> {
    let context = context?;
    if !kind.supports_install() {
        return None;
    }
    let mut fallback = None;

    for (index, entry) in entries.iter().enumerate() {
        if !context.game_version.is_empty()
            && !entry
                .supported_versions
                .iter()
                .any(|version| version.eq_ignore_ascii_case(&context.game_version))
        {
            continue;
        }
        if kind == ContentKind::Mod && !loader_matches(&entry.loaders, &context.loader) {
            continue;
        }

        if entry.release {
            entries[index].recommended = true;
            return Some(entries[index].clone());
        }
        fallback.get_or_insert(index);
    }

    let index = fallback?;
    entries[index].recommended = true;
    Some(entries[index].clone())
}

fn loader_matches(loaders: &[String], instance_loader: &str) -> bool {
    let instance_loader = instance_loader.trim();
    if instance_loader.is_empty() || instance_loader.eq_ignore_ascii_case("原版") {
        return true;
    }

    loaders.iter().any(|loader| {
        loader.eq_ignore_ascii_case(instance_loader)
            || (instance_loader.eq_ignore_ascii_case("Quilt")
                && loader.eq_ignore_ascii_case("Fabric"))
    })
}

fn load_modrinth_versions(
    downloader: &Downloader,
    kind: ContentKind,
    project_id: &str,
) -> Result<(Vec<VersionEntry>, Vec<VersionChoice>), String> {
    let versions = modrinth::project_versions(downloader, project_id)
        .map_err(|err| format!("获取版本信息失败：{err}"))?;

    let mut entries = Vec::new();
    let mut choices = Vec::new();

    for version in versions {
        let Some(file) = modrinth::main_file(&version) else {
            continue;
        };

        let key = format!("modrinth:{}", version.id);
        let name = if !version.name.trim().is_empty() {
            version.name.clone()
        } else if !version.version_number.trim().is_empty() {
            version.version_number.clone()
        } else {
            file.filename.clone()
        };

        let safe_name = safe_file_name(&file.filename);
        let downloadable = kind.supports_install() && !file.url.is_empty() && safe_name.is_some();
        if downloadable && let Some(safe_name) = safe_name {
            choices.push(VersionChoice {
                key: key.clone(),
                version_id: version.id.clone(),
                version_number: if version.version_number.trim().is_empty() {
                    name.clone()
                } else {
                    version.version_number.clone()
                },
                url: file.url.clone(),
                file_name: safe_name.to_string(),
                sha1: file.sha1().map(str::to_string),
                size: file.size,
            });
        }

        let game_versions = version
            .game_versions
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();

        entries.push(VersionEntry {
            key,
            name,
            game_versions: display_game_versions(&game_versions, kind),
            supported_versions: version.game_versions.clone(),
            loaders: loader_tags(&version.loaders, kind),
            channel: modrinth_channel(&version.version_type).to_string(),
            release: version.version_type.eq_ignore_ascii_case("release"),
            file_name: file.filename.clone(),
            size: format_size(file.size),
            downloadable,
            groups: version_group_keys(&version.game_versions, kind),
            recommended: false,
        });
    }

    Ok((entries, choices))
}

fn load_curseforge_versions(
    downloader: &Downloader,
    kind: ContentKind,
    project_id: &str,
) -> Result<(Vec<VersionEntry>, Vec<VersionChoice>), String> {
    let api_key = curseforge_api_key()?;
    let mut files = curseforge::mod_files(downloader, project_id, None, &api_key)
        .map_err(|err| format!("获取文件列表失败：{err}"))?;
    // CurseForge 的返回顺序不一定是发布时间，这里统一按 fileDate 倒序（ISO 8601 可直接比较）。
    files.sort_by(|left, right| right.file_date.cmp(&left.file_date));

    let mut entries = Vec::new();
    let mut choices = Vec::new();

    for file in files {
        let key = format!("curseforge:{}", file.id);
        let name = if !file.display_name.trim().is_empty() {
            file.display_name.clone()
        } else {
            file.file_name.clone()
        };

        let safe_name = safe_file_name(&file.file_name);
        let downloadable = kind.supports_install() && file.downloadable() && safe_name.is_some();
        if downloadable && let Some(safe_name) = safe_name {
            choices.push(VersionChoice {
                key: key.clone(),
                version_id: file.id.to_string(),
                version_number: if file.display_name.trim().is_empty() {
                    file.file_name.clone()
                } else {
                    file.display_name.clone()
                },
                url: file.download_url.clone().unwrap_or_default(),
                file_name: safe_name.to_string(),
                sha1: file.sha1().map(str::to_string),
                size: file.file_length,
            });
        }

        let raw_versions = file
            .game_versions
            .iter()
            .filter(|token| !is_loader(token) && !is_environment(token))
            .cloned()
            .collect::<Vec<_>>();

        entries.push(VersionEntry {
            key,
            name,
            game_versions: curseforge_game_versions(&file.game_versions, kind),
            supported_versions: raw_versions.clone(),
            loaders: loader_tags(&file.game_versions, kind),
            channel: curseforge_channel(file.release_type).to_string(),
            release: file.release_type == 1,
            file_name: file.file_name.clone(),
            size: format_size(file.file_length),
            downloadable,
            groups: version_group_keys(&raw_versions, kind),
            recommended: false,
        });
    }

    Ok((entries, choices))
}

/// 把版本行按游戏版本分组；支持版本太多的光影 / 资源包放进“多版本”。
fn group_project_versions(
    entries: Vec<VersionEntry>,
    game_rank: &HashMap<String, usize>,
) -> Vec<VersionGroup> {
    let mut order = Vec::<String>::new();
    let mut grouped = HashMap::<String, Vec<VersionEntry>>::new();

    for entry in entries {
        let groups = if entry.groups.is_empty() {
            vec!["通用".to_string()]
        } else {
            entry.groups.clone()
        };
        for group in groups {
            if !grouped.contains_key(&group) {
                order.push(group.clone());
            }
            grouped.entry(group).or_default().push(entry.clone());
        }
    }

    // 游戏版本按 Mojang 清单的新旧顺序排（清单里越靠前越新）；“多版本 / 通用”放最后。
    order.sort_by(|left, right| {
        let left_last = matches!(left.as_str(), "多版本" | "通用");
        let right_last = matches!(right.as_str(), "多版本" | "通用");
        if left_last != right_last {
            return left_last.cmp(&right_last);
        }
        match (game_rank.get(left), game_rank.get(right)) {
            (Some(left), Some(right)) => left.cmp(right),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => compare_game_versions(right, left),
        }
    });

    order
        .into_iter()
        .map(|label| {
            let versions = grouped.remove(&label).unwrap_or_default();
            VersionGroup {
                game_version: label,
                versions,
            }
        })
        .collect()
}

/// 清单里没有的版本按数字段倒序比较（1.21.4 > 1.21 > 1.9）。
fn compare_game_versions(left: &str, right: &str) -> std::cmp::Ordering {
    fn key(version: &str) -> Vec<u64> {
        version
            .split(|character: char| !character.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse().ok())
            .collect()
    }

    key(left).cmp(&key(right)).then_with(|| left.cmp(right))
}

fn entry_to_info(entry: VersionEntry) -> ProjectVersionInfo {
    ProjectVersionInfo {
        key: entry.key.into(),
        name: entry.name.into(),
        game_versions: entry.game_versions.into(),
        loaders: ModelRc::new(VecModel::from(
            entry
                .loaders
                .into_iter()
                .map(Into::into)
                .collect::<Vec<slint::SharedString>>(),
        )),
        channel: entry.channel.into(),
        file_name: entry.file_name.into(),
        size: entry.size.into(),
        downloadable: entry.downloadable,
        recommended: entry.recommended,
    }
}

/// 在 UI 线程把纯数据转换成 Slint 模型。
fn version_group_model(groups: Vec<VersionGroup>) -> ModelRc<ProjectVersionGroup> {
    let groups = groups
        .into_iter()
        .map(|group| ProjectVersionGroup {
            game_version: group.game_version.into(),
            versions: ModelRc::new(VecModel::from(
                group
                    .versions
                    .into_iter()
                    .map(entry_to_info)
                    .collect::<Vec<_>>(),
            )),
        })
        .collect::<Vec<_>>();

    ModelRc::new(VecModel::from(groups))
}

/// 光影 / 资源包通常兼容很多版本，列表里最多展开 4 个；其它类别 6 个。
fn version_display_limit(kind: ContentKind) -> usize {
    match kind {
        ContentKind::Shader | ContentKind::ResourcePack => 4,
        _ => 6,
    }
}

fn version_group_keys(versions: &[String], kind: ContentKind) -> Vec<String> {
    let limit = version_display_limit(kind);

    let mut groups = Vec::new();
    for version in versions {
        let version = version.trim();
        if version.is_empty() {
            continue;
        }
        if !groups.iter().any(|existing| existing == version) {
            groups.push(version.to_string());
        }
    }

    if groups.is_empty() {
        vec!["通用".to_string()]
    } else if groups.len() > limit {
        vec!["多版本".to_string()]
    } else {
        groups
    }
}

fn download_choice(
    choice: &VersionChoice,
    kind: ContentKind,
    target: Option<&Path>,
) -> Result<PathBuf, String> {
    let dir = content_install_dir(kind, target);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("创建 {} 目录失败：{err}", kind.target_dir()))?;

    let dest = dir.join(&choice.file_name);
    let downloader = Downloader::new(Progress::silent());
    downloader
        .download(&choice.url, &dest, choice.sha1.as_deref(), choice.size)
        .map_err(|err| format!("下载失败：{err}"))?;

    Ok(dest)
}

/// 下载目录：指定实例时放进实例对应的内容目录，否则用默认 `.minecraft`。
fn content_install_dir(kind: ContentKind, target: Option<&Path>) -> PathBuf {
    match target {
        Some(instance) => instance.join(kind.target_dir()),
        None => default_game_dir().join(kind.target_dir()),
    }
}

fn update_info(candidate: &UpdateCandidate) -> ContentUpdateInfo {
    ContentUpdateInfo {
        key: candidate.key.clone().into(),
        name: candidate.old_name.clone().into(),
        current_version: candidate.current_version.clone().into(),
        new_version: candidate.new_version.version.clone().into(),
        source: candidate.source.name().into(),
        file_name: candidate.new_version.file_name.clone().into(),
        size: format_size(candidate.new_version.size).into(),
    }
}

/// 用新版本替换实例里的旧文件，并同步更新来源记录。
fn replace_content(candidate: &UpdateCandidate) -> Result<PathBuf, String> {
    let directory = candidate.instance.join(&candidate.folder);
    std::fs::create_dir_all(&directory)
        .map_err(|err| format!("创建 {} 目录失败：{err}", candidate.folder))?;

    let old_enabled = directory.join(&candidate.old_name);
    let old_disabled = directory.join(format!("{}.disabled", candidate.old_name));
    let was_disabled = !old_enabled.exists() && old_disabled.exists();

    let downloaded = directory.join(&candidate.new_version.file_name);
    let downloader = Downloader::new(Progress::silent());
    downloader
        .download(
            &candidate.new_version.url,
            &downloaded,
            candidate.new_version.sha1.as_deref(),
            candidate.new_version.size,
        )
        .map_err(|err| format!("下载更新失败：{err}"))?;

    let final_path = if was_disabled {
        let disabled = directory.join(format!("{}.disabled", candidate.new_version.file_name));
        if downloaded != disabled {
            let _ = std::fs::remove_file(&disabled);
            std::fs::rename(&downloaded, &disabled)
                .map_err(|err| format!("重命名更新文件失败：{err}"))?;
        }
        disabled
    } else {
        downloaded
    };

    let old_path = if old_enabled.exists() {
        old_enabled
    } else {
        old_disabled
    };
    if old_path != final_path {
        std::fs::remove_file(&old_path).map_err(|err| format!("删除旧文件失败：{err}"))?;
    }

    let mut metadata = instances::read_content_metadata(&candidate.instance);
    metadata.untrack(&candidate.folder, &candidate.old_name);
    metadata.track(
        &candidate.folder,
        &candidate.new_version.file_name,
        instances::TrackedContent {
            source: candidate.source.key().to_string(),
            project_id: candidate.project_id.clone(),
            version_id: candidate.new_version.version_id.clone(),
            version: candidate.new_version.version.clone(),
            file_name: candidate.new_version.file_name.clone(),
        },
    );
    if let Err(err) = instances::write_content_metadata(&candidate.instance, &metadata) {
        eprintln!("记录内容来源失败：{err}");
    }

    Ok(final_path)
}

fn find_modrinth_update(
    downloader: &Downloader,
    project_id: &str,
    game_version: &str,
    loader: &str,
    kind: ContentKind,
) -> Result<Option<NewVersion>, String> {
    let versions = modrinth::project_versions(downloader, project_id)
        .map_err(|err| format!("Modrinth 请求失败：{err}"))?;
    Ok(pick_modrinth_update(&versions, game_version, loader, kind))
}

fn pick_modrinth_update(
    versions: &[modrinth::ProjectVersion],
    game_version: &str,
    loader: &str,
    kind: ContentKind,
) -> Option<NewVersion> {
    let mut best: Option<&modrinth::ProjectVersion> = None;

    for version in versions {
        if !version.version_type.eq_ignore_ascii_case("release") {
            continue;
        }
        if !game_version.is_empty()
            && !version
                .game_versions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(game_version))
        {
            continue;
        }
        if kind == ContentKind::Mod && !loader_matches(&version.loaders, loader) {
            continue;
        }
        if best.is_none_or(|best| version.date_published > best.date_published) {
            best = Some(version);
        }
    }

    let version = best?;
    let file = modrinth::main_file(version)?;
    if file.url.is_empty() {
        return None;
    }
    let file_name = safe_file_name(&file.filename)?;

    Some(NewVersion {
        version_id: version.id.clone(),
        version: if version.version_number.trim().is_empty() {
            version.name.clone()
        } else {
            version.version_number.clone()
        },
        url: file.url.clone(),
        file_name: file_name.to_string(),
        sha1: file.sha1().map(str::to_string),
        size: file.size,
    })
}

fn find_curseforge_update(
    downloader: &Downloader,
    project_id: &str,
    game_version: &str,
    loader: &str,
    kind: ContentKind,
) -> Result<Option<NewVersion>, String> {
    let api_key = curseforge_api_key()?;
    let mut files = curseforge::mod_files(downloader, project_id, None, &api_key)
        .map_err(|err| format!("CurseForge 请求失败：{err}"))?;
    files.sort_by(|left, right| right.file_date.cmp(&left.file_date));
    Ok(pick_curseforge_update(&files, game_version, loader, kind))
}

fn pick_curseforge_update(
    files: &[curseforge::ModFile],
    game_version: &str,
    loader: &str,
    kind: ContentKind,
) -> Option<NewVersion> {
    for file in files {
        if file.release_type != 1 || !file.downloadable() {
            continue;
        }
        if !game_version.is_empty()
            && !file
                .game_versions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(game_version))
        {
            continue;
        }
        if kind == ContentKind::Mod && !loader_matches(&file.game_versions, loader) {
            continue;
        }

        let Some(url) = file.download_url.clone().filter(|url| !url.is_empty()) else {
            continue;
        };
        let Some(file_name) = safe_file_name(&file.file_name).map(str::to_string) else {
            continue;
        };

        return Some(NewVersion {
            version_id: file.id.to_string(),
            version: if file.display_name.trim().is_empty() {
                file.file_name.clone()
            } else {
                file.display_name.clone()
            },
            url,
            file_name,
            sha1: file.sha1().map(str::to_string),
            size: file.file_length,
        });
    }

    None
}

/// 游戏版本太多时（尤其是光影 / 资源包）不再逐条列出。
fn display_game_versions(versions: &[&str], kind: ContentKind) -> String {
    if versions.is_empty() {
        return "—".to_string();
    }

    let limit = version_display_limit(kind);

    if versions.len() > limit {
        match kind {
            // 光影 / 资源包通常兼容一大段版本，直接不写，避免撑满一行。
            ContentKind::Shader | ContentKind::ResourcePack => String::new(),
            _ => "多个版本".to_string(),
        }
    } else {
        versions.join(", ")
    }
}

/// 只取文件名部分，避免远端返回 `../` 之类的路径把文件写到目录外。
fn safe_file_name(name: &str) -> Option<&str> {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();

    if name.is_empty() || name == "." || name == ".." {
        None
    } else {
        Some(name)
    }
}

/// 版本支持的加载器标签；模组是模组加载器，光影是光影加载器。
///
/// 资源包不需要加载器，因此不显示标签；没有加载器信息的版本用「通用」兜底。
fn loader_tags(tokens: &[String], kind: ContentKind) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();

    for token in tokens {
        if !is_loader(token) {
            continue;
        }
        let name = loader_name(token);
        if name.is_empty() || name == "Minecraft" {
            continue;
        }
        if !names.iter().any(|existing| existing == &name) {
            names.push(name);
        }
    }

    if names.is_empty() && kind != ContentKind::ResourcePack {
        names.push("通用".to_string());
    }

    names
}

fn curseforge_game_versions(tokens: &[String], kind: ContentKind) -> String {
    let versions = tokens
        .iter()
        .map(String::as_str)
        .filter(|token| !is_loader(token) && !is_environment(token))
        .collect::<Vec<_>>();

    display_game_versions(&versions, kind)
}

fn is_loader(token: &str) -> bool {
    matches!(
        token.trim().to_ascii_lowercase().as_str(),
        "fabric"
            | "forge"
            | "neoforge"
            | "quilt"
            | "liteloader"
            | "modloader"
            | "rift"
            | "iris"
            | "optifine"
            | "canvas"
            | "minecraft"
            | "vanilla"
    )
}

fn is_environment(token: &str) -> bool {
    matches!(
        token.trim().to_ascii_lowercase().as_str(),
        "client" | "server"
    )
}

fn loader_name(loader: &str) -> String {
    match loader.trim().to_ascii_lowercase().as_str() {
        "" => String::new(),
        "fabric" => "Fabric".to_string(),
        "forge" => "Forge".to_string(),
        "neoforge" => "NeoForge".to_string(),
        "quilt" => "Quilt".to_string(),
        "liteloader" => "LiteLoader".to_string(),
        "rift" => "Rift".to_string(),
        "iris" => "Iris".to_string(),
        "optifine" => "OptiFine".to_string(),
        "minecraft" => "Minecraft".to_string(),
        other => capitalize(other),
    }
}

fn modrinth_channel(channel: &str) -> &'static str {
    match channel.to_ascii_lowercase().as_str() {
        "release" => "正式版",
        "beta" => "Beta",
        "alpha" => "Alpha",
        _ => "其他",
    }
}

fn curseforge_channel(release_type: u8) -> &'static str {
    match release_type {
        1 => "正式版",
        2 => "Beta",
        3 => "Alpha",
        _ => "其他",
    }
}

fn format_size(size: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;

    let size = size as f64;
    if size >= MB {
        format!("{:.1} MB", size / MB)
    } else if size >= KB {
        format!("{:.1} KB", size / KB)
    } else {
        format!("{size:.0} B")
    }
}

fn curseforge_api_key() -> Result<String, String> {
    if let Some(key) = CURSEFORGE_KEY.get().and_then(|key| key.as_deref()) {
        return Ok(key.to_string());
    }

    // 兜底：init 没有被调用时（例如单独测试）仍然尝试读环境变量。
    for name in curseforge::API_KEY_ENVS {
        if let Ok(key) = std::env::var(name) {
            let key = key.trim();
            if !key.is_empty() {
                return Ok(key.to_string());
            }
        }
    }

    Err(format!(
        "CurseForge 需要 API Key：请在 .env 中设置 {}",
        curseforge::API_KEY_ENVS[0]
    ))
}

/// 搜索结果的统一视图，抹平 Modrinth / CurseForge 的字段差异。
#[derive(Debug, Clone)]
struct DiscoverHit {
    project_id: String,
    title: String,
    description: String,
    author: String,
    downloads: u64,
    icon_url: Option<String>,
    /// 客户端/服务端、分类、加载器等蓝色标签
    tags: Vec<String>,
}

impl DiscoverHit {
    fn from_modrinth(hit: modrinth::ProjectHit) -> Self {
        let tags = project_tags(&hit.client_side, &hit.server_side, &hit.categories);
        Self {
            project_id: hit.project_id,
            title: hit.title,
            description: hit.description,
            author: hit.author,
            downloads: hit.downloads,
            icon_url: hit.icon_url,
            tags,
        }
    }

    fn from_curseforge(hit: curseforge::Mod) -> Self {
        let author = hit.author();
        let icon_url = hit.icon_url().map(str::to_string);
        let categories = hit
            .categories
            .iter()
            .map(|category| category.name.clone())
            .collect::<Vec<_>>();
        let tags = project_tags("", "", &categories);

        Self {
            project_id: hit.id.to_string(),
            title: hit.name,
            description: hit.summary,
            author,
            downloads: hit.download_count,
            icon_url,
            tags,
        }
    }
}

/// 把客户端/服务端支持、分类和加载器拼成一组蓝色标签。
fn project_tags(client_side: &str, server_side: &str, categories: &[String]) -> Vec<String> {
    fn side_supported(side: &str) -> bool {
        !matches!(
            side.trim().to_ascii_lowercase().as_str(),
            "" | "unsupported" | "unknown"
        )
    }

    let mut tags = Vec::new();
    if side_supported(client_side) {
        tags.push("客户端".to_string());
    }
    if side_supported(server_side) {
        tags.push("服务端".to_string());
    }

    let mut loaders = Vec::new();
    for category in categories {
        let category = category.trim();
        if category.is_empty() || is_environment(category) {
            continue;
        }
        if is_loader(category) {
            let name = loader_name(category);
            if !name.is_empty() && !loaders.contains(&name) {
                loaders.push(name);
            }
        } else if !tags.iter().any(|tag| tag.eq_ignore_ascii_case(category)) {
            tags.push(category.to_string());
        }
    }

    for loader in loaders {
        if !tags.iter().any(|tag| tag == &loader) {
            tags.push(loader);
        }
    }
    tags
}

struct DiscoverResponse {
    hits: Vec<DiscoverHit>,
    total: u64,
}

/// 按下载源搜索项目，统一成 [`DiscoverResponse`]。
#[allow(clippy::too_many_arguments)]
fn search_projects(
    downloader: &Downloader,
    source: DiscoverSource,
    kind: ContentKind,
    query: Option<&str>,
    game_version: Option<&str>,
    loader: Option<&str>,
    category: Option<&str>,
    sort: &str,
    limit: usize,
) -> Result<DiscoverResponse, String> {
    match source {
        DiscoverSource::Modrinth => {
            let response = modrinth::search(
                downloader,
                kind,
                query,
                game_version,
                loader,
                category,
                sort,
                limit,
            )
            .map_err(|err| err.to_string())?;
            Ok(DiscoverResponse {
                total: response.total_hits,
                hits: response
                    .hits
                    .into_iter()
                    .map(DiscoverHit::from_modrinth)
                    .collect(),
            })
        }
        DiscoverSource::CurseForge => {
            let api_key = curseforge_api_key()?;
            let loader = loader.and_then(curseforge::loader_type);
            let category = category.and_then(|id| id.parse::<u32>().ok());
            let sort = sort.parse::<u8>().unwrap_or(6);
            let response = curseforge::search(
                downloader,
                kind,
                query,
                game_version,
                loader,
                category,
                sort,
                limit,
                &api_key,
            )
            .map_err(|err| err.to_string())?;
            Ok(DiscoverResponse {
                total: response.pagination.total_count,
                hits: response
                    .data
                    .into_iter()
                    .map(DiscoverHit::from_curseforge)
                    .collect(),
            })
        }
    }
}

fn to_project_info(hit: &DiscoverHit, icon: Option<slint::Image>) -> ProjectInfo {
    let has_icon = icon.is_some();
    let tags = hit
        .tags
        .iter()
        .map(|tag| tag.as_str().into())
        .collect::<Vec<slint::SharedString>>();

    ProjectInfo {
        project_id: hit.project_id.clone().into(),
        title: hit.title.clone().into(),
        initial: first_char(&hit.title).into(),
        subtitle: format!("{} · {} 次下载", hit.author, short_number(hit.downloads)).into(),
        description: hit.description.clone().into(),
        icon: icon.unwrap_or_default(),
        has_icon,
        tags: ModelRc::new(VecModel::from(tags)),
    }
}

/// 下载并缓存项目图标的原始字节，返回与 `hits` 一一对应的数据。
///
/// 图标来自搜索结果里的 `icon_url`；缓存命中就不联网，单个失败时返回 `None`，
/// 由界面回退到首字母头像。缓存文件名带下载源前缀，避免两边 id 撞车。
fn load_project_icon_bytes(source: DiscoverSource, hits: &[DiscoverHit]) -> Vec<Option<Vec<u8>>> {
    let dir = crate::accounts::cache_dir().join("icons");
    let _ = std::fs::create_dir_all(&dir);

    let cache_name = |project_id: &str| format!("{}-{project_id}.bin", source.key());

    let mut jobs = Vec::new();
    for hit in hits {
        let Some(url) = hit.icon_url.as_deref().filter(|url| !url.is_empty()) else {
            continue;
        };
        let path = dir.join(cache_name(&hit.project_id));
        let cached = std::fs::metadata(&path)
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false);
        if !cached {
            let _ = std::fs::remove_file(&path);
            jobs.push((url.to_string(), path));
        }
    }

    if !jobs.is_empty() {
        // 图标小、数量多，开几个线程并发下载；失败的直接忽略。
        let workers = 4.min(jobs.len());
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    let downloader = Downloader::new(Progress::silent());
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some((url, path)) = jobs.get(index) else {
                            break;
                        };
                        if let Ok(bytes) = downloader.get_bytes(url)
                            && !bytes.is_empty()
                        {
                            let _ = std::fs::write(path, &bytes);
                        }
                    }
                });
            }
        });
    }

    hits.iter()
        .map(|hit| {
            let bytes = std::fs::read(dir.join(cache_name(&hit.project_id))).ok()?;
            (!bytes.is_empty()).then_some(bytes)
        })
        .collect()
}

fn content_label(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mod => "模组",
        ContentKind::Shader => "光影包",
        ContentKind::ResourcePack => "材质包",
        ContentKind::ModPack => "整合包",
    }
}

/// 1 亿以上用「亿」、1 万以上用「万」，其余原样。
fn short_number(value: u64) -> String {
    if value >= 100_000_000 {
        format!("{:.1} 亿", value as f64 / 100_000_000.0)
    } else if value >= 10_000 {
        format!("{:.1} 万", value as f64 / 10_000.0)
    } else {
        value.to_string()
    }
}

fn first_char(text: &str) -> String {
    text.chars()
        .next()
        .map(|ch| ch.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_discover_source_keys() {
        assert_eq!(
            DiscoverSource::parse("curseforge"),
            DiscoverSource::CurseForge
        );
        assert_eq!(
            DiscoverSource::parse("CurseForge"),
            DiscoverSource::CurseForge
        );
        assert_eq!(DiscoverSource::parse("modrinth"), DiscoverSource::Modrinth);
        assert_eq!(DiscoverSource::parse(""), DiscoverSource::Modrinth);
    }

    #[test]
    fn game_version_choices_start_with_all_and_only_include_releases() {
        let manifest: VersionManifest = serde_json::from_str(
            r#"{
              "latest": { "release": "1.21.1", "snapshot": "24w33a" },
              "versions": [
                { "id": "1.21.1", "type": "release", "url": "" },
                { "id": "24w33a", "type": "snapshot", "url": "" },
                { "id": "1.21", "type": "release", "url": "" }
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(
            game_version_choices(&manifest),
            vec![
                ALL_GAME_VERSIONS.to_string(),
                "1.21.1".to_string(),
                "1.21".to_string()
            ]
        );
    }

    #[test]
    fn project_tags_include_environment_categories_and_loaders() {
        let categories = vec![
            "fabric".to_string(),
            "neoforge".to_string(),
            "optimization".to_string(),
            "quilt".to_string(),
        ];
        assert_eq!(
            project_tags("required", "unsupported", &categories),
            vec![
                "客户端".to_string(),
                "optimization".to_string(),
                "Fabric".to_string(),
                "NeoForge".to_string(),
                "Quilt".to_string()
            ]
        );

        let both = project_tags("required", "optional", &categories);
        assert_eq!(both[0], "客户端");
        assert_eq!(both[1], "服务端");

        let curseforge = vec![
            "Fabric".to_string(),
            "API and Library".to_string(),
            "Adventure".to_string(),
        ];
        assert_eq!(
            project_tags("", "", &curseforge),
            vec![
                "API and Library".to_string(),
                "Adventure".to_string(),
                "Fabric".to_string()
            ]
        );
    }

    #[test]
    fn april_fools_filter_matches_known_versions() {
        let manifest: VersionManifest = serde_json::from_str(
            r#"{
              "latest": { "release": "1.21.1", "snapshot": "26w14a" },
              "versions": [
                { "id": "1.21.1", "type": "release", "url": "" },
                { "id": "26w14a", "type": "snapshot", "url": "" },
                { "id": "26.2-snapshot-1", "type": "snapshot", "url": "" },
                { "id": "24w14potato", "type": "snapshot", "url": "" }
              ]
            }"#,
        )
        .unwrap();

        let fools = build_version_items(&manifest, Filter::Fools);
        assert_eq!(
            fools
                .iter()
                .map(|version| version.version_id.as_str())
                .collect::<Vec<_>>(),
            vec!["26w14a", "24w14potato"]
        );
        assert!(fools.iter().all(|version| version.kind_label == "愚人节"));

        let snapshots = build_version_items(&manifest, Filter::Snapshot);
        assert_eq!(
            snapshots
                .iter()
                .map(|version| version.version_id.as_str())
                .collect::<Vec<_>>(),
            vec!["26w14a", "26.2-snapshot-1", "24w14potato"]
        );

        assert!(is_april_fools("1.RV-Pre1"));
        assert!(is_april_fools("22w13oneblockatatime"));
        assert!(!is_april_fools("26w14b"));
    }

    #[test]
    fn reads_curseforge_key_without_expanding_dollar_signs() {
        let dir = std::env::temp_dir().join(format!("neutrino-cf-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(
            &path,
            "OTHER=value\nCURSEFORGE_API_KEY=$2a$10$abcdefghijklmnopqrstuv\n",
        )
        .unwrap();

        assert_eq!(
            read_curseforge_key(&path).as_deref(),
            Some("$2a$10$abcdefghijklmnopqrstuv")
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reads_quoted_or_commented_curseforge_key() {
        let dir = std::env::temp_dir().join(format!("neutrino-cf-key-q-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");

        std::fs::write(&path, "CURSEFORGE_API_KEY='$2a$10$quoted'\n").unwrap();
        assert_eq!(read_curseforge_key(&path).as_deref(), Some("$2a$10$quoted"));

        std::fs::write(
            &path,
            "export NEUTRINO_CURSEFORGE_API_KEY=plain-key # comment\n",
        )
        .unwrap();
        assert_eq!(read_curseforge_key(&path).as_deref(), Some("plain-key"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn curseforge_tokens_split_versions_and_loaders() {
        let tokens = vec![
            "1.21.1".to_string(),
            "Fabric".to_string(),
            "Client".to_string(),
            "Server".to_string(),
        ];

        assert_eq!(
            curseforge_game_versions(&tokens, ContentKind::Mod),
            "1.21.1"
        );
        assert_eq!(
            loader_tags(&tokens, ContentKind::Mod),
            vec!["Fabric".to_string()]
        );
    }

    #[test]
    fn loader_tags_deduplicate_and_fall_back() {
        let loaders = vec![
            "fabric".to_string(),
            "Fabric".to_string(),
            "forge".to_string(),
            "NeoForge".to_string(),
        ];

        assert_eq!(
            loader_tags(&loaders, ContentKind::Mod),
            vec![
                "Fabric".to_string(),
                "Forge".to_string(),
                "NeoForge".to_string()
            ]
        );
        assert_eq!(loader_tags(&[], ContentKind::Mod), vec!["通用".to_string()]);
        assert_eq!(
            loader_tags(
                &["iris".to_string(), "optifine".to_string()],
                ContentKind::Shader
            ),
            vec!["Iris".to_string(), "OptiFine".to_string()]
        );
        assert!(loader_tags(&["minecraft".to_string()], ContentKind::ResourcePack).is_empty());
    }

    #[test]
    fn content_install_dir_follows_instance_or_default() {
        let instance = Path::new("C:/games/Test");
        assert_eq!(
            content_install_dir(ContentKind::Mod, Some(instance)),
            instance.join("mods")
        );
        assert_eq!(
            content_install_dir(ContentKind::ResourcePack, Some(instance)),
            instance.join("resourcepacks")
        );
        assert_eq!(
            content_install_dir(ContentKind::Shader, Some(instance)),
            instance.join("shaderpacks")
        );
        assert_eq!(
            content_install_dir(ContentKind::ModPack, Some(instance)),
            instance.join("modpacks")
        );
    }

    #[test]
    fn formats_file_sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn channel_labels_match_source_enums() {
        assert_eq!(modrinth_channel("release"), "正式版");
        assert_eq!(modrinth_channel("beta"), "Beta");
        assert_eq!(modrinth_channel("alpha"), "Alpha");
        assert_eq!(curseforge_channel(1), "正式版");
        assert_eq!(curseforge_channel(2), "Beta");
        assert_eq!(curseforge_channel(3), "Alpha");
        assert_eq!(curseforge_channel(0), "其他");
    }

    #[test]
    fn sanitizes_remote_file_names() {
        assert_eq!(safe_file_name("mod.jar"), Some("mod.jar"));
        assert_eq!(safe_file_name("../../evil.jar"), Some("evil.jar"));
        assert_eq!(safe_file_name("dir\\nested.jar"), Some("nested.jar"));
        assert_eq!(safe_file_name(".."), None);
        assert_eq!(safe_file_name(""), None);
    }

    #[test]
    fn hides_long_game_version_lists() {
        let versions = ["1.20", "1.21", "1.21.1", "1.21.2", "1.21.3"];

        assert_eq!(
            display_game_versions(&versions[..3], ContentKind::ResourcePack),
            "1.20, 1.21, 1.21.1"
        );
        assert_eq!(
            display_game_versions(&versions, ContentKind::ResourcePack),
            ""
        );
        assert_eq!(display_game_versions(&versions, ContentKind::Shader), "");
        assert_eq!(
            display_game_versions(&versions, ContentKind::Mod),
            "1.20, 1.21, 1.21.1, 1.21.2, 1.21.3"
        );
        assert_eq!(
            display_game_versions(&["1.20"], ContentKind::Shader),
            "1.20"
        );
    }

    #[test]
    fn formats_category_labels() {
        assert_eq!(pretty_category("game-mechanics"), "Game Mechanics");
        assert_eq!(pretty_category("worldgen"), "Worldgen");
        assert_eq!(pretty_category("512x+"), "512x+");

        assert_eq!(
            category_label(DiscoverSource::Modrinth, "game-mechanics"),
            "游戏机制"
        );
        assert_eq!(category_label(DiscoverSource::Modrinth, "16x"), "16x");
        assert_eq!(
            category_label(DiscoverSource::CurseForge, "Map and Information"),
            "地图与信息"
        );
        assert_eq!(
            category_label(DiscoverSource::CurseForge, "RLCraft"),
            "RLCraft"
        );
    }

    #[test]
    fn loader_choices_follow_source_and_kind() {
        let modrinth = loader_choices(DiscoverSource::Modrinth, ContentKind::Mod);
        assert_eq!(modrinth[0].label, ALL_LOADERS);
        assert!(
            modrinth
                .iter()
                .any(|choice| choice.label == "Fabric" && choice.key == "fabric")
        );

        let curseforge = loader_choices(DiscoverSource::CurseForge, ContentKind::Mod);
        assert!(
            curseforge
                .iter()
                .any(|choice| choice.label == "Fabric" && choice.key == "fabric")
        );

        assert_eq!(
            loader_choices(DiscoverSource::CurseForge, ContentKind::Shader).len(),
            1
        );
        assert_eq!(
            loader_choices(DiscoverSource::Modrinth, ContentKind::ResourcePack).len(),
            1
        );
    }

    #[test]
    fn sort_choices_match_sources() {
        let modrinth = sort_choices(DiscoverSource::Modrinth);
        assert_eq!(modrinth[0].label, "相关度");
        assert_eq!(modrinth[0].key, "relevance");
        assert!(
            modrinth
                .iter()
                .any(|choice| choice.label == "关注量" && choice.key == "follows")
        );
        assert!(
            modrinth
                .iter()
                .any(|choice| choice.label == "发布日期" && choice.key == "newest")
        );
        assert!(
            modrinth
                .iter()
                .any(|choice| choice.label == "更新日期" && choice.key == "updated")
        );

        let curseforge = sort_choices(DiscoverSource::CurseForge);
        assert_eq!(curseforge[0].label, "热度");
        assert_eq!(curseforge[0].key, "2");
        assert!(
            curseforge
                .iter()
                .any(|choice| choice.label == "名称" && choice.key == "4")
        );
        assert!(
            curseforge
                .iter()
                .any(|choice| choice.label == "最近更新" && choice.key == "3")
        );
        assert!(
            curseforge
                .iter()
                .any(|choice| choice.label == "创建日期" && choice.key == "11")
        );
        assert!(
            curseforge
                .iter()
                .any(|choice| choice.label == "下载量" && choice.key == "6")
        );
    }

    #[test]
    fn loader_matches_handles_quilt_and_unknown_loaders() {
        assert!(loader_matches(&[], ""));
        assert!(loader_matches(&["fabric".to_string()], "quilt"));
        assert!(loader_matches(&["neoforge".to_string()], "neoforge"));
        assert!(!loader_matches(&["fabric".to_string()], "forge"));
    }

    #[test]
    fn picks_newest_compatible_modrinth_release() {
        let version = |id: &str, channel: &str, game: &str, loader: &str, date: &str| {
            modrinth::ProjectVersion {
                id: id.to_string(),
                name: format!("Version {id}"),
                version_number: format!("{id}.0"),
                version_type: channel.to_string(),
                game_versions: vec![game.to_string()],
                loaders: vec![loader.to_string()],
                date_published: date.to_string(),
                files: vec![modrinth::VersionFile {
                    url: format!("https://example.invalid/{id}.jar"),
                    filename: format!("{id}.jar"),
                    primary: true,
                    size: 100,
                    hashes: std::collections::HashMap::new(),
                }],
            }
        };

        let versions = vec![
            version("old", "release", "1.21.1", "fabric", "2025-01-01"),
            version("new", "release", "1.21.1", "fabric", "2026-01-01"),
            version("beta", "beta", "1.21.1", "fabric", "2026-02-01"),
            version("forge", "release", "1.21.1", "forge", "2026-03-01"),
        ];
        let picked = pick_modrinth_update(&versions, "1.21.1", "fabric", ContentKind::Mod).unwrap();
        assert_eq!(picked.version_id, "new");
        assert_eq!(picked.file_name, "new.jar");
        assert_eq!(picked.version, "new.0");

        let shader = version("shader", "release", "1.21.1", "iris", "2026-01-01");
        assert!(pick_modrinth_update(&[shader], "1.20", "fabric", ContentKind::Shader).is_none());
    }

    #[test]
    fn picks_compatible_curseforge_release() {
        let file = |id: u64, release: u8, game: &str, loader: &str| curseforge::ModFile {
            id,
            display_name: format!("File {id}"),
            file_name: format!("file-{id}.jar"),
            download_url: Some(format!("https://example.invalid/{id}.jar")),
            release_type: release,
            file_date: "2026-01-01T00:00:00Z".to_string(),
            file_length: 100,
            game_versions: vec![game.to_string(), loader.to_string()],
            hashes: Vec::new(),
        };

        let files = vec![
            file(1, 1, "1.21.1", "Fabric"),
            file(2, 2, "1.21.1", "Fabric"),
            file(3, 1, "1.21.1", "Forge"),
        ];
        let picked = pick_curseforge_update(&files, "1.21.1", "fabric", ContentKind::Mod).unwrap();
        assert_eq!(picked.version_id, "1");
        assert_eq!(picked.file_name, "file-1.jar");
    }

    fn version_entry(
        name: &str,
        groups: &[&str],
        supported: &[&str],
        loaders: &[&str],
        release: bool,
    ) -> VersionEntry {
        VersionEntry {
            key: name.to_string(),
            name: name.to_string(),
            game_versions: supported.join(", "),
            supported_versions: supported.iter().map(|value| value.to_string()).collect(),
            loaders: loaders.iter().map(|value| value.to_string()).collect(),
            channel: if release { "正式版" } else { "Beta" }.to_string(),
            release,
            file_name: format!("{name}.jar"),
            size: "1 MB".to_string(),
            downloadable: true,
            groups: groups.iter().map(|value| value.to_string()).collect(),
            recommended: false,
        }
    }

    #[test]
    fn sorts_version_groups_by_manifest_order() {
        let entries = vec![
            version_entry("old", &["1.20.1"], &["1.20.1"], &["Fabric"], true),
            version_entry("new", &["1.21.4"], &["1.21.4"], &["Fabric"], true),
            version_entry("mid", &["1.21.1"], &["1.21.1"], &["Fabric"], true),
            version_entry("multi", &["多版本"], &["1.20", "1.21"], &["Fabric"], true),
        ];
        let mut game_rank = HashMap::new();
        game_rank.insert("1.21.4".to_string(), 0);
        game_rank.insert("1.21.1".to_string(), 1);
        game_rank.insert("1.20.1".to_string(), 2);

        let groups = group_project_versions(entries, &game_rank);
        let labels = groups
            .iter()
            .map(|group| group.game_version.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["1.21.4", "1.21.1", "1.20.1", "多版本"]);
    }

    #[test]
    fn recommends_latest_compatible_release() {
        let context = DownloadContext {
            game_version: "1.21.1".to_string(),
            loader: "Fabric".to_string(),
        };
        let mut entries = vec![
            version_entry("beta", &["1.21.1"], &["1.21.1"], &["Fabric"], false),
            version_entry("release", &["1.21.1"], &["1.21.1"], &["Fabric"], true),
            version_entry("wrong-game", &["1.20.1"], &["1.20.1"], &["Fabric"], true),
            version_entry("wrong-loader", &["1.21.1"], &["1.21.1"], &["Forge"], true),
        ];

        let recommended = mark_recommended(&mut entries, Some(&context), ContentKind::Mod).unwrap();
        assert_eq!(recommended.name, "release");
        assert!(recommended.recommended);

        let quilt = DownloadContext {
            game_version: "1.21.1".to_string(),
            loader: "Quilt".to_string(),
        };
        let mut entries = vec![version_entry(
            "fabric",
            &["1.21.1"],
            &["1.21.1"],
            &["Fabric"],
            true,
        )];
        assert!(mark_recommended(&mut entries, Some(&quilt), ContentKind::Mod).is_some());
        assert!(mark_recommended(&mut entries, None, ContentKind::Mod).is_none());
    }
}
