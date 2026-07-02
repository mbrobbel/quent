// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Generate scaffold crates (instrumentation wrapper + per-target bridge) from a
//! [`Manifest`]. Generated crates are detached (own `[workspace]`) and reference
//! quent/model crates via relative path or git deps computed by [`config`].

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{Manifest, Source, render_dep};
use crate::error::{Error, Result};
use crate::names::to_pkg_stem;
use crate::paths::{absolute, normalize};
use crate::templates::{INSTR_CARGO, INSTR_LIB, PY_BUILD, PY_CARGO, PY_LIB, PYPROJECT, fill};

/// Write `content` to `path`, creating parent directories.
fn write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, content).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Names derived from the model, shared across targets.
struct Names {
    instr_pkg: String,
    instr_lib: String,
    model_lib: String,
    model_type: String,
}

impl Names {
    fn from(manifest: &Manifest) -> Self {
        let instr_pkg = format!("{}-instrumentation", to_pkg_stem(&manifest.model.name));
        // Cargo derives the lib name from the package name (`-` → `_`); keep
        // instr_lib in lockstep so generated `build.rs` references resolve.
        let instr_lib = instr_pkg.replace('-', "_");
        Self {
            instr_pkg,
            instr_lib,
            model_lib: manifest.model.model_lib(),
            model_type: manifest.model.model_type(),
        }
    }
}

/// Generate the instrumentation wrapper crate at `dir`.
fn scaffold_instrumentation(
    manifest: &Manifest,
    manifest_dir: &Path,
    quent: &Source,
    dir: &Path,
    names: &Names,
) -> Result<()> {
    if !manifest.model.instrumentation.generate {
        return Err(Error::Config(
            "this phase requires [model.instrumentation] generate = true".into(),
        ));
    }
    let dir_abs = normalize(&absolute(dir));
    let quent_model_dep = render_dep(quent, Some("crates/model"), &dir_abs, manifest_dir)?;
    let model_dep = render_dep(&manifest.model.source(), None, &dir_abs, manifest_dir)?;

    write(
        &dir.join("Cargo.toml"),
        &fill(
            INSTR_CARGO,
            &[
                ("__INSTR_PKG__", &names.instr_pkg),
                ("__QUENT_MODEL_DEP__", &quent_model_dep),
                ("__MODEL_PKG__", &manifest.model.package),
                ("__MODEL_DEP__", &model_dep),
            ],
        ),
    )?;
    write(
        &dir.join("src/lib.rs"),
        &fill(
            INSTR_LIB,
            &[
                ("__MODEL_LIB__", &names.model_lib),
                ("__MODEL_NAME__", &manifest.model.name),
            ],
        ),
    )?;
    Ok(())
}

