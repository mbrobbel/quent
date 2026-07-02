// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Name derivations for generated crates and modules.

/// Convert a model name (e.g. `QueryEngine`) to `snake_case` (`query_engine`).
/// Existing separators are preserved; only camelCase boundaries insert `_`.
pub fn to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// A valid Rust `lib.name` / crate identifier: lowercase with `-` mapped to `_`.
pub fn to_ident(pkg: &str) -> String {
    pkg.replace('-', "_")
}

/// A kebab-case cargo package name: `_` mapped to `-`.
pub fn to_kebab(s: &str) -> String {
    s.replace('_', "-")
}

/// A cargo-package-name stem derived from a model name: `snake_case` → kebab
/// with leading/trailing `-` trimmed (Cargo rejects names starting or ending
/// with `-`, which a leading/trailing `_` in the model name would produce).
/// Falls back to `model` if nothing usable remains.
pub fn to_pkg_stem(name: &str) -> String {
    let kebab = to_kebab(&to_snake(name));
    let trimmed = kebab.trim_matches('-');
    if trimmed.is_empty() {
        "model".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_from_camel() {
        assert_eq!(to_snake("QueryEngine"), "query_engine");
        assert_eq!(to_snake("App"), "app");
    }

    #[test]
    fn snake_preserves_existing() {
        assert_eq!(to_snake("query_engine"), "query_engine");
        assert_eq!(to_snake("query-engine"), "query-engine");
    }

    #[test]
    fn ident_and_kebab() {
        assert_eq!(
            to_ident("quent-query-engine-model"),
            "quent_query_engine_model"
        );
        assert_eq!(to_kebab("query_engine"), "query-engine");
    }

    #[test]
    fn pkg_stem_trims_leading_underscore() {
        assert_eq!(to_pkg_stem("QueryEngine"), "query-engine");
        assert_eq!(to_pkg_stem("_App"), "app");
        assert_eq!(to_pkg_stem("_"), "model");
    }
}
