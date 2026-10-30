//! Microsoft account authentication for Minecraft.
//!
//! The flow is the same one used by other third-party launchers:
//!
//! 1. Request a device code and let the user sign in at
//!    <https://www.microsoft.com/link>.
//! 2. Exchange the Microsoft OAuth access token for an Xbox Live token.
//! 3. Authorize that token against the Minecraft relying party with XSTS.
//! 4. Exchange the XSTS token for a Minecraft services access token.
//! 5. Read the player profile (name + UUID) from the Minecraft API.
//!
//! Only the device code flow is implemented: it needs no redirect URI, no
//! local HTTP listener and no client secret, which keeps the launcher usable
//! from a terminal without any manual URL handling.

use std::time::{Duration, Instant};

use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::auth::{Account, AccountKind};
use crate::download::build_http_client;
use crate::error::{Error, Result};
use crate::progress::{Phase, ProgressEvent, ProgressSink};
use crate::util::unix_now;

/// Environment variable used to configure the Azure application client id.
///
/// When set while building `shccl-core`, the value is baked into the binary via
/// [`COMPILED_CLIENT_ID`]. A runtime value takes precedence, which keeps local
/// development and testing convenient.
pub const CLIENT_ID_ENV: &str = "SHCCL_MICROSOFT_CLIENT_ID";

/// Azure application client id supplied at compile time.
///
/// Distribution builds should set `SHCCL_MICROSOFT_CLIENT_ID` so end users do
/// not have to configure anything. The app is a public client: no secret is
/// involved.
pub const COMPILED_CLIENT_ID: Option<&str> = option_env!("SHCCL_MICROSOFT_CLIENT_ID");

/// Explain how to obtain a usable client id.
pub const CLIENT_ID_HELP: &str = "Microsoft login needs an Azure application client ID. \
Register a public client app at https://portal.azure.com (supported account types: personal \
Microsoft accounts; Authentication -> Allow public client flows: Yes; no redirect URI or client \
secret needed), then apply for Minecraft API access at https://aka.ms/mce-reviewappid. \
Build the launcher with SHCCL_MICROSOFT_CLIENT_ID=<ID>, or set that variable at runtime, then run \
`shccl login microsoft` again.";

/// Resolve the client id, preferring a runtime override over the compiled-in
/// value. Empty values are ignored.
pub fn configured_client_id() -> Option<String> {
    fn clean(value: &str) -> Option<String> {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    }

    std::env::var(CLIENT_ID_ENV)
        .ok()
        .and_then(|value| clean(&value))
        .or_else(|| COMPILED_CLIENT_ID.and_then(clean))
}

const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const SCOPE: &str = "XboxLive.signin offline_access";
const XBOX_LIVE_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MINECRAFT_LOGIN_URL: &str =
    "https://api.minecraftservices.com/authentication/login_with_xbox";
const MINECRAFT_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// Legacy Live Connect client id used by the official Minecraft launcher.
///
/// It only supports the browser authorization flow; the device code flow is
/// not available for this client. Use it through the opt-in legacy login.
pub const LEGACY_CLIENT_ID: &str = "00000000402b5328";

/// Fixed redirect URI registered for [`LEGACY_CLIENT_ID`]. Localhost callbacks
/// are rejected, so the user has to paste the final redirect URL.
pub const LEGACY_REDIRECT_URI: &str = "https://login.live.com/oauth20_desktop.srf";

const LEGACY_AUTHORIZE_URL: &str = "https://login.live.com/oauth20_authorize.srf";
const LEGACY_TOKEN_URL: &str = "https://login.live.com/oauth20_token.srf";
const LEGACY_REDIRECT_URI_ENCODED: &str = "https%3A%2F%2Flogin.live.com%2Foauth20_desktop.srf";

/// Browser URL for the legacy Live Connect authorization code flow.
pub fn legacy_authorization_url() -> String {
    format!(
        "{LEGACY_AUTHORIZE_URL}?client_id={LEGACY_CLIENT_ID}&response_type=code\
         &scope=XboxLive.signin%20offline_access\
         &redirect_uri={LEGACY_REDIRECT_URI_ENCODED}&prompt=select_account"
    )
}

/// Error returned when no client id is configured.
pub fn missing_client_id() -> Error {
    Error::Auth(CLIENT_ID_HELP.to_string())
}

