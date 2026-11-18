//! Configuration (`api-v1.md` §8.6): TOML file, overridden key by key by
//! `TOMI_RELAY_<KEY IN CAPITALS>` environment variables. An unknown key (file or
//! environment) is an error: a typo must not silently run with a default.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ipnet::IpNet;
use serde::{Deserialize, Serialize};

/// Environment prefix of the overrides.
pub const ENV_PREFIX: &str = "TOMI_RELAY_";
/// Default configuration file.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/tomi-relay/config.toml";

/// The smallest default quota the web panel accepts: a sanity floor against a typo. It is below
/// the largest envelope (1,06 Mio), so at exactly this value an item at the envelope limit is
/// refused; ordinary items stay far smaller.
pub const MIN_DEFAULT_SPACE_QUOTA_BYTES: u64 = 1024 * 1024;
/// The largest quota the web panel accepts: `2^53 - 1`, the largest integer a JavaScript client
/// (the panel itself) writes exactly. Far above any disk.
pub const MAX_DEFAULT_SPACE_QUOTA_BYTES: u64 = (1 << 53) - 1;

/// A token bucket: refill per minute and capacity (§8.5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rate {
    pub per_minute: f64,
    pub burst: u32,
}

impl Rate {
    const fn new(per_minute: f64, burst: u32) -> Self {
        Self { per_minute, burst }
    }
    /// "N per hour" expressed per minute.
    const fn per_hour(n: f64, burst: u32) -> Self {
        Self {
            per_minute: n / 60.0,
            burst,
        }
    }
}

/// `json` or `text` log lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Json,
    Text,
}

