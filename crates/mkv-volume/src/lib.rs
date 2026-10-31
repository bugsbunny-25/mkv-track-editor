//! Scoped, path-traversal-safe access to configured library/output root
//! directories. Used only by the server: the desktop build relies on native
//! file dialogs for trusted absolute paths instead.
//!
//! Every path that arrives from a client is treated as untrusted: it's
//! validated component-by-component, joined under a configured root,
//! canonicalized (which resolves `..` *and* symlinks), and then checked to
//! still be inside that root's canonical path. Client-supplied absolute
//! paths are never honored.

mod error;

pub use error::VolumeError;

use mkv_types::{DirEntry, RootInfo};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct VolumeRoot {
    pub id: String,
    pub label: String,
    /// Always an absolute, canonicalized path.
    root: PathBuf,
}

pub struct VolumeManager {
    roots: Vec<VolumeRoot>,
    output_root: VolumeRoot,
}

const OUTPUT_ROOT_ID: &str = "output";

impl VolumeManager {
    pub fn new(
        root_specs: Vec<(String, PathBuf)>,
        output_root_override: Option<PathBuf>,
    ) -> Result<Self, VolumeError> {
        if root_specs.is_empty() {
            return Err(VolumeError::InvalidConfig(
                "at least one library root must be configured".into(),
            ));
        }

        let mut roots = Vec::with_capacity(root_specs.len());
        for (id, path) in &root_specs {
            let canonical = canonicalize(path)?;
            roots.push(VolumeRoot {
                id: id.clone(),
                label: id.clone(),
                root: canonical,
            });
        }

        let output_root_path = output_root_override.unwrap_or_else(|| root_specs[0].1.join("_output"));
        std::fs::create_dir_all(&output_root_path).map_err(|source| VolumeError::Io {
            path: output_root_path.clone(),
            source,
        })?;
        let output_root = VolumeRoot {
            id: OUTPUT_ROOT_ID.to_string(),
            label: "Output".to_string(),
            root: canonicalize(&output_root_path)?,
        };

        Ok(Self { roots, output_root })
    }

    /// Reads `MKV_LIBRARY_ROOTS` (`name=path,name2=path2`) and
    /// `MKV_OUTPUT_ROOT` (optional; defaults to `<first root>/_output`).
    pub fn from_env() -> Result<Self, VolumeError> {
        let spec = std::env::var("MKV_LIBRARY_ROOTS")
            .map_err(|_| VolumeError::InvalidConfig("MKV_LIBRARY_ROOTS is not set".into()))?;
        let roots = parse_roots_spec(&spec)?;
        let output_override = std::env::var("MKV_OUTPUT_ROOT").ok().map(PathBuf::from);
        Self::new(roots, output_override)
    }

    pub fn list_roots(&self) -> Vec<RootInfo> {
        self.roots
            .iter()
            .map(|r| RootInfo {
                id: r.id.clone(),
                label: r.label.clone(),
            })
            .collect()
    }

    pub fn default_root_id(&self) -> &str {
        &self.roots[0].id
    }

    fn find_root(&self, root_id: &str) -> Result<&VolumeRoot, VolumeError> {
        if root_id == OUTPUT_ROOT_ID {
            return Ok(&self.output_root);
        }
        self.roots
            .iter()
            .find(|r| r.id == root_id)
            .ok_or_else(|| VolumeError::UnknownRoot(root_id.to_string()))
    }

    /// Resolves a client-supplied relative path under `root_id` to a safe,
    /// canonical absolute path. The target must already exist.
    pub fn resolve(&self, root_id: &str, relative: &str) -> Result<PathBuf, VolumeError> {
        let root = self.find_root(root_id)?;
        let rel = validate_relative(relative)?;
        let candidate = root.root.join(&rel);
        let canonical = canonicalize(&candidate)?;
        ensure_within(&canonical, &root.root)
    }

    /// Resolves a client-supplied relative path under the output root to an
    /// absolute path suitable as a *new* file's destination: the parent
    /// directory must exist and stay inside the root, but the leaf file
    /// itself need not exist yet.
    pub fn resolve_output(&self, relative: &str) -> Result<PathBuf, VolumeError> {
        let rel = validate_relative(relative)?;
        let file_name = rel.file_name().ok_or(VolumeError::InvalidComponent)?.to_owned();
        let parent_rel = rel.parent().unwrap_or_else(|| Path::new(""));
        let parent_abs = self.output_root.root.join(parent_rel);
        let canonical_parent = canonicalize(&parent_abs)?;
        ensure_within(&canonical_parent, &self.output_root.root)?;
        Ok(canonical_parent.join(file_name))
    }

