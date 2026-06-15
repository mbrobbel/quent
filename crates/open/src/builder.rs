// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

use crate::{
    config::{RustEntrypoint, ViewerConfig, relative_source_path},
    error::{OpenError, Result},
};

pub struct BuiltViewer {
    pub binary: PathBuf,
    pub ui_dist: Option<PathBuf>,
}

pub struct ViewerBuilder {
    build_root: PathBuf,
    workspace_root: PathBuf,
}

impl ViewerBuilder {
    pub fn new(build_root: PathBuf, workspace_root: PathBuf) -> Self {
        Self {
            build_root,
            workspace_root,
        }
    }

    pub fn build(&self, viewer: &ViewerConfig) -> Result<BuiltViewer> {
        let cache_key = cache_key(viewer);
        let build_dir = self.build_root.join(cache_key);
        let source_dir = build_dir.join("source");

        fs::create_dir_all(&build_dir)?;
        self.checkout(&viewer.source.git, &viewer.source.git_ref, &source_dir)?;

        // The model crate may be a member of its own cargo workspace (e.g.
        // sirius's `instrumentation-model` lives in `rust/Cargo.toml`). Cargo's
        // `[patch]` only applies from the workspace root being built and does NOT
        // reach a path dependency belonging to a *foreign* workspace, so an
        // external wrapper that path-depends on such a model cannot redirect the
        // model's `rapidsai/quent` git deps to this local checkout. When the
        // model lives in a workspace, build the wrapper as a member of that
        // workspace and inject the patch into the workspace root; otherwise use a
        // standalone wrapper (which is its own workspace root and patches itself).
        let model_dir = source_dir.join(relative_source_path(&viewer.source.path));
        let manifest_dir = match find_workspace_root(&model_dir, &source_dir) {
            Some(workspace_root) => {
                let wrapper_dir = workspace_root.join("quent-open-wrapper");
                self.write_wrapper(viewer, &source_dir, &wrapper_dir, false)?;
                add_workspace_member_and_patch(
                    &workspace_root,
                    "quent-open-wrapper",
                    &self.workspace_root,
                )?;
                workspace_root
            }
            None => {
                let wrapper_dir = build_dir.join("wrapper");
                self.write_wrapper(viewer, &source_dir, &wrapper_dir, true)?;
                wrapper_dir
            }
        };

        eprintln!(
            "quent-open: building viewer '{}' (first build compiles dependencies and may take several minutes)",
            viewer.name
        );
        run_command(
            "cargo",
            &[
                "build".to_string(),
                "--package".to_string(),
                "quent-open-wrapper".to_string(),
                "--manifest-path".to_string(),
                manifest_dir.join("Cargo.toml").display().to_string(),
            ],
            Some(&manifest_dir),
        )?;
        let binary = manifest_dir
            .join("target")
            .join("debug")
            .join(executable_name("quent-open-wrapper"));

        let ui_dist = if let Some(ui) = &viewer.ui {
            // Build the frontend from a dedicated UI source when configured,
            // otherwise from the viewer's analyzer-source checkout.
            let ui_root = match &ui.git {
                Some(git) => {
                    let ui_ref = ui.git_ref.as_deref().ok_or_else(|| {
                        OpenError::Config(
                            "viewers.ui.ref is required when viewers.ui.git is set".to_string(),
                        )
                    })?;
                    let ui_source_dir = build_dir.join("ui-source");
                    self.checkout(git, ui_ref, &ui_source_dir)?;
                    ui_source_dir
                }
                None => source_dir.clone(),
            };
            // Inject the engine's generated TypeScript bindings so the frontend
            // renders this engine's entities/timelines.
            if let Some(bindings_dir) = &ui.bindings_dir {
                let target = ui_root.join(bindings_dir);
                fs::create_dir_all(&target)?;
                eprintln!("quent-open: exporting UI bindings to {}", target.display());
                run_command(
                    &binary.display().to_string(),
                    &[
                        "--export-ui-bindings".to_string(),
                        target.display().to_string(),
                    ],
                    None,
                )?;
            }
            eprintln!("quent-open: building UI ({})", ui.build_command);
            run_shell(&ui.build_command, &ui_root.join(&ui.build_dir))?;
            Some(ui_root.join(&ui.dist_dir))
        } else {
            None
        };

        eprintln!("quent-open: viewer built, starting...");
        Ok(BuiltViewer { binary, ui_dist })
    }