/// Generate the Python bridge crate (+ pyproject) and instrumentation wrapper.
/// Returns the maturin project directory (`<scaffold_dir>/python`).
pub fn scaffold_python(
    manifest: &Manifest,
    manifest_dir: &Path,
    quent: &Source,
    scaffold_dir: &Path,
) -> Result<PathBuf> {
    let python = manifest
        .python
        .as_ref()
        .ok_or_else(|| Error::Config("no [python] target in manifest".into()))?;
    let names = Names::from(manifest);

    let instr_dir = scaffold_dir.join("instrumentation");
    let py_dir = scaffold_dir.join("python");
    scaffold_instrumentation(manifest, manifest_dir, quent, &instr_dir, &names)?;

    let py_dir_abs = normalize(&absolute(&py_dir));
    let quent_codegen_dep = render_dep(quent, Some("crates/codegen"), &py_dir_abs, manifest_dir)?;

    let py_pkg = format!("{}-python", to_pkg_stem(&manifest.model.name));
    let dist = python.dist_name();

    write(
        &py_dir.join("Cargo.toml"),
        &fill(
            PY_CARGO,
            &[
                ("__PY_PKG__", &py_pkg),
                ("__MODULE__", &python.module_name),
                ("__INSTR_PKG__", &names.instr_pkg),
                ("__QUENT_CODEGEN_DEP__", &quent_codegen_dep),
            ],
        ),
    )?;
    write(
        &py_dir.join("build.rs"),
        &fill(
            PY_BUILD,
            &[
                ("__INSTR_LIB__", &names.instr_lib),
                ("__MODEL_TYPE__", &names.model_type),
                ("__MODEL_NAME__", &manifest.model.name),
                ("__MODULE__", &python.module_name),
            ],
        ),
    )?;
    write(&py_dir.join("src/lib.rs"), PY_LIB)?;
    write(
        &py_dir.join("pyproject.toml"),
        &fill(
            PYPROJECT,
            &[
                ("__DIST__", &dist),
                ("__VERSION__", &python.version),
                ("__MODULE__", &python.module_name),
            ],
        ),
    )?;
    Ok(py_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qe_manifest() -> Manifest {
        let toml = r#"
            [model]
            package = "quent-query-engine-model"
            path = "model"
            name = "QueryEngine"
            [quent]
            path = "../.."
            [python]
            module_name = "quent_qe"
        "#;
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn scaffolds_expected_python_layout() {
        let tmp = tempfile::tempdir().unwrap();
        // Emulate a manifest at <tmp>/domains/query_engine so the relative deps
        // resolve to <tmp>/crates/... and <tmp>/domains/query_engine/model.
        let manifest_dir = normalize(&tmp.path().join("domains/query_engine"));
        let scaffold = manifest_dir.join("dist/.quent-scaffold");

        let manifest = qe_manifest();
        let quent = manifest.quent.source();
        let py_dir = scaffold_python(&manifest, &manifest_dir, &quent, &scaffold).unwrap();
        assert_eq!(py_dir, scaffold.join("python"));

        let instr_cargo = fs::read_to_string(scaffold.join("instrumentation/Cargo.toml")).unwrap();
        assert!(
            instr_cargo.contains("[workspace]\n"),
            "must detach workspace"
        );
        assert!(instr_cargo.contains("quent-model = { path = \"../../../../../crates/model\" }"));
        assert!(instr_cargo.contains("quent-query-engine-model = { path = \"../../../model\" }"));

        let instr_lib = fs::read_to_string(scaffold.join("instrumentation/src/lib.rs")).unwrap();
        assert!(instr_lib.contains("pub use quent_query_engine_model::*;"));
        assert!(instr_lib.contains("quent_model::instrumentation!(QueryEngine);"));

        let py_cargo = fs::read_to_string(py_dir.join("Cargo.toml")).unwrap();
        assert!(py_cargo.contains("[workspace]\n"));
        assert!(py_cargo.contains("name = \"quent_qe\""));
        assert!(py_cargo.contains("crate-type = [\"cdylib\"]"));
        assert!(
            py_cargo.contains("query-engine-instrumentation = { path = \"../instrumentation\" }")
        );
        assert!(py_cargo.contains("quent-codegen = { path = \"../../../../../crates/codegen\" }"));

        let build = fs::read_to_string(py_dir.join("build.rs")).unwrap();
        assert!(
            build
                .contains("query_engine_instrumentation::QueryEngineModel::build(\"QueryEngine\")")
        );
        assert!(build.contains("module_name: \"quent_qe\""));

        let pyproject = fs::read_to_string(py_dir.join("pyproject.toml")).unwrap();
        assert!(pyproject.contains("name = \"quent-qe\""));
        assert!(pyproject.contains("module-name = \"quent_qe\""));
        assert!(pyproject.contains("quent_qe/__init__.pyi"));
        assert!(pyproject.contains("quent_qe/py.typed"));
    }

    #[test]
    fn git_quent_source_emits_git_dep() {
        let toml = r#"
            [model]
            package = "acme-model"
            path = "."
            name = "App"
            [quent]
            git = "https://github.com/rapidsai/quent"
            rev = "deadbeef"
            [python]
            module_name = "acme"
        "#;
        let manifest: Manifest = toml::from_str(toml).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let manifest_dir = normalize(tmp.path());
        let scaffold = manifest_dir.join("dist/.quent-scaffold");
        scaffold_python(
            &manifest,
            &manifest_dir,
            &manifest.quent.source(),
            &scaffold,
        )
        .unwrap();

        let instr = fs::read_to_string(scaffold.join("instrumentation/Cargo.toml")).unwrap();
        assert!(instr.contains(
            "quent-model = { git = \"https://github.com/rapidsai/quent\", rev = \"deadbeef\" }"
        ));
        let py = fs::read_to_string(scaffold.join("python/Cargo.toml")).unwrap();
        assert!(py.contains(
            "quent-codegen = { git = \"https://github.com/rapidsai/quent\", rev = \"deadbeef\" }"
        ));
    }

    #[test]
    fn quent_override_replaces_manifest_source() {
        // Manifest uses a path `[quent]`, but the override (as the workflow
        // passes for external callers) must win so one ref drives everything.
        let manifest = qe_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let manifest_dir = normalize(tmp.path());
        let scaffold = manifest_dir.join("dist/.quent-scaffold");
        let override_src = Source {
            path: None,
            git: Some("https://github.com/rapidsai/quent".into()),
            rev: Some("cafef00d".into()),
            tag: None,
            branch: None,
        };
        scaffold_python(&manifest, &manifest_dir, &override_src, &scaffold).unwrap();

        let instr = fs::read_to_string(scaffold.join("instrumentation/Cargo.toml")).unwrap();
        assert!(instr.contains(
            "quent-model = { git = \"https://github.com/rapidsai/quent\", rev = \"cafef00d\" }"
        ));
        // The model dep still comes from the manifest (path), not the override.
        assert!(instr.contains("quent-query-engine-model = { path ="));
    }
}