fn require_client_id(client_id: &str) -> Result<&str> {
    let client_id = client_id.trim();
    if client_id.is_empty() {
        Err(missing_client_id())
    } else {
        Ok(client_id)
    }
}

/// A pending Microsoft device-code login.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    /// Short code the user types into the verification page.
    #[serde(default)]
    pub user_code: String,
    /// Opaque code polled against the token endpoint.
    #[serde(default)]
    pub device_code: String,
    /// Page the user must open (normally <https://www.microsoft.com/link>).
    #[serde(default)]
    pub verification_uri: String,
    /// Seconds between polls requested by Microsoft.
    #[serde(default = "default_interval")]
    pub interval: u64,
    /// Lifetime of the device code in seconds.
    #[serde(default = "default_expires_in")]
    pub expires_in: u64,
    /// Human readable instructions returned by Microsoft.
    #[serde(default)]
    pub message: String,
}

fn default_interval() -> u64 {
    5
}

fn default_expires_in() -> u64 {
    900
}

/// Tokens returned by the Microsoft OAuth token endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct MsTokens {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: u64,
}

/// The subset of the Minecraft profile used for launching.
#[derive(Debug, Clone, Deserialize)]
pub struct MinecraftProfile {
    /// Profile id: 32 lowercase hex characters without dashes.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// A fully authenticated Minecraft session.
#[derive(Debug, Clone)]
pub struct MinecraftSession {
    pub access_token: String,
    pub expires_in: u64,
    pub xuid: String,
    pub profile: MinecraftProfile,
}

/// Client for the Microsoft / Xbox / Minecraft authentication chain.
#[derive(Clone)]
pub struct MicrosoftAuth {
    http: Client,
}

impl MicrosoftAuth {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: build_http_client()?,
        })
    }

    pub(crate) fn with_client(http: Client) -> Self {
        Self { http }
    }

    /// Start a device-code login. The returned code must be shown to the user
    /// before calling [`MicrosoftAuth::wait_for_login`].
    pub async fn request_device_code(&self, client_id: &str) -> Result<DeviceCode> {
        let client_id = require_client_id(client_id)?;
        let response = self
            .http
            .post(DEVICE_CODE_URL)
            .form(&[("client_id", client_id), ("scope", SCOPE)])
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(device_code_error(status, &body));
        }
        let code: DeviceCode = serde_json::from_str(&body)?;
        if code.device_code.is_empty() || code.user_code.is_empty() {
            return Err(Error::Auth(
                "Microsoft returned an incomplete device code".into(),
            ));
        }
        Ok(code)
    }

    /// Poll until the user finishes signing in, the code expires or the login
    /// is declined. Progress is reported as [`Phase::Auth`] steps.
    pub async fn wait_for_login(
        &self,
        client_id: &str,
        code: &DeviceCode,
        sink: &ProgressSink,
    ) -> Result<MsTokens> {
        let client_id = require_client_id(client_id)?;
        let mut interval = Duration::from_secs(code.interval.max(1));
        let deadline = Instant::now() + Duration::from_secs(code.expires_in.max(1));
        sink(ProgressEvent::Step {
            phase: Phase::Auth,
            message: format!(
                "waiting for Microsoft sign-in (code {} at {})",
                code.user_code, code.verification_uri
            ),
        });

        loop {
            if Instant::now() >= deadline {
                return Err(Error::Auth(
                    "the Microsoft device code expired; run the login command again".into(),
                ));
            }
            tokio::time::sleep(interval).await;
            match self.poll_token(client_id, &code.device_code).await? {
                Poll::Tokens(tokens) => return Ok(tokens),
                Poll::Pending => {}
                Poll::SlowDown => interval += Duration::from_secs(5),
            }
        }
    }

    /// Renew a Microsoft session with a refresh token.
    pub async fn refresh(&self, client_id: &str, refresh_token: &str) -> Result<MsTokens> {
        let client_id = require_client_id(client_id)?;
        let response = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if status.is_success() {
            let tokens: MsTokens = serde_json::from_str(&body)?;
            if tokens.access_token.is_empty() {
                return Err(Error::Auth(
                    "Microsoft returned an empty access token while refreshing".into(),
                ));
            }
            return Ok(tokens);
        }

        let error: OAuthError = serde_json::from_str(&body).unwrap_or_default();
        Err(Error::Auth(format!(
            "refreshing the Microsoft session failed ({}){}; sign in again with `shccl login microsoft`",
            error.error,
            describe(&error.error_description)
        )))
    }

    /// Exchange a legacy Live Connect authorization code for tokens.
    pub async fn legacy_exchange_code(&self, code: &str) -> Result<MsTokens> {
        let code = code.trim();
        if code.is_empty() {
            return Err(Error::Auth("no authorization code was provided".into()));
        }
        let response = self
            .http
            .post(LEGACY_TOKEN_URL)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", LEGACY_CLIENT_ID),
                ("redirect_uri", LEGACY_REDIRECT_URI),
                ("code", code),
                ("scope", SCOPE),
            ])
            .send()
            .await?;
        parse_tokens(response, "exchanging the legacy authorization code").await
    }

    /// Refresh a legacy Live Connect session.
    pub async fn legacy_refresh(&self, refresh_token: &str) -> Result<MsTokens> {
        let response = self
            .http
            .post(LEGACY_TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", LEGACY_CLIENT_ID),
                ("refresh_token", refresh_token),
                ("scope", SCOPE),
            ])
            .send()
            .await?;
        parse_tokens(response, "refreshing the legacy Microsoft session").await
    }

    /// Run the Xbox Live → XSTS → Minecraft chain for a Microsoft access token.
    pub async fn sign_in(&self, microsoft_access_token: &str) -> Result<MinecraftSession> {
        let (xbox_token, uhs, xbox_xuid) = self.xbox_live(microsoft_access_token).await?;
        let (xsts_token, xsts_xuid) = self.xsts(&xbox_token, &uhs).await?;
        // XSTS does not always include the XUID; fall back to the Xbox Live
        // claim and tolerate a missing value (it only feeds `--xuid`).
        let xuid = xsts_xuid.or(xbox_xuid).unwrap_or_default();
        let minecraft = self.minecraft_login(&uhs, &xsts_token).await?;
        let profile = self.minecraft_profile(&minecraft.access_token).await?;
        Ok(MinecraftSession {
            access_token: minecraft.access_token,
            expires_in: minecraft.expires_in,
            xuid,
            profile,
        })
    }

    async fn poll_token(&self, client_id: &str, device_code: &str) -> Result<Poll> {
        let response = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", client_id),
                ("device_code", device_code),
            ])
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if status.is_success() {
            let tokens: MsTokens = serde_json::from_str(&body)?;
            if tokens.access_token.is_empty() {
                return Err(Error::Auth(
                    "Microsoft returned an empty access token".into(),
                ));
            }
            return Ok(Poll::Tokens(tokens));
        }

        let error: OAuthError = serde_json::from_str(&body).unwrap_or_default();
        match error.error.as_str() {
            "authorization_pending" => Ok(Poll::Pending),
            "slow_down" => Ok(Poll::SlowDown),
            "expired_token" => Err(Error::Auth(
                "the Microsoft device code expired; run the login command again".into(),
            )),
            "authorization_declined" => {
                Err(Error::Auth("the Microsoft sign-in was declined".into()))
            }
            "bad_verification_code" => Err(Error::Auth(
                "Microsoft rejected the device code; run the login command again".into(),
            )),
            other => Err(Error::Auth(format!(
                "Microsoft sign-in failed ({other}){}",
                describe(&error.error_description)
            ))),
        }
    }

    async fn xbox_live(
        &self,
        microsoft_access_token: &str,
    ) -> Result<(String, String, Option<String>)> {
        let body = json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={microsoft_access_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        });
        let response = self.http.post(XBOX_LIVE_URL).json(&body).send().await?;
        let token: XboxResponse = read_json(response, "authenticating with Xbox Live").await?;
        let token_value = required(&token.token, "Xbox Live")?;
        let uhs = required(&token.uhs(), "Xbox Live")?;
        let xuid = token.xuid();
        Ok((token_value, uhs, (!xuid.is_empty()).then_some(xuid)))
    }

    async fn xsts(&self, xbox_token: &str, _uhs: &str) -> Result<(String, Option<String>)> {
        let body = json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbox_token],
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        });
        let response = self.http.post(XSTS_URL).json(&body).send().await?;

        let status = response.status();
        let text = response.text().await?;
        if status == StatusCode::UNAUTHORIZED {
            let error: XstsError = serde_json::from_str(&text).unwrap_or_default();
            return Err(Error::Auth(xsts_message(error.xerr, &error.message)));
        }
        if !status.is_success() {
            return Err(Error::Auth(format!(
                "Xbox authorization failed (HTTP {status}): {}",
                compact(&text)
            )));
        }

        let token: XboxResponse = serde_json::from_str(&text)?;
        if token.token.is_empty() {
            return Err(Error::Auth(
                "XSTS returned an empty token; verify that this account is allowed to use Xbox Live"
                    .into(),
            ));
        }
        let xuid = token.xuid();
        Ok((token.token, (!xuid.is_empty()).then_some(xuid)))
    }

    async fn minecraft_login(&self, uhs: &str, xsts_token: &str) -> Result<McLoginResponse> {
        let body = json!({
            "identityToken": format!("XBL3.0 x={uhs};{xsts_token}"),
        });
        let response = self
            .http
            .post(MINECRAFT_LOGIN_URL)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if status == StatusCode::FORBIDDEN {
            return Err(minecraft_api_forbidden());
        }
        if !status.is_success() {
            return Err(Error::Auth(format!(
                "signing in to Minecraft services failed (HTTP {status}): {}",
                compact(&body)
            )));
        }
        let login: McLoginResponse = serde_json::from_str(&body)?;
        if login.access_token.is_empty() {
            return Err(Error::Auth(
                "Minecraft services returned an empty access token".into(),
            ));
        }
        Ok(login)
    }

    async fn minecraft_profile(&self, access_token: &str) -> Result<MinecraftProfile> {
        let response = self
            .http
            .get(MINECRAFT_PROFILE_URL)
            .bearer_auth(access_token)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if status == StatusCode::NOT_FOUND {
            return Err(Error::Auth(
                "this Microsoft account does not own Minecraft: Java Edition".into(),
            ));
        }
        if status == StatusCode::FORBIDDEN {
            return Err(minecraft_api_forbidden());
        }
        if !status.is_success() {
            return Err(Error::Auth(format!(
                "reading the Minecraft profile failed (HTTP {status}): {}",
                compact(&body)
            )));
        }
        let profile: MinecraftProfile = serde_json::from_str(&body)?;
        if profile.id.is_empty() || profile.name.is_empty() {
            return Err(Error::Auth(
                "Minecraft services returned an incomplete profile".into(),
            ));
        }
        Ok(profile)
    }
}

