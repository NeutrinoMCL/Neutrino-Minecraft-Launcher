use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountKind {
    Microsoft,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skin {
    Steve,
    Alex,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicrosoftAuth {
    pub uuid: String,
    /// Minecraft services access token.
    pub access_token: String,
    /// Microsoft OAuth refresh token.
    pub refresh_token: String,
    /// Unix timestamp (seconds) when the Minecraft access token expires.
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub kind: AccountKind,
    pub skin: Skin,
    /// 仅离线角色有：自定义披风图片路径。
    pub cape: Option<String>,
    /// 是否在 3D 预览里渲染披风。
    pub cape_enabled: bool,
    /// 仅 Microsoft 账号有；离线账号为 None。
    pub microsoft: Option<MicrosoftAuth>,
}

#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
    accounts: Vec<Account>,
    active: Option<String>,
}

/// `account.toml` 的顶层结构。
#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    accounts: Vec<AccountFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AccountFile {
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    skin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skin_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cape_path: Option<String>,
    #[serde(default)]
    cape_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    microsoft: Option<MicrosoftAuthFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MicrosoftAuthFile {
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_at: u64,
}

impl Store {
    pub fn load_default() -> Self {
        Self::load(accounts_file())
    }

    pub fn load(path: PathBuf) -> Self {
        if path.is_file() {
            let (accounts, active) = read_toml(&path);
            return Self {
                path,
                accounts,
                active,
            };
        }

        // 从旧版 accounts.json 自动迁移一次；TOML 写成功后才删除旧文件。
        let legacy = path.with_file_name("accounts.json");
        if legacy.is_file()
            && let Some((accounts, active)) = read_legacy_json(&legacy)
        {
            let store = Self {
                path,
                accounts,
                active,
            };
            if store.save().is_ok() {
                let _ = fs::remove_file(&legacy);
            }
            return store;
        }

        Self {
            path,
            accounts: Vec::new(),
            active: None,
        }
    }

    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    pub fn account(&self, name: &str) -> Option<&Account> {
        self.index_of(name).map(|index| &self.accounts[index])
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        self.accounts
            .iter()
            .position(|account| account.name.eq_ignore_ascii_case(name))
    }

    pub fn active_account(&self) -> Option<&Account> {
        self.active().and_then(|name| self.account(name))
    }

    /// 执行一次修改并保存；保存失败时恢复内存里的账号列表和当前账号。
    fn update<T>(
        &mut self,
        apply: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let previous_accounts = self.accounts.clone();
        let previous_active = self.active.clone();

        let result = apply(self);
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.accounts = previous_accounts;
                self.active = previous_active;
                return Err(error);
            }
        };

        if let Err(error) = self.save() {
            self.accounts = previous_accounts;
            self.active = previous_active;
            return Err(error);
        }

        Ok(value)
    }

    pub fn add_offline(&mut self, name: &str) -> Result<(), String> {
        validate_name(name)?;
        if self.account(name).is_some() {
            return Err("该角色名已存在".into());
        }

        self.update(|store| {
            store.accounts.push(Account {
                name: name.to_string(),
                kind: AccountKind::Offline,
                skin: Skin::Steve,
                cape: None,
                cape_enabled: false,
                microsoft: None,
            });
            store.active = Some(name.to_string());
            Ok(())
        })
    }

    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        if self.account(name).is_none() {
            return Err("未找到该角色".into());
        }

        self.update(|store| {
            store
                .accounts
                .retain(|account| !account.name.eq_ignore_ascii_case(name));

            if store
                .active
                .as_deref()
                .is_some_and(|active| active.eq_ignore_ascii_case(name))
            {
                store.active = store.accounts.first().map(|account| account.name.clone());
            }
            Ok(())
        })
    }

    /// 登录成功后写入 / 更新 Microsoft 账号，并设为当前账号。
    pub fn upsert_microsoft(
        &mut self,
        name: &str,
        uuid: &str,
        access_token: &str,
        refresh_token: &str,
        expires_at: u64,
    ) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("Microsoft 账号名为空".into());
        }
        if uuid.trim().is_empty() {
            return Err("Microsoft 账号 UUID 为空".into());
        }

        self.update(|store| {
            let auth = MicrosoftAuth {
                uuid: uuid.to_string(),
                access_token: access_token.to_string(),
                refresh_token: refresh_token.to_string(),
                expires_at,
            };

            // 优先按 UUID 找（改名时更新名称），其次按名字找（把离线角色升级成 Microsoft 账号）。
            let index = store
                .accounts
                .iter()
                .position(|account| {
                    account
                        .microsoft
                        .as_ref()
                        .is_some_and(|auth| auth.uuid == uuid)
                })
                .or_else(|| store.index_of(name));

            if let Some(index) = index {
                let account = &mut store.accounts[index];
                account.name = name.to_string();
                account.kind = AccountKind::Microsoft;
                account.cape = None;
                account.cape_enabled = false;
                account.microsoft = Some(auth);
            } else {
                store.accounts.push(Account {
                    name: name.to_string(),
                    kind: AccountKind::Microsoft,
                    skin: Skin::Steve,
                    cape: None,
                    cape_enabled: false,
                    microsoft: Some(auth),
                });
            }

            store.active = Some(name.to_string());
            Ok(())
        })
    }

    /// 刷新 token 后更新账号里保存的认证信息。
    pub fn update_microsoft_tokens(
        &mut self,
        name: &str,
        access_token: &str,
        refresh_token: &str,
        expires_at: u64,
    ) -> Result<(), String> {
        self.update(|store| {
            let Some(account) = store
                .accounts
                .iter_mut()
                .find(|account| account.name.eq_ignore_ascii_case(name))
            else {
                return Err("未找到该角色".into());
            };
            let Some(auth) = account.microsoft.as_mut() else {
                return Err("该角色不是 Microsoft 账号".into());
            };

            auth.access_token = access_token.to_string();
            auth.refresh_token = refresh_token.to_string();
            auth.expires_at = expires_at;
            Ok(())
        })
    }

    pub fn set_active(&mut self, name: &str) -> Result<(), String> {
        if self.account(name).is_none() {
            return Err("未找到该角色".into());
        }

        self.update(|store| {
            store.active = Some(name.to_string());
            Ok(())
        })
    }

    pub fn set_skin(&mut self, name: &str, skin: Skin) -> Result<(), String> {
        self.update(|store| {
            let Some(account) = store
                .accounts
                .iter_mut()
                .find(|account| account.name.eq_ignore_ascii_case(name))
            else {
                return Err("未找到该角色".into());
            };

            account.skin = skin;
            Ok(())
        })
    }

    /// 设置离线角色的自定义披风；传 `None` 表示清除。设置后默认开启渲染。
    pub fn set_cape(&mut self, name: &str, cape: Option<String>) -> Result<(), String> {
        self.update(|store| {
            let Some(account) = store
                .accounts
                .iter_mut()
                .find(|account| account.name.eq_ignore_ascii_case(name))
            else {
                return Err("未找到该角色".into());
            };
            if account.kind != AccountKind::Offline {
                return Err("只有离线角色可以设置自定义披风".into());
            }

            account.cape_enabled = cape.is_some();
            account.cape = cape;
            Ok(())
        })
    }

    /// 开启 / 关闭披风渲染，不影响已经设置的披风文件。
    pub fn set_cape_enabled(&mut self, name: &str, enabled: bool) -> Result<(), String> {
        self.update(|store| {
            let Some(account) = store
                .accounts
                .iter_mut()
                .find(|account| account.name.eq_ignore_ascii_case(name))
            else {
                return Err("未找到该角色".into());
            };
            if account.cape.is_none() {
                return Err("该角色还没有自定义披风".into());
            }

            account.cape_enabled = enabled;
            Ok(())
        })
    }

    fn save(&self) -> Result<(), String> {
        let file = StoreFile {
            accounts: self
                .accounts
                .iter()
                .map(|account| {
                    let (skin, skin_path) = match &account.skin {
                        Skin::Steve => ("steve".to_string(), None),
                        Skin::Alex => ("alex".to_string(), None),
                        Skin::Custom(path) => ("custom".to_string(), Some(path.clone())),
                    };

                    AccountFile {
                        name: account.name.clone(),
                        kind: match account.kind {
                            AccountKind::Microsoft => "microsoft",
                            AccountKind::Offline => "offline",
                        }
                        .to_string(),
                        skin,
                        skin_path,
                        cape_path: account.cape.clone(),
                        cape_enabled: account.cape_enabled,
                        microsoft: account.microsoft.as_ref().map(|auth| MicrosoftAuthFile {
                            uuid: auth.uuid.clone(),
                            access_token: auth.access_token.clone(),
                            refresh_token: auth.refresh_token.clone(),
                            expires_at: auth.expires_at,
                        }),
                    }
                })
                .collect(),
            active: self.active.clone(),
        };

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("创建账号目录失败：{error}"))?;
        }

        // token 保存在 %APPDATA%/Neutrino/account.toml（Windows），
        // 属于当前用户；不要把这份文件提交到仓库或分享出去。
        let text = toml::to_string_pretty(&file)
            .map_err(|error| format!("序列化账号数据失败：{error}"))?;
        fs::write(&self.path, text).map_err(|error| format!("保存账号数据失败：{error}"))
    }
}