/// All the keys of §8.6 and §2.8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub listen: String,
    pub allow_public_listen: bool,
    pub public_url: String,
    pub data_dir: PathBuf,
    pub trusted_proxies: Vec<String>,
    /// Serves the web administration panel (`admin_web.rs`) under a secret path. Off by default:
    /// every path of the panel then answers like any unknown resource.
    pub admin_web: bool,
    /// Refuse to start on a `data_dir` that holds neither `routing.db`, nor `spaces/`, nor the
    /// older layout (§8.6): a volume that is not mounted, or a wrong `data_dir`, would make the
    /// relay look new (`space_not_initialized` to every device). `false` for a first install.
    pub require_existing_data_dir: bool,
    /// How long the receipt of a deletion of a space is kept (§4.4.2).
    pub deletion_receipt_ttl_secs: u64,

    pub session_ttl_secs: u64,
    pub challenge_ttl_secs: u64,
    pub inactive_after_days: u64,
    pub presence_ttl_secs: u64,
    pub release_grace_secs: u64,
    pub purge_interval_secs: u64,
    pub event_retention_days: u64,
    pub blob_orphan_ttl_secs: u64,

    pub pairing_ttl_secs: u64,
    pub pairing_max_open: u64,
    pub pairing_max_messages: u64,
    pub pairing_max_message_bytes: u64,

    pub max_json_body_bytes: u64,
    pub max_envelope_bytes: u64,
    pub max_blob_bytes: u64,
    pub max_item_blob_bytes: u64,
    pub max_blobs_per_item: u64,
    pub max_upload_part_bytes: u64,
    pub max_ttl_secs: u64,
    pub max_devices: u64,
    /// Total of `used_bytes` of every space of the relay (§8.4).
    pub max_relay_bytes: u64,
    /// Quota of one space, for every space that has none of its own. Absent or `0`: no limit per
    /// space (only the total `max_relay_bytes` applies). The administrator can replace it, without
    /// a restart, from the web panel (stored in `routing.db`, wins over this key); an override of
    /// `admin quota` wins over both, space by space (see [`Config::configured_space_quota`] and
    /// `Spaces::space_max`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_space_bytes: Option<u64>,
    pub max_transfer_bytes: u64,
    pub max_blobs_per_transfer: u64,
    pub transfer_ttl_secs: u64,
    pub transfer_max_open: u64,
    pub max_item_state_bytes: u64,
    pub min_free_disk_bytes: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub fcm_service_account_file: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fcm_project_id: Option<String>,

    pub rl_ip_public: Rate,
    pub rl_ip_space_create: Rate,
    pub rl_ip_auth: Rate,
    pub rl_ip_claim: Rate,
    pub rl_global_claim: Rate,
    pub rl_ip_ws: Rate,
    pub rl_ip_static: Rate,
    pub rl_device: Rate,
    pub rl_device_items: Rate,
    pub rl_device_tombstones: Rate,
    pub rl_device_item_states: Rate,
    pub rl_device_presence: Rate,
    pub rl_device_pairing: Rate,
    pub rl_device_transfers: Rate,
    pub rl_device_blob_parts: Rate,
    pub rl_pairing_poll: Rate,

    pub log_format: LogFormat,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".into(),
            allow_public_listen: false,
            public_url: "https://relay.example.org".into(),
            data_dir: PathBuf::from("/var/lib/tomi-relay"),
            trusted_proxies: vec!["127.0.0.1/32".into(), "::1/128".into()],
            admin_web: false,
            require_existing_data_dir: false,
            deletion_receipt_ttl_secs: 7_776_000,
            session_ttl_secs: 3600,
            challenge_ttl_secs: 60,
            inactive_after_days: 30,
            presence_ttl_secs: 120,
            release_grace_secs: 300,
            purge_interval_secs: 30,
            event_retention_days: 45,
            blob_orphan_ttl_secs: 86_400,
            pairing_ttl_secs: 120,
            pairing_max_open: 2,
            pairing_max_messages: 8,
            pairing_max_message_bytes: 8192,
            max_json_body_bytes: 2_097_152,
            max_envelope_bytes: 1_114_112,
            max_blob_bytes: 26_279_936,
            max_item_blob_bytes: 105_119_744,
            max_blobs_per_item: 32,
            max_upload_part_bytes: 1_048_576,
            max_ttl_secs: 2_592_000,
            max_devices: 20,
            max_relay_bytes: 1_073_741_824,
            max_space_bytes: None,
            max_transfer_bytes: 1_073_741_824,
            max_blobs_per_transfer: 10_000,
            transfer_ttl_secs: 604_800,
            transfer_max_open: 2,
            max_item_state_bytes: 4096,
            min_free_disk_bytes: 1_073_741_824,
            fcm_service_account_file: None,
            fcm_project_id: None,
            rl_ip_public: Rate::new(30.0, 10),
            rl_ip_space_create: Rate::new(1.0, 5),
            rl_ip_auth: Rate::new(20.0, 10),
            rl_ip_claim: Rate::new(5.0, 5),
            rl_global_claim: Rate::new(60.0, 20),
            rl_ip_ws: Rate::new(30.0, 10),
            rl_ip_static: Rate::new(120.0, 60),
            rl_device: Rate::new(300.0, 100),
            rl_device_items: Rate::new(60.0, 20),
            rl_device_tombstones: Rate::new(120.0, 30),
            rl_device_item_states: Rate::new(60.0, 30),
            rl_device_presence: Rate::new(12.0, 4),
            rl_device_pairing: Rate::per_hour(10.0, 3),
            rl_device_transfers: Rate::per_hour(10.0, 3),
            rl_device_blob_parts: Rate::new(1200.0, 200),
            rl_pairing_poll: Rate::new(60.0, 10),
            log_format: LogFormat::Json,
        }
    }
}

/// Configuration problem (startup is refused).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read the configuration file {path}: {reason}")]
    Read { path: String, reason: String },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

