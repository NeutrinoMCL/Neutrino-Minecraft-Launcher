//! 安装核心的公共入口与常量。

pub mod curseforge;
pub mod error;
pub mod hash;
pub mod install;
pub mod launch;
pub mod loader;
pub mod manifest;
pub mod modrinth;
pub mod natives;
pub mod net;
pub mod paths;
pub mod progress;

pub use error::{CoreError, Result};
pub use install::{
    InstallOptions, InstallReport, InstanceInstallOptions, install, install_instance,
};
pub use launch::{LaunchAccount, LaunchCommand, LaunchOptions, offline_uuid};
pub use loader::{LoaderKind, LoaderVersion};
pub use manifest::{VersionJson, VersionManifest, VersionSummary};
pub use modrinth::{ContentKind, ProjectHit, ProjectVersion, SearchResponse, VersionFile};
pub use net::{DownloadJob, DownloadOutcome, DownloadStats, Downloader};
pub use paths::{GamePaths, default_game_dir};
pub use progress::{Progress, ProgressSnapshot};

/// Mojang 官方版本清单（v2 带 sha1 与 complianceLevel）。
pub const DEFAULT_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// 资源文件（assets/objects）下载根地址。
pub const RESOURCES_BASE_URL: &str = "https://resources.download.minecraft.net";

/// Maven 仓库根地址，用于补齐缺少 `downloads` 的旧版本依赖。
pub const LIBRARIES_BASE_URL: &str = "https://libraries.minecraft.net/";