impl Account {
    /// Build an account from a completed Microsoft login.
    pub(crate) fn from_session(
        session: &MinecraftSession,
        refresh_token: Option<String>,
        client_id: String,
        legacy: bool,
    ) -> Result<Self> {
        let id = session.profile.id.trim();
        if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Auth(format!(
                "Minecraft returned an invalid profile id: {id:?}"
            )));
        }
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &id[0..8],
            &id[8..12],
            &id[12..16],
            &id[16..20],
            &id[20..32]
        );

        Ok(Self {
            username: session.profile.name.clone(),
            uuid,
            kind: AccountKind::Microsoft,
            access_token: Some(session.access_token.clone()),
            refresh_token,
            expires_at: Some(unix_now().saturating_add(session.expires_in)),
            xuid: (!session.xuid.is_empty()).then(|| session.xuid.clone()),
            client_id: Some(client_id),
            legacy,
        })
    }
}

enum Poll {
    Tokens(MsTokens),
    Pending,
    SlowDown,
}

#[derive(Debug, Default, Deserialize)]
struct OAuthError {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

#[derive(Debug, Default, Deserialize)]
struct XstsError {
    #[serde(rename = "XErr", default)]
    xerr: Option<u64>,
    #[serde(rename = "Message", default)]
    message: String,
}

#[derive(Debug, Default, Deserialize)]
struct XboxResponse {
    #[serde(rename = "Token", default)]
    token: String,
    #[serde(rename = "DisplayClaims", default)]
    display_claims: DisplayClaims,
}

impl XboxResponse {
    fn uhs(&self) -> String {
        self.display_claims
            .xui
            .first()
            .map(|claim| claim.uhs.clone())
            .unwrap_or_default()
    }

