// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! `quent.toml` manifest schema and dependency-source rendering.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::paths::{absolute, normalize, relative, to_toml_path};

/// A parsed `quent.toml`.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    /// How to obtain the model and its instrumentation.
    pub model: Model,
    /// Dependency source for quent crates injected into generated crates.
    pub quent: QuentDep,
    /// Python target configuration (present to build the `python` target).
    pub python: Option<PythonTarget>,
}

/// The `[model]` table.
#[derive(Debug, Deserialize)]
pub struct Model {
    /// Model front-end kind. Only `rust-crate` is implemented.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Cargo package name of the model crate.
    pub package: String,
    /// Model name passed to `<type>::build("<name>")`.
    pub name: String,
    /// Model type; defaults to `<name>Model`.
    #[serde(rename = "type")]
    pub type_: Option<String>,
    /// Model crate Rust library name (its `[lib].name`). Defaults to `package`
    /// with `-` mapped to `_`; set explicitly when the crate renames its lib.
    pub rust_name: Option<String>,
    /// Model crate source: filesystem path (relative to the manifest).
    pub path: Option<String>,
    /// Model crate source: git URL.
    pub git: Option<String>,
    /// Git revision.
    pub rev: Option<String>,
    /// Git tag.
    pub tag: Option<String>,
    /// Git branch.
    pub branch: Option<String>,
    /// Instrumentation crate configuration.
    #[serde(default)]
    pub instrumentation: Instrumentation,
}

/// The `[model.instrumentation]` table.
#[derive(Debug, Deserialize)]
pub struct Instrumentation {
    /// Generate a wrapper crate (`pub use <model>::*; instrumentation!(<name>)`).
    #[serde(default = "default_true")]
    pub generate: bool,
    /// Existing instrumentation crate package name (when `generate = false`).
    pub package: Option<String>,
    /// Existing instrumentation crate lib name (when `generate = false`).
    pub rust_name: Option<String>,
}

impl Default for Instrumentation {
    fn default() -> Self {
        Self {
            generate: true,
            package: None,
            rust_name: None,
        }
    }
}

/// The `[quent]` dependency-source table injected into generated crates.
#[derive(Debug, Deserialize)]
pub struct QuentDep {
    /// Path to a quent checkout (relative to the manifest).
    pub path: Option<String>,
    /// Git URL of the quent repository.
    pub git: Option<String>,
    /// Git revision.
    pub rev: Option<String>,
    /// Git tag.
    pub tag: Option<String>,
    /// Git branch.
    pub branch: Option<String>,
}

/// The `[python]` target table.
#[derive(Debug, Deserialize)]
pub struct PythonTarget {
    /// Python extension module name (also the cdylib `[lib].name`).
    pub module_name: String,
    /// Distribution/wheel name; defaults to `module_name` with `_` mapped to `-`.
    pub package: Option<String>,
    /// Wheel version.
    #[serde(default = "default_version")]
    pub version: String,
}

/// A dependency source (path or git), extracted from a manifest table.
#[derive(Debug, Clone)]
pub struct Source {
    /// Filesystem path relative to the manifest directory.
    pub path: Option<String>,
    /// Git URL.
    pub git: Option<String>,
    /// Git revision.
    pub rev: Option<String>,
    /// Git tag.
    pub tag: Option<String>,
    /// Git branch.
    pub branch: Option<String>,
}

impl Model {
    /// The model crate dependency source.
    pub fn source(&self) -> Source {
        Source {
            path: self.path.clone(),
            git: self.git.clone(),
            rev: self.rev.clone(),
            tag: self.tag.clone(),
            branch: self.branch.clone(),
        }
    }

    /// The model type (`<name>Model` unless overridden).
    pub fn model_type(&self) -> String {
        self.type_
            .clone()
            .unwrap_or_else(|| format!("{}Model", self.name))
    }

