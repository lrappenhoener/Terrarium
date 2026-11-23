# sim-core-Benchmarks

Rechner `zen`, AMD Ryzen 7 9800X3D (8 Kerne, 16 Threads), x86-64, Linux 7.3.0. Rust 1.97.1, Cargo-Release-Profil mit Thin LTO. Die Standardläufe umfassen 5.000 Ticks und der Sweep 2.000 Ticks je Stufe. Beide verwenden Seed 42 und Phasenmessung. Die Starts des Sweeps haben dieselbe Dichte; die tatsächlich aktive Population kann mit der Simulation variieren. Die Nenner pro Phase verwenden über Hash-Checkpoints gemittelte Dimensionen. Kandidaten sind Einträge vor dem Distanztest, gemittelt über alle Radiusabfragen des Laufs.

`perf` war auf dem Rechner nicht verfügbar. Hardwarezähler für Zyklen, Instruktionen und Cache-Misses fehlen daher.

## Baseline

Drei vollständige Wiederholungen, Median pro Kennzahl. Die letzte Wiederholung schrieb beide vollständigen JSON-Reports und meldete danach einen Shell-Parserfehler, weil das temporäre Messskript noch während der Ausführung bearbeitet wurde. Die Reports wurden als JSON eingelesen und beide Läufe enthielten die erwartete Tickzahl.

| Schritt | Code-Commit | Läufe | TPS | p50 µs | p99 µs | ns/Tier/Tick | Hash bei Tick 5.000 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| Baseline | `d3f1588` | 3 | 475.692 | 2030.310 | 4845.550 | 168.345 | 0x31cc391dc4a2dd1a |

| Schritt | Seitenlänge | Ø Tiere | Step ns/Tier/Tick | Decide ns/Tier/Tick | Environment ns/Kachel/Tick | Kandidaten/Abfrage | Abfragen/Tick |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Baseline | 256 | 3209.750 | 117.397 | 85.236 | 0.087 | 21.396 | 799.628 |
| Baseline | 512 | 12686.000 | 130.072 | 96.394 | 0.113 | 21.586 | 3163.664 |
| Baseline | 1024 | 50670.150 | 198.460 | 149.489 | 0.295 | 22.196 | 12625.815 |
| Baseline | 2048 | 195779.000 | 411.684 | 329.535 | 0.391 | 19.854 | 48734.404 |

Bei 512² und 2.048² lagen die mittleren tatsächlich aktiven Dichten bei 4,84 % und 4,67 %. Die Step-Kosten stiegen dennoch von 130,07 auf 411,68 ns pro Tier und Tick (Faktor 3,17). Die geprüften Kandidaten sanken von 21,59 auf 19,85 pro Abfrage. Der Anstieg steckt vor allem in `decide` (96,39 auf 329,53 ns pro Tier und Tick). Das stützt einen Lokalitäts- oder Working-Set-Effekt. Die Daten zeigen keine höhere Kandidatenzahl, und ohne Hardwarezähler lässt sich die Rolle von Cache-Misses nicht direkt bestimmen.
