//! 版本安装主流程：解析清单 → 解析依赖 → 下载校验 → 解压 natives → 落盘。
//!
//! 一组约定：
//! - 所有路径都由 [`GamePaths`] 推导，和官方启动器目录结构完全一致；
//! - 写文件一律“下载到 `.part` → 校验 SHA-1 → rename”，中途失败不会留下半截文件；
//! - 已存在且校验通过的文件直接跳过，所以重复执行是幂等的（断点续传的效果）。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::error::{CoreError, Result, io_err};
use super::loader::{self, LoaderKind};
use super::manifest::{
    AssetIndex, AssetIndexRef, MavenCoordinate, VersionJson, VersionManifest, maven_url,
    natives_classifier, rules_allow,
};
use super::natives;
use super::net::{DownloadJob, DownloadStats, Downloader};
use super::paths::GamePaths;
use super::progress::Progress;
use super::{DEFAULT_MANIFEST_URL, LIBRARIES_BASE_URL, RESOURCES_BASE_URL};

/// 安装参数。
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// `.minecraft` 根目录。
    pub game_dir: PathBuf,
    /// 要安装的版本 id，例如 `1.20.4`。
    pub version: String,
    /// 版本清单地址（换镜像时改这里）。
    pub manifest_url: String,
    /// 资源文件下载根地址。
    pub resources_base: String,
    /// 依赖库 Maven 根地址。
    pub libraries_base: String,
    /// 是否下载资源文件（assets，几百 MB；关掉可以只装“能启动的最小集合”）。
    pub assets: bool,
    /// 是否为 1.6 及更早的版本建立 virtual 资源目录。
    pub virtual_assets: bool,
    /// 是否解压旧版本的 natives。
    pub extract_natives: bool,
    /// 并发下载线程数。
    pub threads: usize,
    /// 单个文件失败重试次数。
    pub retries: u32,
    /// 只统计不下盘（用于预演体积）。
    pub dry_run: bool,
}

impl InstallOptions {
    /// 用默认的 Mojang 官方源创建参数。
    pub fn new(game_dir: impl Into<PathBuf>, version: impl Into<String>) -> Self {
        Self {
            game_dir: game_dir.into(),
            version: version.into(),
            manifest_url: DEFAULT_MANIFEST_URL.to_string(),
            resources_base: RESOURCES_BASE_URL.to_string(),
            libraries_base: LIBRARIES_BASE_URL.to_string(),
            assets: true,
            virtual_assets: false,
            extract_natives: true,
            threads: 8,
            retries: 3,
            dry_run: false,
        }
    }
}

/// 安装结果。
#[derive(Debug, Clone)]
pub struct InstallReport {
    pub version: String,
    pub game_dir: PathBuf,
    pub version_dir: PathBuf,
    pub client_jar: Option<PathBuf>,
    pub main_class: String,
    /// 需要下载的依赖库数量（不含 natives）。
    pub library_count: usize,
    /// 需要解压的 natives jar 数量。
    pub native_library_count: usize,
    /// 实际解压出的 natives 文件数量。
    pub native_file_count: usize,
    /// 资源文件数量。
    pub asset_count: usize,
    /// 依赖库计划下载字节数。
    pub library_bytes: u64,
    /// 资源文件计划下载字节数。
    pub asset_bytes: u64,
    pub library_stats: DownloadStats,
    /// 该版本要求的 Java 大版本。
    pub java_required: Option<u32>,
    /// 本机检测到的 Java 大版本。
    pub java_detected: Option<i32>,
    pub elapsed: Duration,
    pub dry_run: bool,
}

impl InstallReport {
    /// 本机 Java 是否满足该版本要求；信息不足时返回 `None`。
    pub fn java_ok(&self) -> Option<bool> {
        match (self.java_required, self.java_detected) {
            (Some(required), Some(detected)) => Some(detected >= required as i32),
            _ => None,
        }
    }

    /// 该版本目录下的 natives 目录。
    pub fn natives_dir(&self) -> PathBuf {
        self.version_dir.join("natives")
    }
}

/// 执行安装（原版，写到 `versions/<version>/`）。
pub fn install(options: &InstallOptions, progress: &Progress) -> Result<InstallReport> {
    let started = Instant::now();
    let paths = GamePaths::new(&options.game_dir);
    let downloader = Downloader::new(progress.clone()).with_retries(options.retries);

    // 1. 版本清单
    progress.begin_stage("获取版本清单", 0);
    let manifest: VersionManifest = downloader.get_json(&options.manifest_url, "版本清单")?;
    let summary = manifest
        .find(&options.version)
        .cloned()
        .ok_or_else(|| CoreError::UnknownVersion(options.version.clone()))?;
    progress.finish_stage();

    // 2. 版本 JSON（支持 inheritsFrom 继承链）
    progress.begin_stage("获取版本信息", 0);
    let fetched = resolve_version(&downloader, &manifest, &summary.url, 0)?;
    progress.finish_stage();

    install_resolved(
        &paths,
        &options.version,
        &fetched.json,
        &fetched.raw,
        &downloader,
        options,
        progress,
        started,
    )
}

