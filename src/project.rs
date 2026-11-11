//! Works out which project the current directory belongs to.
//!
//! Order, first match wins:
//! 1. a `.son-project` file in the directory or any parent
//! 2. the repo name from the git `origin` remote
//! 3. the directory name of the git repository root
//! 4. the name of the directory itself

use std::path::Path;
use std::process::{Command, Stdio};

pub const MARKER: &str = ".son-project";

pub fn detect(cwd: &Path) -> String {
    from_marker(cwd)
        .or_else(|| git(cwd, &["config", "--get", "remote.origin.url"]).and_then(|u| repo_name(&u)))
        .or_else(|| {
            git(cwd, &["rev-parse", "--show-toplevel"]).and_then(|p| dir_name(Path::new(&p)))
        })
        .or_else(|| dir_name(cwd))
        .unwrap_or_else(|| "default".to_string())
}

fn from_marker(cwd: &Path) -> Option<String> {
    cwd.ancestors().find_map(|dir| {
        let text = std::fs::read_to_string(dir.join(MARKER)).ok()?;
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
