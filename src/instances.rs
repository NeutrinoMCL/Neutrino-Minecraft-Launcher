use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct InstanceFolder {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentFile {
    pub name: String,
    pub size: u64,
    pub enabled: bool,
}

/// 通过启动器下载到实例的内容来源，用来检查更新。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContentMetadata {
    /// 目录名（mods / resourcepacks / shaderpacks）→ 启用时的文件名 → 来源信息。
    #[serde(default)]
    pub entries: HashMap<String, HashMap<String, TrackedContent>>,
}

/// 单个已下载内容的来源信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedContent {
    /// `modrinth` / `curseforge`
    pub source: String,
    pub project_id: String,
    /// Modrinth version id / CurseForge file id
    pub version_id: String,
    /// 下载时显示的版本号
    #[serde(default)]
    pub version: String,
    /// 下载时的文件名
    #[serde(default)]
    pub file_name: String,
}

const CONTENT_METADATA_FILE: &str = "neutrino-content.json";

impl ContentMetadata {
    pub fn track(&mut self, folder: &str, name: &str, content: TrackedContent) {
        self.entries
            .entry(folder.to_string())
            .or_default()
            .insert(name.to_string(), content);
    }

    pub fn untrack(&mut self, folder: &str, name: &str) -> bool {
        let Some(entries) = self.entries.get_mut(folder) else {
            return false;
        };
        let removed = entries.remove(name).is_some();
        if entries.is_empty() {
            self.entries.remove(folder);
        }
        removed
    }
}

pub fn content_metadata_path(instance: &Path) -> PathBuf {
    instance.join(CONTENT_METADATA_FILE)
}

pub fn read_content_metadata(instance: &Path) -> ContentMetadata {
    std::fs::read(content_metadata_path(instance))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn write_content_metadata(instance: &Path, metadata: &ContentMetadata) -> Result<(), String> {
    let path = content_metadata_path(instance);

    if metadata.entries.is_empty() {
        let _ = std::fs::remove_file(path);
        return Ok(());
    }

    let bytes =
        serde_json::to_vec_pretty(metadata).map_err(|err| format!("序列化内容来源失败：{err}"))?;
    std::fs::write(&path, bytes).map_err(|err| format!("写入内容来源失败：{err}"))
}

/// 删除内容后同步清理来源记录。
pub fn forget_tracked_content(instance: &str, folder: &str, name: &str) {
    let root = Path::new(instance);
    let mut metadata = read_content_metadata(root);
    if metadata.untrack(folder, name) {
        let _ = write_content_metadata(root, &metadata);
    }
}

#[derive(Debug, Clone, Default)]
pub struct InstanceContent {
    pub mods: Vec<ContentFile>,
    pub resource_packs: Vec<ContentFile>,
    pub shader_packs: Vec<ContentFile>,
}

/// 实例当前使用的模组加载器信息（从实例 JSON / `neutrino.json` 读取）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceLoaderInfo {
    pub game_version: String,
    /// `fabric` / `forge` / `neoforge` / `quilt`；空字符串表示原版。
    pub loader: String,
    pub loader_name: String,
    pub loader_version: String,
}

