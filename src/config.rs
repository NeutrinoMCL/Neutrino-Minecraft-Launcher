//! `conf.toml` 的读取与保存。
//!
//! 查找顺序（第一个存在的文件就是本次使用的配置文件）：
//! 1. 环境变量 `NEUTRINO_CONFIG` 指定的路径；
//! 2. 当前工作目录下的 `conf.toml`（开发 / 便携模式）；
//! 3. `%APPDATA%/Neutrino/conf.toml`（用户配置目录）。
//!
//! 保存时写回同一个文件；如果文件不存在，用户配置目录会被创建。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 默认背景：内置壁纸 `Jokull`。
pub const DEFAULT_BACKGROUND: &str = "%built-in:Jokull";

/// 纯色背景的默认颜色。
pub const DEFAULT_BACKGROUND_COLOR: &str = "#1E3A8A";

/// 整个 `conf.toml`。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 旧版本示例里写成了 `apperance`，这里继续兼容。
    #[serde(alias = "apperance")]
    pub appearance: Appearance,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: ThemeConfig,
    pub background: BackgroundConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// `light` / `dark` / `system`。
    #[serde(rename = "colormode", alias = "color-mode", alias = "color_mode")]
    pub color_mode: ColorMode,
}

/// 颜色模式。未知取值按 `system` 处理，避免整份配置解析失败。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ColorMode {
    Light,
    Dark,
    #[default]
    System,
}

impl ColorMode {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }
}

impl From<String> for ColorMode {
    fn from(value: String) -> Self {
        Self::parse(&value)
    }
}

impl From<ColorMode> for String {
    fn from(value: ColorMode) -> Self {
        value.as_str().to_string()
    }
}

/// 背景类型。未知取值按 `image` 处理。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum BackgroundMode {
    #[default]
    Image,
    Color,
}

impl BackgroundMode {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "color" | "solid" => Self::Color,
            _ => Self::Image,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Color => "color",
        }
    }
}

impl From<String> for BackgroundMode {
    fn from(value: String) -> Self {
        Self::parse(&value)
    }
}

impl From<BackgroundMode> for String {
    fn from(value: BackgroundMode) -> Self {
        value.as_str().to_string()
    }
}

/// 解析 `#RGB` / `#RRGGBB`（`#` 可省略），返回 (r, g, b)。
pub fn parse_hex_color(value: &str) -> Option<(u8, u8, u8)> {
    let value = value.trim().trim_start_matches('#');
    if !value.is_ascii() {
        return None;
    }

    match value.len() {
        3 => {
            let mut chars = value.chars();
            let red = hex_nibble(chars.next()?)?;
            let green = hex_nibble(chars.next()?)?;
            let blue = hex_nibble(chars.next()?)?;
            Some((red * 17, green * 17, blue * 17))
        }
        6 => Some((
            u8::from_str_radix(&value[0..2], 16).ok()?,
            u8::from_str_radix(&value[2..4], 16).ok()?,
            u8::from_str_radix(&value[4..6], 16).ok()?,
        )),
        _ => None,
    }
}

/// 规范成 `#RRGGBB`（大写），供配置和 UI 共用。
pub fn normalize_hex_color(value: &str) -> Option<String> {
    let (red, green, blue) = parse_hex_color(value)?;
    Some(hex_color(red, green, blue))
}

pub fn hex_color(red: u8, green: u8, blue: u8) -> String {
    format!("#{red:02X}{green:02X}{blue:02X}")
}

fn hex_nibble(value: char) -> Option<u8> {
    value.to_digit(16).map(|value| value as u8)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundConfig {
    /// `image`（图片 / 内置壁纸）或 `color`（纯色）。
    pub mode: BackgroundMode,
    /// `%built-in:<name>`（内置壁纸）或 `%file:<path>`（自定义图片）。
    pub choice: String,
    /// 已添加的自定义壁纸路径（按添加顺序）。
    pub wallpapers: Vec<String>,
    /// 启动时是否从 `wallpapers` 里随机挑一张。
    pub random: bool,
    /// 纯色背景的颜色（`#RRGGBB`）。
    pub color: String,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            mode: BackgroundMode::Image,
            choice: DEFAULT_BACKGROUND.to_string(),
            wallpapers: Vec::new(),
            random: false,
            color: DEFAULT_BACKGROUND_COLOR.to_string(),
        }
    }
}