impl Config {
    /// Loads the file (a missing *default* file is not an error: the deployment may only
    /// use environment variables) then applies the process environment.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        Self::load_with_env(path, std::env::vars())
    }

    /// Same as [`Config::load`] with an explicit environment (tests).
    pub fn load_with_env(
        path: Option<&Path>,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let mut table = toml::Table::new();
        let (file, explicit) = match path {
            Some(p) => (p.to_path_buf(), true),
            None => (PathBuf::from(DEFAULT_CONFIG_PATH), false),
        };
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                table = text.parse::<toml::Table>().map_err(|e| {
                    ConfigError::Invalid(format!("{}: {}", file.display(), e.message()))
                })?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => {}
            Err(e) => {
                return Err(ConfigError::Read {
                    path: file.display().to_string(),
                    reason: e.to_string(),
                })
            }
        }
        apply_env(&mut table, env)?;
        let cfg: Config = table
            .try_into()
            .map_err(|e: toml::de::Error| ConfigError::Invalid(e.message().to_owned()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Checks the cross-field rules that make startup fail (§2.1, §8.2).
    pub fn validate(&self) -> Result<(), ConfigError> {
        let addr = self.listen_addr()?;
        if !addr.ip().is_loopback() && !self.allow_public_listen {
            return Err(ConfigError::Invalid(format!(
                "listen address {addr} is not a loopback address: TLS is terminated by the \
                 reverse proxy in front of the relay; set allow_public_listen = true only for development"
            )));
        }
        if self.event_retention_days <= self.inactive_after_days {
            return Err(ConfigError::Invalid(format!(
                "event_retention_days ({}) must be greater than inactive_after_days ({})",
                self.event_retention_days, self.inactive_after_days
            )));
        }
        self.trusted_nets()?;
        // The panel signs in with a passkey bound to the host of `public_url`, and its cookie is
        // `Secure`: both need an https URL with a domain name.
        if self.admin_web && crate::admin_web::origin_and_rp_id(&self.public_url).is_none() {
            return Err(ConfigError::Invalid(format!(
                "admin_web needs public_url to be an https URL with a domain name (got {:?})",
                self.public_url
            )));
        }
        if self
            .max_space_bytes
            .is_some_and(|n| n > MAX_DEFAULT_SPACE_QUOTA_BYTES)
        {
            return Err(ConfigError::Invalid(format!(
                "max_space_bytes must be at most {MAX_DEFAULT_SPACE_QUOTA_BYTES} (0 or absent means no limit per space)"
            )));
        }
        for (name, v) in [
            ("max_upload_part_bytes", self.max_upload_part_bytes),
            ("max_blob_bytes", self.max_blob_bytes),
            ("max_envelope_bytes", self.max_envelope_bytes),
            ("max_devices", self.max_devices),
            ("max_relay_bytes", self.max_relay_bytes),
            ("session_ttl_secs", self.session_ttl_secs),
            ("pairing_max_messages", self.pairing_max_messages),
            ("max_json_body_bytes", self.max_json_body_bytes),
        ] {
            if v == 0 {
                return Err(ConfigError::Invalid(format!(
                    "{name} must be greater than 0"
                )));
            }
        }
        Ok(())
    }

    /// The listen address.
    pub fn listen_addr(&self) -> Result<SocketAddr, ConfigError> {
        self.listen.parse().map_err(|_| {
            ConfigError::Invalid(format!("listen: {:?} is not host:port", self.listen))
        })
    }

    /// Parsed `trusted_proxies` (CIDR ranges or single addresses).
    pub fn trusted_nets(&self) -> Result<Vec<IpNet>, ConfigError> {
        self.trusted_proxies
            .iter()
            .map(|s| {
                s.parse::<IpNet>()
                    .or_else(|_| s.parse::<IpAddr>().map(IpNet::from))
                    .map_err(|_| {
                        ConfigError::Invalid(format!("trusted_proxies: {s:?} is not a CIDR range"))
                    })
            })
            .collect()
    }

    /// Configuration for the integration tests: a temporary data directory, the rate
    /// limits out of the way (the dedicated tests restore the defaults).
    pub fn for_tests(data_dir: &Path) -> Self {
        let huge = Rate::new(1_000_000.0, 1_000_000);
        Self {
            data_dir: data_dir.to_path_buf(),
            public_url: "https://relay.test".into(),
            min_free_disk_bytes: 0,
            rl_ip_public: huge,
            rl_ip_space_create: huge,
            rl_ip_auth: huge,
            rl_ip_claim: huge,
            rl_global_claim: huge,
            rl_ip_ws: huge,
            rl_ip_static: huge,
            rl_device: huge,
            rl_device_items: huge,
            rl_device_tombstones: huge,
            rl_device_item_states: huge,
            rl_device_presence: huge,
            rl_device_pairing: huge,
            rl_device_transfers: huge,
            rl_device_blob_parts: huge,
            rl_pairing_poll: huge,
            ..Self::default()
        }
    }

    /// The default quota of a space that this configuration gives (what applies when the web
    /// panel has set nothing): `max_space_bytes`; absent or `0` is no limit (`None`).
    pub fn configured_space_quota(&self) -> Option<u64> {
        self.max_space_bytes.filter(|&n| n > 0)
    }

    /// Where [`Config::configured_space_quota`] comes from: `"config"` when `max_space_bytes` is a
    /// positive number, else `"none"` (no limit).
    pub fn configured_space_quota_source(&self) -> &'static str {
        if self.configured_space_quota().is_some() {
            "config"
        } else {
            "none"
        }
    }

    /// `<data_dir>/<name>`.
    pub fn path(&self, name: &str) -> PathBuf {
        self.data_dir.join(name)
    }
}

