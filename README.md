# MLHSM Modules

The central source and release repository for MLHSM external modules.

## One repository, two responsibilities

- `modules/<module-id>/` contains the module source, its runtime manifest, and module-specific tests.
- `registry.json` is the catalog MLHSM reads. It points to each module manifest and carries the published release summary.
- GitHub Actions builds a module from this repository and publishes a versioned release asset. MLHSM downloads that asset to the MLHSM host and runs it as a supervised child process.

GitHub hosts source code and downloadable release assets; it does **not** run the module backend. The service itself runs on the machine hosting MLHSM.

## Module layout

A buildable Go module should use this layout:

```text
modules/<module-id>/
  go.mod
  main.go
  module.json
  ...tests and source files...
```

The release workflow builds a Linux x86_64 executable and packages it as:

```text
module.json
runtime/<entry.command>
```

The executable name in `entry.command` must be a bare filename. MLHSM resolves it from the extracted module's `runtime/` directory. The process must bind only to loopback and use the port passed by MLHSM through `MLHSM_MODULE_PORT` (or the argument when `prefer_arg_port` is enabled).

## Publishing a module release

1. Add module source under `modules/<module-id>/` and ensure `go test ./...` and `go build ./...` pass.
2. Keep the runtime manifest version in `module.json` equal to the catalog version in `registry.json`.
3. Push a tag named `module-<module-id>-v<version>`, for example `module-social-uploader-v0.1.0`.
4. The release workflow builds and tests the module, packages the runtime, creates a GitHub Release asset, computes its SHA-256 and byte size, then updates the module manifest and registry on `main`.
5. MLHSM reads the updated manifest from the registry and verifies both hash and size before extraction.

The release workflow deliberately fails closed if the tag, module ID, version, entry command, or expected files do not agree. A release cannot be published just because a registry entry exists.

## Manifest compatibility

The host currently deserializes the installable artifact metadata as `tag`, `asset`, `url`, `sha256`, and `size`. Keep `schema/manifest.schema.json` aligned with this contract. The catalog's `release` object is display metadata; the installer consumes the `artifact` object from the per-module manifest.

## Current scope and limits

- The initial release workflow targets Linux x86_64, matching the first self-hosted server target. Windows and ARM release selection require a host-side platform-aware artifact contract and are not claimed by this pipeline.
- The existing MLGSM and ModelMesh source files are not yet standalone release projects with a build definition. This workflow does not pretend to package them.
- Publishing a module release does not automatically grant requested permissions. MLHSM operator grants remain a separate host-side decision.
