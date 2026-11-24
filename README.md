# Terrarium

Deterministische, headless Simulationen in Rust: `sim-host` (Vertrag), `sim-core` (domänenneutrale Bausteine), `sim-runner` (Ausführung, Verifikation, Replays), `sim-io` (Snapshots, Replays, Configs) sowie die Spiele `dummy-sim`, `jungle-sim` und `fortress-sim`, dazu der grafische Client `apps/fortress-viewer`. Regeln stehen in [AGENTS.md](AGENTS.md), Nachweise in [VERIFICATION.md](VERIFICATION.md), Messungen in [BENCHMARKS.md](BENCHMARKS.md).

## Lokal prüfen

```sh
scripts/ci.sh                                   # vollständige lokale Verifikation
scripts/bench-ab.sh <commit-a> <commit-b> ...   # Performance-Vergleich zweier Stände
```

## CI auf einem privaten Remote

`.github/workflows/ci.yml` läuft auf Linux, Windows und macOS (ARM) und vergleicht die Hash-Dumps aller Plattformen; jede Abweichung lässt CI scheitern. Die `release-checked`-Langläufe laufen nächtlich oder manuell.

1. Auf GitHub ein **privates** Repository ohne Initialinhalt anlegen.
2. Remote eintragen und pushen:
   ```sh
   git remote add origin git@github.com:<konto>/terrarium.git
   git push -u origin main
   ```
3. Unter *Actions* startet der Workflow bei jedem Push. Den Langlauf startet *Run workflow* (`workflow_dispatch`).
