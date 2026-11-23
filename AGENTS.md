# Entwicklungsrichtlinien

Diese Regeln gelten für das MVP und alle künftigen Arbeiten am Workspace.

## Architektur

- Drei Schichten: `sim-host` und `sim-runner` stellen generische Schnittstellen ohne Domänenlogik bereit, `sim-core` enthält wiederverwendbare Simulationsbausteine, `games/*` enthält Regeln und Inhalte.
- Externe ECS-Crates benötigen eine dokumentierte Begründung. Speicherlayout und Iterationsreihenfolge bleiben unter unserer Kontrolle.

- Die Library von `sim-runner` und ihre Module kennen als Spielschnittstelle ausschließlich `sim-host`. Spielregeln und konkrete Spieltypen gehören in die Spiel-Crates.
- Nur `crates/sim-runner/src/games.rs` referenziert konkrete Spiele. Ein neues Spiel benötigt im Runner lediglich eine Dependency und einen Registry-Eintrag.
- Spiele kommunizieren mit dem Runner ausschließlich über den generisch verwendeten `Simulation`-Trait aus `sim-host`.

## Determinismus

- Im Simulationszustand niemals über `HashMap` oder `HashSet` iterieren. `Vec`, feste Indizes oder `BTreeMap` verwenden und stets in aufsteigender Index- beziehungsweise Schlüsselreihenfolge iterieren.
- Im Simulationskern keine Systemzeit, kein `thread_rng` und keine Abhängigkeiten von Umgebung oder Speicheradressen. Für systemweite Entscheidungen ist ein per Seed initialisierter `rand_chacha::ChaCha8Rng` zulässig.
- Zufallswerte ausschließlich über `next_u64()` oder `next_u32()` aus `rand_core` beziehen (`RngCore`, ab 0.10 `Rng`). Werte selbst konvertieren, etwa für `f64` in [0, 1) mit `(x >> 11) as f64 * (1.0 / (1u64 << 53) as f64)`. Keine `rand`-Distributionen oder `gen_range`.
- Für Gleitkommaarithmetik im Simulationskern nur `+ - * /`, `sqrt`, `abs`, `min`, `max`, `clamp` und Vergleiche verwenden. Transzendente Funktionen wie `sin`, `cos`, `tan`, `exp`, `ln`, `powf`, `hypot` und `atan2` bei Bedarf ausschließlich über `libm` beziehen. Kein `mul_add`.
- Parallelisierung ist nur bei festgelegter Reduktionsreihenfolge zulässig.
- `state_hash()` muss den gesamten zukunftsrelevanten Zustand mit `sim_host::StateHasher` abdecken. `std::hash::DefaultHasher` ist dafür unzulässig.
- Die stabile Rust-Toolchain exakt pinnen und `Cargo.lock` versionieren. Dependency- und Toolchain-Updates bewusst vornehmen und dabei die Golden-Hash-Tests prüfen.
- Fest erwartete Golden Hashes nur bei beabsichtigten Änderungen am Simulationsverhalten aktualisieren. Den Grund im Commit oder CHANGELOG dokumentieren.

- In `sim-core` und den darauf aufbauenden Spielen besteht der Simulationszustand ausschließlich aus Integer- oder Fixed-Point-Werten. Beide Crates erzwingen `#![deny(clippy::float_arithmetic)]`. Nur `metrics()` darf mit lokalem `#[allow]` und Kommentar Gleitkommaarithmetik verwenden. Das bestehende `dummy-sim` ist ausgenommen.
- Zufall pro Entität ist zustandslos und hängt ausschließlich von Seed, Tick, generationaler Entitäts-ID und Salt ab. ChaCha-Streams sind systemweiten Entscheidungen wie Weltgenerierung vorbehalten. Das bestehende `dummy-sim` behält seinen Stream.
- Interaktionen lesen in der Entscheidungsphase ausschließlich über Referenzen den Zustand am Tick-Beginn, sammeln Absichten, lösen Konflikte nach einem vollständigen Schlüssel mit Entitäts-ID als letztem Kriterium und schreiben erst in der Anwendungsphase. Sammelreihenfolge beeinflusst das Ergebnis nicht.
- Über Tick-Grenzen hinweg ausschließlich generationale IDs speichern. Ziele, Herden und Elternreferenzen vor jeder Nutzung validieren.
- Entitäten gesammelt am Tick-Ende über einen Command-Buffer in Reihenfolge der auslösenden ID und eines eindeutigen Ordinals erzeugen und entfernen.
- Teure Updates mit `(tick + key) % interval == 0` über Entitäten oder Kacheln verteilen.

## Runner-Schnittstelle

- Spiel-Configs verwenden `#[serde(default, deny_unknown_fields)]` für Teil-Configs, Overrides und präzise Tippfehlerdiagnose.
- `PhaseProbe` meldet ausschließlich Zeitgrenzen. Die Simulation liest keine Information daraus und verzweigt niemals abhängig vom Probe.

## Performance