    fn xuid(&self) -> String {
        self.display_claims
            .xui
            .first()
            .map(|claim| claim.xid.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug, Default, Deserialize)]
struct DisplayClaims {
    #[serde(default)]
    xui: Vec<Xui>,
}

#[derive(Debug, Default, Deserialize)]
struct Xui {
    #[serde(default)]
    uhs: String,
    #[serde(default)]
    xid: String,
}

#[derive(Debug, Deserialize)]
struct McLoginResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

async fn read_json<T: DeserializeOwned>(response: reqwest::Response, what: &str) -> Result<T> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(Error::Auth(format!(
            "{what} failed (HTTP {status}): {}",
            compact(&body)
        )));
    }
    Ok(serde_json::from_str(&body)?)
}

async fn parse_tokens(response: reqwest::Response, what: &str) -> Result<MsTokens> {
    let status = response.status();
    let body = response.text().await?;
    if status.is_success() {
        let tokens: MsTokens = serde_json::from_str(&body)?;
        if tokens.access_token.is_empty() {
            return Err(Error::Auth(format!(
                "{what} returned an empty access token"
            )));
        }
        return Ok(tokens);
    }

    let error: OAuthError = serde_json::from_str(&body).unwrap_or_default();
    let detail = if error.error.is_empty() {
        compact(&body)
    } else {
        format!("{}{}", error.error, describe(&error.error_description))
    };
    Err(Error::Auth(format!(
        "{what} failed (HTTP {status}): {detail}"
    )))
}

