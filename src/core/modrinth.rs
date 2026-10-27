//! Modrinth API v2 的最小封装：搜索项目、取最新文件。
//!
//! 只实现浏览与下载需要的那部分字段，接口地址：<https://docs.modrinth.com/api/>
//! 注意：Modrinth 要求带可识别的 User-Agent，这里复用 [`Downloader`] 里的全局 UA。

use std::collections::HashMap;

use serde::Deserialize;

use super::error::{CoreError, Result};
use super::net::{Downloader, encode_component};

const API_BASE: &str = "https://api.modrinth.com/v2";

/// 发现页的内容类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Mod,
    Shader,
    ResourcePack,
    ModPack,
}

impl ContentKind {
    /// 解析 UI 传来的类别键。
    pub fn parse(key: &str) -> Option<Self> {
        match key {
            "mod" => Some(Self::Mod),
            "shader" => Some(Self::Shader),
            "resourcepack" => Some(Self::ResourcePack),
            "modpack" => Some(Self::ModPack),
            _ => None,
        }
    }

    /// Modrinth 的 `project_type` 取值。
    pub fn project_type(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Shader => "shader",
            Self::ResourcePack => "resourcepack",
            Self::ModPack => "modpack",
        }
    }

    /// 下载后落盘的子目录（相对 `.minecraft`）。
    pub fn target_dir(self) -> &'static str {
        match self {
            Self::Mod => "mods",
            Self::Shader => "shaderpacks",
            Self::ResourcePack => "resourcepacks",
            Self::ModPack => "modpacks",
        }
    }

    /// 现阶段是否支持"一键下载安装"。
    pub fn supports_install(self) -> bool {
        // 整合包需要解析 .mrpack 并生成版本 JSON，暂未实现
        !matches!(self, Self::ModPack)
    }
}

/// 搜索结果。
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub hits: Vec<ProjectHit>,
    #[serde(default)]
    pub total_hits: u64,
}

/// 搜索结果里的单个项目。
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectHit {
    pub project_id: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub categories: Vec<String>,
    /// `required` / `optional` / `unsupported` / `unknown`
    #[serde(default)]
    pub client_side: String,
    /// `required` / `optional` / `unsupported` / `unknown`
    #[serde(default)]
    pub server_side: String,
    #[serde(default)]
    pub project_type: String,
    #[serde(default)]
    pub downloads: u64,
    /// 项目图标的 CDN 地址（可能为空）。
    #[serde(default)]
    pub icon_url: Option<String>,
}

/// 项目的一个版本。
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectVersion {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    /// ISO 8601 发布时间；用来在兼容版本里挑最新的
    #[serde(default)]
    pub date_published: String,
    #[serde(default)]
    pub files: Vec<VersionFile>,
}

/// 版本里的文件。
#[derive(Debug, Clone, Deserialize)]
pub struct VersionFile {
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub hashes: HashMap<String, String>,
}

impl VersionFile {
    /// SHA-1（Modrinth 一定会给）。
    pub fn sha1(&self) -> Option<&str> {
        self.hashes.get("sha1").map(String::as_str)
    }
}

