# sim-core-Benchmarks

Rechner `zen`, AMD Ryzen 7 9800X3D (8 Kerne, 16 Threads), x86-64, Linux 7.3.0. Rust 1.97.1, Cargo-Release-Profil mit Thin LTO. Die Standardläufe umfassen 5.000 Ticks und der Sweep 2.000 Ticks je Stufe. Beide verwenden Seed 42 und Phasenmessung. Die Starts des Sweeps haben dieselbe Dichte. Die tatsächlich aktive Population kann mit der Simulation variieren. Die Nenner pro Phase verwenden über Hash-Checkpoints gemittelte Dimensionen. Kandidaten sind Einträge vor dem Distanztest, gemittelt über alle Radiusabfragen des Laufs.

`perf` war auf dem Rechner nicht verfügbar. Hardwarezähler für Zyklen, Instruktionen und Cache-Misses fehlen daher.

## Standardlauf

Die Baseline enthält den Median aus drei Wiederholungen je Kennzahl. Für die Layout-Zwischenstände und den finalen Stand wurde je ein Standardlauf und ein vollständiger Sweep gemessen. In Schritt 3 änderte sich nur die Zustandshash-Kodierung. Der Hash wird außerhalb der Step-Zeitmessung berechnet.

Die dritte Baseline-Wiederholung schrieb beide vollständigen JSON-Reports. Danach meldete das temporäre Messskript einen Parserfehler, weil es noch während der Ausführung bearbeitet wurde. Alle sechs Reports wurden als JSON eingelesen und die Tickzahlen geprüft.

| Schritt | Code-Commit | Läufe | TPS | p50 µs | p99 µs | ns/Tier/Tick | Hash bei Tick 5.000 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| Baseline | `d3f1588` | 3 | 475.692 | 2030.310 | 4845.550 | 168.345 | 0x31cc391dc4a2dd1a |
| 4 · Dichte Slots | `0e85e25` | 1 | 559.675 | 1808.480 | 2590.330 | 143.084 | 0x19eb3e87d333006d |
| 5 · Morton-Umsortierung | `962dc12` | 1 | 626.295 | 1620.160 | 2718.800 | 127.864 | 0x19eb3e87d333006d |
| 6 · ChunkedGrid | `b21cd49` | 1 | 572.081 | 1739.820 | 2812.050 | 139.981 | 0x19eb3e87d333006d |
| 7 · Kategorieindex | `1cd65c9` | 1 | 943.586 | 1031.120 | 2108.020 | 84.868 | 0x19eb3e87d333006d |
| 8 · Invarianten und Overflow | `f5e1a2b` | 1 | 1379.466 | 718.320 | 1702.190 | 58.052 | 0x19eb3e87d333006d |

## Sweep mit konstanter Startdichte