    /// Permanently deletes one file under `root_id`. Directories are refused,
    /// and so is a symlink: it's resolved like any other path, so only the
    /// file it points to inside the root could be reached, never the link.
    pub fn remove_file(&self, root_id: &str, relative: &str) -> Result<PathBuf, VolumeError> {
        let path = self.resolve(root_id, relative)?;
        let metadata = std::fs::metadata(&path).map_err(|source| VolumeError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(VolumeError::NotAFile);
        }
        std::fs::remove_file(&path).map_err(|source| VolumeError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }

    pub fn list_dir(&self, root_id: &str, relative: &str) -> Result<Vec<DirEntry>, VolumeError> {
        let dir = self.resolve(root_id, relative)?;
        let read_dir = std::fs::read_dir(&dir).map_err(|source| VolumeError::Io {
            path: dir.clone(),
            source,
        })?;

        let mut entries = Vec::new();
        for entry in read_dir {
            let entry = entry.map_err(|source| VolumeError::Io {
                path: dir.clone(),
                source,
            })?;
            let metadata = entry.metadata().map_err(|source| VolumeError::Io {
                path: entry.path(),
                source,
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_rel = if relative.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", relative.trim_end_matches('/'), name)
            };
            entries.push(DirEntry {
                name,
                path: child_rel,
                is_dir: metadata.is_dir(),
                size: if metadata.is_dir() { None } else { Some(metadata.len()) },
            });
        }
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
        Ok(entries)
    }
}

/// Rejects absolute paths and any `..`/root/prefix component, then returns
/// the cleaned relative path. This runs *before* canonicalization as
/// defense in depth; canonicalization (which also catches symlink escapes)
/// is the authoritative check, done by the caller via [`ensure_within`].
fn validate_relative(relative: &str) -> Result<PathBuf, VolumeError> {
    let path = Path::new(relative);
    if path.is_absolute() {
        return Err(VolumeError::AbsolutePath);
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(seg) => clean.push(seg),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(VolumeError::PathEscape);
            }
        }
    }
    Ok(clean)
}