- Structure-of-Arrays (SoA), flache `Vec`s und Index- oder Arena-Strukturen bevorzugen. Kein `Box<dyn Trait>` pro Einheit und keine verschachtelten Maps.
- Im Normalbetrieb keine Heap-Allokationen in `step()`. Kapazitäten in `new()` reservieren und Scratch-Buffer als Struct-Felder wiederverwenden.
- Tote Einheiten über eine Free-List wiederverwenden, um das Wachstum der Arrays zu begrenzen.
- `metrics()` darf außerhalb des Hot Paths allokieren.
- Ausschließlich `step()` messen. Hashing, Invariantenprüfung und I/O liegen außerhalb der Step-Zeitmessung.
- O(n·m)-Brute-Force ist nur für kleine m zulässig. Bei wachsenden Knotenzahlen ein uniformes räumliches Gitter einführen.

## Rust-Qualität und Prüfungen

- Kleine, modulare Änderungen umsetzen und neue Logik testen.
- Fehlertypen mit `thiserror` definieren. `unwrap()` und `expect()` sind nur in Tests erlaubt. `panic!` ist keine Fehlerbehandlung.
- Die Workspace-Lints in jedem Crate übernehmen. Vor Abschluss müssen `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` und `cargo test --workspace` erfolgreich laufen.

## Kompatibilität von sim-host

- Neue Trait-Methoden benötigen eine Default-Implementierung. Bestehende Signaturen bleiben kompatibel.
- Jede API-Änderung mit Begründung in `crates/sim-host/CHANGELOG.md` dokumentieren.
- Nach jeder Änderung an `sim-host` müssen alle Spiele im Workspace kompilieren und ihre Tests bestehen.

## Neues Spiel anbinden

1. Unter `games/<name>` ein Crate anlegen und die Workspace-Lints übernehmen.
2. `sim_host::Simulation` mit eigenen serialisierbaren Commands und einer deserialisierbaren, klonbaren Config mit Defaults implementieren.
3. Determinismus, Invarianten und Commands testen. Für einen festen Seed, eine feste Config und eine feste Tickzahl einen Golden-Hash-Test einrichten.
4. Das Spiel als Dependency von `sim-runner` und in `crates/sim-runner/src/games.rs` als Registry-Eintrag ergänzen.

## Skalierung und Diagnostik

- Diagnosezähler gehören nicht zum Zustandshash und beeinflussen das Simulationsverhalten nicht.
- Änderungen mit Performance-Bezug aktualisieren `BENCHMARKS.md`. Das Jungle-Standardbudget beträgt p99 ≤ 10.000 µs (`--budget-p99-us 10000`). Änderungen am Budget werden begründet.
- Zustandshashes folgen der logischen Kachel- und Entitäts-ID-Reihenfolge, unabhängig vom Speicherlayout. Verhaltensneutrale Refactorings reproduzieren die Referenz-Hashes aus `BENCHMARKS.md` exakt.
- Verhaltenslogik verwendet ausschließlich stabile `EntityId`-Werte. Dichte Slots dienen nur dem Speicherzugriff und können sich jederzeit ändern.
- Neue Dense-Komponenten werden ausschließlich über `entity_columns!` angelegt. Hash, Snapshot-Zeile und Dump entstehen aus derselben Deklaration. Spielspezifische Werttypen implementieren `StateHash` über `state_hash_struct!` oder `state_hash_enum!`.
- Jedes Spiel mit umsortierbaren Slots testet bei deterministisch zufälliger Slot-Permutation pro Tick dieselbe logische Hash-Folge. Neue Spiele richten diesen Shuffle-Test ein. Das bestehende Dummy-Spiel behält sein festes Slot-Modell.
- Der Allokationszähler erfasst nur den ausführenden Thread. Vor der Einführung paralleler Steps muss die Messung auch Worker-Threads erfassen.
- Release verwendet keine globalen Overflow-Checks. Der Standardbenchmark zeigte 4,78 % Mehrkosten. Langlaufzähler und Zeitgrenzen verwenden explizite Sättigung, IDs und Ereignisfolgen geprüfte Nachfolger und der PRNG explizites Wrapping. In Hot Paths bleiben arithmetische Operationen mit Grenzen aus geprüften Dimensionen, Kapazitäten und Wertebereichen zulässig. Bei Änderungen diese Grenzen erneut prüfen.
- Neue Benchmark-Zahlen in `BENCHMARKS.md` sind Mediane aus mindestens drei Läufen.
- Performance-Ruckler mit `--worst-ticks` untersuchen. `scripts/ci.sh` muss vor Abschluss erfolgreich sein. `release-checked` aktiviert Overflow-Checks für Langläufe.

## Snapshots und Replays

- Snapshots speichern den vollständigen logischen Zustand. Indizes, Caches, Puffer und Slot-Reihenfolge werden beim Laden neu aufgebaut. Formatänderungen erhöhen die Snapshot-Version und benötigen eine Migration oder einen dokumentierten Bruch. Der Golden Snapshot muss weiterhin laden.
- Neuer Zustand gehört gleichzeitig in Snapshot, Hash, `debug_dump` und Invarianten. Spiele mit Snapshot-Unterstützung prüfen Save/Load im Runner.
- Replays sind das Standardformat für Bug-Reports. Simulationsfehler möglichst zuerst als Replay oder Snapshot reproduzieren und anschließend als Regressionstest erhalten.
- Replays werden während des Laufs als längenpräfixierte Datensätze mit Prüfsumme geschrieben und überstehen Abstürze bis zum letzten vollständigen Datensatz. Panics in `step` schließt der Runner mit einem Panic-Datensatz ab und endet mit Exit-Code 7.