    fn checkout(&self, git: &str, git_ref: &str, dir: &Path) -> Result<()> {
        if !dir.exists() {
            eprintln!("quent-open: cloning source {git}");
            run_command(
                "git",
                &[
                    "clone".to_string(),
                    git.to_string(),
                    dir.display().to_string(),
                ],
                None,
            )?;
        }
        // Fetch the requested ref and hard-reset to the fetched commit. A plain
        // `git checkout <branch>` does NOT fast-forward an existing local branch,
        // so a cached clone would otherwise stay pinned to its original commit and
        // silently build a stale source forever.
        eprintln!("quent-open: fetching {git_ref} from {git}");
        run_command(
            "git",
            &[
                "fetch".to_string(),
                "--tags".to_string(),
                "origin".to_string(),
                git_ref.to_string(),
            ],
            Some(dir),
        )?;
        eprintln!("quent-open: checking out {git_ref}");
        run_command(
            "git",
            &[
                "checkout".to_string(),
                "--force".to_string(),
                "--detach".to_string(),
                "FETCH_HEAD".to_string(),
            ],
            Some(dir),
        )
    }

    fn write_wrapper(
        &self,
        viewer: &ViewerConfig,
        source_dir: &Path,
        wrapper_dir: &Path,
        standalone: bool,
    ) -> Result<()> {
        let package = viewer.source.package.clone().unwrap_or_else(|| {
            package_name_from_git(&viewer.source.git).unwrap_or_else(|| viewer.name.clone())
        });
        let source_path = source_dir.join(relative_source_path(&viewer.source.path));
        let server_path = self.workspace_root.join("domains/query_engine/server");
        let analyzer_path = self.workspace_root.join("domains/query_engine/analyzer");
        let model_path = self.workspace_root.join("domains/query_engine/model");
        let ui_model_path = self.workspace_root.join("domains/query_engine/ui");
        let analyzer_core_path = self.workspace_root.join("crates/analyzer");
        let events_path = self.workspace_root.join("crates/events");
        let ui_path = self.workspace_root.join("crates/ui");
        let server_features = if viewer.ui.is_some() {
            String::new()
        } else {
            ", features = [\"ui\"]".to_string()
        };

        let mut cargo_toml = format!(
            r#"[package]
name = "quent-open-wrapper"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
serde = {{ version = "1.0.228", features = ["derive"] }}
tokio = {{ version = "1.48.0", features = ["macros", "rt-multi-thread"] }}
quent-query-engine-server = {{ path = {}{} }}
quent-query-engine-analyzer = {{ path = {} }}
quent-query-engine-model = {{ path = {} }}
quent-query-engine-ui = {{ path = {} }}
quent-analyzer = {{ path = {} }}
quent-events = {{ path = {} }}
quent-ui = {{ path = {} }}
{} = {{ package = {}, path = {} }}
"#,
            toml_string(&server_path.display().to_string()),
            server_features,
            toml_string(&analyzer_path.display().to_string()),
            toml_string(&model_path.display().to_string()),
            toml_string(&ui_model_path.display().to_string()),
            toml_string(&analyzer_core_path.display().to_string()),
            toml_string(&events_path.display().to_string()),
            toml_string(&ui_path.display().to_string()),
            package,
            toml_string(&package),
            toml_string(&source_path.display().to_string()),
        );
        // A standalone wrapper is its own workspace root, so it carries the
        // `[patch]`. When the wrapper is a member of the model's workspace the
        // patch lives in that workspace root instead (see `build`).
        if standalone {
            cargo_toml.push_str(&patch_section(&self.workspace_root, false));
        }

        fs::create_dir_all(wrapper_dir.join("src"))?;
        fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
        fs::write(wrapper_dir.join("src/main.rs"), wrapper_main_rs(viewer)?)?;
        Ok(())
    }
}

