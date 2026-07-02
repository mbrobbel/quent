// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Command-line frontend for `quent-codegen`.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use quent_codegen_cli::build::{self, Target};
use quent_codegen_cli::config::{self, Manifest, Source};
use quent_codegen_cli::error::{Error, Result};

#[derive(Debug, Parser)]
#[command(
    name = "quent-codegen",
    about = "Scaffold and build codegen artifacts (Python/C++/Rust) from a quent model",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate bridge and instrumentation crates + packaging files.
    Scaffold {
        #[command(flatten)]
        common: CommonArgs,
    },
    /// Scaffold, then build and package artifacts into the output directory.
    Build {
        #[command(flatten)]
        common: CommonArgs,
        /// Artifact output directory.
        #[arg(long, default_value = "dist")]
        out: PathBuf,
        /// Build without the `--release` profile.
        #[arg(long)]
        debug: bool,
    },
    /// Remove generated scaffold and artifact directories.
    Clean {
        #[command(flatten)]
        common: CommonArgs,
        /// Artifact output directory to clean.
        #[arg(long, default_value = "dist")]
        out: PathBuf,
    },
}

#[derive(Debug, Args)]
struct CommonArgs {
    /// Path to the quent.toml manifest.
    #[arg(long, default_value = "quent.toml")]
    manifest: PathBuf,
    /// Comma-separated targets (default: all present in the manifest).
    #[arg(long)]
    targets: Option<String>,
    /// Directory for generated crates (default: `<out>/.quent-scaffold`).
    #[arg(long)]
    scaffold_dir: Option<PathBuf>,
    /// Override the manifest's `[quent]` dependency with this git URL, so the
    /// same ref drives both the CLI and the quent code compiled into artifacts.
    #[arg(long)]
    quent_git: Option<String>,
    /// Git revision for `--quent-git`.
    #[arg(long)]
    quent_rev: Option<String>,
}

impl CommonArgs {
    /// The quent dependency source for generated crates: the `--quent-git`
    /// override when set, otherwise the manifest's `[quent]` table.
    fn quent_source(&self, manifest: &Manifest) -> Source {
        match &self.quent_git {
            Some(git) => Source {
                path: None,
                git: Some(git.clone()),
                rev: self.quent_rev.clone(),
                tag: None,
                branch: None,
            },
            None => manifest.quent.source(),
        }
    }
}

fn resolve_targets(spec: &Option<String>, manifest: &Manifest) -> Result<Vec<Target>> {
    match spec {
        Some(s) => build::parse_targets(s),
        None => {
            let targets = build::default_targets(manifest);
            if targets.is_empty() {
                Err(Error::Config(
                    "manifest declares no targets; add a [python] table or pass --targets".into(),
                ))
            } else {
                Ok(targets)
            }
        }
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Scaffold { common } => {
            let (manifest, dir) = config::load(&common.manifest)?;
            let quent = common.quent_source(&manifest);
            let targets = resolve_targets(&common.targets, &manifest)?;
            let scaffold_dir = common
                .scaffold_dir
                .unwrap_or_else(|| PathBuf::from("dist/.quent-scaffold"));
            build::scaffold_only(&manifest, &dir, &quent, &targets, &scaffold_dir)
        }
        Command::Build { common, out, debug } => {
            let (manifest, dir) = config::load(&common.manifest)?;
            let quent = common.quent_source(&manifest);
            let targets = resolve_targets(&common.targets, &manifest)?;
            let scaffold_dir = common
                .scaffold_dir
                .unwrap_or_else(|| out.join(".quent-scaffold"));
            build::build(
                &manifest,
                &dir,
                &quent,
                &targets,
                &out,
                &scaffold_dir,
                !debug,
            )
        }
        Command::Clean { common, out } => {
            let scaffold_dir = common
                .scaffold_dir
                .unwrap_or_else(|| out.join(".quent-scaffold"));
            build::clean(&out, &scaffold_dir)
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