/// Real-time constants that are fixed by the specification but must be reducible in tests
/// (the production values are the defaults and are asserted by a unit test).
#[derive(Debug, Clone)]
pub struct Timing {
    /// Delay for the first WebSocket message (§5.1).
    pub ws_auth_timeout: Duration,
    /// Interval between server pings (§5.3).
    pub ws_ping_interval: Duration,
    /// Time allowed for the answer to a ping (§5.3).
    pub ws_pong_timeout: Duration,
    /// Minimum interval between two FCM sends to the same device (§6.3).
    pub push_min_interval: Duration,
    /// Longest long-poll wait of the pairing channel (§4.6).
    pub max_long_poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ws_auth_timeout: Duration::from_secs(10),
            ws_ping_interval: Duration::from_secs(25),
            ws_pong_timeout: Duration::from_secs(20),
            push_min_interval: Duration::from_secs(2),
            max_long_poll: Duration::from_secs(25),
        }
    }
}

/// Applies the `TOMI_RELAY_*` variables onto the file's table. The type of each value comes
/// from the default configuration.
fn apply_env(
    table: &mut toml::Table,
    env: impl IntoIterator<Item = (String, String)>,
) -> Result<(), ConfigError> {
    let defaults = toml::Table::try_from(Config::default())
        .map_err(|e| ConfigError::Invalid(format!("internal: {e}")))?;
    let optional_strings = ["fcm_service_account_file", "fcm_project_id"];
    // Keys that are absent from the defaults (`None` is not serialised) but are integers.
    let optional_integers = ["max_space_bytes"];
    for (name, raw) in env {
        let Some(key) = name.strip_prefix(ENV_PREFIX) else {
            continue;
        };
        let key = key.to_ascii_lowercase();
        let value = match defaults.get(&key) {
            Some(default) => env_value(&name, &raw, default)?,
            None if optional_strings.contains(&key.as_str()) => toml::Value::String(raw),
            None if optional_integers.contains(&key.as_str()) => {
                env_value(&name, &raw, &toml::Value::Integer(0))?
            }
            None => {
                return Err(ConfigError::Invalid(format!(
                    "unknown environment variable {name}"
                )))
            }
        };
        table.insert(key, value);
    }
    Ok(())
}

