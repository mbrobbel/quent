// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Lexical path helpers used to emit relative dependency paths into generated
//! `Cargo.toml` files. These operate purely on path components and never touch
//! the filesystem (no symlink resolution), so results are deterministic and
//! testable.

use std::path::{Component, Path, PathBuf};

/// Make `p` absolute by prepending the current directory if it is relative.
/// Does not collapse `.`/`..` (use [`normalize`] for that).
pub fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    }
}

/// Lexically collapse `.` and `..` components. Intended for absolute paths;
/// `..` at the root is dropped rather than escaping above it.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop a real (Normal) segment; keep root/prefix intact.
                let popped =
                    matches!(out.components().next_back(), Some(Component::Normal(_))) && out.pop();
                if !popped {
                    // Preserve leading `..` for relative inputs.
                    if !out.has_root() {
                        out.push("..");
                    }
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Compute a relative path from `from` (a directory) to `to`. Both should be
/// absolute and normalized. Returns `.` when they are equal.
pub fn relative(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();

    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();

    let mut result = PathBuf::new();
    for _ in common..from.len() {
        result.push("..");
    }
    for comp in &to[common..] {
        result.push(comp.as_os_str());
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}

/// Render a path as a forward-slash string suitable for a `Cargo.toml` value
/// (Cargo accepts `/` separators on every platform).
pub fn to_toml_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_parent_dirs() {
        assert_eq!(
            normalize(Path::new("/a/b/c/../../d")),
            PathBuf::from("/a/d")
        );
        assert_eq!(normalize(Path::new("/a/./b")), PathBuf::from("/a/b"));
    }

    #[test]
    fn relative_walks_up_then_down() {
        assert_eq!(
            relative(Path::new("/r/a/b/c"), Path::new("/r/x/y")),
            PathBuf::from("../../../x/y")
        );
    }

    #[test]
    fn relative_equal_is_dot() {
        assert_eq!(
            relative(Path::new("/r/a"), Path::new("/r/a")),
            PathBuf::from(".")
        );
    }

    #[test]
    fn relative_matches_deep_scaffold_layout() {
        // scaffold at <root>/domains/query_engine/dist/.quent-scaffold/instrumentation
        // depending on <root>/crates/model.
        let from = Path::new("/root/domains/query_engine/dist/.quent-scaffold/instrumentation");
        let to = Path::new("/root/crates/model");
        assert_eq!(
            relative(from, to),
            PathBuf::from("../../../../../crates/model")
        );
    }
}
