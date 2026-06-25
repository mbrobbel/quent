// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Turn a context directory's `model.qmi` sidecar into a [`ViewerSpec`]: the
//! pinned git sources, analyzer package, and artifact format needed to generate
//! and build a viewer for it.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use quent_build_info::{ArtifactInfo, BuildInfo, SIDECAR_FILE_NAME};

use crate::error::{OpenError, Result};

/// Recursively discover context directories (those containing a `model.qmi`
/// sidecar) under the given `paths`. A directory that is itself a context is not
/// descended into; hidden directories (dotfiles, e.g. `.git`) and symlinks (to
/// avoid cycles) are skipped during the walk. Results are canonicalized and
/// deduplicated, preserving discovery order.
pub fn discover_contexts(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        collect_contexts(path, &mut found, &mut seen)?;
    }
    Ok(found)
}

fn collect_contexts(
    dir: &Path,
    found: &mut Vec<PathBuf>,
    seen: &mut std::collections::HashSet<PathBuf>,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    if dir.join(SIDECAR_FILE_NAME).is_file() {
        let canonical = dir.canonicalize()?;
        if seen.insert(canonical.clone()) {
            found.push(canonical);
        }
        return Ok(()); // a context is a leaf; do not descend into its entity dirs
    }
    for entry in std::fs::read_dir(dir)?.flatten() {
        // `file_type()` does not traverse symlinks, so a symlinked directory is
        // neither hidden-checked away nor recursed into — this keeps the walk
        // cycle-safe (a symlink back to an ancestor can't loop).
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let child = entry.path();
        let hidden = child
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if !hidden {
            collect_contexts(&child, found, seen)?;
        }
    }
    Ok(())
}

/// Serialization format of an artifact's event streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ndjson,
    Msgpack,
    Postcard,
}

impl Format {
    /// File extension of an event stream in this format.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Ndjson => "ndjson",
            Format::Msgpack => "msgpack",
            Format::Postcard => "postcard",
        }
    }

    /// The `quent_exporter::FileSystemFormat` variant name, for generated code.
    pub fn variant(self) -> &'static str {
        match self {
            Format::Ndjson => "Ndjson",
            Format::Msgpack => "Msgpack",
            Format::Postcard => "Postcard",
        }
    }

    fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "ndjson" => Some(Format::Ndjson),
            "msgpack" => Some(Format::Msgpack),
            "postcard" => Some(Format::Postcard),
            _ => None,
        }
    }
}

/// A git source pinned to an exact commit, as recorded in the sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPin {
    pub remote: String,
    pub commit: String,
}

impl GitPin {
    /// The remote as a URL Cargo accepts in a `git = "..."` dependency.
    ///
    /// Git records `origin` as an scp-style address (`git@host:path`), which
    /// Cargo rejects; rewrite it to `ssh://git@host/path`. URLs that already
    /// carry a scheme (`https://`, `ssh://`, …) are left unchanged, as are local
    /// paths — matching git, a remote is scp-style only when the first colon has
    /// no slash before it (so `/tmp/foo:bar` stays a path).
    pub fn cargo_url(&self) -> String {
        if self.remote.contains("://") {
            return self.remote.clone();
        }
        match self.remote.split_once(':') {
            Some((host, path)) if !host.contains('/') => format!("ssh://{host}/{path}"),
            _ => self.remote.clone(),
        }
    }

    /// Extract a pin from a [`BuildInfo`], validating the (untrusted) remote and
    /// commit so they can't inject into the generated `Cargo.toml`.
    fn from_build_info(info: &BuildInfo, what: &str) -> Result<Self> {
        match (&info.remote, &info.commit) {
            (Some(remote), Some(commit)) => {
                validate_remote(remote)?;
                validate_commit(commit)?;
                Ok(GitPin {
                    remote: remote.clone(),
                    commit: commit.clone(),
                })
            }
            _ => Err(OpenError::MissingProvenance { what: what.into() }),
        }
    }
}

