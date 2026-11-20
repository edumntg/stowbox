//! Every provider Stowbox can drive, as data. The shell and the UI iterate
//! [`ALL`]; adding a provider adds a [`ProviderKind`] here and nothing there.

use crate::capabilities::Capabilities;
use crate::provider::Provider;
use crate::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The credential field of a provider that connects with one API key.
pub const API_KEY: &str = "api_key";
/// Shorter than any provider's real key: a paste that was cut off.
const MIN_KEY_LEN: usize = 20;
/// Longer than any credential; bounds what reaches the keychain.
const MAX_VALUE_LEN: usize = 4096;

/// What a person entered to connect a provider. Printing it shows `***`.
#[derive(Clone, Default, PartialEq)]
pub struct Credentials(BTreeMap<String, String>);

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials(***)")
    }
}

impl Credentials {
    pub fn new(values: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(values.into_iter().map(|(k, v)| (k, v.trim().to_string())).collect())
    }

    /// What the keychain holds: a JSON object, or the bare API key that
    /// Stowbox stored before providers could have several fields.
    pub fn decode(stored: &str) -> Self {
        match serde_json::from_str::<BTreeMap<String, String>>(stored) {
            Ok(values) => Self::new(values),
            Err(_) => Self::new([(API_KEY.to_string(), stored.to_string())]),
        }
    }

    /// The one JSON value kept in the keychain.
    pub fn encode(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.0)?)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str).filter(|v| !v.is_empty())
    }

    /// The API key, for providers that connect with one.
    pub fn api_key(&self) -> Result<String> {
        self.get(API_KEY).map(str::to_string).ok_or_else(|| Error::Auth("no API key was given".into()))
    }

    /// Checks the values against what the provider asks for, before the
    /// network. Messages name fields and never repeat a value.
    pub fn validate(&self, caps: &Capabilities) -> Result<()> {
        if let Some(extra) = self.0.keys().find(|name| !caps.credential_fields.iter().any(|f| f.name == **name)) {
            return Err(Error::Auth(format!("{} does not take a field named {extra}", caps.label)));
        }
        for field in &caps.credential_fields {
            let Some(value) = self.get(&field.name) else {
                if field.optional {
                    continue;
                }
                return Err(Error::Auth(format!("{} needs its {}", caps.label, field.label)));
            };
            if value.len() > MAX_VALUE_LEN {
                return Err(Error::Auth(format!("That {} {} is too long", caps.label, field.label)));
            }
        }
        if let (Some(key), Some(field)) = (self.get(API_KEY), caps.credential_fields.iter().find(|f| f.name == API_KEY)) {
            let known = caps.key_prefixes.is_empty() || caps.key_prefixes.iter().any(|p| key.starts_with(p.as_str()));
            if !known || key.len() < MIN_KEY_LEN {
                return Err(Error::Auth(format!(
                    "That does not look like a {} API key. Keys start with {}. {}",
                    caps.label,
                    caps.key_prefixes.join(" or "),
                    field.hint
                )));
            }
        }
        Ok(())
    }
}

/// What a provider's client may need from the app.
#[derive(Debug, Clone)]
pub struct ConnectContext {
    pub data_dir: PathBuf,
}

impl ConnectContext {
    pub fn new(data_dir: &Path) -> Self {
        Self { data_dir: data_dir.to_path_buf() }
    }

    /// A file of the provider's in the app's SSH directory: its key or its
    /// known hosts.
    pub fn ssh_file(&self, name: &str) -> PathBuf {
        self.data_dir.join("ssh").join(name)
    }
}

/// One provider Stowbox can drive.
pub struct ProviderKind {
    pub id: &'static str,
    pub capabilities: fn() -> Capabilities,
    /// Builds a client from the stored credentials. Must run inside the Tokio
    /// runtime: boxd's lazy gRPC channel binds to the runtime it is made in.
    pub connect: fn(Credentials, &ConnectContext) -> Result<Arc<dyn Provider>>,
}

/// Every provider, in the order the UI lists them.
pub const ALL: &[ProviderKind] =
    &[crate::boxd::KIND, crate::boat::KIND, crate::daytona::KIND, crate::agent37::KIND, crate::e2b::KIND, crate::vercel::KIND, crate::sshhost::KIND];