/// 实例安装参数（GUI 的“添加实例”流程）。
#[derive(Debug, Clone)]
pub struct InstanceInstallOptions {
    pub game_dir: PathBuf,
    /// 实例名，也是 `versions/<instance>/` 的目录名。
    pub instance: String,
    /// 原版版本 id（例如 `1.20.4`）。
    pub version: String,
    /// 模组加载器；`None` 表示原版。
    pub loader: Option<LoaderKind>,
    /// 加载器版本（Fabric / Forge），跳过加载器时忽略。
    pub loader_version: Option<String>,
    /// 是否下载资源文件（assets）。
    pub assets: bool,
    pub threads: usize,
    pub retries: u32,
}

impl InstanceInstallOptions {
    pub fn new(
        game_dir: impl Into<PathBuf>,
        instance: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            game_dir: game_dir.into(),
            instance: instance.into(),
            version: version.into(),
            loader: None,
            loader_version: None,
            assets: true,
            threads: 8,
            retries: 3,
        }
    }

    fn install_options(&self) -> InstallOptions {
        let mut options = InstallOptions::new(&self.game_dir, &self.version);
        options.assets = self.assets;
        options.threads = self.threads;
        options.retries = self.retries;
        options
    }
}

/// 创建一个自包含的实例：`versions/<instance>/` 里放合并后的版本 JSON 和客户端 jar。
pub fn install_instance(
    options: &InstanceInstallOptions,
    progress: &Progress,
) -> Result<InstallReport> {
    let started = Instant::now();
    let paths = GamePaths::new(&options.game_dir);
    let downloader = Downloader::new(progress.clone()).with_retries(options.retries);
    let install_options = options.install_options();

    // 1. 原版版本 JSON
    progress.begin_stage("获取版本清单", 0);
    let manifest: VersionManifest =
        downloader.get_json(&install_options.manifest_url, "版本清单")?;
    let summary = manifest
        .find(&options.version)
        .cloned()
        .ok_or_else(|| CoreError::UnknownVersion(options.version.clone()))?;
    progress.finish_stage();

    progress.begin_stage("获取版本信息", 0);
    let vanilla = resolve_version(&downloader, &manifest, &summary.url, 0)?;
    progress.finish_stage();

    // 2. 加载器
    let loader_raw = match options.loader {
        None => None,
        Some(LoaderKind::Fabric) => {
            let version = options
                .loader_version
                .as_deref()
                .ok_or_else(|| CoreError::Other("没有选择 Fabric 版本".to_string()))?;
            progress.begin_stage("获取 Fabric 配置", 0);
            let raw = loader::fabric_profile(&downloader, &options.version, version)?;
            progress.finish_stage();
            Some(raw)
        }
        Some(LoaderKind::Forge) => {
            let version = options
                .loader_version
                .as_deref()
                .ok_or_else(|| CoreError::Other("没有选择 Forge 版本".to_string()))?;
            let java = crate::java::detect_java()
                .map(|info| info.executable())
                .ok_or_else(|| {
                    CoreError::Other("未检测到 Java，无法运行 Forge 安装器".to_string())
                })?;

            // Forge 安装器要求原版版本目录存在；装完再决定是否清掉。
            // 这里只装客户端和依赖库，assets 留到实例阶段再下，避免重复等待。
            let base_dir_existed = paths.version_dir(&options.version).exists();
            let mut base_options = install_options.clone();
            base_options.assets = false;
            install_resolved(
                &paths,
                &options.version,
                &vanilla.json,
                &vanilla.raw,
                &downloader,
                &base_options,
                progress,
                started,
            )?;

            // 原版客户端 jar 已经在缓存里，复制到实例目录，避免重复下载。
            copy_file_if_missing(
                &paths.client_jar(&options.version),
                &paths.client_jar(&options.instance),
            );

            progress.begin_stage("运行 Forge 安装器", 0);
            let forge = loader::install_forge(
                &downloader,
                &options.game_dir,
                &options.version,
                version,
                &java,
            );
            // 即使安装器失败，也把临时原版目录清掉（重试时会重新装）。
            if !base_dir_existed {
                let _ = fs::remove_dir_all(paths.version_dir(&options.version));
            }
            progress.finish_stage();
            let forge = forge?;

            // 只清理本次安装器新建的临时版本目录，别误删用户已有的 Forge 版本。
            if forge.created {
                loader::cleanup_generated(&options.game_dir, &forge.id, &options.version);
            }

            Some(forge.raw)
        }
    };

    // 3. 合并成自包含版本 JSON（去掉 inheritsFrom，id 改成实例名）
    let merged_raw = merge_instance_json(&vanilla.raw, loader_raw.as_deref(), &options.instance)?;
    let merged: VersionJson =
        serde_json::from_slice(&merged_raw).map_err(|source| CoreError::Json {
            context: "合并后的版本 JSON".to_string(),
            source,
        })?;

    // 4. 下载依赖库、资源并落盘
    let report = install_resolved(
        &paths,
        &options.instance,
        &merged,
        &merged_raw,
        &downloader,
        &install_options,
        progress,
        started,
    )?;

    // 5. 补齐实例常用的游戏目录（版本隔离后，游戏会直接读写这里）
    if !install_options.dry_run {
        create_instance_layout(&paths, &options.instance);
        write_instance_metadata(&paths, options)?;
    }

    Ok(report)
}