| Schritt | Seitenlänge | Ø Tiere | Step ns/Tier/Tick | Decide ns/Tier/Tick | Environment ns/Kachel/Tick | Kandidaten/Abfrage | Abfragen/Tick |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Baseline | 256 | 3209.750 | 117.397 | 85.236 | 0.087 | 21.396 | 799.628 |
| Baseline | 512 | 12686.000 | 130.072 | 96.394 | 0.113 | 21.586 | 3163.664 |
| Baseline | 1024 | 50670.150 | 198.460 | 149.489 | 0.295 | 22.196 | 12625.815 |
| Baseline | 2048 | 195779.000 | 411.684 | 329.535 | 0.391 | 19.854 | 48734.404 |
| 4 · Dichte Slots | 256 | 3209.750 | 108.084 | 81.634 | 0.074 | 21.396 | 799.628 |
| 4 · Dichte Slots | 512 | 12686.000 | 126.574 | 97.635 | 0.120 | 21.586 | 3163.664 |
| 4 · Dichte Slots | 1024 | 50670.150 | 181.703 | 139.644 | 0.281 | 22.196 | 12625.815 |
| 4 · Dichte Slots | 2048 | 195779.000 | 283.993 | 225.361 | 0.310 | 19.854 | 48734.404 |
| 5 · Morton-Umsortierung | 256 | 3209.750 | 94.170 | 68.691 | 0.064 | 21.396 | 799.628 |
| 5 · Morton-Umsortierung | 512 | 12686.000 | 102.992 | 75.732 | 0.080 | 21.586 | 3163.664 |
| 5 · Morton-Umsortierung | 1024 | 50670.150 | 140.837 | 102.108 | 0.248 | 22.196 | 12625.815 |
| 5 · Morton-Umsortierung | 2048 | 195779.000 | 203.779 | 144.639 | 0.301 | 19.854 | 48734.404 |
| 6 · ChunkedGrid | 256 | 3209.750 | 98.949 | 71.990 | 0.133 | 21.396 | 799.628 |
| 6 · ChunkedGrid | 512 | 12686.000 | 113.839 | 83.717 | 0.151 | 21.586 | 3163.664 |
| 6 · ChunkedGrid | 1024 | 50670.150 | 145.351 | 106.561 | 0.288 | 22.196 | 12625.815 |
| 6 · ChunkedGrid | 2048 | 195779.000 | 209.882 | 148.042 | 0.366 | 19.854 | 48734.404 |
| 7 · Kategorieindex | 256 | 3209.750 | 65.149 | 36.954 | 0.136 | 2.761 | 874.672 |
| 7 · Kategorieindex | 512 | 12686.000 | 80.395 | 48.839 | 0.154 | 2.902 | 3464.088 |
| 7 · Kategorieindex | 1024 | 50670.150 | 133.771 | 83.092 | 0.380 | 2.966 | 13824.475 |
| 7 · Kategorieindex | 2048 | 195779.000 | 97.746 | 59.820 | 0.206 | 2.616 | 53645.325 |
| 8 · Invarianten und Overflow | 256 | 3209.750 | 50.334 | 31.597 | 0.118 | 2.761 | 874.672 |
| 8 · Invarianten und Overflow | 512 | 12686.000 | 54.822 | 34.028 | 0.119 | 2.902 | 3464.088 |
| 8 · Invarianten und Overflow | 1024 | 50670.150 | 58.893 | 36.079 | 0.120 | 2.966 | 13824.475 |
| 8 · Invarianten und Overflow | 2048 | 195779.000 | 75.822 | 49.126 | 0.164 | 2.616 | 53645.325 |

Bei 512² und 2.048² lagen die mittleren tatsächlich aktiven Dichten bei 4,84 % und 4,67 %. Die Step-Kosten stiegen dennoch von 130,07 auf 411,68 ns pro Tier und Tick (Faktor 3,17). Die geprüften Kandidaten sanken von 21,59 auf 19,85 pro Abfrage. Der Anstieg steckt vor allem in `decide` (96,39 auf 329,53 ns pro Tier und Tick). Das stützt einen Lokalitäts- oder Working-Set-Effekt. Die Daten zeigen keine höhere Kandidatenzahl, und ohne Hardwarezähler lässt sich die Rolle von Cache-Misses nicht direkt bestimmen.

Im Endstand kostet der Sweep 50,33, 54,82, 58,89 und 75,82 ns pro Tier und Tick. Zwischen 512² und 2.048² beträgt der Faktor 1,38. Über alle Stufen liegt der Faktor zwischen Minimum und Maximum bei 1,51. Restliche Unterschiede stecken vor allem in `decide`, wo 512² und 2.048² 34,03 beziehungsweise 49,13 ns pro Tier und Tick kosten. Die Daten messen die gesamte Phase. Einzelne Kosten für Speicherzugriffe und Sortierung sind daraus nicht isolierbar.

Dichte Slots und Morton-Umsortierung haben im großen Lauf einen deutlichen Effekt. Die Umstellung des Kachellayouts allein brachte in diesem Lauf keinen Gewinn. Der Kategorieindex reduzierte anschließend bei 2.048² sowohl Kandidaten pro Anfrage (19,85 auf 2,62) als auch Kandidaten pro Tick trotz zusätzlicher typgetrennter Anfragen. Im Endstand werden dort 2,62 Kandidaten pro Anfrage bei 53.645,32 Anfragen pro Tick geprüft.

## Bindende Referenz-Hashes nach Schritt 3

Jungle mit Seed 42 und Standardkonfiguration, Hashformat `jungle-logical-v1`. Dummy bleibt bei `0x20f21cbeee53c309` nach 1.000 Ticks.

