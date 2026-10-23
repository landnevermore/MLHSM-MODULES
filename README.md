# MLHSM Modules

This repository contains the distributable source packages and registry manifests for external MLHSM modules.

## Build from source

Install a stable Rust toolchain and the native build prerequisites required by your platform. From the repository root:

```sh
cargo check --workspace --all-targets
cargo build --release -p mlgsm -p modelmesh
python3 scripts/validate_registry.py
```

The binaries are written to `target/release/mlgsm` and `target/release/modelmesh` (Windows adds the `.exe` suffix). Install each binary beside its matching `module.json` in the MLHSM module runtime directory. MLHSM must grant the permissions listed by each manifest before enabling the module.

The shared `hsm-model` crate contains only the public data contracts used by these modules; it is maintained here so the module builds do not depend on a path inside the MLHSM application repository.

## Platform notes

MLGSM requires access to a compatible Docker Engine. ModelMesh requires outbound access to whichever AI providers the operator configures. A successful compile does not replace an integration test against a running MLHSM host.