/// 写 `versions/<instance>/neutrino.json`，供实例详情页读取加载器信息。
fn write_instance_metadata(paths: &GamePaths, options: &InstanceInstallOptions) -> Result<()> {
    let metadata = serde_json::json!({
        "game_version": options.version,
        "loader": options.loader.map(LoaderKind::key).unwrap_or(""),
        "loader_version": options.loader_version.as_deref().unwrap_or(""),
    });
    let path = paths.version_dir(&options.instance).join("neutrino.json");
    let bytes = serde_json::to_vec_pretty(&metadata).map_err(|source| CoreError::Json {
        context: "实例元数据".to_string(),
        source,
    })?;
    fs::write(&path, bytes).map_err(|err| io_err(&path, err))
}

/// 把已解析的版本 JSON 安装到 `versions/<target_id>/`。
#[allow(clippy::too_many_arguments)]
fn install_resolved(
    paths: &GamePaths,
    target_id: &str,
    version: &VersionJson,
    raw: &[u8],
    downloader: &Downloader,
    options: &InstallOptions,
    progress: &Progress,
    started: Instant,
) -> Result<InstallReport> {
    if version.main_class.is_empty() {
        return Err(CoreError::MissingField("mainClass"));
    }

    let version_dir = paths.version_dir(target_id);
    let version_json_path = paths.version_json(target_id);

    // 4. 客户端 jar
    let client_size = version
        .downloads
        .client
        .as_ref()
        .map(|client| client.size.unwrap_or(0))
        .unwrap_or(0);
    progress.begin_stage("下载客户端", client_size);

    let mut client_jar = None;
    if let Some(client) = &version.downloads.client {
        let dest = paths.client_jar(target_id);
        if !options.dry_run {
            downloader.download(
                &client.url,
                &dest,
                client.sha1.as_deref(),
                client.size.unwrap_or(0),
            )?;
        }
        client_jar = Some(dest);
    }
    progress.finish_stage();

    // 5. 依赖库
    let mut library_jobs = Vec::new();
    let mut native_jars = Vec::new();
    collect_library_jobs(
        paths,
        version,
        &options.libraries_base,
        &mut library_jobs,
        &mut native_jars,
    );

    let library_bytes = DownloadJob::total_size(&library_jobs);
    progress.begin_stage("下载依赖库", library_bytes);
    let library_stats = if options.dry_run {
        DownloadStats::default()
    } else {
        downloader.download_all(&library_jobs, options.threads)?
    };
    progress.finish_stage();

    // 6. natives（仅 1.18 及更早版本）
    let mut native_file_count = 0;
    if options.extract_natives && !native_jars.is_empty() {
        // 进度按 jar 的字节数统计，和前面几个阶段的单位保持一致
        let natives_bytes: u64 = native_jars
            .iter()
            .filter_map(|(jar, _)| jar.metadata().ok())
            .map(|meta| meta.len())
            .sum();
        progress.begin_stage("解压 natives", natives_bytes);

        let natives_dir = paths.natives_dir(target_id);
        for (jar, excludes) in &native_jars {
            let size = jar.metadata().map(|meta| meta.len()).unwrap_or(0);
            if !options.dry_run && jar.is_file() {
                native_file_count += natives::extract_natives(jar, &natives_dir, excludes)?;
            }
            progress.advance(size);
        }
        progress.finish_stage();
    }

    // 7. 资源文件
    let mut asset_count = 0;
    let mut asset_bytes = 0;
    if options.assets
        && let Some(index_ref) = &version.asset_index
    {
        progress.begin_stage("下载资源索引", 0);
        let index = load_asset_index(downloader, paths, index_ref, options.dry_run)?;
        progress.finish_stage();

        let asset_jobs = build_asset_jobs(paths, &index, &options.resources_base);
        asset_count = asset_jobs.len();
        asset_bytes = DownloadJob::total_size(&asset_jobs);

        progress.begin_stage("下载资源文件", asset_bytes);
        if !options.dry_run {
            downloader.download_all(&asset_jobs, options.threads)?;
        }
        progress.finish_stage();

        if options.virtual_assets
            && (index.virtual_assets || index.map_to_resources)
            && !options.dry_run
        {
            copy_virtual_assets(paths, &index, &index_ref.id, progress)?;
        }
    }

    // 最后写版本 JSON：中途失败时不会留下一个“看起来已安装”的目录。
    if !options.dry_run {
        fs::create_dir_all(&version_dir).map_err(|err| io_err(&version_dir, err))?;
        fs::write(&version_json_path, raw).map_err(|err| io_err(&version_json_path, err))?;
    }

    let java_required = version
        .java_version
        .as_ref()
        .map(|java| java.major_version)
        .filter(|major| *major > 0);
    let java_detected = crate::java::detect_java().map(|info| info.major);

    Ok(InstallReport {
        version: target_id.to_string(),
        game_dir: paths.root().to_path_buf(),
        version_dir,
        client_jar,
        main_class: version.main_class.clone(),
        library_count: library_jobs.len(),
        native_library_count: native_jars.len(),
        native_file_count,
        asset_count,
        library_bytes,
        asset_bytes,
        library_stats,
        java_required,
        java_detected,
        elapsed: started.elapsed(),
        dry_run: options.dry_run,
    })
}