/// A git commit must be a hex object id (sha-1 or sha-256, possibly abbreviated).
fn validate_commit(commit: &str) -> Result<()> {
    let ok = (7..=64).contains(&commit.len()) && commit.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then_some(())
        .ok_or_else(|| OpenError::InvalidProvenance {
            field: "commit".into(),
            value: commit.into(),
        })
}

/// A git remote must use an authenticated/integrity-checked transport — `https`
/// or `ssh` (incl. scp-style `user@host:path`) — and be free of characters that
/// could break out of the generated TOML string. Unauthenticated `http`/`git`
/// transports are rejected so a trusted source can't be silently downgraded
/// (the scheme is dropped during trust canonicalization).
fn validate_remote(remote: &str) -> Result<()> {
    let inject = remote
        .bytes()
        .any(|b| b.is_ascii_control() || b == b'"' || b == b'\\');
    // scp-style: `[user@]host:path` — a `:` before any `/`, user optional
    // (matches git; `cargo_url` rewrites it to an `ssh://` URL).
    let shaped = matches!(remote.split_once("://"), Some(("https" | "ssh", _)))
        || (!remote.contains("://")
            && remote
                .split_once(':')
                .is_some_and(|(host, _)| !host.is_empty() && !host.contains('/')));
    (!inject && shaped)
        .then_some(())
        .ok_or_else(|| OpenError::InvalidProvenance {
            field: "remote".into(),
            value: remote.into(),
        })
}

/// A cargo package name: ASCII alphanumerics, `-`, and `_` only. Keeps the name
/// safe to interpolate into the manifest and `use <crate>::Viewer`.
fn validate_package(package: &str) -> Result<()> {
    let ok = !package.is_empty()
        && package
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then_some(())
        .ok_or_else(|| OpenError::InvalidProvenance {
            field: "analyzer_package".into(),
            value: package.into(),
        })
}

/// Everything needed to generate and build a viewer (the contexts it serves are
/// tracked separately, since one viewer can serve several same-spec contexts).
#[derive(Debug, Clone)]
pub struct ViewerSpec {
    /// Event serialization format, detected from the on-disk streams.
    pub format: Format,
    /// Cargo package of the analyzer crate providing `Viewer` (`QuentViewer`).
    pub analyzer_package: String,
    /// Quent framework source, pinned to the build commit.
    pub quent: GitPin,
    /// Analyzer crate source, pinned to the build commit (the model's source).
    pub analyzer: GitPin,
}

impl ViewerSpec {
    /// Derive a spec from a sidecar and its context directory.
    pub fn from_artifact(root: &Path, info: &ArtifactInfo) -> Result<Self> {
        let analyzer_package =
            info.model
                .analyzer_package
                .clone()
                .ok_or_else(|| OpenError::NoAnalyzer {
                    model: info.model.name.clone(),
                })?;
        validate_package(&analyzer_package)?;
        Ok(Self {
            format: detect_format(root)?,
            analyzer_package,
            quent: GitPin::from_build_info(&info.quent, "quent")?,
            analyzer: GitPin::from_build_info(&info.model.source, "analyzer source")?,
        })
    }

    /// Rust crate identifier of the analyzer package (hyphens to underscores), to
    /// name `<crate>::Viewer` in generated code.
    pub fn analyzer_crate(&self) -> String {
        self.analyzer_package.replace('-', "_")
    }

    /// Full, unambiguous identity of a build — every input that affects its
    /// output (analyzer package, format, and both git pins incl. their remotes
    /// and full commits). Use this to group/dedup contexts into viewers.
    pub fn group_key(&self) -> String {
        // Unit separator between fields so values can't run together.
        [
            self.analyzer_package.as_str(),
            self.format.extension(),
            &self.quent.remote,
            &self.quent.commit,
            &self.analyzer.remote,
            &self.analyzer.commit,
        ]
        .join("\u{1f}")
    }

