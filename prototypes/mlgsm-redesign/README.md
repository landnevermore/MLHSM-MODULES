# MLGSM Redesign Prototype

Canvas-only UX prototype for the MLGSM module redesign.

## Preview
- Cloudflare Pages project: `mlgsm-redesign-preview`
- Branch: `prototype/mlgsm-redesign`
- Pull request: https://github.com/landnevermore/MLHSM-MODULES/pull/3

## Design constraints
- The prototype represents only the module content canvas. It deliberately contains no MLHSM app shell, global sidebar, global header, or second navbar.
- The production implementation must reuse MLHSM's existing UI kit, design tokens, interaction patterns, and zero-radius styling. This standalone HTML is a UX sketch, not a substitute for those components.
- Aternos is workflow inspiration for server lifecycle and operations; Modrinth is inspiration for content hierarchy and discovery. Neither should be copied literally.
- Server management and monitoring receive equal priority in the overview.
- Existing MLHSM shell and the existing server create/lifecycle/details flows must be preserved and reused. Do not treat this prototype as a new API contract or replace existing endpoints.

## Included in this sketch
- Summary metrics, server list, status filter and search.
- Host monitoring and recent events beside server management.
- A server details section with lifecycle actions, address, backup summary and console preview.
- A create-server dialog and simple demo interactions.

## Limitations
- All values are illustrative mock data. Buttons do not call APIs, control Docker, perform backups, or read live metrics.
- The create form adds only a temporary row to the browser view.
- No production components or runtime behavior are changed by this prototype.
- The actual code still needs a component-level implementation within the existing MLHSM dashboard shell, using the established kit and preserving the current API/migration boundary.
