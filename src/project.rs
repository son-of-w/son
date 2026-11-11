//! Works out which collection a command works in when `--collection` does
//! not name one.
//!
//! Order, first match wins:
//! 1. the `SON_COLLECTION` environment variable
//! 2. a `.son` file in the directory or any parent, with the line
//!    `collection = <name>`
//! 3. the repo name from the git `origin` remote
//! 4. the directory name of the git repository root
//! 5. the name of the directory itself
//!
//! 1 and 2 give an agent that does not work in a repository (household or
//! travel memory, say) a collection that does not depend on where it runs.
//!
//! `.son` is the one settings file for a folder: `key = value` lines, `#`
//! starts a comment, and keys this version does not know are left alone, so
//! later settings can go in the same file. `collection` is the only key
//! now. The files of earlier versions, `.son-collection` and `.son-project`
//! (one line: the name), are still read.

use std::path::Path;
use std::process::{Command, Stdio};

pub const SETTINGS: &str = ".son";
const OLD_MARKERS: [&str; 2] = [".son-collection", ".son-project"];

/// The value of `key` in the text of a `.son` file.
pub fn setting(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.split('#').next()?.trim();
        let (name, value) = line.split_once('=')?;
        let value = value.trim().trim_matches('"').trim();
        (name.trim().eq_ignore_ascii_case(key) && !value.is_empty()).then(|| value.to_string())
    })
}
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
        // A `.son` that names no collection does not hide an older file.
        if let Some(name) = std::fs::read_to_string(dir.join(SETTINGS))
            .ok()
            .and_then(|text| setting(&text, "collection"))
        {
            return Some(name);
        }
        OLD_MARKERS.iter().find_map(|file| {
            let text = std::fs::read_to_string(dir.join(file)).ok()?;
            let name = text.lines().next()?.trim();
            (!name.is_empty()).then(|| name.to_string())
        })
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

/// `https://github.com/son-of-w/son.git` and
/// `git@github.com:son-of-w/son.git` both give `son`.
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
            repo_name("https://github.com/son-of-w/son.git").as_deref(),
            Some("son")
        );
        assert_eq!(
            repo_name("https://github.com/son-of-w/son").as_deref(),
            Some("son")
        );
        assert_eq!(
            repo_name("git@github.com:son-of-w/son.git").as_deref(),
            Some("son")
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
        std::fs::write(
            root.join(SETTINGS),
            "# settings for this folder\nfuture_key = 1\n collection = \"my-project\"  # the name\n",
        )
        .unwrap();
        assert_eq!(detect(&nested), "my-project");
        // A nearer folder wins, and an older one-line file is still read.
        std::fs::write(
            root.join("a").join(".son-collection"),
            " older-name \nignored\n",
        )
        .unwrap();
        assert_eq!(detect(&nested), "older-name");
        std::fs::write(root.join("a").join(SETTINGS), "collection=nearer\n").unwrap();
        assert_eq!(detect(&nested), "nearer");
        assert_eq!(setting("collection =\n", "collection"), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
