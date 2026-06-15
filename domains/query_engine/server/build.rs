// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build the UI with pnpm when the ui feature is enabled.
    if std::env::var("CARGO_FEATURE_UI").is_ok() {
        let ui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../ui");
        println!("cargo:rerun-if-changed={}", ui_dir.join("src").display());
        // The app imports most of its code from the workspace packages
        // (@quent/components, @quent/hooks, …); without tracking these a change
        // there does not trigger a UI rebuild and the embedded bundle goes stale.
        println!(
            "cargo:rerun-if-changed={}",
            ui_dir.join("packages").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            ui_dir.join("index.html").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            ui_dir.join("package.json").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            ui_dir.join("pnpm-lock.yaml").display()
        );

        let run_pnpm = |args: &[&str]| -> Result<(), Box<dyn std::error::Error>> {
            let output = std::process::Command::new("pnpm")
                .args(args)
                .current_dir(&ui_dir)
                .env("CI", "true")
                .output()?;
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Vite prefixes warnings with "(!)".
            let has_warnings = stderr.lines().any(|l| l.trim_start().starts_with("(!)"));
            if !output.status.success() || has_warnings {
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    println!("cargo:warning=pnpm: {line}");
                }
                for line in stderr.lines() {
                    println!("cargo:warning=pnpm: {line}");
                }
            }
            if !output.status.success() {
                return Err(format!("pnpm {} failed", args.join(" ")).into());
            }
            Ok(())
        };

        run_pnpm(&["install", "--frozen-lockfile"])?;
        run_pnpm(&["build"])?;
    }

    Ok(())
}