    /// The model crate's Rust library name (`rust_name`, else `package` with
    /// `-` mapped to `_`).
    pub fn model_lib(&self) -> String {
        self.rust_name
            .clone()
            .unwrap_or_else(|| self.package.replace('-', "_"))
    }
}

impl QuentDep {
    /// The quent dependency source.
    pub fn source(&self) -> Source {
        Source {
            path: self.path.clone(),
            git: self.git.clone(),
            rev: self.rev.clone(),
            tag: self.tag.clone(),
            branch: self.branch.clone(),
        }
    }
}

impl PythonTarget {
    /// The distribution/wheel name (`package`, else `module_name` with `_` → `-`).
    pub fn dist_name(&self) -> String {
        self.package
            .clone()
            .unwrap_or_else(|| self.module_name.replace('_', "-"))
    }
}

fn default_kind() -> String {
    "rust-crate".to_string()
}
fn default_true() -> bool {
    true
}
fn default_version() -> String {
    "0.1.0".to_string()
}

/// Rust keywords (strict + reserved, incl. edition 2024) that are rejected as
/// generated identifiers because they produce invalid Rust (e.g. `fn type`).
const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
    "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
    "use", "where", "while", "async", "await", "abstract", "become", "box", "do", "final", "gen",
    "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
];

/// Whether `s` is usable as a generated Rust/Python identifier: a leading letter
/// or `_`, then letters/digits/`_`, and not the bare `_` or a Rust keyword.
fn is_valid_ident(s: &str) -> bool {
    if s == "_" || RUST_KEYWORDS.contains(&s) {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn validate_ident(field: &str, value: &str) -> Result<()> {
    if is_valid_ident(value) {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "{field} `{value}` must be a valid identifier (letters, digits, `_`; no dots, not a Rust keyword)"
        )))
    }
}

/// Render `s` as an escaped TOML basic string (with surrounding quotes) so
/// values containing `"`, `\`, or control characters produce valid TOML.
fn toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether `s` is a valid PEP 508 project name: starts and ends with an
/// alphanumeric, body of ASCII alphanumerics plus `.`, `-`, `_`.
fn is_valid_dist_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    match (bytes.first(), bytes.last()) {
        (Some(f), Some(l)) if f.is_ascii_alphanumeric() && l.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Load and parse a manifest, returning it plus the manifest's (absolute,
/// normalized) directory.
pub fn load(path: &Path) -> Result<(Manifest, PathBuf)> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::ReadManifest {
        path: path.to_path_buf(),
        source,
    })?;
    let manifest: Manifest = toml::from_str(&text).map_err(|source| Error::ParseManifest {
        path: path.to_path_buf(),
        source,
    })?;
    if manifest.model.kind != "rust-crate" {
        return Err(Error::Config(format!(
            "unsupported [model] kind `{}` (only `rust-crate` is implemented)",
            manifest.model.kind
        )));
    }
    validate_ident("[model] name", &manifest.model.name)?;
    validate_ident("[model] rust_name", &manifest.model.model_lib())?;
    if let Some(python) = &manifest.python {
        // A dotted (submodule) module name would produce an invalid Rust
        // `[lib].name` and mismatched stub paths; reject it for now.
        validate_ident("[python] module_name", &python.module_name)?;
        let dist = python.dist_name();
        if !is_valid_dist_name(&dist) {
            return Err(Error::Config(format!(
                "[python] distribution name `{dist}` is not a valid PEP 508 name; set [python] package explicitly"
            )));
        }
    }
    let dir = normalize(&absolute(path))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| Error::Config("manifest path has no parent directory".into()))?;
    Ok((manifest, dir))
}

