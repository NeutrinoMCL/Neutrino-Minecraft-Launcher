//! CurseForge API v1 的最小封装：搜索项目、取项目文件。
//!
//! 接口地址：<https://docs.curseforge.com/rest-api/>
//! 所有请求都要带 `x-api-key`，Key 从 [`API_KEY_ENV`] 读取（在 `.env` 里配置）。

use serde::Deserialize;

use super::error::Result;
use super::modrinth::ContentKind;
use super::net::{Downloader, encode_component};

const API_BASE: &str = "https://api.curseforge.com/v1";
/// Minecraft 在 CurseForge 里的 gameId。
const MINECRAFT_GAME_ID: u32 = 432;
/// 支持的 API Key 环境变量，按优先级排列。
pub const API_KEY_ENVS: [&str; 2] = [
    "CURSEFORGE_API_KEY",
    // 兼容本项目早期文档里带前缀的写法
    "NEUTRINO_CURSEFORGE_API_KEY",
];

/// 搜索结果。
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub data: Vec<Mod>,
    #[serde(default)]
    pub pagination: Pagination,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Pagination {
    #[serde(default, rename = "totalCount")]
    pub total_count: u64,
}

/// 搜索结果里的单个项目。
#[derive(Debug, Clone, Deserialize)]
pub struct Mod {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub authors: Vec<Author>,
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default, rename = "downloadCount")]
    pub download_count: u64,
    #[serde(default)]
    pub logo: Option<Logo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Author {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Category {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Logo {
    #[serde(default, rename = "thumbnailUrl")]
    pub thumbnail_url: String,
    #[serde(default)]
    pub url: String,
}

impl Mod {
    /// 作者名，多个作者用逗号连接。
    pub fn author(&self) -> String {
        self.authors
            .iter()
            .map(|author| author.name.as_str())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// 图标地址，优先用缩略图。
    pub fn icon_url(&self) -> Option<&str> {
        let logo = self.logo.as_ref()?;
        [logo.thumbnail_url.as_str(), logo.url.as_str()]
            .into_iter()
            .find(|url| !url.is_empty())
    }
}

/// 项目的文件列表。
#[derive(Debug, Clone, Deserialize)]
pub struct FilesResponse {
    #[serde(default)]
    pub data: Vec<ModFile>,
}

/// 项目的一个文件。
#[derive(Debug, Clone, Deserialize)]
pub struct ModFile {
    pub id: u64,
    #[serde(default, rename = "displayName")]
    pub display_name: String,
    #[serde(default, rename = "fileName")]
    pub file_name: String,
    #[serde(default, rename = "downloadUrl")]
    pub download_url: Option<String>,
    /// 1 = release, 2 = beta, 3 = alpha
    #[serde(default, rename = "releaseType")]
    pub release_type: u8,
    #[serde(default, rename = "fileDate")]
    pub file_date: String,
    #[serde(default, rename = "fileLength")]
    pub file_length: u64,
    #[serde(default, rename = "gameVersions")]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub hashes: Vec<FileHash>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileHash {
    #[serde(default)]
    pub value: String,
    /// 1 = SHA-1, 2 = MD5
    #[serde(default)]
    pub algo: u8,
}

impl ModFile {
    /// SHA-1 校验值（CurseForge 的 `hashes` 里 algo=1）。
    pub fn sha1(&self) -> Option<&str> {
        self.hashes
            .iter()
            .find(|hash| hash.algo == 1 && !hash.value.is_empty())
            .map(|hash| hash.value.as_str())
    }

    pub fn downloadable(&self) -> bool {
        self.download_url
            .as_deref()
            .is_some_and(|url| !url.is_empty())
    }

    fn supports(&self, game_version: Option<&str>) -> bool {
        game_version.is_none_or(|game| self.game_versions.iter().any(|version| version == game))
    }
}

/// 按类别搜索项目。
///
/// `modLoaderType` 按 CurseForge 的要求必须和 `gameVersion` 一起用；
/// 调用方在没有选游戏版本时不要传 loader。`sort` 对应 `sortField`。
#[allow(clippy::too_many_arguments)]
pub fn search(
    downloader: &Downloader,
    kind: ContentKind,
    query: Option<&str>,
    game_version: Option<&str>,
    loader: Option<u8>,
    category: Option<u32>,
    sort: u8,
    limit: usize,
    api_key: &str,
) -> Result<SearchResponse> {
    let mut url = format!(
        "{API_BASE}/mods/search?gameId={MINECRAFT_GAME_ID}&classId={}&index=0&pageSize={}&sortField={sort}&sortOrder=desc",
        class_id(kind),
        limit.clamp(1, 50)
    );
    if let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) {
        url.push_str("&searchFilter=");
        url.push_str(&encode_component(query));
    }
    if let Some(game_version) = game_version {
        url.push_str("&gameVersion=");
        url.push_str(&encode_component(game_version));
    }
    if let Some(loader) = loader {
        url.push_str("&modLoaderType=");
        url.push_str(&loader.to_string());
    }
    if let Some(category) = category {
        url.push_str("&categoryId=");
        url.push_str(&category.to_string());
    }

    let headers = [("x-api-key", api_key)];
    downloader.get_json_with_headers(&url, "CurseForge 搜索结果", &headers)
}

/// CurseForge 的 `ModLoaderType` 枚举。
pub fn loader_type(key: &str) -> Option<u8> {
    match key.trim().to_ascii_lowercase().as_str() {
        "forge" => Some(1),
        "cauldron" => Some(2),
        "liteloader" => Some(3),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

/// `/v1/categories` 里的单个分类。
#[derive(Debug, Clone, Deserialize)]
pub struct CategoryInfo {
    pub id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "isClass")]
    pub is_class: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct CategoriesResponse {
    #[serde(default)]
    data: Vec<CategoryInfo>,
}

/// 取某个内容类别（classId）下的全部分类。
pub fn categories(
    downloader: &Downloader,
    kind: ContentKind,
    api_key: &str,
) -> Result<Vec<CategoryInfo>> {
    let url = format!(
        "{API_BASE}/categories?gameId={MINECRAFT_GAME_ID}&classId={}",
        class_id(kind)
    );
    let headers = [("x-api-key", api_key)];
    let response: CategoriesResponse =
        downloader.get_json_with_headers(&url, "CurseForge 分类列表", &headers)?;
    Ok(response.data)
}

/// 取某个项目的文件列表（接口默认按上传时间倒序返回）。
///
/// CurseForge 单页最多 50 条，这里连续翻页，最多取 [`MAX_FILES`] 个文件。
pub fn mod_files(
    downloader: &Downloader,
    mod_id: &str,
    game_version: Option<&str>,
    api_key: &str,
) -> Result<Vec<ModFile>> {
    const PAGE_SIZE: usize = 50;
    const MAX_FILES: usize = 500;

    let headers = [("x-api-key", api_key)];
    let mut files = Vec::new();

    while files.len() < MAX_FILES {
        let mut url = format!(
            "{API_BASE}/mods/{mod_id}/files?index={}&pageSize={PAGE_SIZE}",
            files.len()
        );
        if let Some(game_version) = game_version {
            url.push_str("&gameVersion=");
            url.push_str(&encode_component(game_version));
        }

        let response: FilesResponse =
            downloader.get_json_with_headers(&url, "CurseForge 文件列表", &headers)?;
        let count = response.data.len();
        files.extend(response.data);

        if count < PAGE_SIZE {
            break;
        }
    }

    files.truncate(MAX_FILES);
    Ok(files)
}

/// 从文件列表里挑一个"最新且可下载"的文件。
///
/// 优先 `release`，没有就退回列表里的第一个可下载文件；
/// 指定游戏版本时只考虑 `gameVersions` 匹配的文件。
pub fn pick_file<'a>(files: &'a [ModFile], game_version: Option<&str>) -> Option<&'a ModFile> {
    let mut fallback = None;
    for file in files {
        if !file.downloadable() || !file.supports(game_version) {
            continue;
        }
        if file.release_type == 1 {
            return Some(file);
        }
        if fallback.is_none() {
            fallback = Some(file);
        }
    }
    fallback
}

/// CurseForge 的 classId。
fn class_id(kind: ContentKind) -> u32 {
    match kind {
        ContentKind::Mod => 6,
        ContentKind::ResourcePack => 12,
        ContentKind::ModPack => 4471,
        ContentKind::Shader => 6552,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ids_match_curseforge_sections() {
        assert_eq!(class_id(ContentKind::Mod), 6);
        assert_eq!(class_id(ContentKind::ResourcePack), 12);
        assert_eq!(class_id(ContentKind::ModPack), 4471);
        assert_eq!(class_id(ContentKind::Shader), 6552);
    }

    #[test]
    fn parses_search_response() {
        let json = r#"{
          "data": [{
            "id": 238222,
            "name": "Just Enough Items",
            "summary": "View items and recipes",
            "downloadCount": 123456,
            "authors": [{ "name": "mezz" }],
            "categories": [{ "name": "Map and Information" }],
            "logo": { "thumbnailUrl": "https://media.forgecdn.net/thumb.png", "url": "https://media.forgecdn.net/logo.png" }
          }],
          "pagination": { "index": 0, "pageSize": 30, "resultCount": 1, "totalCount": 999 }
        }"#;

        let parsed: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.pagination.total_count, 999);
        assert_eq!(parsed.data[0].name, "Just Enough Items");
        assert_eq!(parsed.data[0].author(), "mezz");
        assert_eq!(
            parsed.data[0].icon_url(),
            Some("https://media.forgecdn.net/thumb.png")
        );
    }

    #[test]
    fn picks_release_for_game_version() {
        let json = r#"[
          {
            "id": 1, "fileName": "old.jar", "downloadUrl": "https://x/old.jar",
            "releaseType": 1, "gameVersions": ["1.20.1"],
            "hashes": [{ "value": "aa", "algo": 1 }]
          },
          {
            "id": 2, "fileName": "no-url.jar", "downloadUrl": null,
            "releaseType": 1, "gameVersions": ["1.21.1"]
          },
          {
            "id": 3, "fileName": "beta.jar", "downloadUrl": "https://x/beta.jar",
            "releaseType": 2, "gameVersions": ["1.21.1"]
          },
          {
            "id": 4, "fileName": "new.jar", "downloadUrl": "https://x/new.jar",
            "releaseType": 1, "gameVersions": ["1.21.1"],
            "hashes": [{ "value": "bb", "algo": 1 }]
          }
        ]"#;

        let files: Vec<ModFile> = serde_json::from_str(json).unwrap();
        let selected = pick_file(&files, Some("1.21.1")).unwrap();
        assert_eq!(selected.file_name, "new.jar");
        assert_eq!(selected.sha1(), Some("bb"));

        // 不限制版本时仍然跳过没有下载地址的文件
        assert_eq!(pick_file(&files, None).unwrap().file_name, "old.jar");
    }

    #[test]
    fn falls_back_to_non_release() {
        let json = r#"[
          {
            "id": 1, "fileName": "beta.jar", "downloadUrl": "https://x/beta.jar",
            "releaseType": 2, "gameVersions": ["1.21.1"]
          }
        ]"#;

        let files: Vec<ModFile> = serde_json::from_str(json).unwrap();
        assert_eq!(
            pick_file(&files, Some("1.21.1")).unwrap().file_name,
            "beta.jar"
        );
        assert!(pick_file(&files, Some("1.20.1")).is_none());
    }

    #[test]
    fn maps_curseforge_loader_types() {
        assert_eq!(loader_type("forge"), Some(1));
        assert_eq!(loader_type("LiteLoader"), Some(3));
        assert_eq!(loader_type("fabric"), Some(4));
        assert_eq!(loader_type("quilt"), Some(5));
        assert_eq!(loader_type("NeoForge"), Some(6));
        assert_eq!(loader_type("unknown"), None);
    }

    #[test]
    fn parses_categories() {
        let json = r#"{
          "data": [
            { "id": 423, "name": "Map and Information", "isClass": false },
            { "id": 6, "name": "Mods", "isClass": true }
          ]
        }"#;

        let parsed: CategoriesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(parsed.data[0].id, 423);
        assert_eq!(parsed.data[0].name, "Map and Information");
        assert_eq!(parsed.data[0].is_class, Some(false));
        assert_eq!(parsed.data[1].is_class, Some(true));
    }
}