impl Default for InstanceLoaderInfo {
    fn default() -> Self {
        Self {
            game_version: String::new(),
            loader: String::new(),
            loader_name: "原版".to_string(),
            loader_version: String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct InstanceMetadataFile {
    #[serde(default)]
    game_version: String,
    #[serde(default)]
    loader: String,
    #[serde(default)]
    loader_version: String,
}

/// 读取实例的加载器类型和版本。
///
/// 新实例的安装过程会写 `neutrino.json`；旧实例没有这个文件时，从版本 JSON 的
/// 依赖库坐标推断：Fabric 看 `net.fabricmc:fabric-loader` + `net.fabricmc:intermediary`，
/// Forge 看 `net.minecraftforge:forge:<mc>-<loader>`。
pub fn detect_instance_loader(instance_path: &str) -> InstanceLoaderInfo {
    let root = Path::new(instance_path);
    let Some(name) = root.file_name().and_then(|name| name.to_str()) else {
        return InstanceLoaderInfo::default();
    };

    let json_path = root.join(format!("{name}.json"));
    let Ok(text) = std::fs::read_to_string(&json_path) else {
        return InstanceLoaderInfo::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return InstanceLoaderInfo::default();
    };

    if let Ok(meta_text) = std::fs::read_to_string(root.join("neutrino.json"))
        && let Ok(meta) = serde_json::from_str::<InstanceMetadataFile>(&meta_text)
        && (!meta.loader.is_empty() || !meta.game_version.is_empty())
    {
        return loader_info(&meta.game_version, &meta.loader, &meta.loader_version);
    }

    detect_loader_from_libraries(&value)
}

fn detect_loader_from_libraries(value: &serde_json::Value) -> InstanceLoaderInfo {
    let Some(libraries) = value.get("libraries").and_then(|value| value.as_array()) else {
        return InstanceLoaderInfo::default();
    };
    let names = libraries
        .iter()
        .filter_map(|library| library.get("name").and_then(|name| name.as_str()))
        .collect::<Vec<_>>();

    if let Some(loader_version) = names
        .iter()
        .find_map(|name| name.strip_prefix("net.fabricmc:fabric-loader:"))
    {
        let game_version = names
            .iter()
            .find_map(|name| name.strip_prefix("net.fabricmc:intermediary:"))
            .map(maven_version)
            .unwrap_or_default();
        return loader_info(game_version, "fabric", maven_version(loader_version));
    }

    if let Some(version) = names
        .iter()
        .find_map(|name| name.strip_prefix("net.minecraftforge:forge:"))
    {
        let version = maven_version(version);
        let (game_version, loader_version) = version.split_once('-').unwrap_or(("", version));
        return loader_info(game_version, "forge", loader_version);
    }

    if let Some(version) = names
        .iter()
        .find_map(|name| name.strip_prefix("net.neoforged:forge:"))
    {
        let version = maven_version(version);
        let (game_version, loader_version) = version.split_once('-').unwrap_or(("", version));
        return loader_info(game_version, "neoforge", loader_version);
    }

    if let Some(loader_version) = names
        .iter()
        .find_map(|name| name.strip_prefix("net.neoforged:neoforge:"))
    {
        return loader_info("", "neoforge", maven_version(loader_version));
    }

    if let Some(loader_version) = names
        .iter()
        .find_map(|name| name.strip_prefix("org.quiltmc:quilt-loader:"))
    {
        let game_version = names
            .iter()
            .find_map(|name| name.strip_prefix("org.quiltmc:quilted:"))
            .or_else(|| {
                names
                    .iter()
                    .find_map(|name| name.strip_prefix("net.fabricmc:intermediary:"))
            })
            .map(maven_version)
            .unwrap_or_default();
        return loader_info(game_version, "quilt", maven_version(loader_version));
    }

    InstanceLoaderInfo::default()
}

/// 从 Maven 坐标的“版本:classifier”里取出版本部分。
fn maven_version(value: &str) -> &str {
    value.split([':', '@']).next().unwrap_or(value)
}

fn loader_info(game_version: &str, loader: &str, loader_version: &str) -> InstanceLoaderInfo {
    let loader = loader.to_ascii_lowercase();
    InstanceLoaderInfo {
        game_version: game_version.to_string(),
        loader_name: loader_display_name(&loader),
        loader,
        loader_version: loader_version.to_string(),
    }
}

fn loader_display_name(loader: &str) -> String {
    match loader {
        "" => "原版".to_string(),
        "fabric" => "Fabric".to_string(),
        "forge" => "Forge".to_string(),
        "neoforge" => "NeoForge".to_string(),
        "quilt" => "Quilt".to_string(),
        other => other.to_string(),
    }
}

pub fn detect_instances(game_dir: &str) -> Vec<InstanceFolder> {
    let versions_dir = Path::new(game_dir).join("versions");
    let Ok(entries) = std::fs::read_dir(versions_dir) else {
        return Vec::new();
    };

    let mut instances = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }

            let name = entry.file_name().to_string_lossy().into_owned();
            // 只把带 `<目录名>.json` 的目录当作实例；下载失败留下的半成品目录不显示。
            if !path.join(format!("{name}.json")).is_file() {
                return None;
            }

            Some(InstanceFolder {
                name,
                path: path.display().to_string(),
            })
        })
        .collect::<Vec<_>>();

    instances.sort_by_key(|a| a.name.to_lowercase());
    instances
}

pub fn detect_instance_content(instance_path: &str) -> InstanceContent {
    let root = Path::new(instance_path);
    InstanceContent {
        mods: scan_folder(&root.join("mods"), &["jar"]),
        resource_packs: scan_folder(&root.join("resourcepacks"), &["zip"]),
        shader_packs: scan_folder(&root.join("shaderpacks"), &["zip"]),
    }
}