| Tick | Jungle-Hash |
| ---: | --- |
| 1.000 | `0x5317cfd6e2e12024` |
| 2.000 | `0x44474e7d7fe88ec9` |
| 3.000 | `0xf3dab8f7d47ddfe7` |
| 4.000 | `0xb351dbf937784ff1` |
| 5.000 | `0x19eb3e87d333006d` |

Alle gemessenen Layoutschritte bestätigten diese fünf Referenz-Checkpoints. Die vier Sweep-Hashes bei Tick 2.000 waren ab Schritt 4 ebenfalls durchgehend identisch. Mit deaktivierter Morton-Umsortierung stimmten alle fünf Standardreferenzen überein. Der Shuffle-Test vergleicht 1.000 aufeinanderfolgende Tick-Hashes. Ohne die stabile Sortierung der Gestationsabsichten schlug die Gegenprobe bei Tick 2 fehl, weil ihre Reihenfolge die Sequenznummern gleichzeitiger Scheduler-Ereignisse beeinflusst.

## Overflow-Vergleich

Drei Standardläufe pro Release-Variante, in wechselnder Reihenfolge auf identischem Code nach Entfernen des zweiten Raumindex-Aufbaus. Das Entscheidungskriterium ist die mediane Step-Zeit pro Tick.

| Release-Overflow-Checks | Step ns/Tick | TPS | p99 µs |
| --- | ---: | ---: | ---: |
| Aus | 722.921,78 | 1.383,28 | 1.705,17 |
| An | 757.463,20 | 1.320,20 | 1.732,81 |

Die Mehrkosten von **4,78 %** lagen über der 3-%-Grenze. Globales Prüfen bleibt im Release-Profil aus. Die arithmetische Prüfung hat besonders die laufenden Tick-, Geburts- und Todeszähler sowie die Ereignisfolge betrachtet. Die Zähler sättigen jetzt explizit. Der Scheduler lehnt neue Ereignisse bei erschöpfter Sequenz ab, sodass keine Sortierschlüssel wiederverwendet werden. Kachel- und Slot-Indexarithmetik ist durch geprüfte Konstruktorgrößen und gültige Indizes begrenzt. Distanzquadrate und Staffelung rechnen in verbreiterten Typen, Biomasse und Bedürfnisse bleiben durch die lokalen Zuteilungs- und Sättigungsregeln begrenzt. Die Referenz-Hashes stimmten mit beiden globalen Einstellungen überein.

## Weitere Entscheidungen und Abschlussprüfung

- Die dichte Spaltengruppe führt Position und Vitalzustand gemeinsam durch Erzeugung, `swap_remove` und Permutationen. Seltene Komponenten bleiben nach stabiler ID indiziert. Der Raumindex enthält stabile IDs und transiente Slots. Die Masken des Kategorieindex wählen pro Zelle die betroffenen Listen.
- Die 32×32-Kachelblöcke sind an rechteckigen Rändern gepolstert. Logische Iteration überspringt Padding. Die Pflanzenschleife besucht Blockzeilen in physischer Reihenfolge. Jede Kachel aktualisiert ihren eigenen Wert mit derselben logischen Streifenbedingung wie zuvor. Belegte Kacheln und Gelände nutzen dasselbe Chunk-Layout.
- Die Entscheidung liest Herdenbindungen nur bei gültiger Generation und Reichweite. Bei gültiger Bindung und fehlender Paarungseignung entfällt die Herdensuche. Räuber suchen Artgenossen nur für eine mögliche Paarung.
- Die Invariantenprüfung erstellt ihren Raumindex außerhalb der Step-Zeitmessung. Der Allokationszähler erfasst derzeit nur den ausführenden Thread. Dummy nutzt weiterhin sein festes Slot-Modell und den unveränderten Golden Hash.
- Sweep-Listen werden in Flag-Reihenfolge elementweise kombiniert. Warnungen vergleichen benachbarte Stufen. Phasenkosten werden über alle ausgeführten Ticks normalisiert, auch bei seltenen Phasen wie `reorder`. Das p99-Budget vergleicht den ungerundeten Messwert.

Formatprüfung, striktes Clippy, 60 Tests im Standard-Testprofil, 60 Release-Tests sowie die drei abschließenden CLI-Aufrufe waren erfolgreich. Der Jungle-Verifikationslauf bestätigte 50 Checkpoints, null Step-Allokationen und p99 **1.732,01 µs** bei einem Budget von 10.000 µs.