    /// Filesystem-safe cache directory name for this viewer's generated crate and
    /// build. A readable prefix plus a hash of [`group_key`](Self::group_key), so
    /// distinct builds never share a directory even when their short commits or
    /// packages match.
    pub fn cache_key(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.group_key().hash(&mut hasher);
        format!(
            "{}-{}-{}-{:016x}",
            self.analyzer_package,
            short_commit(&self.analyzer.commit),
            self.format.extension(),
            hasher.finish(),
        )
    }
}

/// First 12 chars of a commit hash, for compact cache keys.
fn short_commit(commit: &str) -> &str {
    let end = commit.len().min(12);
    &commit[..end]
}

/// Detect the artifact format by finding an `events.<ext>` stream in any of the
/// context directory's per-entity subdirectories.
fn detect_format(root: &Path) -> Result<Format> {
    let entries = std::fs::read_dir(root).map_err(|source| OpenError::Sidecar {
        path: root.to_path_buf(),
        source,
    })?;
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if let Ok(files) = std::fs::read_dir(entry.path()) {
            for file in files.flatten() {
                if let Some(ext) = Path::new(&file.file_name()).extension()
                    && let Some(format) = ext.to_str().and_then(Format::from_extension)
                {
                    return Ok(format);
                }
            }
        }
    }
    Err(OpenError::UnknownFormat {
        root: root.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quent_build_info::ModelInfo;

    fn artifact_with(analyzer_package: Option<&str>, commit: &str) -> ArtifactInfo {
        let mut model = ModelInfo::unknown();
        model.name = "Simulator".into();
        model.analyzer_package = analyzer_package.map(str::to_string);
        model.source = BuildInfo {
            remote: Some("https://example.com/sirius".into()),
            commit: Some(commit.into()),
            ..BuildInfo::unknown()
        };
        let mut info = ArtifactInfo::new(model);
        info.quent = BuildInfo {
            remote: Some("https://example.com/quent".into()),
            commit: Some("0123456789abcdef".into()),
            ..BuildInfo::unknown()
        };
        info
    }

    fn ctx_with_stream(name: &str, file: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let entity = dir.path().join(name);
        std::fs::create_dir_all(&entity).unwrap();
        std::fs::write(entity.join(file), b"").unwrap();
        dir
    }

    fn make_context(dir: &Path) {
        std::fs::create_dir_all(dir.join("engine")).unwrap();
        std::fs::write(dir.join("engine").join("events.ndjson"), b"").unwrap();
        std::fs::write(dir.join(SIDECAR_FILE_NAME), b"{}").unwrap();
    }

    #[test]
    fn discover_finds_nested_contexts_and_skips_hidden() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_context(&root.join("a"));
        make_context(&root.join("nested/b"));
        make_context(&root.join(".hidden/c")); // under a dotdir: must be skipped

        let found = discover_contexts(&[root.to_path_buf()]).unwrap();
        let mut names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);

        // Passing a context directly yields just it (no descent into entity dirs).
        let direct = discover_contexts(&[root.join("a")]).unwrap();
        assert_eq!(direct.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_does_not_follow_symlink_cycles() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_context(&root.join("a"));
        // A symlink back to the root would loop a naive recursive walk.
        std::os::unix::fs::symlink(root, root.join("loop")).unwrap();

        let found = discover_contexts(&[root.to_path_buf()]).unwrap(); // must terminate
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn detects_format_from_entity_subdir() {
        let ctx = ctx_with_stream("engine", "events.msgpack");
        assert_eq!(detect_format(ctx.path()).unwrap(), Format::Msgpack);
    }

    #[test]
    fn unknown_format_when_no_streams() {
        let ctx = ctx_with_stream("engine", "notes.txt");
        assert!(matches!(
            detect_format(ctx.path()),
            Err(OpenError::UnknownFormat { .. })
        ));
    }

    #[test]
    fn validators_accept_good_and_reject_injection() {
        assert!(validate_commit("0123456789abcdef0123456789abcdef01234567").is_ok());
        assert!(validate_commit("deadbeef").is_ok());
        assert!(validate_commit("nothex!!").is_err());
        assert!(validate_commit("abc").is_err()); // too short

        assert!(validate_remote("https://github.com/rapidsai/quent").is_ok());
        assert!(validate_remote("git@github.com:rapidsai/quent.git").is_ok());
        assert!(validate_remote("github.com:rapidsai/quent.git").is_ok()); // scp, no user
        assert!(validate_remote("ssh://git@github.com/rapidsai/quent.git").is_ok());
        assert!(validate_remote("https://x/y\"\n[dependencies]\nevil=\"1").is_err());
        assert!(validate_remote("file:///etc/passwd").is_err());
        // Unauthenticated transports are rejected (no silent downgrade).
        assert!(validate_remote("http://github.com/rapidsai/quent").is_err());
        assert!(validate_remote("git://github.com/rapidsai/quent").is_err());

        assert!(validate_package("quent-simulator-analyzer").is_ok());
        assert!(validate_package("evil\"]\nfoo = { path = \"/").is_err());
        assert!(validate_package("").is_err());
    }

    #[test]
    fn spec_requires_analyzer_package() {
        let ctx = ctx_with_stream("engine", "events.ndjson");
        let info = artifact_with(None, "abc");
        assert!(matches!(
            ViewerSpec::from_artifact(ctx.path(), &info),
            Err(OpenError::NoAnalyzer { .. })
        ));
    }

    #[test]
    fn cargo_url_normalizes_scp_style_but_leaves_real_urls() {
        let scp = GitPin {
            remote: "git@github.com:mbrobbel/quent.git".into(),
            commit: "c".into(),
        };
        assert_eq!(scp.cargo_url(), "ssh://git@github.com/mbrobbel/quent.git");
        let https = GitPin {
            remote: "https://github.com/rapidsai/quent".into(),
            commit: "c".into(),
        };
        assert_eq!(https.cargo_url(), "https://github.com/rapidsai/quent");
        // A local path with a colon after a slash is not scp-style: leave it.
        let local = GitPin {
            remote: "/tmp/foo:bar.git".into(),
            commit: "c".into(),
        };
        assert_eq!(local.cargo_url(), "/tmp/foo:bar.git");
    }

    #[test]
    fn spec_derives_crate_ident_and_keys() {
        let ctx = ctx_with_stream("engine", "events.ndjson");
        let info = artifact_with(Some("quent-simulator-analyzer"), "feedface99887766");
        let spec = ViewerSpec::from_artifact(ctx.path(), &info).unwrap();
        assert_eq!(spec.analyzer_crate(), "quent_simulator_analyzer");
        assert_eq!(spec.format, Format::Ndjson);
        assert!(
            spec.cache_key()
                .starts_with("quent-simulator-analyzer-feedface9988-ndjson-")
        );
    }

    #[test]
    fn keys_distinguish_full_pins_not_just_short_commit() {
        let ctx = ctx_with_stream("engine", "events.ndjson");
        // Same package, format, and 12-char commit prefix, but different full
        // analyzer commits — must NOT collide.
        let a =
            ViewerSpec::from_artifact(ctx.path(), &artifact_with(Some("p"), "abcabcabcabc1111"))
                .unwrap();
        let b =
            ViewerSpec::from_artifact(ctx.path(), &artifact_with(Some("p"), "abcabcabcabc2222"))
                .unwrap();
        assert_ne!(a.group_key(), b.group_key());
        assert_ne!(a.cache_key(), b.cache_key());
        // Identical inputs group together and are deterministic.
        let a2 =
            ViewerSpec::from_artifact(ctx.path(), &artifact_with(Some("p"), "abcabcabcabc1111"))
                .unwrap();
        assert_eq!(a.group_key(), a2.group_key());
        assert_eq!(a.cache_key(), a2.cache_key());
    }
}