/// 启用 / 禁用实例内容（模组、资源包、光影）。
///
/// Minecraft 和主流启动器用 `.disabled` 后缀表示禁用，所以这里只做重命名；
/// 返回操作后的启用状态。文件已经不在时返回 `NotFound`。
pub fn toggle_content(instance_path: &str, folder: &str, name: &str) -> std::io::Result<bool> {
    let enabled_path = content_path(instance_path, folder, name);
    let disabled_path = with_disabled_suffix(&enabled_path);

    if enabled_path.exists() {
        std::fs::rename(&enabled_path, &disabled_path)?;
        Ok(false)
    } else if disabled_path.exists() {
        std::fs::rename(&disabled_path, &enabled_path)?;
        Ok(true)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("内容不存在：{}", enabled_path.display()),
        ))
    }
}

/// 删除实例内容（启用中和已禁用的都能删）；返回是否真的删掉了东西。
pub fn delete_content(instance_path: &str, folder: &str, name: &str) -> std::io::Result<bool> {
    let enabled_path = content_path(instance_path, folder, name);
    let disabled_path = with_disabled_suffix(&enabled_path);

    if enabled_path.exists() {
        remove_path(&enabled_path)?;
        Ok(true)
    } else if disabled_path.exists() {
        remove_path(&disabled_path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn content_path(instance_path: &str, folder: &str, name: &str) -> PathBuf {
    Path::new(instance_path).join(folder).join(name)
}

fn with_disabled_suffix(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".disabled");
    PathBuf::from(name)
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// 从 jar / 资源包里读出来的附加信息。
#[derive(Debug, Default)]
pub struct ArchiveInfo {
    /// 内嵌图标（原始字节）。
    pub icon: Option<Vec<u8>>,
    /// 模组版本（从 `fabric.mod.json` / `mods.toml` 读取）。
    pub version: String,
    /// 模组加载器：Fabric / Quilt / Forge / NeoForge。
    pub loader: String,
}

/// 从 mod jar / 资源包 zip / 资源包文件夹里读取图标、模组版本和加载器。
///
/// 图标优先 `pack.png`（资源包），其次根目录 `icon.png` / `logo.png`，
/// 最后 `assets/<namespace>/icon.png`（Fabric / Forge 模组常见布局）。
///
/// 只遍历一次中央目录里的文件名来挑图标，元数据文件按固定名字精确读取，
/// 避免对每个 jar 逐条目解压（这是实例页点击卡顿的主要来源之一）。
pub fn read_archive_info(path: &Path) -> ArchiveInfo {
    let mut info = ArchiveInfo::default();

    if path.is_dir() {
        for name in [
            "pack.png",
            "pack.webp",
            "icon.png",
            "icon.webp",
            "logo.png",
            "logo.webp",
        ] {
            if let Ok(bytes) = std::fs::read(path.join(name)) {
                info.icon = Some(bytes);
                break;
            }
        }
        return info;
    }

    let Ok(file) = std::fs::File::open(path) else {
        return info;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return info;
    };

    let fabric = read_named_entry(&mut archive, &["fabric.mod.json"]);
    let quilt = read_named_entry(&mut archive, &["quilt.mod.json"]);
    let neoforge = read_named_entry(&mut archive, &["META-INF/neoforge.mods.toml"]);
    let forge = read_named_entry(&mut archive, &["META-INF/mods.toml"]);

    if let Some(bytes) = &fabric {
        info.loader = "Fabric".to_string();
        info.version = json_value(bytes, &["version"]).unwrap_or_default();
    } else if let Some(bytes) = &quilt {
        info.loader = "Quilt".to_string();
        info.version = json_value(bytes, &["quilt_loader", "version"])
            .or_else(|| json_value(bytes, &["version"]))
            .unwrap_or_default();
    } else if let Some(bytes) = &neoforge {
        info.loader = "NeoForge".to_string();
        info.version = toml_mod_version(bytes);
    } else if let Some(bytes) = &forge {
        info.loader = "Forge".to_string();
        info.version = toml_mod_version(bytes);
    }

    // 元数据里声明的图标最准确（Fabric 的 icon / Forge 的 logoFile），
    // 没有时再按常见文件名在整个压缩包里找。
    let metadata_icon = fabric
        .as_deref()
        .and_then(|bytes| json_icon_path(bytes, &["icon"]))
        .or_else(|| {
            quilt
                .as_deref()
                .and_then(|bytes| json_icon_path(bytes, &["quilt_loader", "metadata", "icon"]))
        })
        .or_else(|| {
            quilt
                .as_deref()
                .and_then(|bytes| json_icon_path(bytes, &["icon"]))
        })
        .or_else(|| neoforge.as_deref().and_then(toml_logo_file))
        .or_else(|| forge.as_deref().and_then(toml_logo_file))
        .and_then(|path| normalize_icon_path(&path));

    let icon_name = metadata_icon
        .filter(|name| archive.by_name(name).is_ok())
        .or_else(|| icon_entry_name(&mut archive));

    if let Some(name) = icon_name {
        info.icon = archive
            .by_name(&name)
            .ok()
            .and_then(|mut entry| read_entry(&mut entry));
    }

    info
}

/// 在中央目录里挑一个优先级最高的图标条目（名字，不解压）。
///
/// 同优先级下取体积最大的，避免把 `config-icon.png` 之类的小图当成模组图标。
fn icon_entry_name<R: Read + Seek>(archive: &mut zip::ZipArchive<R>) -> Option<String> {
    let mut best: Option<(usize, u64, String)> = None;

    for index in 0..archive.len() {
        let Ok(file) = archive.by_index_raw(index) else {
            continue;
        };
        if file.is_dir() {
            continue;
        }
        let Ok(name) = file.name() else {
            continue;
        };
        let Some(priority) = icon_priority(&name) else {
            continue;
        };
        let size = file.size();
        let better = match &best {
            None => true,
            Some((best_priority, best_size, _)) => {
                priority < *best_priority || (priority == *best_priority && size > *best_size)
            }
        };
        if better {
            best = Some((priority, size, name.to_string()));
        }
    }

    best.map(|(_, _, name)| name)
}

/// 按名字精确读取条目内容（zip 里的名字区分大小写，所以要给出候选列表）。
fn read_named_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    names: &[&str],
) -> Option<Vec<u8>> {
    for name in names {
        if let Ok(mut entry) = archive.by_name(name) {
            return read_entry(&mut entry);
        }
    }
    None
}

fn read_entry(reader: &mut impl Read) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn json_value(bytes: &[u8], path: &[&str]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let mut current = &value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(str::to_string)
}

/// 读取 `fabric.mod.json` / `quilt.mod.json` 里的图标路径。
///
/// Fabric 的 `icon` 可以是字符串，也可以是 `{ "128": "icon_128.png" }` 这样的尺寸表。
fn json_icon_path(bytes: &[u8], path: &[&str]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let mut current = &value;
    for key in path {
        current = current.get(*key)?;
    }

    match current {
        serde_json::Value::String(path) => Some(path.clone()),
        serde_json::Value::Object(sizes) => sizes
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_str()
                    .map(|path| (key.parse::<u64>().unwrap_or(0), path))
            })
            .max_by_key(|(size, _)| *size)
            .map(|(_, path)| path.to_string()),
        _ => None,
    }
}

