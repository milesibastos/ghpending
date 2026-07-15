use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const LOCAL_CONFIG_NAME: &str = ".ghpending.toml";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub user: Option<String>,
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

/// Where the active config came from, in precedence order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// `--config <path>` flag.
    Flag,
    /// `GHPENDING_CONFIG` environment variable.
    Env,
    /// A `.ghpending.toml` discovered by walking up from the cwd.
    Local,
    /// The user-global config file.
    Global,
}

fn global_config_path() -> Result<PathBuf> {
    let proj =
        ProjectDirs::from("", "", "ghpending").context("could not determine config directory")?;
    Ok(proj.config_dir().join("config.toml"))
}

/// Precedence table: flag > env > local > global. Pure so it can be unit-tested.
fn choose_source(
    flag: Option<PathBuf>,
    env: Option<PathBuf>,
    local: Option<PathBuf>,
    global: impl FnOnce() -> Result<PathBuf>,
) -> Result<(PathBuf, ConfigSource)> {
    if let Some(p) = flag {
        Ok((p, ConfigSource::Flag))
    } else if let Some(p) = env {
        Ok((p, ConfigSource::Env))
    } else if let Some(p) = local {
        Ok((p, ConfigSource::Local))
    } else {
        Ok((global()?, ConfigSource::Global))
    }
}

/// Walk up from `start`, returning the first `.ghpending.toml` found. The search
/// stops after checking a directory that contains `.git`, so it never escapes
/// the project. Filesystem access is injected via `exists` for testability.
fn find_local_config(start: &Path, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join(LOCAL_CONFIG_NAME);
        if exists(&candidate) {
            return Some(candidate);
        }
        if exists(&dir.join(".git")) {
            break;
        }
    }
    None
}

/// Resolve which config file is active. `flag` is the `--config` value.
pub fn resolve_path(flag: Option<&Path>) -> Result<(PathBuf, ConfigSource)> {
    let flag = flag.map(PathBuf::from);
    let env = std::env::var_os("GHPENDING_CONFIG")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let local = std::env::current_dir()
        .ok()
        .and_then(|cwd| find_local_config(&cwd, |p| p.exists()));
    choose_source(flag, env, local, global_config_path)
}

pub fn load_from(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let cfg: Config =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

pub fn save_to(cfg: &Config, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating dir {}", parent.display()))?;
    }
    let text = toml::to_string(cfg).context("serializing config")?;
    std::fs::write(path, &text).with_context(|| format!("writing {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_user() {
        let cfg = Config {
            user: Some("octocat".into()),
            repos: vec!["owner/repo".into(), "foo/bar".into()],
            theme: None,
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.user.as_deref(), Some("octocat"));
        assert_eq!(back.repos, vec!["owner/repo", "foo/bar"]);
    }

    #[test]
    fn round_trip_user_none() {
        let cfg = Config {
            user: None,
            repos: vec!["owner/repo".into()],
            theme: None,
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert!(back.user.is_none());
        assert_eq!(back.repos, vec!["owner/repo"]);
    }

    #[test]
    fn default_on_missing_file() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.user.is_none());
        assert!(cfg.repos.is_empty());
        assert!(cfg.theme.is_none());
    }

    #[test]
    fn round_trip_with_theme() {
        let cfg = Config {
            user: Some("octocat".into()),
            repos: vec!["owner/repo".into()],
            theme: Some("nerv".into()),
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.theme.as_deref(), Some("nerv"));
    }

    #[test]
    fn round_trip_theme_none_omitted() {
        let cfg = Config {
            user: None,
            repos: vec![],
            theme: None,
        };
        let s = toml::to_string(&cfg).unwrap();
        assert!(!s.contains("theme"));
        let back: Config = toml::from_str(&s).unwrap();
        assert!(back.theme.is_none());
    }

    #[test]
    fn choose_source_prefers_flag() {
        let (path, src) = choose_source(
            Some("/flag.toml".into()),
            Some("/env.toml".into()),
            Some("/local.toml".into()),
            || Ok("/global.toml".into()),
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/flag.toml"));
        assert_eq!(src, ConfigSource::Flag);
    }

    #[test]
    fn choose_source_env_beats_local_and_global() {
        let (path, src) = choose_source(
            None,
            Some("/env.toml".into()),
            Some("/local.toml".into()),
            || Ok("/global.toml".into()),
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/env.toml"));
        assert_eq!(src, ConfigSource::Env);
    }

    #[test]
    fn choose_source_local_beats_global() {
        let (path, src) = choose_source(None, None, Some("/local.toml".into()), || {
            Ok("/global.toml".into())
        })
        .unwrap();
        assert_eq!(path, PathBuf::from("/local.toml"));
        assert_eq!(src, ConfigSource::Local);
    }

    #[test]
    fn choose_source_falls_back_to_global() {
        let (path, src) = choose_source(None, None, None, || Ok("/global.toml".into())).unwrap();
        assert_eq!(path, PathBuf::from("/global.toml"));
        assert_eq!(src, ConfigSource::Global);
    }

    #[test]
    fn find_local_config_finds_in_cwd() {
        let present: [PathBuf; 1] = ["/home/me/proj/.ghpending.toml".into()];
        let found = find_local_config(Path::new("/home/me/proj"), |p| {
            present.contains(&p.to_path_buf())
        });
        assert_eq!(found, Some(PathBuf::from("/home/me/proj/.ghpending.toml")));
    }

    #[test]
    fn find_local_config_walks_up_to_ancestor() {
        let present: [PathBuf; 1] = ["/home/me/proj/.ghpending.toml".into()];
        let found = find_local_config(Path::new("/home/me/proj/src/deep"), |p| {
            present.contains(&p.to_path_buf())
        });
        assert_eq!(found, Some(PathBuf::from("/home/me/proj/.ghpending.toml")));
    }

    #[test]
    fn find_local_config_stops_at_git_root() {
        // `.ghpending.toml` sits ABOVE the repo root; the `.git` at the repo
        // root must halt the walk before it is reached.
        let present: [PathBuf; 2] = [
            "/home/me/.ghpending.toml".into(),
            "/home/me/proj/.git".into(),
        ];
        let found = find_local_config(Path::new("/home/me/proj/src"), |p| {
            present.contains(&p.to_path_buf())
        });
        assert_eq!(found, None);
    }

    #[test]
    fn find_local_config_none_when_absent() {
        let found = find_local_config(Path::new("/home/me/proj"), |_| false);
        assert_eq!(found, None);
    }
}
