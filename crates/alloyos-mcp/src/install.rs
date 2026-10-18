//! Public review descriptor for a pending, immutable remote app installation.
use alloc::string::String;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageChangeAction { #[default] Install, Remove }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingRemoteInstall {
    #[serde(default)]
    pub action: PackageChangeAction,
    pub id: String,
    pub app_id: String,
    pub name: String,
    pub endpoint: String,
    pub source: String,
    pub publisher: String,
}
impl PendingRemoteInstall {
    pub fn is_valid(&self) -> bool {
        !self.id.is_empty() && self.id.len() <= 64
            && self.id.bytes().all(|b|b.is_ascii_alphanumeric() || b == b'-')
            && !self.app_id.is_empty() && self.app_id.len() <= 128
            && self.app_id.bytes().all(|b|b.is_ascii_alphanumeric() || matches!(b,b'-'|b'_'))
            && !self.name.is_empty() && self.name.len() <= 256
            && self.endpoint.len() <= 8192
            && (self.endpoint.starts_with("https://") || self.action == PackageChangeAction::Remove && self.endpoint.is_empty())
            && self.source.len() <= 256 && self.publisher.len() <= 256
            && [&self.name,&self.endpoint,&self.source,&self.publisher].iter().all(|s|!s.chars().any(char::is_control))
    }
}