/// 按类别搜索项目。
///
/// `game_version`、`loader`、`category` 为空表示不限制对应条件；
/// `sort` 对应 Modrinth 的 `index`，例如 relevance / downloads / follows / newest / updated。
#[allow(clippy::too_many_arguments)]
pub fn search(
    downloader: &Downloader,
    kind: ContentKind,
    query: Option<&str>,
    game_version: Option<&str>,
    loader: Option<&str>,
    category: Option<&str>,
    sort: &str,
    limit: usize,
) -> Result<SearchResponse> {
    // facets 形如 [["project_type:mod"],["versions:1.21.1"],["categories:fabric"]]，
    // 外层数组是 AND，内层数组是 OR；加载器和分类共用 categories 字段。
    let mut facets = format!(r#"[["project_type:{}"]"#, kind.project_type());
    if let Some(game_version) = game_version {
        facets.push_str(&format!(r#",["versions:{game_version}"]"#));
    }
    if let Some(category) = category {
        facets.push_str(&format!(r#",["categories:{category}"]"#));
    }
    if let Some(loader) = loader {
        facets.push_str(&format!(r#",["categories:{loader}"]"#));
    }
    facets.push(']');

    let sort = if sort.is_empty() { "relevance" } else { sort };
    let mut url = format!(
        "{API_BASE}/search?limit={limit}&index={sort}&facets={}",
        encode_component(&facets)
    );
    if let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) {
        url.push_str("&query=");
        url.push_str(&encode_component(query));
    }
    downloader.get_json(&url, "Modrinth 搜索结果")
}

/// `/tag/category` 里的单个分类。
#[derive(Debug, Clone, Deserialize)]
pub struct CategoryTag {
    pub name: String,
    #[serde(default, rename = "project_type")]
    pub project_type: String,
}

/// 取 Modrinth 全部分类标签（调用方按 project_type 过滤）。
pub fn categories(downloader: &Downloader) -> Result<Vec<CategoryTag>> {
    let url = format!("{API_BASE}/tag/category");
    downloader.get_json(&url, "Modrinth 分类列表")
}

/// 取某个项目的全部版本（接口按发布时间倒序返回）。
pub fn project_versions(downloader: &Downloader, project_id: &str) -> Result<Vec<ProjectVersion>> {
    let url = format!("{API_BASE}/project/{project_id}/version");
    downloader.get_json(&url, "Modrinth 版本列表")
}

/// 从版本列表里挑一个"最新且可下载"的文件。
///
/// 策略：优先最新的正式版（`version_type == "release"`），没有就退回最新版本；
/// 每个版本内部优先 `primary` 文件。指定 `game_version` 时只考虑支持该版本的文件。
pub fn pick_file<'a>(
    versions: &'a [ProjectVersion],
    game_version: Option<&str>,
) -> Option<&'a VersionFile> {
    let matches = |version: &ProjectVersion| {
        game_version.is_none_or(|game| version.game_versions.iter().any(|version| version == game))
    };

    versions
        .iter()
        .filter(|version| matches(version))
        .find(|version| version.version_type == "release")
        .and_then(primary_file)
        .or_else(|| {
            versions
                .iter()
                .filter(|version| matches(version))
                .find_map(primary_file)
        })
}

fn primary_file(version: &ProjectVersion) -> Option<&VersionFile> {
    version
        .files
        .iter()
        .find(|file| file.primary)
        .or_else(|| version.files.first())
}

/// 取版本里的主文件（优先 `primary`，否则第一个）。
pub fn main_file(version: &ProjectVersion) -> Option<&VersionFile> {
    primary_file(version)
}

/// 校验类别键是否合法，顺便给出错误信息。
pub fn parse_kind(key: &str) -> Result<ContentKind> {
    ContentKind::parse(key).ok_or_else(|| CoreError::Other(format!("未知的内容类别：{key}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_maps_to_modrinth_type_and_dir() {
        assert_eq!(ContentKind::Mod.project_type(), "mod");
        assert_eq!(ContentKind::Shader.target_dir(), "shaderpacks");
        assert_eq!(ContentKind::ResourcePack.target_dir(), "resourcepacks");
        assert!(ContentKind::Mod.supports_install());
        assert!(!ContentKind::ModPack.supports_install());
    }

    #[test]
    fn parses_search_hit() {
        let json = r#"{
          "hits": [{
            "project_id": "P7dR8mSH",
            "slug": "fabric-api",
            "title": "Fabric API",
            "description": "Lightweight and modular API",
            "author": "modmuss50",
            "categories": ["fabric", "library"],
            "project_type": "mod",
            "downloads": 286109303,
            "icon_url": "https://cdn.modrinth.com/data/P7dR8mSH/icon.png"
          }],
          "total_hits": 12345
        }"#;
        let parsed: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.total_hits, 12345);
        assert_eq!(parsed.hits[0].title, "Fabric API");
        assert_eq!(parsed.hits[0].downloads, 286109303);
        assert_eq!(
            parsed.hits[0].icon_url.as_deref(),
            Some("https://cdn.modrinth.com/data/P7dR8mSH/icon.png")
        );
    }

    #[test]
    fn prefers_release_then_primary_file() {
        let json = r#"[
          {
            "id": "new-beta", "version_type": "beta", "version_number": "2.0-beta",
            "files": [{ "url": "https://x/beta.jar", "filename": "beta.jar", "primary": true, "hashes": {"sha1": "bb"} }]
          },
          {
            "id": "old-release", "version_type": "release", "version_number": "1.5",
            "files": [
              { "url": "https://x/extra.jar", "filename": "extra.jar", "primary": false, "hashes": {} },
              { "url": "https://x/main.jar", "filename": "main.jar", "primary": true, "hashes": {"sha1": "aa"} }
            ]
          }
        ]"#;
        let versions: Vec<ProjectVersion> = serde_json::from_str(json).unwrap();
        let file = pick_file(&versions, None).unwrap();
        assert_eq!(file.filename, "main.jar");
        assert_eq!(file.sha1(), Some("aa"));
    }

    #[test]
    fn falls_back_to_newest_version() {
        let json = r#"[
          {
            "id": "b", "version_type": "beta",
            "files": [{ "url": "https://x/a.jar", "filename": "a.jar" }]
          }
        ]"#;
        let versions: Vec<ProjectVersion> = serde_json::from_str(json).unwrap();
        assert_eq!(pick_file(&versions, None).unwrap().filename, "a.jar");
        assert!(pick_file(&[], None).is_none());
    }

    #[test]
    fn filters_versions_by_game_version() {
        let json = r#"[
          {
            "id": "old", "version_type": "release", "game_versions": ["1.20.1"],
            "files": [{ "url": "https://x/old.jar", "filename": "old.jar", "primary": true }]
          },
          {
            "id": "new", "version_type": "beta", "game_versions": ["1.21.1"],
            "files": [{ "url": "https://x/new.jar", "filename": "new.jar", "primary": true }]
          }
        ]"#;
        let versions: Vec<ProjectVersion> = serde_json::from_str(json).unwrap();

        assert_eq!(
            pick_file(&versions, Some("1.21.1")).unwrap().filename,
            "new.jar"
        );
        assert_eq!(
            pick_file(&versions, Some("1.20.1")).unwrap().filename,
            "old.jar"
        );
        assert!(pick_file(&versions, Some("1.19.4")).is_none());
    }

    #[test]
    fn parses_category_tag() {
        let json = r#"{
          "name": "game-mechanics",
          "project_type": "mod",
          "header": "Game Mechanics",
          "icon": "https://cdn.modrinth.com/data/cached_images/x.png"
        }"#;
        let tag: CategoryTag = serde_json::from_str(json).unwrap();
        assert_eq!(tag.name, "game-mechanics");
        assert_eq!(tag.project_type, "mod");
    }
}