fn canonicalize(path: &Path) -> Result<PathBuf, VolumeError> {
    std::fs::canonicalize(path).map_err(|source| VolumeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn ensure_within(candidate: &Path, root: &Path) -> Result<PathBuf, VolumeError> {
    if candidate.starts_with(root) {
        Ok(candidate.to_path_buf())
    } else {
        Err(VolumeError::PathEscape)
    }
}

fn parse_roots_spec(spec: &str) -> Result<Vec<(String, PathBuf)>, VolumeError> {
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|entry| {
            let (name, path) = entry.split_once('=').ok_or_else(|| {
                VolumeError::InvalidConfig(format!("expected `name=path`, got `{entry}`"))
            })?;
            Ok((name.trim().to_string(), PathBuf::from(path.trim())))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_manager() -> (tempfile::TempDir, VolumeManager) {
        let tmp = tempfile::tempdir().unwrap();
        let lib_root = tmp.path().join("library");
        fs::create_dir_all(lib_root.join("Season 1")).unwrap();
        fs::write(lib_root.join("Season 1").join("ep1.mkv"), b"fake").unwrap();
        fs::write(lib_root.join("top.mkv"), b"fake").unwrap();

        // Explicit output root outside the library dir so listing the
        // library root isn't polluted by the default `_output` subdir.
        let manager = VolumeManager::new(
            vec![("movies".to_string(), lib_root.clone())],
            Some(tmp.path().join("out")),
        )
        .unwrap();
        (tmp, manager)
    }

    #[test]
    fn resolves_plain_nested_path() {
        let (_tmp, manager) = make_manager();
        let resolved = manager.resolve("movies", "Season 1/ep1.mkv").unwrap();
        assert!(resolved.ends_with("Season 1/ep1.mkv"));
    }

    #[test]
    fn rejects_dotdot_traversal() {
        let (_tmp, manager) = make_manager();
        let err = manager.resolve("movies", "../../etc/passwd").unwrap_err();
        assert!(matches!(err, VolumeError::PathEscape));
    }

    #[test]
    fn rejects_dotdot_traversal_mixed_with_valid_segments() {
        let (_tmp, manager) = make_manager();
        let err = manager
            .resolve("movies", "Season 1/../../../etc/passwd")
            .unwrap_err();
        assert!(matches!(err, VolumeError::PathEscape));
    }

    #[test]
    fn rejects_absolute_path() {
        let (_tmp, manager) = make_manager();
        let err = manager.resolve("movies", "/etc/passwd").unwrap_err();
        assert!(matches!(err, VolumeError::AbsolutePath));
    }

    #[test]
    fn rejects_unknown_root() {
        let (_tmp, manager) = make_manager();
        let err = manager.resolve("nope", "top.mkv").unwrap_err();
        assert!(matches!(err, VolumeError::UnknownRoot(_)));
    }

    #[test]
    fn rejects_symlink_escaping_root() {
        let tmp = tempfile::tempdir().unwrap();
        let lib_root = tmp.path().join("library");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&lib_root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"top secret").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, lib_root.join("escape")).unwrap();

        let manager = VolumeManager::new(vec![("movies".to_string(), lib_root)], None).unwrap();
        let err = manager.resolve("movies", "escape/secret.txt").unwrap_err();
        assert!(matches!(err, VolumeError::PathEscape));
    }

    #[test]
    fn allows_symlink_staying_inside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let lib_root = tmp.path().join("library");
        let real_dir = lib_root.join("real");
        fs::create_dir_all(&real_dir).unwrap();
        fs::write(real_dir.join("a.mkv"), b"fake").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_dir, lib_root.join("alias")).unwrap();

        let manager = VolumeManager::new(vec![("movies".to_string(), lib_root)], None).unwrap();
        let resolved = manager.resolve("movies", "alias/a.mkv").unwrap();
        assert!(resolved.ends_with("real/a.mkv"));
    }

    #[test]
    fn list_dir_returns_sorted_entries() {
        let (_tmp, manager) = make_manager();
        let entries = manager.list_dir("movies", "").unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "Season 1");
        assert_eq!(entries[1].name, "top.mkv");
        assert_eq!(entries[1].size, Some(4));
    }

    #[test]
    fn resolve_output_defaults_under_source_root() {
        let tmp = tempfile::tempdir().unwrap();
        let lib_root = tmp.path().join("library");
        fs::create_dir_all(&lib_root).unwrap();
        let manager = VolumeManager::new(vec![("movies".to_string(), lib_root)], None).unwrap();

        let out = manager.resolve_output("renamed.mkv").unwrap();
        assert!(out.to_string_lossy().contains("_output"));
        assert!(out.ends_with("renamed.mkv"));
    }

    #[test]
    fn resolve_output_rejects_traversal() {
        let (_tmp, manager) = make_manager();
        let err = manager.resolve_output("../../etc/passwd").unwrap_err();
        assert!(matches!(err, VolumeError::PathEscape));
    }

    #[test]
    fn resolve_output_rejects_absolute_path() {
        let (_tmp, manager) = make_manager();
        let err = manager.resolve_output("/etc/passwd").unwrap_err();
        assert!(matches!(err, VolumeError::AbsolutePath));
    }

    #[test]
    fn remove_file_deletes_only_files_inside_the_root() {
        let (tmp, manager) = make_manager();
        let removed = manager.remove_file("movies", "Season 1/ep1.mkv").unwrap();
        assert!(removed.ends_with("Season 1/ep1.mkv"));
        assert!(!tmp.path().join("library/Season 1/ep1.mkv").exists());

        assert!(matches!(manager.remove_file("movies", "Season 1"), Err(VolumeError::NotAFile)));
        assert!(matches!(manager.remove_file("movies", "../library/top.mkv"), Err(VolumeError::PathEscape)));
        assert!(matches!(manager.remove_file("movies", "gone.mkv"), Err(VolumeError::Io { .. })));
        assert!(tmp.path().join("library/top.mkv").exists());
    }

    #[test]
    fn custom_output_root_is_honored() {
        let tmp = tempfile::tempdir().unwrap();
        let lib_root = tmp.path().join("library");
        let out_root = tmp.path().join("custom_out");
        fs::create_dir_all(&lib_root).unwrap();

        let manager = VolumeManager::new(
            vec![("movies".to_string(), lib_root)],
            Some(out_root.clone()),
        )
        .unwrap();
        let out = manager.resolve_output("x.mkv").unwrap();
        assert!(out.starts_with(fs::canonicalize(&out_root).unwrap()));
    }
}
