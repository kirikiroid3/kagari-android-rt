//! Settings and saves on disk: one file per engine store name.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Where the engine's stores live.
pub struct Store {
    /// `<internal data>/saves`; `None` when the app has no data directory.
    saves_root: Option<PathBuf>,
    dir: Option<PathBuf>,
}

impl Store {
    /// A store without a directory until [`Store::set_title`].
    pub fn new(saves_root: Option<PathBuf>) -> Store {
        Store {
            saves_root,
            dir: None,
        }
    }

    /// Resolves `<saves root>/<sanitized title>`.
    pub fn set_title(&mut self, title: &str) {
        self.dir = self
            .saves_root
            .as_ref()
            .map(|root| root.join(sanitize_title(title)));
        match &self.dir {
            Some(dir) => log::info!(target: "kagari::android", "data directory: {}", dir.display()),
            None => log::warn!("no data directory found; settings and saves are not kept"),
        }
    }

    pub fn read_store(&mut self, name: &str) -> io::Result<Option<Vec<u8>>> {
        let path = self.path(name)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Writes `{name}.tmp`, syncs it and renames it over `name`.
    pub fn write_store(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let path = self.path(name)?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_file_name(format!("{name}.tmp"));
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &path)
    }

    pub fn delete_store(&mut self, name: &str) -> io::Result<()> {
        let path = self.path(name)?;
        match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    fn path(&self, name: &str) -> io::Result<PathBuf> {
        if !valid_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid store name `{name}`"),
            ));
        }
        let dir: &Path = self
            .dir
            .as_deref()
            .ok_or_else(|| io::Error::other("no data directory"))?;
        Ok(dir.join(name))
    }
}

/// `[a-z0-9-]+\.kisave`
fn valid_name(name: &str) -> bool {
    name.strip_suffix(".kisave").is_some_and(|stem| {
        !stem.is_empty()
            && stem
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// A game title as a directory name: path and reserved characters and controls become `_`,
/// surrounding whitespace and trailing dots go, and an empty result is `Kagari Game`.
pub fn sanitize_title(title: &str) -> String {
    let replaced: String = title
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = replaced.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "Kagari Game".to_owned()
    } else {
        trimmed.to_owned()
    }
}