impl BackgroundConfig {
    /// `%built-in:Jokull` -> `Some("Jokull")`。
    pub fn builtin_name(&self) -> Option<&str> {
        self.choice
            .strip_prefix("%built-in:")
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }

    /// `%file:C:\Pictures\bg.png` -> `Some("C:\Pictures\bg.png")`。
    pub fn custom_path(&self) -> Option<&Path> {
        self.choice
            .strip_prefix("%file:")
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(Path::new)
    }

    /// 已添加的自定义壁纸。旧配置只有 `%file:` 时，把它也当作列表里的一项。
    pub fn wallpaper_paths(&self) -> Vec<&Path> {
        let mut paths = self
            .wallpapers
            .iter()
            .map(|path| Path::new(path.trim()))
            .filter(|path| !path.as_os_str().is_empty())
            .collect::<Vec<_>>();

        if let Some(path) = self.custom_path()
            && !paths.contains(&path)
        {
            paths.push(path);
        }
        paths
    }

    /// 旧配置（只有一个 `%file:`）迁移成壁纸列表。
    fn ensure_wallpaper_list(&mut self) {
        if let Some(path) = self.custom_path() {
            let value = path.display().to_string();
            if !self.wallpapers.iter().any(|existing| existing == &value) {
                self.wallpapers.push(value);
            }
        }
    }
}

/// 配置文件的路径 + 内存中的内容。
pub struct Store {
    path: PathBuf,
    config: Config,
}

impl Store {
    pub fn load_default() -> Self {
        Self::load(default_path())
    }

    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut config = match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!(
                        "配置文件 {} 解析失败，使用默认设置：{error}",
                        path.display()
                    );
                    Config::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(error) => {
                eprintln!("读取配置文件 {} 失败：{error}", path.display());
                Config::default()
            }
        };
        config.appearance.background.ensure_wallpaper_list();
        config.appearance.background.color =
            normalize_hex_color(&config.appearance.background.color)
                .unwrap_or_else(|| DEFAULT_BACKGROUND_COLOR.to_string());

        Self { path, config }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 改配置并立刻保存；保存失败时回滚内存里的内容。
    fn update(&mut self, apply: impl FnOnce(&mut Config)) -> Result<(), String> {
        let previous = self.config.clone();
        apply(&mut self.config);
        if self.config == previous {
            return Ok(());
        }

        if let Err(error) = self.save() {
            self.config = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn set_color_mode(&mut self, mode: ColorMode) -> Result<(), String> {
        self.update(|config| config.appearance.theme.color_mode = mode)
    }

    /// 添加自定义壁纸；如果没开随机模式，自动选中第一张新添加的图。
    pub fn add_custom_wallpapers(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        self.update(|config| {
            let background = &mut config.appearance.background;
            background.mode = BackgroundMode::Image;
            background.ensure_wallpaper_list();

            let mut first_new = None;
            for path in paths {
                let value = path.display().to_string();
                let exists = background
                    .wallpapers
                    .iter()
                    .any(|existing| Path::new(existing) == path.as_path());
                if !exists {
                    first_new.get_or_insert(value.clone());
                    background.wallpapers.push(value);
                }
            }

            if !background.random
                && let Some(path) = first_new
            {
                background.choice = format!("%file:{path}");
            }
        })
    }

    /// 选中某张已添加的壁纸，并关闭随机模式。
    pub fn select_custom_wallpaper(&mut self, path: &str) -> Result<(), String> {
        self.update(|config| {
            let background = &mut config.appearance.background;
            background.mode = BackgroundMode::Image;
            if !background
                .wallpapers
                .iter()
                .any(|existing| existing == path)
            {
                background.wallpapers.push(path.to_string());
            }
            background.choice = format!("%file:{path}");
            background.random = false;
        })
    }

    /// 从列表里移除壁纸；如果移除的是当前壁纸，自动切到下一张或内置壁纸。
    pub fn remove_custom_wallpaper(&mut self, path: &str) -> Result<(), String> {
        self.update(|config| {
            let background = &mut config.appearance.background;
            background.wallpapers.retain(|existing| existing != path);

            if background
                .custom_path()
                .is_some_and(|current| current == Path::new(path))
            {
                background.choice = background
                    .wallpapers
                    .first()
                    .map(|next| format!("%file:{next}"))
                    .unwrap_or_else(|| DEFAULT_BACKGROUND.to_string());
            }

            if background.wallpapers.is_empty() {
                background.random = false;
            }
        })
    }

    /// 开启 / 关闭启动时随机选择壁纸。
    pub fn set_random_background(&mut self, random: bool) -> Result<(), String> {
        self.update(|config| {
            let background = &mut config.appearance.background;
            if random {
                background.mode = BackgroundMode::Image;
            }
            background.random = random && !background.wallpapers.is_empty();
        })
    }

    /// 切换图片 / 纯色背景。
    pub fn set_background_mode(&mut self, mode: BackgroundMode) -> Result<(), String> {
        self.update(|config| {
            let background = &mut config.appearance.background;
            background.mode = mode;
            if mode == BackgroundMode::Color {
                background.color = normalize_hex_color(&background.color)
                    .unwrap_or_else(|| DEFAULT_BACKGROUND_COLOR.to_string());
            }
        })
    }

    /// 设置纯色背景并切到纯色模式。
    pub fn set_background_color(&mut self, value: &str) -> Result<(), String> {
        let color = normalize_hex_color(value).ok_or_else(|| "颜色格式应为 #RRGGBB".to_string())?;

        self.update(|config| {
            let background = &mut config.appearance.background;
            background.mode = BackgroundMode::Color;
            background.color = color;
        })
    }

    /// 当前背景恢复成内置 Jokull（保留已添加的壁纸列表）。
    pub fn reset_background(&mut self) -> Result<(), String> {
        self.update(|config| {
            let background = &mut config.appearance.background;
            background.mode = BackgroundMode::Image;
            background.choice = DEFAULT_BACKGROUND.to_string();
            background.random = false;
        })
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建配置目录 {} 失败：{error}", parent.display()))?;
        }

        let text = toml::to_string_pretty(&self.config)
            .map_err(|error| format!("序列化配置失败：{error}"))?;
        std::fs::write(&self.path, text)
            .map_err(|error| format!("写入配置文件 {} 失败：{error}", self.path.display()))
    }
}

