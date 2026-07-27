use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use rusqlite::{params, Connection};
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
    #[allow(dead_code)] // will drive default_view/default_group/palette in v1.1
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

/// Merge `loser_id` into `keeper_id`: append/update the user aliases file so
/// the loser's raw (name, email) pairs canonicalize onto the keeper's identity,
/// then remap the DB in one pass. Repo `.mailmap` is never written.
pub fn merge_authors(
    aliases_path: &Path,
    conn: &Connection,
    keeper_id: i64,
    loser_id: i64,
) -> Result<()> {
    let keeper: (String, String) = conn.query_row(
        "SELECT canonical_name, canonical_email FROM authors WHERE id = ?1",
        params![keeper_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    let loser_raws: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT raw_name, raw_email FROM author_aliases WHERE author_id = ?1",
        )?;
        let rows = stmt.query_map(params![loser_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    // Load existing aliases file (or default), append, write back.
    let mut aliases: Aliases = if aliases_path.exists() {
        read_toml(aliases_path).unwrap_or_default()
    } else {
        Aliases::default()
    };

    let entry = aliases
        .entries
        .iter_mut()
        .find(|e| e.canonical_name == keeper.0 && e.canonical_email == keeper.1);

    let raws_owned: Vec<RawIdentity> = loser_raws
        .iter()
        .cloned()
        .map(|(name, email)| RawIdentity { name, email })
        .collect();

    match entry {
        Some(e) => {
            for r in raws_owned {
                if !e.raw.iter().any(|x| x.name == r.name && x.email == r.email) {
                    e.raw.push(r);
                }
            }
        }
        None => aliases.entries.push(AliasEntry {
            canonical_name: keeper.0.clone(),
            canonical_email: keeper.1.clone(),
            raw: raws_owned,
        }),
    }

    if let Some(parent) = aliases_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let toml_text = toml::to_string_pretty(&aliases)?;
    std::fs::write(aliases_path, toml_text)
        .with_context(|| format!("writing {}", aliases_path.display()))?;

    // Repoint the DB: remap loser's aliases to keeper and delete the loser row.
    conn.execute(
        "UPDATE author_aliases SET author_id = ?1 WHERE author_id = ?2",
        params![keeper_id, loser_id],
    )?;
    conn.execute(
        "UPDATE commits SET author_id = ?1 WHERE author_id = ?2",
        params![keeper_id, loser_id],
    )?;
    conn.execute("DELETE FROM authors WHERE id = ?1", params![loser_id])?;

    Ok(())
}