/// Walk up from `model_dir` (inclusive) to `source_dir` (inclusive) looking for
/// the cargo workspace root whose `Cargo.toml` declares `[workspace]`. Returns
/// `None` when the model crate is standalone (no enclosing workspace).
fn find_workspace_root(model_dir: &Path, source_dir: &Path) -> Option<PathBuf> {
    let mut dir = model_dir;
    loop {
        if let Ok(contents) = fs::read_to_string(dir.join("Cargo.toml")) {
            if contents
                .lines()
                .any(|line| line.trim_start().starts_with("[workspace]"))
            {
                return Some(dir.to_path_buf());
            }
        }
        if dir == source_dir {
            return None;
        }
        dir = dir.parent()?;
    }
}

/// Register the wrapper as a member of the model's workspace and inject the
/// `rapidsai/quent` patch into that workspace root. The patch must live in the
/// build-root workspace and cover every quent crate any member references (incl.
/// the bridge's `quent-codegen`), since cargo resolves the whole workspace.
///
/// The manifest is edited as structured TOML rather than by string surgery, so
/// it copes with a workspace that has no `members` list and merges into any
/// pre-existing `[patch]` table instead of emitting a duplicate header. Comments
/// and formatting are not preserved, but this manifest lives in the throwaway
/// build checkout and is reset by `checkout` on every run.
fn add_workspace_member_and_patch(
    source_workspace: &Path,
    member: &str,
    quent_workspace: &Path,
) -> Result<()> {
    let manifest = source_workspace.join("Cargo.toml");
    let contents = fs::read_to_string(&manifest)?;
    let mut doc: toml::Table = toml::from_str(&contents)
        .map_err(|e| OpenError::Build(format!("parsing {}: {e}", manifest.display())))?;

    let workspace = doc
        .entry("workspace")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let Some(ws) = workspace.as_table_mut() {
        let members = ws
            .entry("members")
            .or_insert_with(|| toml::Value::Array(Vec::new()));
        if let Some(members) = members.as_array_mut() {
            if !members.iter().any(|m| m.as_str() == Some(member)) {
                members.push(toml::Value::String(member.to_string()));
            }
        }
    }

    let patch = doc
        .entry("patch")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let Some(patch) = patch.as_table_mut() {
        let quent = patch
            .entry("https://github.com/rapidsai/quent.git")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let Some(quent) = quent.as_table_mut() {
            for (name, rel) in patch_entries(true) {
                let mut dep = toml::Table::new();
                dep.insert(
                    "path".to_string(),
                    toml::Value::String(quent_workspace.join(rel).display().to_string()),
                );
                quent.insert(name.to_string(), toml::Value::Table(dep));
            }
        }
    }

    let rendered = toml::to_string(&doc)
        .map_err(|e| OpenError::Build(format!("rendering {}: {e}", manifest.display())))?;
    fs::write(&manifest, rendered)?;
    Ok(())
}

/// Quent crates to redirect from `rapidsai/quent` to this local workspace, as
/// `(crate name, path relative to the workspace root)`. `include_codegen` adds
/// `quent-codegen`, which a workspace member's bridge crate needs but a
/// standalone model does not.
fn patch_entries(include_codegen: bool) -> Vec<(&'static str, &'static str)> {
    let mut entries = vec![
        ("quent-analyzer", "crates/analyzer"),
        ("quent-attributes", "crates/attributes"),
        ("quent-events", "crates/events"),
        ("quent-exporter", "crates/exporter"),
        ("quent-exporter-types", "crates/exporter/types"),
        ("quent-instrumentation", "crates/instrumentation"),
        ("quent-model", "crates/model"),
        ("quent-model-macros", "crates/model-macros"),
        ("quent-query-engine-analyzer", "domains/query_engine/analyzer"),
        ("quent-query-engine-model", "domains/query_engine/model"),
        ("quent-query-engine-ui", "domains/query_engine/ui"),
        ("quent-stdlib", "crates/stdlib"),
        ("quent-time", "crates/time"),
        ("quent-ui", "crates/ui"),
    ];
    if include_codegen {
        entries.push(("quent-codegen", "crates/codegen"));
    }
    entries
}

