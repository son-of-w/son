//! Works out which collection a command works in when `--collection` does
//! not name one.
//!
//! Order, first match wins:
//! 1. the `SON_COLLECTION` environment variable
//! 2. a `.son-collection` file in the directory or any parent
//! 3. the repo name from the git `origin` remote
//! 4. the directory name of the git repository root
//! 5. the name of the directory itself
//!
//! 1 and 2 give an agent that does not work in a repository (household or
//! travel memory, say) a collection that does not depend on where it runs.
//! A `.son-project` file, the name before collections, is still read.

use std::path::Path;
use std::process::{Command, Stdio};

pub const MARKER: &str = ".son-collection";
const OLD_MARKER: &str = ".son-project";
pub const ENV: &str = "SON_COLLECTION";

pub fn detect(cwd: &Path) -> String {
    std::env::var(ENV)
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .or_else(|| from_marker(cwd))
        .or_else(|| git(cwd, &["config", "--get", "remote.origin.url"]).and_then(|u| repo_name(&u)))
        .or_else(|| {
            git(cwd, &["rev-parse", "--show-toplevel"]).and_then(|p| dir_name(Path::new(&p)))
        })
        .or_else(|| dir_name(cwd))
        .unwrap_or_else(|| "default".to_string())
}

/// The folder that holds the project: the git root, else `cwd` itself.
pub fn root(cwd: &Path) -> std::path::PathBuf {
    git(cwd, &["rev-parse", "--show-toplevel"])
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf())
}

fn from_marker(cwd: &Path) -> Option<String> {
    cwd.ancestors().find_map(|dir| {
        let text = std::fs::read_to_string(dir.join(MARKER))
            .or_else(|_| std::fs::read_to_string(dir.join(OLD_MARKER)))
            .ok()?;
        let name = text.lines().next()?.trim();
        (!name.is_empty()).then(|| name.to_string())
    })
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn dir_name(path: &Path) -> Option<String> {
    Some(path.file_name()?.to_str()?.to_string())
}

/// `https://github.com/Raxvis/son-of-w.git` and
/// `git@github.com:Raxvis/son-of-w.git` both give `son-of-w`.
fn repo_name(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let name = url.rsplit(['/', ':']).next()?;
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_name_from_remote_urls() {
        assert_eq!(
            repo_name("https://github.com/Raxvis/son-of-w.git").as_deref(),
            Some("son-of-w")
        );
        assert_eq!(
            repo_name("https://github.com/Raxvis/son-of-w").as_deref(),
            Some("son-of-w")
        );
        assert_eq!(
            repo_name("git@github.com:Raxvis/son-of-w.git").as_deref(),
            Some("son-of-w")
        );
        assert_eq!(
            repo_name("ssh://git@host/team/infra/").as_deref(),
            Some("infra")
        );
        assert_eq!(repo_name("").as_deref(), None);
    }

    #[test]
    fn marker_in_parent_wins() {
        let root = std::env::temp_dir().join(format!("son-marker-{}", std::process::id()));
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join(MARKER), "  my-project \nignored\n").unwrap();
        assert_eq!(detect(&nested), "my-project");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