/// Render a `Cargo.toml` dependency source (the part inside `{ ... }`).
///
/// - Path sources resolve `source.path` (relative to `manifest_dir`) joined with
///   the optional crate `subpath`, then emit a path relative to `from_dir`.
/// - Git sources emit `git = ".."` plus `rev`/`tag`/`branch` and ignore `subpath`
///   (cargo resolves the package by name within the git repository's workspace).
pub fn render_dep(
    source: &Source,
    subpath: Option<&str>,
    from_dir: &Path,
    manifest_dir: &Path,
) -> Result<String> {
    if let Some(p) = &source.path {
        let mut target = manifest_dir.join(p);
        if let Some(s) = subpath {
            target = target.join(s);
        }
        let target = normalize(&target);
        let rel = relative(from_dir, &target);
        Ok(format!("path = {}", toml_basic_string(&to_toml_path(&rel))))
    } else if let Some(g) = &source.git {
        let mut out = format!("git = {}", toml_basic_string(g));
        if let Some(rev) = &source.rev {
            out.push_str(&format!(", rev = {}", toml_basic_string(rev)));
        } else if let Some(tag) = &source.tag {
            out.push_str(&format!(", tag = {}", toml_basic_string(tag)));
        } else if let Some(branch) = &source.branch {
            out.push_str(&format!(", branch = {}", toml_basic_string(branch)));
        }
        Ok(out)
    } else {
        Err(Error::Config(
            "dependency source must set `path` or `git`".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_path_dep_is_relative() {
        let manifest_dir = Path::new("/root/domains/query_engine");
        let from = Path::new("/root/domains/query_engine/dist/.quent-scaffold/instrumentation");
        let src = Source {
            path: Some("../..".into()),
            git: None,
            rev: None,
            tag: None,
            branch: None,
        };
        let dep = render_dep(&src, Some("crates/model"), from, manifest_dir).unwrap();
        assert_eq!(dep, "path = \"../../../../../crates/model\"");
    }

    #[test]
    fn model_lib_uses_rust_name_override() {
        let manifest: Manifest = toml::from_str(
            r#"
            [model]
            package = "acme-model"
            path = "."
            name = "App"
            rust_name = "model_api"
            [quent]
            path = ".."
            [python]
            module_name = "acme"
        "#,
        )
        .unwrap();
        assert_eq!(manifest.model.model_lib(), "model_api");
    }

    fn write_manifest(module_name: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("quent.toml"),
            format!(
                r#"
                [model]
                package = "acme-model"
                path = "."
                name = "App"
                [quent]
                git = "https://github.com/rapidsai/quent"
                rev = "abc"
                [python]
                module_name = "{module_name}"
            "#
            ),
        )
        .unwrap();
        tmp
    }

    #[test]
    fn rejects_keyword_module_name() {
        let tmp = write_manifest("type");
        let err = load(&tmp.path().join("quent.toml"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("module_name"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_invalid_dist_name() {
        // `_native` is a valid identifier but yields dist name `-native`.
        let tmp = write_manifest("_native");
        let err = load(&tmp.path().join("quent.toml"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("PEP 508"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_dotted_module_name() {
        let tmp = write_manifest("acme._native");
        let err = load(&tmp.path().join("quent.toml"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("module_name"), "unexpected error: {err}");
    }

    #[test]
    fn render_dep_escapes_toml_specials() {
        let src = Source {
            path: None,
            git: Some("https://ex.com/\"weird\"".into()),
            rev: Some("a\\b".into()),
            tag: None,
            branch: None,
        };
        let dep = render_dep(&src, None, Path::new("/x"), Path::new("/y")).unwrap();
        assert_eq!(
            dep,
            "git = \"https://ex.com/\\\"weird\\\"\", rev = \"a\\\\b\""
        );
    }

    #[test]
    fn render_git_dep_ignores_subpath() {
        let src = Source {
            path: None,
            git: Some("https://github.com/rapidsai/quent".into()),
            rev: Some("abc123".into()),
            tag: None,
            branch: None,
        };
        let dep = render_dep(&src, Some("crates/model"), Path::new("/x"), Path::new("/y")).unwrap();
        assert_eq!(
            dep,
            "git = \"https://github.com/rapidsai/quent\", rev = \"abc123\""
        );
    }
}
