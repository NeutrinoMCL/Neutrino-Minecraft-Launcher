use std::time::{SystemTime, UNIX_EPOCH};

use minecraft_msa_auth::{MinecraftAuthorizationError, MinecraftAuthorizationFlow, UreqClient};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, ClientId, DeviceAuthorizationUrl, RefreshToken, Scope,
    StandardDeviceAuthorizationResponse, TokenResponse, TokenUrl,
};

const CLIENT_ID_ENV: &str = "NEUTRINO_MSA_CLIENT_ID";
const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const AUTHORIZE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const MINECRAFT_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// 登录成功后拿到的完整 Minecraft 认证信息。
#[derive(Debug, Clone)]
pub struct MsaSession {
    pub player_name: String,
    /// Minecraft 档案里的玩家 UUID（32 位十六进制，无连字符）。
    pub uuid: String,
    /// Minecraft services access token。
    pub access_token: String,
    /// Microsoft OAuth refresh token。
    pub refresh_token: String,
    /// access token 过期时间（Unix 秒）。
    pub expires_at: u64,
}

pub enum LoginEvent {
    DeviceCode {
        verification_uri: String,
        user_code: String,
    },
    Success(MsaSession),
    Error(String),
}

pub fn client_id() -> Result<String, String> {
    if let Some(client_id) = std::env::var(CLIENT_ID_ENV)
        .ok()
        .filter(|client_id| !client_id.trim().is_empty())
    {
        return Ok(client_id);
    }

    option_env!("NEUTRINO_MSA_CLIENT_ID")
        .filter(|client_id| !client_id.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("未设置 {CLIENT_ID_ENV} 环境变量"))
}

pub fn login<F>(client_id: String, on_event: F) -> Result<(), String>
where
    F: Fn(LoginEvent),
{
    let result = login_inner(client_id, &on_event);
    if let Err(error) = &result {
        on_event(LoginEvent::Error(error.clone()));
    }
    result
}

fn login_inner<F>(client_id: String, on_event: &F) -> Result<(), String>
where
    F: Fn(LoginEvent),
{
    let client = BasicClient::new(ClientId::new(client_id))
        .set_auth_uri(AuthUrl::new(AUTHORIZE_URL.to_string()).map_err(|error| error.to_string())?)
        .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).map_err(|error| error.to_string())?)
        .set_device_authorization_url(
            DeviceAuthorizationUrl::new(DEVICE_CODE_URL.to_string())
                .map_err(|error| error.to_string())?,
        );

    let http_client = http_client()?;

    let details: StandardDeviceAuthorizationResponse = client
        .exchange_device_code()
        .add_scope(Scope::new("XboxLive.signin offline_access".to_string()))
        .request(&http_client)
        .map_err(|error| error.to_string())?;

    let verification_uri = details.verification_uri().to_string();
    let user_code = details.user_code().secret().to_string();

    let _ = crate::open_url(&verification_uri);
    on_event(LoginEvent::DeviceCode {
        verification_uri,
        user_code,
    });

    let token = client
        .exchange_device_access_token(&details)
        .request(&http_client, std::thread::sleep, None)
        .map_err(|error| error.to_string())?;

    let refresh_token = token
        .refresh_token()
        .map(|token| token.secret().to_string())
        .ok_or_else(|| "Microsoft 没有返回 refresh token".to_string())?;

    let session = finish_login(token.access_token().secret(), refresh_token)?;
    on_event(LoginEvent::Success(session));
    Ok(())
}

/// 用 refresh token 换一份新的 Microsoft token，再换 Minecraft token。
pub fn refresh_session(client_id: String, refresh_token: String) -> Result<MsaSession, String> {
    if refresh_token.trim().is_empty() {
        return Err("Microsoft refresh token 为空，请重新登录".to_string());
    }

    let client = BasicClient::new(ClientId::new(client_id))
        .set_auth_uri(AuthUrl::new(AUTHORIZE_URL.to_string()).map_err(|error| error.to_string())?)
        .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).map_err(|error| error.to_string())?)
        .set_device_authorization_url(
            DeviceAuthorizationUrl::new(DEVICE_CODE_URL.to_string())
                .map_err(|error| error.to_string())?,
        );
    let http_client = http_client()?;

    let token = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token.clone()))
        .request(&http_client)
        .map_err(|error| format!("刷新 Microsoft token 失败：{error}"))?;

    let refresh_token = token
        .refresh_token()
        .map(|token| token.secret().to_string())
        .unwrap_or(refresh_token);

    finish_login(token.access_token().secret(), refresh_token)
}

fn finish_login(microsoft_access_token: &str, refresh_token: String) -> Result<MsaSession, String> {
    let minecraft = MinecraftAuthorizationFlow::new(UreqClient::default())
        .exchange_microsoft_token(microsoft_access_token)
        .map_err(|error| {
            if matches!(&error, MinecraftAuthorizationError::HttpStatus(status) if status.as_u16() == 403) {
                "Minecraft Services 返回 403 Invalid app registration：当前 Client ID 没有被 Mojang 加入白名单。\
                 请先为应用申请 Minecraft 第三方授权，或使用已获批准的 Client ID。"
                    .to_string()
            } else {
                format!("Xbox / Minecraft 认证失败：{error}")
            }
        })?;

    let access_token = minecraft.access_token().as_ref().to_string();
    let expires_at = unix_now().saturating_add(u64::from(minecraft.expires_in()));
    let profile = fetch_profile(&access_token)?;

    Ok(MsaSession {
        player_name: profile.name,
        uuid: profile.id,
        access_token,
        refresh_token,
        expires_at,
    })
}

struct MinecraftProfile {
    id: String,
    name: String,
}

fn fetch_profile(access_token: &str) -> Result<MinecraftProfile, String> {
    let mut response = ureq::get(MINECRAFT_PROFILE_URL)
        .header("Authorization", &format!("Bearer {access_token}"))
        .call()
        .map_err(|error| {
            if matches!(&error, ureq::Error::StatusCode(status) if *status == 403) {
                "获取 Minecraft 档案失败（403）：当前 Client ID 可能未被 Mojang 批准，\
                 或该账号没有 Minecraft: Java Edition。请先确认账号拥有 Java 版，\
                 并为应用申请 Minecraft 第三方授权。"
                    .to_string()
            } else {
                format!("获取 Minecraft 档案失败（该账号可能没有 Java 版或登录已失效）：{error}")
            }
        })?;

    let profile: serde_json::Value = response
        .body_mut()
        .read_json()
        .map_err(|error| format!("解析 Minecraft 档案失败：{error}"))?;

    let id = profile
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Minecraft 档案缺少 UUID".to_string())?
        .to_string();
    let name = profile
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "Minecraft 档案缺少玩家名".to_string())?
        .to_string();

    Ok(MinecraftProfile { id, name })
}

fn http_client() -> Result<oauth2::reqwest::blocking::Client, String> {
    oauth2::reqwest::blocking::ClientBuilder::new()
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