/// The provider with this id.
pub fn find(id: &str) -> Result<&'static ProviderKind> {
    ALL.iter().find(|k| k.id == id).ok_or_else(|| Error::NotFound(format!("Stowbox does not know the provider {id}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> Credentials {
        Credentials::new([(API_KEY.to_string(), value.to_string())])
    }

    #[test]
    fn printing_credentials_never_shows_a_value() {
        let printed = format!("{:?}", key("bxd_a_very_secret_value_0123"));
        assert_eq!(printed, "Credentials(***)");
    }

    #[test]
    fn a_key_stored_by_an_older_version_still_loads() {
        assert_eq!(Credentials::decode("bxd_plain_key_from_before").api_key().unwrap(), "bxd_plain_key_from_before");
        let stored = key("bxd_plain_key_from_before").encode().unwrap();
        assert_eq!(Credentials::decode(&stored), key("bxd_plain_key_from_before"));
    }

    #[test]
    fn stored_values_of_the_wrong_shape_are_treated_as_a_bare_key() {
        for odd in ["[1,2]", "{\"api_key\":1}", "null", "{"] {
            assert_eq!(Credentials::decode(odd).api_key().unwrap(), odd);
        }
    }

    #[test]
    fn validation_refuses_bad_keys_without_repeating_them() {
        let caps = crate::boxd::capabilities();
        assert!(key("bxd_0123456789abcdefghij").validate(&caps).is_ok());
        for bad in ["sk_live_0123456789abcdefghij", "bxd_short", ""] {
            let message = key(bad).validate(&caps).unwrap_err().to_string();
            assert!(bad.is_empty() || !message.contains(bad), "{message}");
        }
        let long = format!("bxd_{}", "x".repeat(MAX_VALUE_LEN));
        assert!(key(&long).validate(&caps).is_err());
        let extra = Credentials::new([(API_KEY.to_string(), "bxd_0123456789abcdefghij".to_string()), ("other".to_string(), "x".to_string())]);
        assert!(extra.validate(&caps).is_err());
    }

    /// Why a bundled icon may not ship, if it may not.
    fn unsafe_svg(svg: &str) -> Option<&'static str> {
        let lower = svg.to_ascii_lowercase();
        if lower.contains("<script") {
            Some("script")
        } else if lower.contains("<foreignobject") {
            Some("foreignObject")
        } else if lower.contains("<!entity") {
            Some("entity")
        } else if lower.contains("url(") {
            Some("url()")
        } else if lower.split("href=").skip(1).any(|v| !v.trim_start_matches(['"', '\'']).starts_with('#')) {
            Some("external href")
        } else if lower.split(|c: char| c.is_whitespace() || c == '<').any(|word| word.starts_with("on") && word.contains('=')) {
            Some("event attribute")
        } else {
            None
        }
    }

    #[test]
    fn hostile_icons_are_refused() {
        for bad in [
            "<svg><script>alert(1)</script></svg>",
            "<svg><foreignObject/></svg>",
            "<svg onload=\"x()\"/>",
            "<svg><a href=\"https://evil.example\"/></svg>",
            "<svg><style>a{fill:url(https://evil.example)}</style></svg>",
            "<!ENTITY x \"y\"><svg/>",
        ] {
            assert!(unsafe_svg(bad).is_some(), "{bad}");
        }
        assert_eq!(unsafe_svg("<svg viewBox=\"0 0 16 16\"><use href=\"#a\"/><path fill=\"currentColor\" d=\"M0 0\"/></svg>"), None);
    }

    #[test]
    fn bundled_icons_are_inert() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/assets/providers");
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "svg") {
                assert_eq!(unsafe_svg(&std::fs::read_to_string(&path).unwrap()), None, "{}", path.display());
            }
        }
    }

    /// The browser demo reads the same descriptors as the app. A stale copy
    /// fails here; `STOWBOX_REGENERATE=1` writes the new one.
    #[test]
    fn the_mock_gets_the_descriptors() {
        let all: Vec<_> = ALL.iter().map(|k| serde_json::json!({ "id": k.id, "capabilities": (k.capabilities)() })).collect();
        let text = serde_json::to_string_pretty(&all).unwrap() + "\n";
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/bridge/providers.generated.json");
        // A Windows checkout may turn the file's line ends into CRLF.
        if std::fs::read_to_string(&path).ok().map(|t| t.replace("\r\n", "\n")).as_deref() == Some(text.as_str()) {
            return;
        }
        if std::env::var_os("STOWBOX_REGENERATE").is_some() {
            std::fs::write(&path, text).unwrap();
        } else {
            panic!("{} is out of date: run STOWBOX_REGENERATE=1 cargo test -p stowbox-core the_mock_gets_the_descriptors", path.display());
        }
    }

    #[test]
    fn every_provider_describes_itself() {
        let mut ids: Vec<&str> = ALL.iter().map(|k| k.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len(), "provider ids are unique");
        for kind in ALL {
            let caps = (kind.capabilities)();
            assert!(!caps.new_box_blurb.is_empty(), "{}", kind.id);
            assert!(!caps.icon.is_empty(), "{}", kind.id);
            assert!(!caps.credential_fields.is_empty(), "{}", kind.id);
            // A disk size is only a default for a box Stowbox makes.
            assert_eq!(caps.default_disk_gib > 0, caps.create, "{}", kind.id);
            if !caps.create {
                assert!(caps.power.is_empty() && !caps.fork, "{} cannot make boxes, so it cannot power or fork them", kind.id);
            } else if caps.custom_size {
                assert!(!caps.free_sizes.is_empty(), "{} offers free sizes but lists none", kind.id);
            } else {
                assert!(caps.sizes.iter().any(|s| s.id == caps.default_size), "{}'s default size is not on its menu", kind.id);
            }
            assert_eq!(find(kind.id).unwrap().id, kind.id);
        }
        assert!(find("nope").is_err());
    }
}