/// 见模块文档里的查找顺序。
pub fn default_path() -> PathBuf {
    if let Some(path) = std::env::var_os("NEUTRINO_CONFIG").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }

    let local = PathBuf::from("conf.toml");
    if local.is_file() {
        return local;
    }

    crate::accounts::data_dir().join("conf.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_config() {
        let config: Config = toml::from_str(
            r#"
[apperance.theme]
colormode = "light" # dark/system

[apperance.background]
choice = "%built-in:Jokull"
"#,
        )
        .unwrap();

        assert_eq!(config.appearance.theme.color_mode, ColorMode::Light);
        assert_eq!(config.appearance.background.builtin_name(), Some("Jokull"));
        assert_eq!(config.appearance.background.mode, BackgroundMode::Image);
        assert_eq!(config.appearance.background.color, DEFAULT_BACKGROUND_COLOR);
    }

    #[test]
    fn parses_custom_background_path() {
        let config: Config = toml::from_str(
            r#"
[appearance.background]
choice = "%file:C:\\Pictures\\night.png"
"#,
        )
        .unwrap();

        assert_eq!(
            config.appearance.background.custom_path(),
            Some(Path::new(r"C:\Pictures\night.png"))
        );
        assert_eq!(
            config.appearance.background.wallpaper_paths(),
            vec![Path::new(r"C:\Pictures\night.png")]
        );
        assert_eq!(config.appearance.background.builtin_name(), None);
    }

    #[test]
    fn parses_and_normalizes_hex_colors() {
        assert_eq!(normalize_hex_color("#abc").as_deref(), Some("#AABBCC"));
        assert_eq!(normalize_hex_color("1e3a8a").as_deref(), Some("#1E3A8A"));
        assert_eq!(normalize_hex_color(" #00FF00 ").as_deref(), Some("#00FF00"));
        assert_eq!(normalize_hex_color("GGGGGG"), None);
        assert_eq!(normalize_hex_color("#12345"), None);
        assert_eq!(normalize_hex_color("颜色"), None);
        assert_eq!(hex_color(1, 2, 255), "#0102FF");
    }

    #[test]
    fn saves_color_background_mode() {
        let dir =
            std::env::temp_dir().join(format!("neutrino-conf-color-test-{}", std::process::id()));
        let path = dir.join("conf.toml");
        let mut store = Store::load(&path);

        assert_eq!(
            store.config().appearance.background.mode,
            BackgroundMode::Image
        );

        store.set_background_color("#abc").unwrap();
        let reloaded = Store::load(&path);
        let background = &reloaded.config().appearance.background;
        assert_eq!(background.mode, BackgroundMode::Color);
        assert_eq!(background.color, "#AABBCC");

        store.set_background_mode(BackgroundMode::Image).unwrap();
        assert_eq!(
            Store::load(&path).config().appearance.background.mode,
            BackgroundMode::Image
        );
        assert!(store.set_background_color("not-a-color").is_err());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saves_wallpaper_list_and_random_mode() {
        let dir = std::env::temp_dir().join(format!(
            "neutrino-conf-wallpaper-test-{}",
            std::process::id()
        ));
        let path = dir.join("conf.toml");
        let mut store = Store::load(&path);

        let night = PathBuf::from(r"C:\Pictures\night.png");
        let day = PathBuf::from(r"C:\Pictures\day.png");
        store
            .add_custom_wallpapers(&[night.clone(), day.clone()])
            .unwrap();

        let reloaded = Store::load(&path);
        let background = &reloaded.config().appearance.background;
        assert_eq!(
            background.wallpapers,
            vec![
                r"C:\Pictures\night.png".to_string(),
                r"C:\Pictures\day.png".to_string()
            ]
        );
        assert_eq!(background.custom_path(), Some(night.as_path()));
        assert!(!background.random);

        store.set_random_background(true).unwrap();
        assert!(Store::load(&path).config().appearance.background.random);

        store
            .select_custom_wallpaper(&day.display().to_string())
            .unwrap();
        let reloaded = Store::load(&path);
        assert_eq!(
            reloaded.config().appearance.background.custom_path(),
            Some(day.as_path())
        );
        assert!(!reloaded.config().appearance.background.random);

        store
            .remove_custom_wallpaper(&day.display().to_string())
            .unwrap();
        let reloaded = Store::load(&path);
        assert_eq!(
            reloaded.config().appearance.background.custom_path(),
            Some(night.as_path())
        );

        store
            .remove_custom_wallpaper(&night.display().to_string())
            .unwrap();
        let reloaded = Store::load(&path);
        assert!(
            reloaded
                .config()
                .appearance
                .background
                .wallpapers
                .is_empty()
        );
        assert_eq!(
            reloaded.config().appearance.background.builtin_name(),
            Some("Jokull")
        );
        assert!(!reloaded.config().appearance.background.random);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn canonical_keys_round_trip() {
        let config = Config {
            appearance: Appearance {
                theme: ThemeConfig {
                    color_mode: ColorMode::Dark,
                },
                background: BackgroundConfig {
                    choice: "%built-in:Jokull".into(),
                    ..Default::default()
                },
            },
        };

        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("[appearance.theme]"));
        assert!(text.contains("colormode = \"dark\""));
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }

    #[test]
    fn unknown_values_fall_back_to_defaults() {
        let config: Config = toml::from_str("[appearance.theme]\ncolormode = \"auto\"\n").unwrap();

        assert_eq!(config.appearance.theme.color_mode, ColorMode::System);
        assert_eq!(
            config.appearance.background.choice,
            DEFAULT_BACKGROUND.to_string()
        );
    }

    #[test]
    fn missing_fields_use_defaults() {
        let config: Config = toml::from_str("[appearance.theme]\ncolormode = \"dark\"\n").unwrap();

        assert_eq!(config.appearance.theme.color_mode, ColorMode::Dark);
        assert_eq!(config.appearance.background.choice, DEFAULT_BACKGROUND);
    }

    #[test]
    fn saves_to_disk() {
        let dir = std::env::temp_dir().join(format!("neutrino-conf-test-{}", std::process::id()));
        let path = dir.join("conf.toml");
        let mut store = Store::load(&path);

        assert_eq!(
            store.config().appearance.theme.color_mode,
            ColorMode::System
        );
        store.set_color_mode(ColorMode::Light).unwrap();

        let reloaded = Store::load(&path);
        assert_eq!(
            reloaded.config().appearance.theme.color_mode,
            ColorMode::Light
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
