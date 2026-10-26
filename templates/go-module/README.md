# Go external module template

Copy this directory into `modules/<module-id>/`, then replace the sample ID, routes, metadata, and service behavior.

The process must:
- read `MLHSM_MODULE_PORT` (or the port argument when `prefer_arg_port` is true);
- bind to `127.0.0.1`, not `0.0.0.0`;
- serve the declared `health_path`;
- expose only routes declared in `route_prefixes`;
- keep durable state under `data/modules/<module-id>/` or another explicitly host-provided writable location;
- never log credentials or trust a module manifest as a permission grant.

Run locally:

```sh
go test ./...
go run .
```

The central release workflow currently packages Linux x86_64 builds only.