pub fn validate_name(name: &str) -> Result<(), String> {
    let length = name.chars().count();
    if !(3..=16).contains(&length) {
        return Err("角色名长度需为 3-16 个字符".into());
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err("角色名只能包含字母、数字和下划线".into());
    }
    Ok(())
}

/// Opens a native file picker and copies the chosen PNG into the app's skin
/// directory. Returns `Ok(None)` when the user cancels the dialog.
pub fn pick_custom_skin(name: &str) -> Result<Option<Skin>, String> {
    let Some(source) = rfd::FileDialog::new()
        .add_filter("PNG 图片", &["png"])
        .set_title("选择皮肤图片")
        .pick_file()
    else {
        return Ok(None);
    };

    let dir = skins_dir();
    fs::create_dir_all(&dir).map_err(|error| format!("创建皮肤目录失败：{error}"))?;

    let destination = dir.join(format!("{name}.png"));
    fs::copy(&source, &destination).map_err(|error| format!("复制皮肤文件失败：{error}"))?;

    Ok(Some(Skin::Custom(destination.display().to_string())))
}

pub fn pick_custom_cape(name: &str) -> Result<Option<String>, String> {
    let Some(source) = rfd::FileDialog::new()
        .add_filter("PNG 图片", &["png"])
        .set_title("选择披风图片")
        .pick_file()
    else {
        return Ok(None);
    };

    let (width, height) =
        image::image_dimensions(&source).map_err(|error| format!("读取披风图片失败：{error}"))?;
    if width < 64 || width % 64 != 0 || height != width / 2 {
        return Err("披风图片必须是 64×32 或 128×64 的 PNG 模板".to_string());
    }

    let dir = capes_dir();
    fs::create_dir_all(&dir).map_err(|error| format!("创建披风目录失败：{error}"))?;

    let destination = dir.join(format!("{name}.png"));
    fs::copy(&source, &destination).map_err(|error| format!("复制披风文件失败：{error}"))?;

    Ok(Some(destination.display().to_string()))
}

