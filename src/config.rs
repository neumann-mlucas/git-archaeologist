use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub default_bucket: String,
    pub default_view: String,
    pub default_group: String,
    pub palette: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_bucket: "auto".into(),
            default_view: "cumulative".into(),
            default_group: "language".into(),
            palette: "default".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Aliases {
    #[serde(default, rename = "alias")]
    pub entries: Vec<AliasEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasEntry {
    pub canonical_name: String,
    pub canonical_email: String,
    #[serde(default)]
    pub raw: Vec<RawIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawIdentity {
    pub name: String,
    pub email: String,
}

pub struct Loaded {
    pub config: Config,
    pub aliases: Aliases,
    pub aliases_path: PathBuf,
}

pub fn load() -> Result<Loaded> {
    let dirs = ProjectDirs::from("", "", "git-archaeologist")
        .context("resolving user config dir")?;
    let cfg_dir = dirs.config_dir().to_path_buf();
    std::fs::create_dir_all(&cfg_dir).ok();

    let config_path = cfg_dir.join("config.toml");
    let aliases_path = cfg_dir.join("aliases.toml");

    let config = read_toml(&config_path).unwrap_or_default();
    let aliases = read_toml(&aliases_path).unwrap_or_default();

    Ok(Loaded {
        config,
        aliases,
        aliases_path,
    })
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &std::path::Path) -> Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let val = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(val)
}