fn env_value(name: &str, raw: &str, default: &toml::Value) -> Result<toml::Value, ConfigError> {
    let bad = |what: &str| ConfigError::Invalid(format!("{name}: expected {what}"));
    Ok(match default {
        toml::Value::String(_) => toml::Value::String(raw.to_owned()),
        toml::Value::Boolean(_) => match raw.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => toml::Value::Boolean(true),
            "false" | "0" | "no" => toml::Value::Boolean(false),
            _ => return Err(bad("true or false")),
        },
        toml::Value::Integer(_) => {
            toml::Value::Integer(raw.trim().parse().map_err(|_| bad("an integer"))?)
        }
        toml::Value::Float(_) => {
            toml::Value::Float(raw.trim().parse().map_err(|_| bad("a number"))?)
        }
        toml::Value::Array(_) => toml::Value::Array(
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| toml::Value::String(s.to_owned()))
                .collect(),
        ),
        toml::Value::Table(_) => {
            // "per_minute,burst" or "per_minute/burst".
            let (a, b) = raw
                .split_once([',', '/'])
                .ok_or_else(|| bad("per_minute,burst"))?;
            let per_minute: f64 = a.trim().parse().map_err(|_| bad("per_minute,burst"))?;
            let burst: i64 = b.trim().parse().map_err(|_| bad("per_minute,burst"))?;
            let mut t = toml::Table::new();
            t.insert("per_minute".into(), toml::Value::Float(per_minute));
            t.insert("burst".into(), toml::Value::Integer(burst));
            toml::Value::Table(t)
        }
        toml::Value::Datetime(_) => return Err(bad("a supported value")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn defaults_match_the_spec() {
        let c = Config::default();
        assert_eq!(c.listen, "127.0.0.1:8080");
        assert_eq!(c.max_envelope_bytes, 1_114_112);
        assert_eq!(c.max_blob_bytes, 26_279_936);
        assert_eq!(c.max_item_blob_bytes, 105_119_744);
        assert_eq!(c.max_relay_bytes, 1_073_741_824);
        assert_eq!(c.max_space_bytes, None, "nothing configured by default");
        assert_eq!(
            c.configured_space_quota(),
            None,
            "no limit per space unless told otherwise (production behaviour)"
        );
        assert_eq!(c.configured_space_quota_source(), "none");
        assert_eq!(c.session_ttl_secs, 3600);
        assert_eq!(c.event_retention_days, 45);
        assert_eq!(c.rl_ip_auth, Rate::new(20.0, 10));
        assert!(!c.require_existing_data_dir, "a first install must start");
        assert_eq!(c.deletion_receipt_ttl_secs, 90 * 86_400);
        c.validate().unwrap();
    }

    #[test]
    fn the_deletion_keys_come_from_the_environment() {
        let c = Config::load_with_env(
            None,
            env(&[
                ("TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR", "true"),
                ("TOMI_RELAY_DELETION_RECEIPT_TTL_SECS", "60"),
            ]),
        )
        .unwrap();
        assert!(c.require_existing_data_dir);
        assert_eq!(c.deletion_receipt_ttl_secs, 60);
    }

    #[test]
    fn production_timing_is_the_spec_one() {
        let t = Timing::default();
        assert_eq!(t.ws_auth_timeout, Duration::from_secs(10));
        assert_eq!(t.ws_ping_interval, Duration::from_secs(25));
        assert_eq!(t.ws_pong_timeout, Duration::from_secs(20));
        assert_eq!(t.push_min_interval, Duration::from_secs(2));
        assert_eq!(t.max_long_poll, Duration::from_secs(25));
    }

    #[test]
    fn environment_overrides_by_key() {
        let c = Config::load_with_env(
            None, // no config file: only the environment is applied
            env(&[
                ("TOMI_RELAY_DATA_DIR", "/var/lib/tomi"),
                ("TOMI_RELAY_MAX_DEVICES", "7"),
                ("TOMI_RELAY_MAX_SPACE_BYTES", "500000"),
                ("TOMI_RELAY_MAX_RELAY_BYTES", "9000000"),
                ("TOMI_RELAY_ALLOW_PUBLIC_LISTEN", "true"),
                ("TOMI_RELAY_LISTEN", "0.0.0.0:9000"),
                ("TOMI_RELAY_RL_IP_AUTH", "5,3"),
                ("TOMI_RELAY_TRUSTED_PROXIES", "10.0.0.0/8, 192.168.1.1"),
                ("TOMI_RELAY_FCM_SERVICE_ACCOUNT_FILE", "/etc/tomi/fcm.json"),
                ("TOMI_RELAY_LOG_FORMAT", "text"),
                ("PATH", "ignored"),
            ]),
        )
        .unwrap();
        assert_eq!(c.data_dir, PathBuf::from("/var/lib/tomi"));
        assert_eq!(c.max_devices, 7);
        assert_eq!(c.max_space_bytes, Some(500_000));
        assert_eq!(c.max_relay_bytes, 9_000_000);
        assert_eq!(c.rl_ip_auth, Rate::new(5.0, 3));
        assert_eq!(c.trusted_nets().unwrap().len(), 2);
        assert_eq!(
            c.fcm_service_account_file,
            Some(PathBuf::from("/etc/tomi/fcm.json"))
        );
        assert_eq!(c.log_format, LogFormat::Text);
    }

    #[test]
    fn unknown_keys_fail_the_startup() {
        let err = Config::load_with_env(None, env(&[("TOMI_RELAY_LISTN", "x")])).unwrap_err();
        assert!(err.to_string().contains("TOMI_RELAY_LISTN"), "{err}");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("c.toml");
        std::fs::write(&file, "max_devicez = 3\n").unwrap();
        assert!(Config::load_with_env(Some(&file), env(&[])).is_err());
    }

    #[test]
    fn toml_file_is_read_and_env_wins() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("c.toml");
        std::fs::write(
            &file,
            "max_devices = 3\nrl_ip_auth = { per_minute = 2, burst = 1 }\npublic_url = \"https://a.example\"\n",
        )
        .unwrap();
        let c =
            Config::load_with_env(Some(&file), env(&[("TOMI_RELAY_MAX_DEVICES", "9")])).unwrap();
        assert_eq!(c.max_devices, 9);
        assert_eq!(c.rl_ip_auth, Rate::new(2.0, 1));
        assert_eq!(c.public_url, "https://a.example");
    }

    #[test]
    fn refuses_a_public_listen_address_unless_allowed() {
        let mut c = Config {
            listen: "0.0.0.0:8080".into(),
            ..Config::default()
        };
        assert!(c.validate().unwrap_err().to_string().contains("loopback"));
        c.allow_public_listen = true;
        c.validate().unwrap();
        c.listen = "[::1]:8080".into();
        c.allow_public_listen = false;
        c.validate().unwrap();
    }

    #[test]
    fn quota_keys_are_validated() {
        // 0 is "no limit per space"; a number above 2^53 - 1 is refused.
        let none = Config {
            max_space_bytes: Some(0),
            ..Config::default()
        };
        none.validate().unwrap();
        assert_eq!(none.configured_space_quota(), None);
        assert_eq!(none.configured_space_quota_source(), "none");
        let some = Config {
            max_space_bytes: Some(5_000_000),
            ..Config::default()
        };
        assert_eq!(some.configured_space_quota(), Some(5_000_000));
        assert_eq!(some.configured_space_quota_source(), "config");
        let huge = Config {
            max_space_bytes: Some(1 << 53),
            ..Config::default()
        };
        assert!(huge
            .validate()
            .unwrap_err()
            .to_string()
            .contains("max_space_bytes"));
        let zero = Config {
            max_relay_bytes: 0,
            ..Config::default()
        };
        assert!(zero.validate().is_err());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("c.toml");
        std::fs::write(&file, "max_space_bytes = 123\n").unwrap();
        let c = Config::load_with_env(Some(&file), env(&[])).unwrap();
        assert_eq!(c.max_space_bytes, Some(123));
        assert!(
            Config::load_with_env(None, env(&[("TOMI_RELAY_MAX_SPACE_BYTES", "lots")])).is_err()
        );
    }

    #[test]
    fn the_web_panel_is_off_by_default_and_needs_an_https_public_url() {
        assert!(!Config::default().admin_web);
        let on = Config::load_with_env(None, env(&[("TOMI_RELAY_ADMIN_WEB", "true")])).unwrap();
        assert!(on.admin_web);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("c.toml");
        std::fs::write(
            &file,
            "admin_web = true\npublic_url = \"https://tomi.example:8443\"\n",
        )
        .unwrap();
        assert!(
            Config::load_with_env(Some(&file), env(&[]))
                .unwrap()
                .admin_web
        );
        for bad in [
            "http://tomi.example",
            "https://127.0.0.1",
            "https://[::1]",
            "tomi.example",
            "",
        ] {
            let on = Config {
                admin_web: true,
                public_url: bad.into(),
                ..Config::default()
            };
            let why = on.validate().unwrap_err().to_string();
            assert!(why.contains("admin_web"), "{bad}: {why}");
            // Without the panel, the same URL is not checked here.
            let off = Config {
                admin_web: false,
                public_url: bad.into(),
                ..Config::default()
            };
            off.validate().unwrap();
        }
        assert!(Config::load_with_env(None, env(&[("TOMI_RELAY_ADMIN_WEB", "maybe")])).is_err());
    }

    #[test]
    fn retention_must_exceed_inactivity() {
        let c = Config {
            event_retention_days: 30,
            ..Config::default()
        };
        assert!(c.validate().is_err());
    }
}
