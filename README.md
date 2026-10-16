# MLHSM External Modules

Source-Code der externen Module (MLGSM, ModelMesh, etc.).

## Struktur

- **mlgsm/** — MLGameServerManager: Game-Server-Management
- **modelmesh/** — ModelMesh: ML-Modell-Verwaltung

## Architektur

Diese Module wurden aus `hsm-core/src/` migriert (Commit `a1eaab9`, Phase 3E)
und als externe Module im neuen Runtime-System registriert.

### Migration

Vorher: In-process Module (`impl Module for MlgsmModule`)
Nachher: Externe Module mit eigenem Process + Manifest (`module.json`)

### Registrierung

Jedes Modul bringt sein eigenes `module.json` mit und wird über
`runtime::discover()` automatisch erkannt und vom Supervisor gemanagt.
