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
//! later settings can go in the same file. The keys are `collection` and
//! `files` (the folder where the collection is kept as files, see
//! `files.rs`). The files of earlier versions, `.son-collection` and `.son-project`
//! (one line: the name), are still read.

use std::path::{Path, PathBuf};
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

/// The key in `.son` that turns on the shared folder: its value is the
/// folder, relative to the folder that holds `.son`.
pub const FILES_KEY: &str = "files";

/// The text of a `.son` file with `key` set to `value`, or taken out for
/// `None`. Every other line stays as it is: comments, and keys that this
/// version does not know.
pub fn with_setting(text: &str, key: &str, value: Option<&str>) -> String {
    let is_key = |line: &str| {
        line.split('#')
            .next()
            .and_then(|part| part.split_once('='))
            .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(key))
    };
    let mut lines: Vec<String> = Vec::new();
    let mut done = false;
    for line in text.lines() {
        if !is_key(line) {
            lines.push(line.to_string());
        } else if let (Some(value), false) = (value, done) {
            lines.push(format!("{key} = {value}"));
            done = true;
        }
    }
    if let (Some(value), false) = (value, done) {
        lines.push(format!("{key} = {value}"));
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// What a `.son` file says about a shared folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Shared {
    /// The folder that holds the `.son` file.
    pub root: PathBuf,
    /// The value of the `files` key.
    pub folder: String,
    /// The collection that the same file names, when it names one.
    pub collection: Option<String>,
}

impl Shared {
    /// The folder with the files. An error when the value of the key
    /// leaves the folder of `.son`: an absolute path, or one with `..`.
    pub fn dir(&self) -> Result<PathBuf, String> {
        use std::path::Component;
        let folder = Path::new(&self.folder);
        let inside = folder
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
        if !inside || folder.components().all(|part| part == Component::CurDir) {
            return Err(format!(
                "`{FILES_KEY} = {}` in {} must name a folder inside that folder",
                self.folder,
                self.root.join(SETTINGS).display()
            ));
        }
        Ok(self.root.join(folder))
    }
}

/// The shared folder that the `.son` file in `root` turns on, if it does.
pub fn shared_in(root: &Path) -> Option<Shared> {
    let text = std::fs::read_to_string(root.join(SETTINGS)).ok()?;
    Some(Shared {
        root: root.to_path_buf(),
        folder: setting(&text, FILES_KEY)?,
        collection: setting(&text, "collection"),
    })
}

/// The shared folder for work in `cwd`: from the nearest `.son`, in the
/// directory or a parent, that has the `files` key.
pub fn shared(cwd: &Path) -> Option<Shared> {
    cwd.ancestors().find_map(shared_in)
}

/// The folder of the nearest `.son` file, in the directory or a parent.
pub fn settings_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| dir.join(SETTINGS).is_file())
        .map(Path::to_path_buf)
}

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
pub fn root(cwd: &Path) -> PathBuf {
    git(cwd, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
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
    fn a_setting_is_written_and_the_other_lines_stay() {
        let text = "# settings\nfuture_key = 1\ncollection = alpha  # the name\n";
        let on = with_setting(text, FILES_KEY, Some(".son-data"));
        assert_eq!(
            on,
            "# settings\nfuture_key = 1\ncollection = alpha  # the name\nfiles = .son-data\n"
        );
        assert_eq!(setting(&on, FILES_KEY).as_deref(), Some(".son-data"));
        let moved = with_setting(&on, FILES_KEY, Some("docs/memory"));
        assert_eq!(setting(&moved, FILES_KEY).as_deref(), Some("docs/memory"));
        assert_eq!(moved.matches("files =").count(), 1);
        assert_eq!(with_setting(&moved, FILES_KEY, None), text);
        assert_eq!(
            with_setting("", "collection", Some("a")),
            "collection = a\n"
        );
    }

    #[test]
    fn a_shared_folder_must_stay_inside() {
        let shared = |folder: &str| Shared {
            root: PathBuf::from("root"),
            folder: folder.to_string(),
            collection: None,
        };
        assert_eq!(
            shared(".son-data").dir().unwrap(),
            Path::new("root").join(".son-data")
        );
        assert!(shared("docs/memory").dir().is_ok());
        assert!(shared("../other").dir().is_err());
        assert!(shared("/tmp/x").dir().is_err());
        assert!(shared(".").dir().is_err());
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