/// 复制文件（目标已存在或源不存在时跳过）。
fn copy_file_if_missing(source: &Path, dest: &Path) {
    if dest.is_file() || !source.is_file() {
        return;
    }
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::copy(source, dest);
}

/// 建好实例自己的游戏目录。启动时 `--gameDir` 和工作目录都指向这里，
/// 所以存档、配置、模组、资源包都是按实例隔离的。
fn create_instance_layout(paths: &GamePaths, instance: &str) {
    let dir = paths.version_dir(instance);
    for sub in [
        "mods",
        "resourcepacks",
        "shaderpacks",
        "saves",
        "config",
        "logs",
    ] {
        let _ = fs::create_dir_all(dir.join(sub));
    }
}

/// 把原版 JSON 和加载器 profile 合并成自包含实例 JSON。
fn merge_instance_json(
    vanilla_raw: &[u8],
    loader_raw: Option<&[u8]>,
    instance: &str,
) -> Result<Vec<u8>> {
    let mut merged: serde_json::Value =
        serde_json::from_slice(vanilla_raw).map_err(|source| CoreError::Json {
            context: "原版版本 JSON".to_string(),
            source,
        })?;

    if let Some(raw) = loader_raw {
        let loader: serde_json::Value =
            serde_json::from_slice(raw).map_err(|source| CoreError::Json {
                context: "加载器版本 JSON".to_string(),
                source,
            })?;
        merge_profile(&mut merged, &loader);
    }

    if let Some(object) = merged.as_object_mut() {
        object.insert(
            "id".to_string(),
            serde_json::Value::String(instance.to_string()),
        );
        object.remove("inheritsFrom");
    }

    serde_json::to_vec_pretty(&merged).map_err(|source| CoreError::Json {
        context: "合并后的版本 JSON".to_string(),
        source,
    })
}

/// 把加载器 profile 合并到原版 JSON 上（原版在前、加载器在后）。
fn merge_profile(parent: &mut serde_json::Value, child: &serde_json::Value) {
    let (Some(parent), Some(child)) = (parent.as_object_mut(), child.as_object()) else {
        return;
    };

    // libraries：原版在前，加载器在后（classpath 里后出现的优先）。
    if let Some(child_libraries) = child.get("libraries").and_then(|value| value.as_array()) {
        let libraries = parent
            .entry("libraries")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        if let Some(libraries) = libraries.as_array_mut() {
            libraries.extend(child_libraries.iter().cloned());
        }
    }

    // arguments：jvm / game 分别拼接。
    if let Some(child_arguments) = child.get("arguments").and_then(|value| value.as_object()) {
        let arguments = parent
            .entry("arguments")
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(arguments) = arguments.as_object_mut() {
            for key in ["jvm", "game"] {
                if let Some(values) = child_arguments.get(key).and_then(|value| value.as_array()) {
                    let target = arguments
                        .entry(key)
                        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
                    if let Some(target) = target.as_array_mut() {
                        target.extend(values.iter().cloned());
                    }
                }
            }
        }
    }

    // downloads：浅合并，保留原版的 client。
    if let Some(child_downloads) = child.get("downloads").and_then(|value| value.as_object()) {
        let downloads = parent
            .entry("downloads")
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(downloads) = downloads.as_object_mut() {
            for (key, value) in child_downloads {
                downloads.insert(key.clone(), value.clone());
            }
        }
    }

    // 其余字段：加载器有值就覆盖；空字符串和 null 不覆盖。
    for (key, value) in child {
        if matches!(
            key.as_str(),
            "id" | "inheritsFrom" | "libraries" | "arguments" | "downloads"
        ) {
            continue;
        }
        if value.is_null() || value.as_str().is_some_and(str::is_empty) {
            continue;
        }
        parent.insert(key.clone(), value.clone());
    }
}