/// The `[patch."https://github.com/rapidsai/quent.git"]` block redirecting quent
/// crates to this local workspace, for the standalone wrapper which is its own
/// (freshly generated) workspace root.
fn patch_section(quent_workspace: &Path, include_codegen: bool) -> String {
    let mut section = String::from("\n[patch.\"https://github.com/rapidsai/quent.git\"]\n");
    for (name, rel) in patch_entries(include_codegen) {
        section.push_str(&format!(
            "{name} = {{ path = {} }}\n",
            toml_string(&quent_workspace.join(rel).display().to_string())
        ));
    }
    section
}

fn wrapper_main_rs(viewer: &ViewerConfig) -> Result<String> {
    Ok(match viewer.rust_entrypoint()? {
        RustEntrypoint::Analyzer(analyzer_type) => format!(
            r#"use quent_query_engine_server::{{
    initialize_tracing,
    viewer::{{run, DefaultQuentViewer}},
}};

type GeneratedViewer = DefaultQuentViewer<{analyzer_type}>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    initialize_tracing("info");
    run::<GeneratedViewer>().await
}}
"#
        ),
        RustEntrypoint::Viewer(viewer_type) => format!(
            r#"use quent_query_engine_server::{{
    initialize_tracing,
    viewer::viewer_main,
}};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    initialize_tracing("info");
    viewer_main::<{viewer_type}>().await
}}
"#
        ),
        RustEntrypoint::QueryEngineEvent(event_type) => format!(
            r#"use quent_query_engine_analyzer::basic_ui::{{IntoQueryEngineEvent, QueryEngineUiAnalyzer}};
use quent_query_engine_model::QueryEngineEvent;
use quent_query_engine_server::{{
    initialize_tracing,
    viewer::{{viewer_main, DefaultQuentViewer}},
}};

#[derive(serde::Deserialize)]
struct GeneratedEvent(#[serde(with = "serde_event")] {event_type});

impl IntoQueryEngineEvent for GeneratedEvent {{
    fn into_query_engine_event(self) -> QueryEngineEvent {{
        match self.0 {{
            {event_type}::Engine(event) => QueryEngineEvent::Engine(event),
            {event_type}::Worker(event) => QueryEngineEvent::Worker(event),
            {event_type}::QueryGroup(event) => QueryEngineEvent::QueryGroup(event),
            {event_type}::Query(event) => QueryEngineEvent::Query(event),
            {event_type}::Plan(event) => QueryEngineEvent::Plan(event),
            {event_type}::Operator(event) => QueryEngineEvent::Operator(event),
            {event_type}::Port(event) => QueryEngineEvent::Port(event),
        }}
    }}
}}

mod serde_event {{
    use serde::Deserialize;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<{event_type}, D::Error>
    where
        D: serde::Deserializer<'de>,
    {{
        {event_type}::deserialize(deserializer)
    }}
}}

type GeneratedViewer = DefaultQuentViewer<QueryEngineUiAnalyzer<GeneratedEvent>>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    initialize_tracing("info");
    viewer_main::<GeneratedViewer>().await
}}
"#
        ),
    })
}

fn run_shell(command: &str, cwd: &Path) -> Result<()> {
    #[cfg(windows)]
    let (program, args) = ("cmd", vec!["/C".to_string(), command.to_string()]);
    #[cfg(not(windows))]
    let (program, args) = ("sh", vec!["-lc".to_string(), command.to_string()]);
    run_command(program, &args, Some(cwd))
}

fn run_command(program: &str, args: &[String], cwd: Option<&Path>) -> Result<()> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    // Inherit stdio so git/cargo progress streams to the user in real time —
    // these steps can take minutes, and silent capture looks like a hang.
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(OpenError::Process {
            program: program.to_string(),
            args: args.to_vec(),
            stderr: format!("exited with {status}"),
        })
    }
}

fn cache_key(viewer: &ViewerConfig) -> String {
    let mut hash = Sha256::new();
    hash.update(format!("{viewer:?}"));
    to_hex(&hash.finalize())[..16].to_string()
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn package_name_from_git(git: &str) -> Option<String> {
    git.rsplit(['/', ':'])
        .next()
        .map(|name| name.trim_end_matches(".git").to_string())
        .filter(|name| !name.is_empty())
}

fn executable_name(name: &str) -> String {
    #[cfg(windows)]
    {
        format!("{name}.exe")
    }
    #[cfg(not(windows))]
    {
        name.to_string()
    }
}
