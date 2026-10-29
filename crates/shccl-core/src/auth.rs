use std::path::Path;

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::util::{read_json, write_json_private};

/// The authentication mechanism backing an account.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    #[default]
    Offline,
}

impl std::fmt::Display for AccountKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline => f.write_str("offline"),
        }
    }
}

/// An offline-mode player profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub username: String,
    /// Hyphenated UUID, matching Java's `UUID.toString()`.
    pub uuid: String,
    #[serde(default)]
    pub kind: AccountKind,
}

impl Account {
    /// Create an offline account.
    ///
    /// The UUID is derived exactly like Java's
    /// `UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(UTF_8))`,
    /// which keeps worlds and server inventories consistent with other
    /// launchers.
    pub fn offline(username: &str) -> Result<Self> {
        let username = username.trim();
        if username.is_empty() {
            return Err(Error::Auth("offline username cannot be empty".into()));
        }
        if username.chars().count() > 16 {
            return Err(Error::Auth(
                "offline username must be at most 16 characters".into(),
            ));
        }
        if !username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(Error::Auth(
                "offline username may only contain letters, digits and underscores".into(),
            ));
        }

        let uuid = offline_uuid(username);
        Ok(Self {
            username: username.to_string(),
            uuid,
            kind: AccountKind::Offline,
        })
    }

    pub fn user_type(&self) -> &'static str {
        match self.kind {
            AccountKind::Offline => "legacy",
        }
    }

    pub fn access_token(&self) -> &'static str {
        match self.kind {
            AccountKind::Offline => "0",
        }
    }
}

/// Compute the Java-compatible offline player UUID.
pub fn offline_uuid(username: &str) -> String {
    let mut digest: [u8; 16] = Md5::digest(format!("OfflinePlayer:{username}").as_bytes()).into();
    // Java's UUID.nameUUIDFromBytes sets the version to 3 (name based, MD5)
    // and the IETF variant bits.
    digest[6] = (digest[6] & 0x0f) | 0x30;
    digest[8] = (digest[8] & 0x3f) | 0x80;
    format_uuid(&digest)
}

fn format_uuid(bytes: &[u8; 16]) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Accounts known to this launcher plus the selected account.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountStore {
    pub accounts: Vec<Account>,
    pub selected: Option<String>,
}

impl AccountStore {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        read_json(path)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_json_private(path, self)
    }

    pub fn upsert(&mut self, account: Account) {
        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|existing| existing.uuid == account.uuid)
        {
            *existing = account;
        } else {
            self.accounts.push(account);
        }
    }

    pub fn select(&mut self, query: &str) -> Result<Account> {
        let account = self.find(query).cloned().ok_or_else(|| {
            Error::Auth(format!(
                "account '{query}' not found; use `shccl accounts` to list accounts"
            ))
        })?;
        self.selected = Some(account.uuid.clone());
        Ok(account)
    }

    pub fn find(&self, query: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|account| account.uuid == query)
            .or_else(|| {
                self.accounts
                    .iter()
                    .find(|account| account.username.eq_ignore_ascii_case(query))
            })
    }

    pub fn current(&self) -> Option<&Account> {
        let selected = self.selected.as_deref()?;
        self.accounts
            .iter()
            .find(|account| account.uuid == selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_java() {
        // Generated with Java:
        // UUID.nameUUIDFromBytes("OfflinePlayer:Notch".getBytes(UTF_8))
        assert_eq!(
            offline_uuid("Notch"),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn offline_account_rejects_bad_names() {
        assert!(Account::offline("").is_err());
        assert!(Account::offline("name with spaces").is_err());
        assert!(Account::offline("a".repeat(17).as_str()).is_err());
        assert!(Account::offline("Player_1").is_ok());
    }
}
