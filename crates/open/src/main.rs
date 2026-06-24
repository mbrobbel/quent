// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! `quent-open` opens local Quent benchmark artifacts in an application-specific
//! viewer. See <https://github.com/rapidsai/quent/issues/234>.
//!
//! Given a context directory, it reads the `model.qmi` provenance sidecar,
//! generates a small viewer crate pinned to the recorded quent + analyzer commits
//! (see [`wrapper`]), builds and serves it, and opens a browser.
//!
//! Building the viewer fetches the recorded git sources and compiles the embedded
//! UI, which runs `pnpm`/`node` on first build (cached afterwards); these must be
//! available on `PATH`.

mod error;
mod spec;
mod viewer;
mod wrapper;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use quent_build_info::{ArtifactInfo, SIDECAR_FILE_NAME};

use crate::error::{OpenError, Result};
use crate::spec::ViewerSpec;

#[derive(Debug, Parser)]
#[command(name = "quent-open")]
#[command(about = "Open local Quent benchmark artifacts in an application-specific viewer")]
struct Cli {
    /// Config file path. Defaults to ./quent-open.toml, then ~/.config/quent/open.toml.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Do not open a browser.
    #[arg(long, global = true)]
    no_browser: bool,

    /// Print the opened viewer URL.
    #[arg(long, global = true)]
    print_url: bool,

    /// Force a specific viewer by name from the config (skips automatic matching).
    #[arg(long, global = true)]
    viewer: Option<String>,

    #[command(subcommand)]
    command: OpenCommand,
}

#[derive(Debug, Subcommand)]
enum OpenCommand {
    /// Analyze local Quent artifacts directly.
    Local {
        /// Context directories to analyze. A context directory holds a `model.qmi`
        /// provenance sidecar at its root, plus one per-entity subdirectory per
        /// entity containing that entity's event stream.
        #[arg(required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        OpenCommand::Local { paths } => run_local(&cli, paths).await,
    }
}

/// Open local artifacts in a viewer.
///
/// Reads each context directory's `model.qmi` sidecar, then resolves the viewer
/// build spec (analyzer package, pinned git sources, artifact format). Each path
/// is treated as a context directory; resolving a sidecar from a nested per-entity
/// subdirectory is not supported.
///
/// For each context directory: generate a viewer crate from the spec, build it,
/// serve the artifacts, and open a browser. Serving blocks until the viewer
/// exits, so multiple paths are opened one after another.
async fn run_local(cli: &Cli, paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        let info = read_artifact_info(path)?;
        report_artifact(path, &info);
        let spec = ViewerSpec::from_artifact(path, &info)?;
        report_spec(&spec);
        viewer::open(&spec, cli.no_browser, cli.print_url).await?;
    }
    Ok(())
}

/// Print the resolved viewer build spec for `spec`.
fn report_spec(spec: &ViewerSpec) {
    println!(
        "  viewer:   {}::Viewer ({})",
        spec.analyzer_crate(),
        spec.format.extension()
    );
}

/// Read the [`ArtifactInfo`] sidecar from the context directory `dir`.
fn read_artifact_info(dir: &Path) -> Result<ArtifactInfo> {
    ArtifactInfo::read_sidecar(dir).map_err(|source| OpenError::Sidecar {
        path: dir.join(SIDECAR_FILE_NAME),
        source,
    })
}

/// Print the provenance discovered for `path`. The model `source` is what later
/// drives checking out and building a viewer for the producing crate.
fn report_artifact(path: &Path, info: &ArtifactInfo) {
    let model = &info.model;
    println!("{}", path.display());
    println!("  model:    {} ({})", model.name, model.type_path);
    println!("  package:  {}", model.package);
    if let Some(analyzer) = &model.analyzer_package {
        println!("  analyzer: {analyzer}");
    }
    println!("  quent:    {}", describe_build(&info.quent));
    println!("  source:   {}", describe_build(&model.source));
}

/// One-line summary of a [`BuildInfo`](quent_build_info::BuildInfo): version with
/// the commit and remote when present.
fn describe_build(build: &quent_build_info::BuildInfo) -> String {
    let mut out = build.version.clone();
    if let Some(commit) = &build.commit {
        out.push_str(&format!(" ({commit})"));
    }
    if let Some(remote) = &build.remote {
        out.push_str(&format!(" from {remote}"));
    }
    out
}