/// 从 Forge / NeoForge 的 `mods.toml` 里读取 `logoFile`。
fn toml_logo_file(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("logoFile") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = unquote(rest.trim());
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
    }

    None
}

/// 规范化元数据里的 zip 路径，并拒绝跳出压缩包的路径。
fn normalize_icon_path(path: &str) -> Option<String> {
    let path = path.trim().trim_start_matches('/').replace('\\', "/");
    if path.is_empty() || path.starts_with("../") || path.contains("/../") {
        return None;
    }
    Some(path)
}

/// 从 `META-INF/mods.toml` 的 `[[mods]]` 段里取 `version`。
fn toml_mod_version(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut in_mods = false;

    for line in text.lines() {
        let line = line.trim();
        if line == "[[mods]]" {
            in_mods = true;
            continue;
        }
        if !in_mods {
            continue;
        }
        if let Some(rest) = line.strip_prefix("version") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = unquote(rest.trim());
                // Forge 模板里可能出现 `${file.jarVersion}`，这种运行时才解析。
                if !value.is_empty() && !value.contains("${") {
                    return value;
                }
            }
        }
    }

    String::new()
}

fn unquote(value: &str) -> String {
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        return inner.to_string();
    }
    if let Some(inner) = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
    {
        return inner.to_string();
    }
    value.to_string()
}