/// 已下载但未合并的版本 JSON。
struct FetchedVersion {
    json: VersionJson,
    /// 原始字节，落盘时原样写出，避免丢失未建模的字段。
    raw: Vec<u8>,
}

fn fetch_version(downloader: &Downloader, url: &str) -> Result<FetchedVersion> {
    let raw = downloader.get_bytes(url)?;
    let json = serde_json::from_slice(&raw).map_err(|source| CoreError::Json {
        context: "版本信息 JSON".to_string(),
        source,
    })?;
    Ok(FetchedVersion { json, raw })
}

/// 解析版本 JSON，若有 `inheritsFrom` 则递归拉取父版本并合并（Forge / Fabric 依赖此机制）。
fn resolve_version(
    downloader: &Downloader,
    manifest: &VersionManifest,
    url: &str,
    depth: usize,
) -> Result<FetchedVersion> {
    if depth > 8 {
        return Err(CoreError::Other("版本继承链过深（超过 8 层）".to_string()));
    }

    let fetched = fetch_version(downloader, url)?;
    let Some(parent_id) = fetched.json.inherits_from.clone() else {
        return Ok(fetched);
    };

    let parent_summary = manifest
        .find(&parent_id)
        .cloned()
        .ok_or_else(|| CoreError::UnknownVersion(parent_id.clone()))?;
    let parent = resolve_version(downloader, manifest, &parent_summary.url, depth + 1)?;

    Ok(FetchedVersion {
        json: merge_with_parent(fetched.json, parent.json),
        raw: fetched.raw,
    })
}

/// 把父版本的信息补进子版本。
///
/// 只处理安装需要的字段；`arguments` 的深度合并留到启动器阶段再做。
fn merge_with_parent(mut child: VersionJson, parent: VersionJson) -> VersionJson {
    let mut libraries = parent.libraries;
    libraries.extend(std::mem::take(&mut child.libraries));
    child.libraries = libraries;

    if child.main_class.is_empty() {
        child.main_class = parent.main_class;
    }
    if child.assets.is_empty() {
        child.assets = parent.assets;
    }
    if child.asset_index.is_none() {
        child.asset_index = parent.asset_index;
    }
    if child.downloads.client.is_none() {
        child.downloads.client = parent.downloads.client;
    }
    if child.java_version.is_none() {
        child.java_version = parent.java_version;
    }
    if child.arguments.is_none() {
        child.arguments = parent.arguments;
    }
    if child.minecraft_arguments.is_none() {
        child.minecraft_arguments = parent.minecraft_arguments;
    }

    child
}

/// 把版本 JSON 里符合当前平台的依赖库翻译成下载任务。
fn collect_library_jobs(
    paths: &GamePaths,
    version: &VersionJson,
    libraries_base: &str,
    jobs: &mut Vec<DownloadJob>,
    native_jars: &mut Vec<(PathBuf, Vec<String>)>,
) {
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for library in &version.libraries {
        if !rules_allow(&library.rules) {
            continue;
        }

        match library.downloads.as_ref() {
            Some(downloads) => {
                if let Some(artifact) = &downloads.artifact {
                    let relative = artifact.path.clone().or_else(|| {
                        MavenCoordinate::parse(&library.name).map(|coord| coord.relative_path())
                    });

                    if let Some(relative) = relative {
                        let dest = paths.libraries_dir().join(relative);
                        if seen.insert(dest.clone()) {
                            jobs.push(DownloadJob::new(
                                &artifact.url,
                                dest,
                                artifact.sha1.clone(),
                                artifact.size.unwrap_or(0),
                            ));
                        }
                    }
                }

                // 旧版本：natives 以 classifier 形式存在，需要单独下载并解压
                if let Some(classifier) = natives_classifier(library)
                    && let Some(artifact) = downloads.classifiers.get(&classifier)
                {
                    let relative = artifact.path.clone().or_else(|| {
                        MavenCoordinate::parse(&library.name).map(|coord| coord.relative_path())
                    });

                    if let Some(relative) = relative {
                        let dest = paths.libraries_dir().join(relative);
                        if seen.insert(dest.clone()) {
                            jobs.push(DownloadJob::new(
                                &artifact.url,
                                dest.clone(),
                                artifact.sha1.clone(),
                                artifact.size.unwrap_or(0),
                            ));
                        }

                        let excludes = library
                            .extract
                            .as_ref()
                            .map(|extract| extract.exclude.clone())
                            .filter(|excludes| !excludes.is_empty())
                            .unwrap_or_else(|| vec!["META-INF/".to_string()]);
                        native_jars.push((dest, excludes));
                    }
                }
            }
            None => {
                // 极老的版本只给仓库根地址和 Maven 坐标
                if let Some(coord) = MavenCoordinate::parse(&library.name) {
                    let base = library.url.as_deref().unwrap_or(libraries_base);
                    let dest = paths.libraries_dir().join(coord.relative_path());
                    if seen.insert(dest.clone()) {
                        jobs.push(DownloadJob::new(maven_url(base, &coord), dest, None, 0));
                    }
                }
            }
        }
    }
}