/// Extract the authorization code from a pasted redirect URL (or accept a raw
/// code). Legacy Live Connect only allows its fixed desktop redirect URI, so a
/// CLI login has to ask the user to paste the final URL.
pub fn parse_legacy_redirect(input: &str) -> Result<String> {
    let input = input.trim().trim_matches(|c| c == '"' || c == '\'');
    if input.is_empty() {
        return Err(Error::Auth("no authorization code was provided".into()));
    }

    let query = if let Some((_, query)) = input.split_once('?') {
        query
    } else if input.contains("://") {
        return Err(Error::Auth(
            "the pasted URL has no query string; after signing in, copy the full \
             login.live.com/oauth20_desktop.srf?code=... URL"
                .into(),
        ));
    } else if input.starts_with("code=") || input.starts_with("error=") {
        input
    } else {
        return Ok(input.to_string());
    };

    let mut code = None;
    let mut error = None;
    let mut error_description = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(value);
        match key {
            "code" => code = Some(value),
            "error" => error = Some(value),
            "error_description" => error_description = Some(value),
            _ => {}
        }
    }

    if let Some(code) = code.filter(|code| !code.is_empty()) {
        return Ok(code);
    }
    if let Some(error) = error {
        let detail = error_description
            .map(|description| format!(": {description}"))
            .unwrap_or_default();
        return Err(Error::Auth(format!(
            "Microsoft sign-in failed ({error}){detail}"
        )));
    }
    Err(Error::Auth(
        "the pasted URL does not contain an authorization code".into(),
    ))
}