fn accounts_file() -> PathBuf {
    data_dir().join("account.toml")
}

fn skins_dir() -> PathBuf {
    data_dir().join("skins")
}

fn capes_dir() -> PathBuf {
    data_dir().join("capes")
}

/// 启动器缓存目录（图标等）。
pub(crate) fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

pub(crate) fn data_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Neutrino")
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Library")
            .join("Application Support")
            .join("Neutrino")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".local").join("share"))
            })
            .unwrap_or_default()
            .join("neutrino")
    }
}

fn read_toml(path: &Path) -> (Vec<Account>, Option<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return (Vec::new(), None);
    };
    let Ok(file) = toml::from_str::<StoreFile>(&text) else {
        return (Vec::new(), None);
    };

    let accounts = file
        .accounts
        .into_iter()
        .filter_map(account_from_file)
        .collect::<Vec<_>>();
    let active = file.active.filter(|active| {
        accounts
            .iter()
            .any(|account| account.name.eq_ignore_ascii_case(active))
    });

    (accounts, active)
}

fn account_from_file(item: AccountFile) -> Option<Account> {
    let AccountFile {
        name,
        kind,
        skin,
        skin_path,
        cape_path,
        cape_enabled,
        microsoft,
    } = item;
    if name.is_empty() {
        return None;
    }

    let kind = match kind.as_str() {
        "microsoft" => AccountKind::Microsoft,
        _ => AccountKind::Offline,
    };
    let skin = match skin.as_str() {
        "alex" => Skin::Alex,
        "custom" => Skin::Custom(skin_path.unwrap_or_default()),
        _ => Skin::Steve,
    };
    let cape = cape_path.filter(|path| !path.trim().is_empty());
    let cape_enabled = kind == AccountKind::Offline && cape_enabled && cape.is_some();
    let microsoft = microsoft.and_then(|auth| {
        if auth.uuid.is_empty() {
            return None;
        }
        Some(MicrosoftAuth {
            uuid: auth.uuid,
            access_token: auth.access_token,
            refresh_token: auth.refresh_token,
            expires_at: auth.expires_at,
        })
    });

    Some(Account {
        name,
        kind,
        skin,
        cape,
        cape_enabled,
        microsoft,
    })
}