fn load_asset_index(
    downloader: &Downloader,
    paths: &GamePaths,
    index_ref: &AssetIndexRef,
    dry_run: bool,
) -> Result<AssetIndex> {
    if dry_run {
        return downloader.get_json(&index_ref.url, "资源索引");
    }

    let dest = paths.asset_index_file(&index_ref.id);
    downloader.download(
        &index_ref.url,
        &dest,
        index_ref.sha1.as_deref(),
        index_ref.size.unwrap_or(0),
    )?;

    let bytes = fs::read(&dest).map_err(|err| io_err(&dest, err))?;
    serde_json::from_slice(&bytes).map_err(|source| CoreError::Json {
        context: "资源索引".to_string(),
        source,
    })
}

fn build_asset_jobs(
    paths: &GamePaths,
    index: &AssetIndex,
    resources_base: &str,
) -> Vec<DownloadJob> {
    let base = resources_base.trim_end_matches('/');

    index
        .objects
        .values()
        .filter_map(|object| {
            let prefix = object.hash.get(..2)?;
            Some(DownloadJob::new(
                format!("{base}/{prefix}/{}", object.hash),
                paths.object_path(&object.hash),
                Some(object.hash.clone()),
                object.size,
            ))
        })
        .collect()
}

/// 1.6 及更早版本需要把对象文件按资源名铺开成目录结构。
fn copy_virtual_assets(
    paths: &GamePaths,
    index: &AssetIndex,
    asset_id: &str,
    progress: &Progress,
) -> Result<()> {
    let dest_root = if index.map_to_resources {
        paths.root().join("resources")
    } else {
        paths.virtual_assets_dir(asset_id)
    };

    progress.begin_stage("建立虚拟资源目录", index.objects.len() as u64);

    for (name, object) in &index.objects {
        let source = paths.object_path(&object.hash);
        let dest = dest_root.join(name);

        if source.is_file() && !dest.is_file() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|err| io_err(parent, err))?;
            }
            fs::copy(&source, &dest).map_err(|err| io_err(&dest, err))?;
        }

        progress.advance(1);
    }

    progress.finish_stage();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::manifest::os_name;

    fn library_fixture() -> String {
        let json = r#"{
          "id": "test",
          "mainClass": "net.minecraft.client.main.Main",
          "libraries": [
            {
              "name": "com.mojang:logging:1.1.1",
              "downloads": {
                "artifact": {
                  "path": "com/mojang/logging/1.1.1/logging-1.1.1.jar",
                  "sha1": "aa",
                  "size": 5,
                  "url": "https://example.invalid/logging.jar"
                }
              }
            },
            {
              "name": "example:dummy:1.0",
              "rules": [{ "action": "disallow", "os": { "name": "__OS__" } }],
              "downloads": {
                "artifact": {
                  "path": "example/dummy/1.0/dummy-1.0.jar",
                  "sha1": "bb",
                  "size": 1,
                  "url": "https://example.invalid/dummy.jar"
                }
              }
            },
            {
              "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4",
              "downloads": {
                "classifiers": {
                  "natives-windows": {
                    "path": "org/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives-windows.jar",
                    "sha1": "cc",
                    "size": 2,
                    "url": "https://example.invalid/natives-windows.jar"
                  },
                  "natives-linux": {
                    "path": "org/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives-linux.jar",
                    "sha1": "dd",
                    "size": 2,
                    "url": "https://example.invalid/natives-linux.jar"
                  },
                  "natives-osx": {
                    "path": "org/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives-osx.jar",
                    "sha1": "ee",
                    "size": 2,
                    "url": "https://example.invalid/natives-osx.jar"
                  }
                }
              },
              "natives": { "windows": "natives-windows", "linux": "natives-linux", "osx": "natives-osx" },
              "extract": { "exclude": ["META-INF/"] }
            },
            {
              "name": "net.old:thing:1.0",
              "url": "https://repo.example.invalid/"
            }
          ]
        }"#;

        json.replace("__OS__", os_name())
    }

    #[test]
    fn collect_jobs_filters_rules_and_adds_natives() {
        let version: VersionJson = serde_json::from_str(&library_fixture()).unwrap();
        let paths = GamePaths::new("C:/mc");

        let mut jobs = Vec::new();
        let mut natives = Vec::new();
        collect_library_jobs(
            &paths,
            &version,
            "https://libraries.minecraft.net/",
            &mut jobs,
            &mut natives,
        );

        // logging + natives jar + 老的 Maven 兜底；dummy 被 disallow 规则排除
        assert_eq!(jobs.len(), 3, "任务列表：{:?}", jobs);
        assert_eq!(natives.len(), 1);

        let logging = jobs
            .iter()
            .find(|job| job.url.ends_with("logging.jar"))
            .expect("logging 依赖应当被下载");
        assert_eq!(logging.size, 5);
        assert_eq!(logging.sha1.as_deref(), Some("aa"));

        assert!(!jobs.iter().any(|job| job.url.ends_with("dummy.jar")));

        let native = &natives[0];
        assert_eq!(native.1, vec!["META-INF/".to_string()]);
        assert!(native.0.to_string_lossy().contains("natives-"));
    }

    #[test]
    fn missing_downloads_falls_back_to_maven_url() {
        let version: VersionJson = serde_json::from_str(&library_fixture()).unwrap();
        let paths = GamePaths::new("C:/mc");

        let mut jobs = Vec::new();
        let mut natives = Vec::new();
        collect_library_jobs(
            &paths,
            &version,
            "https://libraries.minecraft.net/",
            &mut jobs,
            &mut natives,
        );

        let fallback = jobs
            .iter()
            .find(|job| job.url.contains("thing-1.0.jar"))
            .expect("没有 downloads 的库应按 Maven 坐标拼 URL");
        assert_eq!(
            fallback.url,
            "https://repo.example.invalid/net/old/thing/1.0/thing-1.0.jar"
        );
    }

    #[test]
    fn duplicate_libraries_produce_single_job() {
        // Forge 之类的版本里同一条依赖可能在父子 JSON 里重复出现
        let version: VersionJson = serde_json::from_str(
            r#"{
              "id": "dup",
              "libraries": [
                { "name": "com.mojang:logging:1.1.1",
                  "downloads": { "artifact": { "path": "com/mojang/logging/1.1.1/logging-1.1.1.jar", "url": "https://example.invalid/a.jar" } } },
                { "name": "com.mojang:logging:1.1.1",
                  "downloads": { "artifact": { "path": "com/mojang/logging/1.1.1/logging-1.1.1.jar", "url": "https://example.invalid/a.jar" } } }
              ]
            }"#,
        )
        .unwrap();

        let paths = GamePaths::new("C:/mc");
        let mut jobs = Vec::new();
        let mut natives = Vec::new();
        collect_library_jobs(
            &paths,
            &version,
            "https://libraries.minecraft.net/",
            &mut jobs,
            &mut natives,
        );

        assert_eq!(jobs.len(), 1);
        assert!(natives.is_empty());
    }

    #[test]
    fn parent_version_fills_missing_fields() {
        let parent: VersionJson = serde_json::from_str(
            r#"{
              "id": "1.20.4",
              "mainClass": "net.minecraft.client.main.Main",
              "assets": "12",
              "assetIndex": { "id": "12", "url": "https://example.invalid/12.json" },
              "downloads": { "client": { "url": "https://example.invalid/client.jar", "sha1": "aa", "size": 1 } },
              "javaVersion": { "component": "java-runtime-gamma", "majorVersion": 17 },
              "libraries": [{ "name": "com.mojang:logging:1.1.1" }]
            }"#,
        )
        .unwrap();

        let child: VersionJson = serde_json::from_str(
            r#"{
              "id": "1.20.4-forge",
              "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
              "libraries": [{ "name": "net.minecraftforge:forge:1.0" }]
            }"#,
        )
        .unwrap();

        let merged = merge_with_parent(child, parent);

        assert_eq!(
            merged.main_class,
            "cpw.mods.bootstraplauncher.BootstrapLauncher"
        );
        assert_eq!(merged.assets, "12");
        assert_eq!(merged.libraries.len(), 2);
        assert_eq!(merged.libraries[0].name, "com.mojang:logging:1.1.1");
        assert_eq!(merged.java_version.unwrap().major_version, 17);
        assert_eq!(merged.downloads.client.unwrap().sha1.as_deref(), Some("aa"));
    }

    #[test]
    fn asset_jobs_point_at_hashed_objects() {
        let index: AssetIndex = serde_json::from_str(
            r#"{
              "objects": {
                "icons/icon_16x16.png": { "hash": "1a2b3c4d5e6f708192a3b4c5d6e7f80912345678", "size": 42 },
                "broken": { "hash": "" }
              }
            }"#,
        )
        .unwrap();

        let paths = GamePaths::new("C:/mc");
        let jobs = build_asset_jobs(&paths, &index, "https://resources.download.minecraft.net");

        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].url,
            "https://resources.download.minecraft.net/1a/1a2b3c4d5e6f708192a3b4c5d6e7f80912345678"
        );
        assert_eq!(jobs[0].size, 42);
    }

    #[test]
    fn java_ok_compares_major_versions() {
        let mut report = InstallReport {
            version: "1.20.4".to_string(),
            game_dir: PathBuf::from("C:/mc"),
            version_dir: PathBuf::from("C:/mc/versions/1.20.4"),
            client_jar: None,
            main_class: "a.B".to_string(),
            library_count: 0,
            native_library_count: 0,
            native_file_count: 0,
            asset_count: 0,
            library_bytes: 0,
            asset_bytes: 0,
            library_stats: DownloadStats::default(),
            java_required: Some(17),
            java_detected: Some(21),
            elapsed: Duration::from_secs(1),
            dry_run: false,
        };

        assert_eq!(report.java_ok(), Some(true));
        report.java_detected = Some(8);
        assert_eq!(report.java_ok(), Some(false));
        report.java_detected = None;
        assert_eq!(report.java_ok(), None);
    }

    #[test]
    fn merges_loader_profile_into_self_contained_instance() {
        let vanilla = br#"{
          "id": "1.20.1",
          "mainClass": "net.minecraft.client.main.Main",
          "libraries": [{"name": "vanilla:lib:1"}],
          "downloads": {"client": {"url": "https://x/client.jar", "sha1": "abc", "size": 1}},
          "assets": "5",
          "arguments": {
            "jvm": ["-cp", "${classpath}"],
            "game": ["--username", "${auth_player_name}"]
          }
        }"#;
        let loader = br#"{
          "id": "fabric-loader-0.16.9-1.20.1",
          "inheritsFrom": "1.20.1",
          "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
          "libraries": [{"name": "fabric:loader:0.16.9"}],
          "downloads": {},
          "arguments": {"jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "], "game": []}
        }"#;

        let merged = merge_instance_json(vanilla, Some(loader), "my-instance").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&merged).unwrap();

        assert_eq!(value["id"], "my-instance");
        assert!(value.get("inheritsFrom").is_none());
        assert_eq!(
            value["mainClass"],
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );
        assert_eq!(value["libraries"].as_array().unwrap().len(), 2);
        assert_eq!(value["arguments"]["jvm"].as_array().unwrap().len(), 3);
        assert_eq!(value["arguments"]["game"].as_array().unwrap().len(), 2);
        // 空的 downloads 不能把原版的 client 覆盖掉
        assert_eq!(value["downloads"]["client"]["sha1"], "abc");
        assert_eq!(value["assets"], "5");
    }

    #[test]
    fn vanilla_instance_renames_id_without_loader() {
        let vanilla = br#"{"id": "1.20.1", "mainClass": "a.B", "inheritsFrom": "1.20"}"#;
        let merged = merge_instance_json(vanilla, None, "my-vanilla").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&merged).unwrap();

        assert_eq!(value["id"], "my-vanilla");
        assert!(value.get("inheritsFrom").is_none());
    }

    #[test]
    fn writes_instance_loader_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "neutrino-instance-meta-test-{}",
            std::process::id()
        ));
        let paths = GamePaths::new(&dir);
        let instance = "1.21.1-fabric";
        fs::create_dir_all(paths.version_dir(instance)).unwrap();

        let mut options = InstanceInstallOptions::new(&dir, instance, "1.21.1");
        options.loader = Some(LoaderKind::Fabric);
        options.loader_version = Some("0.16.9".to_string());
        write_instance_metadata(&paths, &options).unwrap();

        let text = fs::read_to_string(paths.version_dir(instance).join("neutrino.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["game_version"], "1.21.1");
        assert_eq!(value["loader"], "fabric");
        assert_eq!(value["loader_version"], "0.16.9");

        let _ = fs::remove_dir_all(dir);
    }
}
