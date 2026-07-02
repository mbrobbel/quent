<!-- SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# quent-codegen

A scaffolder + build orchestrator that turns a Quent model into installable
codegen artifacts. You describe your model in a small `quent.toml`; the CLI
generates the bridge crates and packaging (`pyproject.toml`, etc.) and drives
the toolchain to produce the artifacts.

> Status: Phase A implements the **Python wheel** target. C++ (static library +
> headers) and Rust (instrumentation crate) targets are planned.

## How it works

Quent codegen needs a *compiled* `ModelBuilder` (`<Name>Model::build("<Name>")`
runs at build time), so the CLI does not link your model directly. Instead it
**generates small, detached crates** whose `build.rs` calls
[`quent-codegen`](../codegen)'s emitters — the same pattern the in-tree examples
use — then runs `maturin`/`cargo` and collects the artifacts. Generated crates
carry an empty `[workspace]` table so they never get captured by (or capture)
your repo's Cargo workspace.

## `quent.toml`

```toml
[model]
package = "my-model"     # cargo package that defines `model! { name: MyApp, ... }`
path = "."               # path to it (relative to this file); or use git/rev
name = "MyApp"           # -> MyAppModel::build("MyApp")
# type = "MyAppModel"    # override if the generated type name differs

[model.instrumentation]
generate = true          # generate a `pub use <model>::*; instrumentation!(<name>)` wrapper

[quent]
# Where generated crates get quent-codegen / quent-model from.
# External repos use git + rev; in-repo builds use a path.
git = "https://github.com/rapidsai/quent"
rev = "<commit-sha>"

[python]
package = "my-model"     # wheel/distribution name (PEP 508)
module_name = "my_model" # Python import name (also the cdylib lib name)
# version = "0.1.0"
```

The wheel is built with pyo3 `abi3-py311`, so `requires-python` is fixed at
`>=3.11` (one wheel per platform covers all newer interpreters).

## Local use

```console
quent-codegen build --manifest quent.toml --targets python --out dist
# -> dist/python/<name>-<version>-cp311-abi3-<platform>.whl
```

`maturin`, a C compiler, and a modern `protoc` (>= 3.15) must be on `PATH`
(`protoc` is pulled in transitively by the collector exporter). The
[`pixi`](https://pixi.sh) environment in this repo provides all three.

## CI: the reusable workflow

Build wheels for your own model in CI by calling
[`package.yml`](../../.github/workflows/package.yml):

```yaml
# .github/workflows/build-wheel.yml
name: Build wheel
on: [push, workflow_dispatch]
permissions:
  contents: read
jobs:
  wheel:
    uses: rapidsai/quent/.github/workflows/package.yml@<tag-or-sha>
    with:
      manifest: quent.toml
      quent-ref: <tag-or-sha>   # quent ref for the CLI + the [quent] git dep
```

Your repository needs:

1. a **model crate** using quent's `model!`/`entity!`/`fsm!` macros;
2. a **`quent.toml`** (as above) with `[quent]` set to `git`/`rev`;
3. a **`pixi.toml`** providing the build toolchain — the workflow runs the CLI
   under `pixi run`, so the caller's checkout must have a Pixi environment with
   `rust`, `maturin`, and `libprotobuf` (a modern `protoc`), e.g.:

   ```toml
   [workspace]
   channels = ["conda-forge"]
   platforms = ["linux-64", "linux-aarch64", "osx-arm64"]

   [dependencies]
   rust = ">=1.85"
   maturin = ">=1.14,<2"
   libprotobuf = ">=5"   # provides protoc >= 3.15
   ```

The workflow installs the `quent-codegen` CLI from `rapidsai/quent@<quent-ref>`,
builds one wheel per platform (abi3 → one wheel covers Python ≥ 3.11), and
uploads them as artifacts.

> Generated bridge crates are their own Cargo workspaces and resolve
> dependencies fresh at build time. Pin `[quent].rev` for a stable quent source;
> a committed lockfile for fully reproducible generated builds is a planned
> follow-up.

### Provenance

For accurate model provenance, your **model crate** should capture its own git
in a `build.rs` via `quent_build_info::emit_source()` — the `model!` macro reads
`QUENT_SOURCE_*` from the crate that expands it, which a generated wrapper
cannot supply on its behalf.

### Notes / limitations

- Linux wheels are tagged for the build image's glibc; a portable manylinux
  wheel (via `auditwheel`/a manylinux container) is a planned follow-up.
- macOS wheels are built natively on Apple Silicon (`macos-15`).