/// 读取旧版 `accounts.json`，只用于一次性迁移到 `account.toml`。
fn read_legacy_json(path: &Path) -> Option<(Vec<Account>, Option<String>)> {
    let text = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;

    let accounts = value
        .get("accounts")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name")?.as_str()?.to_string();
                    let kind = match item.get("kind").and_then(Value::as_str) {
                        Some("microsoft") => AccountKind::Microsoft,
                        _ => AccountKind::Offline,
                    };
                    let skin = match item
                        .get("skin")
                        .and_then(|skin| skin.get("type"))
                        .and_then(Value::as_str)
                    {
                        Some("alex") => Skin::Alex,
                        Some("custom") => {
                            let path = item
                                .get("skin")
                                .and_then(|skin| skin.get("path"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            Skin::Custom(path)
                        }
                        _ => Skin::Steve,
                    };
                    let microsoft = item.get("microsoft").and_then(|auth| {
                        Some(MicrosoftAuth {
                            uuid: auth.get("uuid")?.as_str()?.to_string(),
                            access_token: auth.get("access_token")?.as_str()?.to_string(),
                            refresh_token: auth.get("refresh_token")?.as_str()?.to_string(),
                            expires_at: auth.get("expires_at").and_then(Value::as_u64).unwrap_or(0),
                        })
                    });
                    Some(Account {
                        name,
                        kind,
                        skin,
                        cape: None,
                        cape_enabled: false,
                        microsoft,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let active = value
        .get("active")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|active| {
            accounts
                .iter()
                .any(|account| account.name.eq_ignore_ascii_case(active))
        });

    Some((accounts, active))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "neutrino-accounts-test-{label}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn store(&self) -> Store {
            Store::load(self.path.join("account.toml"))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn adds_offline_account_and_activates_it() {
        let temp = TempDir::new("add");
        let mut store = temp.store();

        store.add_offline("Steve").expect("add account");

        assert_eq!(
            store.accounts(),
            &[Account {
                name: "Steve".into(),
                kind: AccountKind::Offline,
                skin: Skin::Steve,
                cape: None,
                cape_enabled: false,
                microsoft: None,
            }]
        );
        assert_eq!(store.active(), Some("Steve"));

        let reloaded = temp.store();
        assert_eq!(reloaded.accounts(), store.accounts());
        assert_eq!(reloaded.active(), Some("Steve"));
    }

    #[test]
    fn saves_account_toml() {
        let temp = TempDir::new("toml");
        let mut store = temp.store();
        store.add_offline("Steve").expect("add account");

        let text = fs::read_to_string(temp.path.join("account.toml")).expect("read toml");
        assert!(text.contains("[[accounts]]"));
        assert!(text.contains("name = \"Steve\""));
        assert!(text.contains("kind = \"offline\""));
        assert!(text.contains("skin = \"steve\""));
    }

    #[test]
    fn migrates_legacy_accounts_json_to_toml() {
        let temp = TempDir::new("migrate");
        let legacy = temp.path.join("accounts.json");
        fs::write(
            &legacy,
            r#"{
                "accounts": [
                    { "name": "Steve", "kind": "offline", "skin": { "type": "steve" } }
                ],
                "active": "Steve"
            }"#,
        )
        .expect("write legacy json");

        let store = Store::load(temp.path.join("account.toml"));
        assert_eq!(store.accounts().len(), 1);
        assert_eq!(store.active(), Some("Steve"));
        assert!(temp.path.join("account.toml").is_file());
        assert!(!legacy.exists());
    }

    #[test]
    fn custom_skin_survives_toml_roundtrip() {
        let temp = TempDir::new("skin-toml");
        let mut store = temp.store();
        store.add_offline("Steve").expect("add account");
        store
            .set_skin("Steve", Skin::Custom(r"C:\skins\steve.png".into()))
            .expect("set custom skin");

        assert_eq!(
            temp.store().accounts()[0].skin,
            Skin::Custom(r"C:\skins\steve.png".into())
        );
    }

    #[test]
    fn custom_cape_survives_toml_roundtrip_and_can_be_toggled() {
        let temp = TempDir::new("cape-toml");
        let mut store = temp.store();
        store.add_offline("Steve").expect("add account");
        store
            .set_cape("Steve", Some(r"C:\capes\steve.png".into()))
            .expect("set custom cape");

        let reloaded = temp.store();
        let account = &reloaded.accounts()[0];
        assert_eq!(account.cape.as_deref(), Some(r"C:\capes\steve.png"));
        assert!(account.cape_enabled);

        store
            .set_cape_enabled("Steve", false)
            .expect("disable cape");
        assert!(!temp.store().accounts()[0].cape_enabled);

        store.set_cape("Steve", None).expect("clear cape");
        assert!(temp.store().accounts()[0].cape.is_none());
        assert!(!temp.store().accounts()[0].cape_enabled);
    }

    #[test]
    fn microsoft_accounts_cannot_set_custom_cape() {
        let temp = TempDir::new("cape-microsoft");
        let mut store = temp.store();
        store
            .upsert_microsoft("Alex", "uuid-1", "access-1", "refresh-1", 100)
            .expect("add microsoft account");

        assert!(
            store
                .set_cape("Alex", Some(r"C:\capes\alex.png".into()))
                .is_err()
        );
    }

    #[test]
    fn rejects_invalid_and_duplicate_names() {
        let temp = TempDir::new("validate");
        let mut store = temp.store();

        assert!(store.add_offline("ab").is_err());
        assert!(store.add_offline("name with space").is_err());
        assert!(store.add_offline("名字").is_err());

        store.add_offline("Steve").expect("add account");
        assert!(store.add_offline("steve").is_err());
    }

    #[test]
    fn stores_microsoft_account_and_refreshes_tokens() {
        let temp = TempDir::new("microsoft");
        let mut store = temp.store();

        store
            .upsert_microsoft("Alex", "uuid-1", "access-1", "refresh-1", 100)
            .expect("add microsoft account");

        assert_eq!(store.active(), Some("Alex"));
        let account = store.account("Alex").expect("account");
        assert_eq!(account.kind, AccountKind::Microsoft);
        let auth = account.microsoft.as_ref().expect("microsoft auth");
        assert_eq!(auth.uuid, "uuid-1");
        assert_eq!(auth.access_token, "access-1");

        let reloaded = temp.store();
        let account = reloaded.account("Alex").expect("reloaded account");
        let auth = account.microsoft.as_ref().expect("reloaded auth");
        assert_eq!(auth.refresh_token, "refresh-1");
        assert_eq!(auth.expires_at, 100);

        store
            .update_microsoft_tokens("Alex", "access-2", "refresh-2", 200)
            .expect("update tokens");
        let reloaded = temp.store();
        let account = reloaded.account("Alex").expect("reloaded account");
        let auth = account.microsoft.as_ref().expect("reloaded auth");
        assert_eq!(auth.access_token, "access-2");
        assert_eq!(auth.refresh_token, "refresh-2");
        assert_eq!(auth.expires_at, 200);
    }

    #[test]
    fn upgrading_an_offline_account_keeps_its_skin() {
        let temp = TempDir::new("microsoft-upgrade");
        let mut store = temp.store();

        store.add_offline("Steve").expect("add offline");
        store.set_skin("Steve", Skin::Alex).expect("set skin");
        store
            .upsert_microsoft("Steve", "uuid-2", "a", "r", 1)
            .expect("upgrade account");

        let account = store.account("Steve").expect("account");
        assert_eq!(account.kind, AccountKind::Microsoft);
        assert_eq!(account.skin, Skin::Alex);
        assert!(account.microsoft.is_some());
    }

    #[test]
    fn removing_active_account_selects_another() {
        let temp = TempDir::new("remove");
        let mut store = temp.store();

        store.add_offline("Steve").expect("add Steve");
        store.add_offline("Alex").expect("add Alex");
        assert_eq!(store.active(), Some("Alex"));

        store.remove("Alex").expect("remove Alex");

        assert_eq!(store.accounts().len(), 1);
        assert_eq!(store.active(), Some("Steve"));

        store.remove("Steve").expect("remove Steve");
        assert!(store.accounts().is_empty());
        assert_eq!(store.active(), None);
    }

    #[test]
    fn only_existing_accounts_can_be_activated() {
        let temp = TempDir::new("active");
        let mut store = temp.store();

        assert!(store.set_active("Steve").is_err());

        store.add_offline("Steve").expect("add Steve");
        store.add_offline("Alex").expect("add Alex");
        store.set_active("Steve").expect("activate Steve");

        assert_eq!(temp.store().active(), Some("Steve"));
    }

    #[test]
    fn skin_changes_persist() {
        let temp = TempDir::new("skin");
        let mut store = temp.store();

        store.add_offline("Steve").expect("add Steve");
        store
            .set_skin("Steve", Skin::Custom("/some/skin.png".into()))
            .expect("set custom skin");
        store.set_skin("Steve", Skin::Alex).expect("set alex skin");

        assert_eq!(temp.store().accounts()[0].skin, Skin::Alex);

        assert!(store.set_skin("Missing", Skin::Alex).is_err());
    }
}