fn icon_priority(name: &str) -> Option<usize> {
    const IMAGE_EXTENSIONS: &[&str] = &["png", "webp", "jpg", "jpeg", "gif"];

    let lower = name.replace('\\', "/").to_ascii_lowercase();
    let (dir, file) = match lower.rsplit_once('/') {
        Some((dir, file)) => (Some(dir), file),
        None => (None, lower.as_str()),
    };
    let (stem, extension) = file.rsplit_once('.')?;
    if !IMAGE_EXTENSIONS.contains(&extension) {
        return None;
    }

    // 有些光影 / 资源包会多套一层文件夹，`pack.png` / `icon.png` 不在压缩包根目录。
    let depth = dir.map_or(0, |dir| dir.matches('/').count() + 1);

    if stem == "pack" {
        return Some(0);
    }
    if depth <= 1 && (stem == "icon" || stem == "logo") {
        return Some(1);
    }
    if depth <= 1
        && (stem.ends_with("-icon")
            || stem.ends_with("_icon")
            || stem.ends_with("-logo")
            || stem.ends_with("_logo"))
    {
        return Some(2);
    }

    if let Some(dir) = dir
        && let Some(namespace) = dir.strip_prefix("assets/")
        && (stem == "icon" || stem == "logo")
        && (!namespace.contains('/') || stem == "logo")
    {
        return Some(3);
    }

    None
}