fn percent_decode(value: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                match (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        decoded.push(high * 16 + low);
                        index += 3;
                    }
                    _ => {
                        decoded.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn required(value: &str, stage: &str) -> Result<String> {
    if value.is_empty() {
        Err(Error::Auth(format!(
            "{stage} returned an incomplete response"
        )))
    } else {
        Ok(value.to_string())
    }
}

fn describe(description: &str) -> String {
    if description.is_empty() {
        String::new()
    } else {
        format!(": {description}")
    }
}

fn compact(text: &str) -> String {
    const LIMIT: usize = 300;
    let text = text.trim();
    if text.chars().count() <= LIMIT {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(LIMIT).collect();
        format!("{truncated}...")
    }
}

fn device_code_error(status: StatusCode, body: &str) -> Error {
    let error: OAuthError = serde_json::from_str(body).unwrap_or_default();
    if error.error == "unauthorized_client" || body.contains("AADSTS700016") {
        return Error::Auth(format!(
            "the configured Azure client ID was rejected (AADSTS700016) because it is unknown to the \
             consumers tenant. Legacy Live Connect client IDs (for example `00000000402b5328`) only \
             support the browser authorization flow, not the device code flow. {CLIENT_ID_HELP}"
        ));
    }
    let detail = if error.error.is_empty() {
        compact(body)
    } else {
        format!("{}{}", error.error, describe(&error.error_description))
    };
    Error::Auth(format!(
        "requesting the Microsoft device code failed (HTTP {status}): {detail}"
    ))
}

fn minecraft_api_forbidden() -> Error {
    Error::Auth(
        "Minecraft services rejected this application (HTTP 403). Apply for Minecraft API access \
         for your Azure app at https://aka.ms/mce-reviewappid; approval is required before login \
         can complete."
            .to_string(),
    )
}

fn xsts_message(xerr: Option<u64>, message: &str) -> String {
    match xerr {
        Some(2148916233) => {
            "this Microsoft account has no Xbox profile; create one at https://xbox.com first"
                .into()
        }
        Some(2148916235) => "Xbox Live is not available in this account's country or region".into(),
        Some(2148916236) | Some(2148916237) => {
            "this account must complete Xbox adult verification".into()
        }
        Some(2148916238) => {
            "this is a child account; a family organizer must add it to a Microsoft family".into()
        }
        Some(code) => format!("Xbox authorization failed (XErr {code})"),
        None if !message.is_empty() => format!("Xbox authorization failed: {message}"),
        None => "Xbox authorization failed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_uuid_is_hyphenated() {
        let session = MinecraftSession {
            access_token: "token".into(),
            expires_in: 86_400,
            xuid: "123".into(),
            profile: MinecraftProfile {
                id: "069a79f444e94726a5befca90e38aaf5".into(),
                name: "Notch".into(),
            },
        };
        let account =
            Account::from_session(&session, Some("refresh".into()), "client-id".into(), false)
                .unwrap();
        assert_eq!(account.uuid, "069a79f4-44e9-4726-a5be-fca90e38aaf5");
        assert_eq!(account.username, "Notch");
        assert_eq!(account.kind, AccountKind::Microsoft);
        assert_eq!(account.user_type(), "msa");
        assert_eq!(account.access_token(), "token");
        assert_eq!(account.xuid(), "123");
        assert!(!account.needs_refresh());
        assert_eq!(
            account.auth_session(),
            "token:token:069a79f444e94726a5befca90e38aaf5"
        );
    }

    #[test]
    fn invalid_profile_ids_are_rejected() {
        let session = MinecraftSession {
            access_token: "token".into(),
            expires_in: 86_400,
            xuid: "123".into(),
            profile: MinecraftProfile {
                id: "not-a-uuid".into(),
                name: "Player".into(),
            },
        };
        assert!(Account::from_session(&session, None, "client-id".into(), false).is_err());
    }

    #[test]
    fn legacy_redirects_are_parsed() {
        let url = "https://login.live.com/oauth20_desktop.srf?code=M.R3_BAY.abc%2Bdef&lc=1033";
        assert_eq!(parse_legacy_redirect(url).unwrap(), "M.R3_BAY.abc+def");
        assert_eq!(
            parse_legacy_redirect("M.R3_BAY.raw").unwrap(),
            "M.R3_BAY.raw"
        );
        assert_eq!(
            parse_legacy_redirect("code=M.R3_BAY.raw").unwrap(),
            "M.R3_BAY.raw"
        );
        assert!(parse_legacy_redirect("https://login.live.com/oauth20_desktop.srf").is_err());
        let error = parse_legacy_redirect(
            "https://login.live.com/oauth20_desktop.srf?error=access_denied&error_description=cancelled",
        )
        .unwrap_err();
        assert!(error.to_string().contains("access_denied"));
    }

    #[test]
    fn legacy_authorization_url_uses_the_fixed_redirect() {
        let url = legacy_authorization_url();
        assert!(url.starts_with(LEGACY_AUTHORIZE_URL));
        assert!(url.contains(LEGACY_CLIENT_ID));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("XboxLive.signin"));
        assert!(url.contains(LEGACY_REDIRECT_URI_ENCODED));
    }

    #[test]
    fn device_code_defaults_are_applied() {
        let code: DeviceCode = serde_json::from_str(
            r#"{"user_code":"ABC","device_code":"dev","verification_uri":"https://example.test"}"#,
        )
        .unwrap();
        assert_eq!(code.interval, 5);
        assert_eq!(code.expires_in, 900);
    }
}
