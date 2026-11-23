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

## Bindende Referenz-Hashes nach Schritt 3

Jungle mit Seed 42 und Standardkonfiguration, Hashformat `jungle-logical-v1`. Dummy bleibt bei `0x20f21cbeee53c309` nach 1.000 Ticks.

| Tick | Jungle-Hash |
| ---: | --- |
| 1.000 | `0x5317cfd6e2e12024` |
| 2.000 | `0x44474e7d7fe88ec9` |
| 3.000 | `0xf3dab8f7d47ddfe7` |
| 4.000 | `0xb351dbf937784ff1` |
| 5.000 | `0x19eb3e87d333006d` |

## Schritt 4 · Dichte Slots

| Schritt | Code-Commit | Läufe | TPS | p50 µs | p99 µs | ns/Tier/Tick | Hash bei Tick 5.000 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 4 · Dichte Slots | `0e85e25` | 1 | 559.675 | 1808.480 | 2590.330 | 143.084 | 0x19eb3e87d333006d |

| Schritt | Seitenlänge | Ø Tiere | Step ns/Tier/Tick | Decide ns/Tier/Tick | Environment ns/Kachel/Tick | Kandidaten/Abfrage | Abfragen/Tick |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 4 · Dichte Slots | 256 | 3209.750 | 108.084 | 81.634 | 0.074 | 21.396 | 799.628 |
| 4 · Dichte Slots | 512 | 12686.000 | 126.574 | 97.635 | 0.120 | 21.586 | 3163.664 |
| 4 · Dichte Slots | 1024 | 50670.150 | 181.703 | 139.644 | 0.281 | 22.196 | 12625.815 |
| 4 · Dichte Slots | 2048 | 195779.000 | 283.993 | 225.361 | 0.310 | 19.854 | 48734.404 |

## Schritt 5 · Morton-Umsortierung

| Schritt | Code-Commit | Läufe | TPS | p50 µs | p99 µs | ns/Tier/Tick | Hash bei Tick 5.000 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 5 · Morton-Umsortierung | `962dc12` | 1 | 626.295 | 1620.160 | 2718.800 | 127.864 | 0x19eb3e87d333006d |

| Schritt | Seitenlänge | Ø Tiere | Step ns/Tier/Tick | Decide ns/Tier/Tick | Environment ns/Kachel/Tick | Kandidaten/Abfrage | Abfragen/Tick |

| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 5 · Morton-Umsortierung | 256 | 3209.750 | 94.170 | 68.691 | 0.064 | 21.396 | 799.628 |
| 5 · Morton-Umsortierung | 512 | 12686.000 | 102.992 | 75.732 | 0.080 | 21.586 | 3163.664 |
| 5 · Morton-Umsortierung | 1024 | 50670.150 | 140.837 | 102.108 | 0.248 | 22.196 | 12625.815 |
| 5 · Morton-Umsortierung | 2048 | 195779.000 | 203.779 | 144.639 | 0.301 | 19.854 | 48734.404 |

## Schritt 6 · ChunkedGrid

| Schritt | Code-Commit | Läufe | TPS | p50 µs | p99 µs | ns/Tier/Tick | Hash bei Tick 5.000 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 6 · ChunkedGrid | `b21cd49` | 1 | 572.081 | 1739.820 | 2812.050 | 139.981 | 0x19eb3e87d333006d |

| Schritt | Seitenlänge | Ø Tiere | Step ns/Tier/Tick | Decide ns/Tier/Tick | Environment ns/Kachel/Tick | Kandidaten/Abfrage | Abfragen/Tick |

| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 6 · ChunkedGrid | 256 | 3209.750 | 98.949 | 71.990 | 0.133 | 21.396 | 799.628 |
| 6 · ChunkedGrid | 512 | 12686.000 | 113.839 | 83.717 | 0.151 | 21.586 | 3163.664 |
| 6 · ChunkedGrid | 1024 | 50670.150 | 145.351 | 106.561 | 0.288 | 22.196 | 12625.815 |
| 6 · ChunkedGrid | 2048 | 195779.000 | 209.882 | 148.042 | 0.366 | 19.854 | 48734.404 |

## Schritt 7 · Kategorieindex

| Schritt | Code-Commit | Läufe | TPS | p50 µs | p99 µs | ns/Tier/Tick | Hash bei Tick 5.000 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 7 · Kategorieindex | `1cd65c9` | 1 | 943.586 | 1031.120 | 2108.020 | 84.868 | 0x19eb3e87d333006d |

| Schritt | Seitenlänge | Ø Tiere | Step ns/Tier/Tick | Decide ns/Tier/Tick | Environment ns/Kachel/Tick | Kandidaten/Abfrage | Abfragen/Tick |

| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 7 · Kategorieindex | 256 | 3209.750 | 65.149 | 36.954 | 0.136 | 2.761 | 874.672 |
| 7 · Kategorieindex | 512 | 12686.000 | 80.395 | 48.839 | 0.154 | 2.902 | 3464.088 |
| 7 · Kategorieindex | 1024 | 50670.150 | 133.771 | 83.092 | 0.380 | 2.966 | 13824.475 |
| 7 · Kategorieindex | 2048 | 195779.000 | 97.746 | 59.820 | 0.206 | 2.616 | 53645.325 |

## Overflow-Vergleich

Drei Standardläufe pro Release-Variante, in wechselnder Reihenfolge auf identischem Code nach Entfernen des zweiten Raumindex-Aufbaus. Das Entscheidungskriterium ist die mediane Step-Zeit pro Tick.

| Release-Overflow-Checks | Step ns/Tick | TPS | p99 µs |
| --- | ---: | ---: | ---: |
| Aus | 722.921,78 | 1.383,28 | 1.705,17 |
| An | 757.463,20 | 1.320,20 | 1.732,81 |

Die Mehrkosten von **4,78 %** lagen über der 3-%-Grenze. Globales Prüfen bleibt im Release-Profil aus. Die arithmetische Prüfung hat besonders die laufenden Tick-, Geburts- und Todeszähler sowie die Ereignisfolge betrachtet. Die Zähler sättigen jetzt explizit. Der Scheduler lehnt neue Ereignisse bei erschöpfter Sequenz ab, sodass keine Sortierschlüssel wiederverwendet werden. Kachel- und Slot-Indexarithmetik ist durch geprüfte Konstruktorgrößen und gültige Indizes begrenzt. Distanzquadrate und Staffelung rechnen in verbreiterten Typen, Biomasse und Bedürfnisse bleiben durch die lokalen Zuteilungs- und Sättigungsregeln begrenzt. Die Referenz-Hashes stimmten mit beiden globalen Einstellungen überein.