fn scan_folder(dir: &Path, extensions: &[&str]) -> Vec<ContentFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let is_dir = path.is_dir();
            let file_name = entry.file_name().to_string_lossy().into_owned();

            if file_name.starts_with('.') {
                return None;
            }

            let (name, enabled) = match file_name.strip_suffix(".disabled") {
                Some(name) => (name.to_string(), false),
                None => (file_name, true),
            };

            if !is_dir {
                let extension = Path::new(&name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default();
                let supported = extensions
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(extension));
                if !supported {
                    return None;
                }
            }

            let size = if is_dir {
                folder_size(&path)
            } else {
                entry.metadata().map(|metadata| metadata.len()).unwrap_or(0)
            };

            Some(ContentFile {
                name,
                size,
                enabled,
            })
        })
        .collect::<Vec<_>>();

    files.sort_by(|a, b| {
        b.enabled
            .cmp(&a.enabled)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    files
}

fn folder_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };

    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                folder_size(&path)
            } else {
                entry.metadata().map(|metadata| metadata.len()).unwrap_or(0)
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "neutrino-instances-test-{label}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_file(path: &Path, contents: &[u8]) {
        fs::create_dir_all(path.parent().expect("file has parent")).expect("create parent dir");
        fs::write(path, contents).expect("write file");
    }

    #[test]
    fn detects_content_and_marks_disabled_files() {
        let temp = TempDir::new("content");
        let instance = temp.path().join("26.1-mc");

        write_file(&instance.join("mods").join("fabric-api.jar"), b"1234");
        write_file(&instance.join("mods").join("old-mod.jar.disabled"), b"12");
        write_file(&instance.join("mods").join("notes.txt"), b"ignored");
        write_file(
            &instance.join("resourcepacks").join("vanilla-tweaks.zip"),
            b"1",
        );
        write_file(
            &instance
                .join("resourcepacks")
                .join("folder-pack")
                .join("pack.mcmeta"),
            b"{}",
        );
        write_file(
            &instance
                .join("shaderpacks")
                .join("complementary.zip.disabled"),
            b"123",
        );

        let content = detect_instance_content(&instance.display().to_string());

        assert_eq!(
            content.mods,
            vec![
                ContentFile {
                    name: "fabric-api.jar".into(),
                    size: 4,
                    enabled: true,
                },
                ContentFile {
                    name: "old-mod.jar".into(),
                    size: 2,
                    enabled: false,
                },
            ]
        );

        let resource_packs = content
            .resource_packs
            .iter()
            .map(|item| (item.name.as_str(), item.enabled))
            .collect::<Vec<_>>();
        assert_eq!(
            resource_packs,
            vec![("folder-pack", true), ("vanilla-tweaks.zip", true)]
        );
        assert_eq!(content.resource_packs[0].size, 2);

        assert_eq!(
            content.shader_packs,
            vec![ContentFile {
                name: "complementary.zip".into(),
                size: 3,
                enabled: false,
            }]
        );
    }

    #[test]
    fn toggles_content_by_adding_and_removing_disabled_suffix() {
        let temp = TempDir::new("toggle");
        let instance = temp.path().join("26.1-mc");
        let instance_path = instance.display().to_string();
        let mod_path = instance.join("mods").join("fabric-api.jar");
        write_file(&mod_path, b"1234");

        assert!(!toggle_content(&instance_path, "mods", "fabric-api.jar").unwrap());
        assert!(!mod_path.exists());
        assert!(
            instance
                .join("mods")
                .join("fabric-api.jar.disabled")
                .exists()
        );

        let content = detect_instance_content(&instance_path);
        assert_eq!(
            content.mods,
            vec![ContentFile {
                name: "fabric-api.jar".into(),
                size: 4,
                enabled: false,
            }]
        );

        assert!(toggle_content(&instance_path, "mods", "fabric-api.jar").unwrap());
        assert!(mod_path.exists());

        let error = toggle_content(&instance_path, "mods", "missing.jar").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn deletes_enabled_and_disabled_content() {
        let temp = TempDir::new("delete");
        let instance = temp.path().join("26.1-mc");
        let instance_path = instance.display().to_string();

        let enabled_mod = instance.join("mods").join("enabled.jar");
        let disabled_mod = instance.join("mods").join("disabled.jar.disabled");
        let disabled_pack = instance.join("resourcepacks").join("pack.zip.disabled");
        let disabled_folder = instance.join("resourcepacks").join("folder-pack.disabled");
        write_file(&enabled_mod, b"1");
        write_file(&disabled_mod, b"2");
        write_file(&disabled_pack, b"3");
        write_file(&disabled_folder.join("pack.mcmeta"), b"{}");

        assert!(delete_content(&instance_path, "mods", "enabled.jar").unwrap());
        assert!(!enabled_mod.exists());
        assert!(delete_content(&instance_path, "mods", "disabled.jar").unwrap());
        assert!(!disabled_mod.exists());
        assert!(delete_content(&instance_path, "resourcepacks", "pack.zip").unwrap());
        assert!(!disabled_pack.exists());
        assert!(delete_content(&instance_path, "resourcepacks", "folder-pack").unwrap());
        assert!(!disabled_folder.exists());

        assert!(!delete_content(&instance_path, "mods", "missing.jar").unwrap());
    }

    #[test]
    fn marks_disabled_folders() {
        let temp = TempDir::new("disabled-folder");
        let instance = temp.path().join("26.1-mc");
        write_file(
            &instance
                .join("resourcepacks")
                .join("old-pack.disabled")
                .join("pack.mcmeta"),
            b"{}",
        );

        let content = detect_instance_content(&instance.display().to_string());
        assert_eq!(
            content.resource_packs,
            vec![ContentFile {
                name: "old-pack".into(),
                size: 2,
                enabled: false,
            }]
        );
    }

    #[test]
    fn detects_fabric_and_forge_loaders() {
        let temp = TempDir::new("loader-info");
        let fabric = temp.path().join("1.21.1-fabric");
        write_file(
            &fabric.join("1.21.1-fabric.json"),
            br#"{
              "libraries": [
                { "name": "net.fabricmc:fabric-loader:0.16.9" },
                { "name": "net.fabricmc:intermediary:1.21.1" }
              ]
            }"#,
        );

        let info = detect_instance_loader(&fabric.display().to_string());
        assert_eq!(info.loader, "fabric");
        assert_eq!(info.loader_name, "Fabric");
        assert_eq!(info.game_version, "1.21.1");
        assert_eq!(info.loader_version, "0.16.9");

        let forge = temp.path().join("1.20.1-forge");
        write_file(
            &forge.join("1.20.1-forge.json"),
            br#"{
              "libraries": [
                { "name": "net.minecraftforge:forge:1.20.1-47.4.28:universal" }
              ]
            }"#,
        );

        let info = detect_instance_loader(&forge.display().to_string());
        assert_eq!(info.loader, "forge");
        assert_eq!(info.loader_name, "Forge");
        assert_eq!(info.game_version, "1.20.1");
        assert_eq!(info.loader_version, "47.4.28");
    }

    #[test]
    fn prefers_instance_metadata_file() {
        let temp = TempDir::new("loader-meta");
        let instance = temp.path().join("custom-name");
        write_file(
            &instance.join("custom-name.json"),
            br#"{ "libraries": [] }"#,
        );
        write_file(
            &instance.join("neutrino.json"),
            br#"{ "game_version": "1.20.4", "loader": "forge", "loader_version": "49.0.30" }"#,
        );

        let info = detect_instance_loader(&instance.display().to_string());
        assert_eq!(info.loader, "forge");
        assert_eq!(info.loader_name, "Forge");
        assert_eq!(info.game_version, "1.20.4");
        assert_eq!(info.loader_version, "49.0.30");
    }

    #[test]
    fn vanilla_instance_has_no_loader() {
        let temp = TempDir::new("loader-vanilla");
        let instance = temp.path().join("vanilla");
        write_file(&instance.join("vanilla.json"), br#"{ "libraries": [] }"#);

        assert_eq!(
            detect_instance_loader(&instance.display().to_string()),
            InstanceLoaderInfo::default()
        );
    }

    #[test]
    fn ignores_unsupported_and_hidden_files() {
        let temp = TempDir::new("filters");
        let instance = temp.path().join("26.1-mc");

        write_file(&instance.join("mods").join(".hidden.jar"), b"1");
        write_file(&instance.join("mods").join("notes.txt"), b"1");
        write_file(&instance.join("mods").join("valid.JAR"), b"1");

        let content = detect_instance_content(&instance.display().to_string());

        assert_eq!(
            content.mods,
            vec![ContentFile {
                name: "valid.JAR".into(),
                size: 1,
                enabled: true,
            }]
        );
    }

    #[test]
    fn returns_empty_content_when_folders_are_missing() {
        let temp = TempDir::new("empty");
        let content = detect_instance_content(&temp.path().display().to_string());

        assert!(content.mods.is_empty());
        assert!(content.resource_packs.is_empty());
        assert!(content.shader_packs.is_empty());
    }

    #[test]
    fn reads_pack_icon_from_resource_pack_folder() {
        let temp = TempDir::new("pack-icon");
        let pack = temp.path().join("MyPack");
        fs::create_dir_all(&pack).unwrap();
        fs::write(pack.join("pack.png"), b"icon-bytes").unwrap();

        assert_eq!(read_archive_info(&pack).icon, Some(b"icon-bytes".to_vec()));
        assert!(
            read_archive_info(&temp.path().join("missing"))
                .icon
                .is_none()
        );
    }

    #[test]
    fn parses_forge_mod_version() {
        let toml = br#"modLoader="javafml"
loaderVersion="[47,)"
license="MIT"
[[mods]]
modId="examplemod"
version="1.2.3"
displayName="Example"
"#;
        assert_eq!(toml_mod_version(toml), "1.2.3");

        let templated = b"[[mods]]\nversion=\"${file.jarVersion}\"\n";
        assert_eq!(toml_mod_version(templated), "");
    }

    #[test]
    fn parses_fabric_and_quilt_versions() {
        let fabric = br#"{"id":"fabric-api","version":"0.120.0+1.21.8"}"#;
        assert_eq!(
            json_value(fabric, &["version"]).as_deref(),
            Some("0.120.0+1.21.8")
        );

        let quilt = br#"{"quilt_loader":{"version":"0.5.0"}}"#;
        assert_eq!(
            json_value(quilt, &["quilt_loader", "version"]).as_deref(),
            Some("0.5.0")
        );
    }

    #[test]
    fn only_directories_with_version_json_are_instances() {
        let temp = TempDir::new("instances");
        let versions = temp.path().join("versions");
        fs::create_dir_all(versions.join("real")).unwrap();
        fs::write(versions.join("real/real.json"), b"{}").unwrap();
        fs::create_dir_all(versions.join("partial")).unwrap();
        fs::write(versions.join("partial/mods.jar"), b"x").unwrap();

        let found = detect_instances(&temp.path().display().to_string());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "real");
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        use std::io::Write;

        let file = fs::File::create(path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        for (name, bytes) in entries {
            writer.start_file(*name, options).expect("start entry");
            writer.write_all(bytes).expect("write entry");
        }
        writer.finish().expect("finish zip");
    }

    #[test]
    fn reads_fabric_metadata_and_nested_icon_from_zip() {
        let temp = TempDir::new("zip-fabric");
        let jar = temp.path().join("example.jar");
        write_zip(
            &jar,
            &[
                ("assets/example/icon.png", b"assets-icon"),
                ("fabric.mod.json", br#"{"id":"example","version":"2.1.0"}"#),
            ],
        );

        let info = read_archive_info(&jar);

        assert_eq!(info.loader, "Fabric");
        assert_eq!(info.version, "2.1.0");
        assert_eq!(info.icon.as_deref(), Some(b"assets-icon".as_slice()));
    }

    #[test]
    fn prefers_root_icon_and_reads_forge_metadata_from_zip() {
        let temp = TempDir::new("zip-forge");
        let jar = temp.path().join("example.jar");
        write_zip(
            &jar,
            &[
                ("assets/example/icon.png", b"assets-icon"),
                ("logo.png", b"logo-icon"),
                (
                    "META-INF/mods.toml",
                    b"modLoader=\"javafml\"\n[[mods]]\nversion=\"1.2.3\"\n",
                ),
            ],
        );

        let info = read_archive_info(&jar);

        assert_eq!(info.loader, "Forge");
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.icon.as_deref(), Some(b"logo-icon".as_slice()));
    }

    #[test]
    fn icon_names_cover_common_layouts() {
        assert_eq!(icon_priority("pack.png"), Some(0));
        assert_eq!(icon_priority("pack.webp"), Some(0));
        assert_eq!(icon_priority("icon.png"), Some(1));
        assert_eq!(icon_priority("sodium-icon.png"), Some(2));
        assert_eq!(icon_priority("config-icon.webp"), Some(2));
        assert_eq!(icon_priority("assets/create/logo.jpg"), Some(3));
        assert_eq!(icon_priority("assets/create/icon.gif"), Some(3));
        assert_eq!(
            icon_priority("assets/create/textures/gui/logo.png"),
            Some(3)
        );
        assert_eq!(icon_priority("assets/create/textures/item/icon.png"), None);
        assert_eq!(icon_priority("Complementary/pack.png"), Some(0));
        assert_eq!(icon_priority("Complementary/icon.webp"), Some(1));
        assert_eq!(icon_priority("readme.txt"), None);
    }

    #[test]
    fn reads_prefixed_root_icon_by_size() {
        let temp = TempDir::new("zip-prefixed-icon");
        let jar = temp.path().join("sodium.jar");
        write_zip(
            &jar,
            &[
                ("config-icon.png", b"small"),
                ("sodium-icon.png", b"the-largest-icon"),
                ("fabric.mod.json", br#"{"id":"sodium","version":"0.6.9"}"#),
            ],
        );

        let info = read_archive_info(&jar);

        assert_eq!(info.loader, "Fabric");
        assert_eq!(info.version, "0.6.9");
        assert_eq!(info.icon.as_deref(), Some(b"the-largest-icon".as_slice()));
    }

    #[test]
    fn prefers_icon_declared_in_fabric_metadata() {
        let temp = TempDir::new("zip-fabric-icon");
        let jar = temp.path().join("example.jar");
        write_zip(
            &jar,
            &[
                ("assets/example/icon.png", b"fallback-icon"),
                ("assets/example/textures/custom.webp", b"metadata-icon"),
                (
                    "fabric.mod.json",
                    br#"{"id":"example","version":"1.0","icon":"assets/example/textures/custom.webp"}"#,
                ),
            ],
        );

        let info = read_archive_info(&jar);

        assert_eq!(info.icon.as_deref(), Some(b"metadata-icon".as_slice()));
    }

    #[test]
    fn reads_logo_file_from_forge_metadata() {
        let temp = TempDir::new("zip-forge-logo");
        let jar = temp.path().join("example.jar");
        write_zip(
            &jar,
            &[
                ("assets/example/icon.png", b"fallback-icon"),
                ("assets/example/branding.png", b"forge-logo"),
                (
                    "META-INF/mods.toml",
                    b"[[mods]]\nversion=\"1.0\"\nlogoFile=\"assets/example/branding.png\"\n",
                ),
            ],
        );

        let info = read_archive_info(&jar);

        assert_eq!(info.icon.as_deref(), Some(b"forge-logo".as_slice()));
    }

    #[test]
    fn tracks_and_forgets_downloaded_content_metadata() {
        let temp = TempDir::new("content-metadata");
        let root = temp.path();
        let mut metadata = ContentMetadata::default();
        metadata.track(
            "mods",
            "example.jar",
            TrackedContent {
                source: "modrinth".into(),
                project_id: "abc".into(),
                version_id: "v1".into(),
                version: "1.0.0".into(),
                file_name: "example.jar".into(),
            },
        );
        write_content_metadata(root, &metadata).unwrap();

        let loaded = read_content_metadata(root);
        let tracked = loaded
            .entries
            .get("mods")
            .and_then(|entries| entries.get("example.jar"))
            .unwrap();
        assert_eq!(tracked.project_id, "abc");
        assert_eq!(tracked.version, "1.0.0");

        forget_tracked_content(&root.display().to_string(), "mods", "example.jar");
        assert!(
            read_content_metadata(root)
                .entries
                .get("mods")
                .and_then(|entries| entries.get("example.jar"))
                .is_none()
        );
        assert!(!content_metadata_path(root).exists());
    }
}
