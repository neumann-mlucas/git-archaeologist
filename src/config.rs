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
    pub default_lens: String,
    pub palette: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_bucket: "auto".into(),
            default_view: "cumulative".into(),
            default_group: "language".into(),
            default_lens: "structure".into(),
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
    /// Path to the user's aliases.toml — currently unused post-Lens-reframe,
    /// held for the pending Ownership-lens first-run wizard.
    #[allow(dead_code)]
    pub aliases_path: PathBuf,
}

impl Loaded {
    pub fn palette_kind(&self) -> crate::ui::palette::PaletteKind {
        crate::ui::palette::PaletteKind::parse(&self.config.palette)
    }

    pub fn default_view(&self) -> crate::query::View {
        match self.config.default_view.trim().to_lowercase().as_str() {
            "delta" => crate::query::View::Delta,
            _ => crate::query::View::Cumulative,
        }
    }

    pub fn default_group(&self) -> crate::query::GroupBy {
        match self.config.default_group.trim().to_lowercase().as_str() {
            "author" => crate::query::GroupBy::Author,
            "module" => crate::query::GroupBy::Module,
            _ => crate::query::GroupBy::Language,
        }
    }

    pub fn default_lens(&self) -> crate::query::Lens {
        match self.config.default_lens.trim().to_lowercase().as_str() {
            "activity" | "churn" => crate::query::Lens::Activity,
            "ownership" | "author" | "blame" => crate::query::Lens::Ownership,
            _ => crate::query::Lens::Structure,
        }
    }

    pub fn default_bucket(&self) -> Option<crate::index::bucket::BucketSize> {
        crate::index::bucket::BucketSize::parse(&self.config.default_bucket)
    }
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
///
/// Currently unused — the auto-heuristic modal was removed as part of the
/// Lens reframe. Kept for the pending Ownership-wizard first-run flow.
#[allow(dead_code)]
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
