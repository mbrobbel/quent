// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Build and serve viewers for discovered contexts. Contexts sharing a build
//! spec (same analyzer + pinned commits + format) are served by a single viewer;
//! distinct viewers are built in parallel and announced as each comes up.

use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::task::JoinSet;

use crate::error::{OpenError, Result};
use crate::spec::ViewerSpec;
use crate::wrapper::{self, PORT_ENV, ROOT_ENV, WRAPPER_PACKAGE};

/// One viewer to build: a representative [`ViewerSpec`] plus every context it
/// should serve (all sharing that spec).
pub struct ViewerGroup {
    pub spec: ViewerSpec,
    pub contexts: Vec<PathBuf>,
}

/// Build and serve every group in parallel, announcing each viewer's URL as soon
/// as it is ready. A browser is opened only when there is a single viewer.
/// Blocks until all viewers exit (e.g. Ctrl-C); a failed build does not stop the
/// others.
pub async fn open_all(groups: Vec<ViewerGroup>, no_browser: bool) -> Result<()> {
    let total: usize = groups.iter().map(|g| g.contexts.len()).sum();
    println!(
        "discovered {total} context(s) -> {} viewer(s)",
        groups.len()
    );
    let open_browser = !no_browser && groups.len() == 1;

    let mut set = JoinSet::new();
    for group in groups {
        set.spawn(async move { open_one(group, open_browser).await });
    }

    let mut failures = 0usize;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                failures += 1;
                eprintln!("viewer failed: {e}");
            }
            Err(e) => {
                failures += 1;
                eprintln!("viewer task error: {e}");
            }
        }
    }
    if failures > 0 {
        Err(OpenError::ViewersFailed { count: failures })
    } else {
        Ok(())
    }
}

/// Build one group's viewer and serve all its contexts.
async fn open_one(group: ViewerGroup, open_browser: bool) -> Result<()> {
    let ViewerGroup { spec, contexts } = group;
    let label = format!("{} ({} context(s))", spec.analyzer_package, contexts.len());
    println!("building: {label}");

    let crate_dir = build_dir(&spec)?;
    wrapper::generate(&spec, &crate_dir)?;
    let bin = cargo_build(&crate_dir).await?;
    let output_root = stage_output_root(&crate_dir, &contexts)?;
    let result = serve(&output_root, &bin, &label, open_browser).await;
    // Best-effort cleanup of this run's staged root (the cached build is kept).
    let _ = std::fs::remove_dir_all(&output_root);
    result
}

/// Cache directory for this viewer's generated crate and build, under the user
/// cache dir keyed by [`ViewerSpec::cache_key`] so identical specs are reused.
fn build_dir(spec: &ViewerSpec) -> Result<PathBuf> {
    let base = dirs::cache_dir().ok_or(OpenError::NoCacheDir)?;
    Ok(base
        .join("quent")
        .join("open")
        .join("builds")
        .join(spec.cache_key()))
}

/// Run `cargo build --release` in `crate_dir`, returning the built binary path.
/// Output goes to `<crate_dir>/build.log` rather than the console so parallel
/// builds don't interleave; on failure the log's tail is folded into the error.
///
/// The first build fetches the pinned git sources and compiles the embedded UI,
/// which invokes `pnpm`/`node`; both must be on `PATH`. Subsequent builds reuse
/// the cached `crate_dir`.
async fn cargo_build(crate_dir: &Path) -> Result<PathBuf> {
    let log_path = crate_dir.join("build.log");
    let log = std::fs::File::create(&log_path)?;
    let log_err = log.try_clone()?;
    let status = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(crate_dir)
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .status()
        .await
        .map_err(|source| OpenError::Spawn {
            what: "cargo build".into(),
            source,
        })?;
    if !status.success() {
        return Err(OpenError::Build {
            status: format!("{status}; last output:\n{}", log_tail(&log_path)),
        });
    }
    Ok(crate_dir
        .join("target")
        .join("release")
        .join(WRAPPER_PACKAGE))
}

/// Last 20 lines of a build log, for surfacing why a build failed.
fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

/// Stage a clean output root symlinking each `context` under its own UUID name.
/// The server scans an output root of `<context-uuid>/` directories; isolating to
/// the requested contexts serves exactly them and avoids unrelated siblings (which
/// may use a different format). The root is unique per process so concurrent runs
/// sharing a cached build dir do not clobber each other.
fn stage_output_root(crate_dir: &Path, contexts: &[PathBuf]) -> Result<PathBuf> {
    let root = crate_dir.join(format!("serve-root-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    for context in contexts {
        let context = context.canonicalize()?;
        let name = context.file_name().ok_or_else(|| {
            OpenError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "context path has no final component",
            ))
        })?;
        symlink_dir(&context, &root.join(name))?;
    }
    Ok(root)
}

/// Symlink a context directory into the staged output root.
#[cfg(unix)]
fn symlink_dir(src: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, link)?;
    Ok(())
}

#[cfg(not(unix))]
fn symlink_dir(_src: &Path, _link: &Path) -> Result<()> {
    Err(OpenError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "serving local artifacts requires symlink support",
    )))
}

/// Spawn the built viewer serving `output_root`, announce its URL once it accepts
/// connections, and run until it exits.
async fn serve(output_root: &Path, bin: &Path, label: &str, open_browser: bool) -> Result<()> {
    let port = free_port()?;
    let url = format!("http://127.0.0.1:{port}/");

    let mut child = Command::new(bin)
        .env(ROOT_ENV, output_root)
        .env(PORT_ENV, port.to_string())
        .spawn()
        .map_err(|source| OpenError::Spawn {
            what: "viewer".into(),
            source,
        })?;

    if wait_until_ready(port).await {
        println!("ready: {label}  {url}");
        if open_browser && let Err(e) = open::that(&url) {
            eprintln!("could not open a browser ({e}); open {url} manually");
        }
    } else {
        eprintln!("warning: {label} did not start listening at {url} within the timeout");
    }

    let status = child.wait().await?;
    if !status.success() {
        return Err(OpenError::ViewerExited {
            status: status.to_string(),
        });
    }
    Ok(())
}

/// Pick a currently-free localhost TCP port. There is a small race between this
/// and the viewer binding it, acceptable for a local dev tool.
fn free_port() -> Result<u16> {
    let listener = StdTcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// Poll `port` until it accepts a connection. Returns `true` once the server is
/// up, or `false` if it never accepts within the timeout window.
async fn wait_until_ready(port: u16) -> bool {
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}
