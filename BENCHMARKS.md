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

## Etappe 2 · Historische Nachmessung

Die Einzelmessungen der Layout-Zwischenstände oben sind historische Werte aus Etappe 1. Alle Zahlen aus neuen Messungen verwenden mindestens drei Läufe. Für die folgende Zuordnung wurde jeder Commit separat gebaut und fünfmal im Jungle-Standardlauf mit `--profile-phases` gemessen. Die erste Messserie mit gemeinsamem Cargo-Zielverzeichnis wurde verworfen, nachdem identische Binärdateien erkannt wurden.

| Commit | Step ns/Tier/Tick | spatial | decide | resolve | apply | lifecycle | reorder | environment ns/Kachel/Tick |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `962dc12` vor Kachelblöcken | 160,176 | 6,760 | 123,978 | 8,244 | 5,705 | 10,090 | 2,075 | 0,148 |
| `b21cd49` nach Kachelblöcken | 176,761 | 6,693 | 136,169 | 8,281 | 7,166 | 10,263 | 2,048 | 0,284 |
| `1cd65c9` vor Invarianten-Umbau | 100,979 | 7,831 | 61,025 | 7,947 | 6,129 | 11,304 | 2,014 | 0,210 |
| `f5e1a2b` nach Invarianten-Umbau | 91,182 | 7,431 | 60,536 | 7,971 | 5,955 | 3,139 | 2,079 | 0,202 |

Die Spalten zwischen Step und environment sind ebenfalls ns pro Tier und Tick. Die Blockumstellung kostete in der Nachmessung rund **10,4 %**, vor allem in `decide`, `apply` und `environment`. Die Blöcke bleiben erhalten. Grasabsichten verwenden jetzt das logische Tupel (y, x) anstelle von Umrechnungen zum und vom linearen Index. Zweierpotenz-Streifen verwenden eine Maske für den Startversatz. Die Größe der verbleibenden Blocklayout-Kosten ist aus dem kombinierten Nachher-Lauf allein nicht isolierbar.

Schritt 8 spart reproduzierbar **9,7 %** statt der historischen Einzelmessung von rund 32 %. Der größte Effekt liegt in `lifecycle` mit **11,304 → 3,139 ns/Tier/Tick (−72,2 %)**. Der zweite Raumindex-Aufbau entfällt in dieser Phase. Der invariante Neuaufbau liegt außerhalb der Step-Zeit.

### Ausreißer vor und nach Counting-Sort und Kachelzugriffs-Optimierung

Drei alternierende Standardläufe pro Variante. `--worst-ticks 5000` erfasst für die Auswertung alle Ticks. Die Top-10-Spalte zählt Umsortierungen unter den zehn langsamsten Ticks.

| Variante | p50 µs | p95 µs | p99 µs | Max µs | p95 ohne reorder µs | Median-Tick mit reorder µs | reorder in Top 10 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Vergleichssortierung | 1.108,44 | 1.451,46 | 3.074,53 | 7.080,75 | 1.416,45 | 2.851,12 | 3 |
| Counting-Sort | 1.076,26 | 1.371,57 | 2.315,75 | 7.364,01 | 1.349,99 | 1.249,28 | 2 |

Der Median eines Umsortierungsticks liegt jetzt unter p95 der übrigen Ticks. Einzelne Ausreißer bleiben. In den Läufen mit medianem Maximum hatten die langsamsten Ticks (vorher Tick 2.298, nachher 3.602) jeweils keine Umsortierung.

Die alte Sweep-Ausgabe enthielt bereits die Warnung für den Übergang von 1.024² auf 2.048². Ein Regressionstest verwendet exakt 58,89 und 75,82 bei Faktor 1,25.

Die getrennte Nachmessung auf dem Counting-Sort-Stand (je drei alternierende Läufe) bestätigt den Effekt der Kachelzugriffs-Optimierung. Der Standardlauf sank von **71,65 auf 69,13 ns/Tier/Tick**. `environment` sank von **0,151 auf 0,098 ns/Kachel/Tick**, `decide` von **48,25 auf 47,00 ns/Tier/Tick**. `apply` lag mit **4,67 und 4,70 ns/Tier/Tick** praktisch gleich. Dieser Vergleich misst die konkreten Zugriffsänderungen, während der historische Vorher/Nachher-Vergleich oben den ganzen Layoutwechsel misst.

Der Median der Anzahl von Umsortierungsticks oberhalb des gewöhnlichen p95 sank von **78 auf 26** pro 5.000 Ticks. Der typische Umsortierungstick liegt unter p95 der übrigen Ticks. Das p95 innerhalb der Umsortierungsgruppe lag nach dem Umbau noch bei **1.939,08 µs**. Seltene Spitzen sind damit weiterhin sichtbar.

### Standard-Snapshot bei Tick 2.500

Seed 42 und Standardkonfiguration, Release-Build auf `zen`. Drei Save/Load-Paare. Der Ladevorgang folgte jeweils direkt auf das Speichern. Die Zeit umfasst Container-Verarbeitung und Datei-I/O außerhalb der Step-Zeitmessung.

| Operation | Median Bytes | Median ms |
| --- | ---: | ---: |
| Speichern | 1.075.429 | 12,10 |
| Laden und Rekonstruktion | 1.075.429 | 15,63 |

Alle fortgesetzten Läufe erreichten den Referenz-Hash `0x19eb3e87d333006d` bei Tick 5.000. Die genauen Abweichungstests und der geprüfte 50.000-Tick-Lauf sind in [VERIFICATION.md](VERIFICATION.md) dokumentiert.

## Etappe 3 – Fortress

Gleicher Rechner, Rust 1.97.1, Release-Profil. Alle Werte sind Mediane aus drei Läufen mit Seed 42, Szenario `games/fortress-sim/data/scenario_dig_in.toml` und Phasenmessung. Jungle erreichte am Messtag 797 TPS (Median, 5.000 Ticks) gegenüber 742 TPS für den Etappe-2-Commit `784c63e` auf demselben Rechner. Die Änderungen kosten also keine Step-Zeit; die absolut niedrigeren Werte gegenüber der Tabelle oben gehen auf den Rechnerzustand zurück.

### Standardlauf (A*, 20.000 Ticks, 96×96×32)

TPS 13.906 · p50 61,42 µs · p99 249,74 µs · Max 4.097 µs · 0 Step-Allokationen. Hash `0xae5ee3d9cc6dbcc2`.

| Phase | p50 µs | p99 µs | Max µs | Anteil |
| --- | ---: | ---: | ---: | ---: |
| `commands` | 0,02 | 0,12 | 341,7 | 0,07 % |
| `connectivity` | 0,02 | 8,38 | 847,4 | 1,02 % |
| `needs` | 0,52 | 8,95 | 1.933,3 | 3,02 % |
| `job_assign` | 39,89 | 145,23 | 3.602,0 | 62,38 % |
| `path` | 0,16 | 63,51 | 2.696,2 | 3,75 % |
| `move` | 0,28 | 4,17 | 328,7 | 0,77 % |
| `work` | 16,15 | 33,63 | 2.855,5 | 24,81 % |
| `lifecycle` | 2,13 | 3,85 | 2.453,1 | 3,45 % |
| `environment` | 0,03 | 0,06 | 26,6 | 0,05 % |

Die zehn langsamsten Ticks (2,9–4,4 ms) liegen in jedem Lauf auf anderen Ticks und in anderen Phasen (z. B. 9.716, 9.232, 17.757). Es handelt sich um Scheduling-Rauschen, nicht um deterministische Spitzen; die Phasenmaxima liegen weit über dem jeweiligen p99.

Pfad- und Erreichbarkeitsstatistik (deterministisch, in allen Läufen identisch): 1,35 Suchen pro Tick, 64,5 Expansionen pro Suche, Ø 14,35 Pfadknoten, 3.099 gefundene Pfade, 0 `Unreachable`, 0 `BudgetExceeded`, 0 vom Vorab-Check übersprungene Anfragen, 0 Ticks Wartezeit in der Queue. Erreichbarkeit: 860 Aktualisierungen, 1.131 geänderte Kacheln, 1.115 Verschmelzungen, 12 Flutfüllungen mit Ø 6.241 Kacheln (die Mauerbauten teilen die große Oberflächenregion).

### A* gegen hierarchisch (Szenario, 20.000 Ticks)

| Modus | TPS | p50 µs | p99 µs | `path` p99 µs | `path`-Anteil | Expansionen/Suche | Ø Pfadknoten | Fallbacks |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A* | 13.906 | 61,42 | 249,74 | 63,51 | 3,75 % | 64,5 | 14,35 | – |
| hierarchisch | 13.617 | 62,19 | 255,97 | 122,07 | 8,09 % | 152,6 | 15,27 (+6,4 %) | 0 |

HPA*-Caches: 561 Chunk-Neuberechnungen gegenüber 16.423 Treffern und 2.742 Flächen-Neuberechnungen. 1.788 Anfragen wurden innerhalb eines Chunks beantwortet. Im Szenario sind die Wege kurz (≈14 Knoten). HPA* bezahlt die lokalen Dijkstra-Suchen von Start und Ziel zu allen Eingängen ihrer Chunks und braucht daher mehr Expansionen als direktes A*.

### Sweep (5.000 Ticks, `initial_residents` und `world_size_xy` paarweise)

| Modus | Bewohner, Seitenlänge | TPS | p50 µs | p99 µs | ns/Entität/Tick | `job_assign` | `work` | `path` | `needs` | `connectivity` |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A* | 25, 96 | 26.006 | 31,98 | 154,27 | 73,78 | 51,8 % | 21,1 % | 12,6 % | 1,7 % | 2,6 % |
| A* | 100, 128 | 16.276 | 42,07 | 265,52 | 94,46 | 60,1 % | 18,4 % | 9,2 % | 3,4 % | 2,5 % |
| A* | 400, 192 | 8.662 | 82,17 | 666,65 | 99,87 | 52,9 % | 15,9 % | 6,8 % | 17,4 % | 2,3 % |
| hierarchisch | 25, 96 | 20.835 | – | 243,13 | 92,7 | – | – | 28,4 % | – | – |
| hierarchisch | 100, 128 | 13.020 | – | 369,16 | 118,1 | – | – | 23,6 % | – | – |
| hierarchisch | 400, 192 | 7.098 | – | 752,61 | 121,9 | – | – | 13,1 % | – | – |

Entitäten umfassen Gegenstände. Von 25 auf 400 Bewohner bei vierfacher Kachelzahl steigen die Kosten pro Entität und Tick um 35 % (A*). Einzig `needs` skaliert nicht: Bei 400 Bewohnern reichen die 60 Startrationen nicht, viele sind gleichzeitig hungrig (81 verhungern), und jeder durchsucht die Liste freier Nahrung. Das kostet O(Hungrige × Nahrung) pro Tick. `connectivity` bleibt niedrig, weil Flutfüllungen selten sind; ihre Kosten wachsen mit der Oberflächenregion, also quadratisch mit der Seitenlänge.

## Etappe 4 – Viewer, Index, Sweep ohne Verhungern

Gemessen am 24.11.2026 auf `zen` (AMD Ryzen 7 9800X3D), Rust 1.97.1, Release-Profil. Absolute Zahlen sind nur informativ; Vergleiche zwischen Ständen laufen über `scripts/bench-ab.sh`.

### Klärung „797 gegen 742 TPS“ und A/B-Nachmessung

In Etappe 3 gehörten 797 TPS zum damaligen Stand (`ad5c637`) und 742 TPS zum Etappe-2-Commit `784c63e`. Der neuere Stand war also nicht der langsamere. Beide Werte stammen aus einer gestörten Sitzung, in der die Maschine allgemein langsamer lief (Jungle sonst ≈1.450 TPS). Nachmessung mit `scripts/bench-ab.sh 784c63e ad5c637 --game jungle --ticks 5000`, 5 abwechselnde Paare:

| Stand | TPS Median | Min | Max | p99 µs Median | Instruktionen (500 Ticks) |
| --- | ---: | ---: | ---: | ---: | ---: |
| A `784c63e` (Etappe 2) | 1.454,87 | 1.454,05 | 1.456,47 | 807,72 | 4.625.304.493 |
| B `ad5c637` (Etappe 3) | 1.442,51 | 1.426,87 | 1.444,76 | 824,57 | 4.628.803.063 |

B liegt bei −0,85 % TPS und +2,09 % p99, die Instruktionszahl unterscheidet sich um +0,076 %. Etappe 3 hat im Jungle-Step keine Regression; der kleine konstante Zeitunterschied bei fast gleichen Instruktionen ist ein Code-Layout-Effekt.

### Sweep vor und nach Index und Spawn-Korrektur (5.000 Ticks, A*, Median aus 3)

| Stand | Bewohner, Seitenlänge | TPS | p99 µs | ns/Entität/Tick | `needs` Anteil | `needs` ns/Bewohner/Tick | Hungertote |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Etappe 3 | 25, 96 | 26.006 | 154,27 | 73,78 | 1,7 % | – | 0 |
| Etappe 3 | 100, 128 | 16.276 | 265,52 | 94,46 | 3,4 % | – | 0 |
| Etappe 3 | 400, 192 | 8.662 | 666,65 | 99,87 | 17,4 % | – | 81 |
| nur Index | 400, 192 | 7.349 | 609,97 | 117,70 | 36,1 % | 135,99 | 81 |
| Index + Spawn-Korrektur, 60 Startrationen | 25, 96 | 44.399 | 82,63 | 43,22 | 0,6 % | 5,73 | 0 |
| Index + Spawn-Korrektur, 60 Startrationen | 100, 128 | 28.397 | 155,52 | 54,14 | 1,4 % | 4,91 | 0 |
| Index + Spawn-Korrektur, 60 Startrationen | 400, 192 | 16.904 | 455,50 | 52,31 | 3,8 % | 5,65 | 0 |
| zusätzlich 3 Rationen pro Bewohner | 25, 96 | 44.757 | 81,43 | 41,96 | 0,6 % | 5,63 | 0 |
| zusätzlich 3 Rationen pro Bewohner | 100, 128 | 21.926 | 180,47 | 51,06 | 1,1 % | 5,17 | 0 |
| zusätzlich 3 Rationen pro Bewohner | 400, 192 | 5.776 | 546,80 | 76,89 | 1,3 % | 5,56 | 0 |

Die Hungertoten kamen nicht von zu wenig Nahrung: Bei 400 Bewohnern reichte die Spawn-Spirale auf die Klippen des Bergs, und genau 81 Bewohner standen unerreichbar von der Nahrung. Seit der Korrektur sterben keine Bewohner mehr. `needs` kostet pro Bewohner konstant 5–6 ns und skaliert damit linear. Mit 1.600 Startrationen dominiert `job_assign` (78 %): Für jede lose Ration besteht ein Transportjob, und das Lager ist voll.

### Lange Wege (`scenario_long_haul.toml`, 20.000 Ticks, Median aus 3)

Konfiguration `long_haul_config.toml` (48 Ebenen, 40 Bewohner). Schacht über 32 Ebenen, Abbau auf z=3, Lager an der Oberfläche, Baustellen östlich des Bergs. 21,5 % aller Wege haben mindestens 100 Knoten, im Mittel 121.

| Modus | TPS | p50 µs | p99 µs | Max µs | `path` p99 µs | `path` Max µs | `path` Anteil | Expansionen/Suche | Ø Kosten | Ø Knoten |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A* | 22.155 | 32,68 | 470,25 | 2.960 | 412,5 | 2.830 | 30,4 % | 463,5 | 592,0 | 48,04 |
| hierarchisch | 23.742 | 34,38 | 235,60 | 1.080 | 153,1 | 958 | 26,4 % | 293,3 | 617,9 (+4,4 %) | 49,83 |

Worst-Ticks: Bei A* trifft es deterministisch Tick 9.001 (2,9–3,0 ms, nach der Designation bei Tick 9.000 suchen viele Arbeiter gleichzeitig) und Tick 1 (2,0–2,6 ms, erste Suchen). Hierarchisch liegt der langsamste Tick 6.002 bei ≈1,1 ms. Es gab weder `Unreachable` noch `BudgetExceeded` oder Fallbacks.

**Empfehlung:** Hierarchisch lohnt sich, sobald typische Wege mehrere Chunks und Ebenen überspannen (hier ab etwa 100 Knoten bzw. mehr als 20 Ebenen Höhendifferenz). Es halbiert p99 und drittelt die Spitzen bei 4–5 % längeren Wegen. Bei kurzen Wegen (Standard-Szenario, ≈14 Knoten) gewinnt A*. Der Standard bleibt `astar`.

### Viewer-Overhead (Standard-Szenario, 20.000 Ticks, Maximalgeschwindigkeit, Median aus 3)

| Lauf | reine Step-TPS | p99 µs | Wanduhr-TPS |
| --- | ---: | ---: | ---: |
| Runner headless | 19.831 | 161,01 | 11.090 (inkl. Hash-Checkpoints und Invarianten alle 100 Ticks) |
| Viewer | 19.658 | 167,90 | 10.742 (inkl. Replay, Checkpoint-Hashes, Frame-Updates, 3 Autosaves) |

Der Simulationsschritt kostet im Viewer-Thread gleich viel wie headless (−0,9 %, Rauschen). Die Differenz der Wanduhr-TPS fällt außerhalb von `step` an: Replay-Checkpoints hashen die Welt alle 100 Ticks, dazu kommen Frame-Kopien höchstens alle 15 ms und die Autosaves.

### Speichern und Laden (Viewer-Session, Standard-Szenario)

| Vorgang | Tick | Größe | Dauer |
| --- | ---: | ---: | ---: |
| Autosave | 6.000 | 25.262 B | 22,4 ms |
| Autosave | 12.000 | 28.174 B | 21,4 ms |
| Autosave | 18.000 | 39.086 B | 22,9 ms |
| Speichern (Median aus 3) | 20.000 | 39.735 B | 15,0 ms |
| Laden (Median aus 3) | 20.000 | 39.735 B | 33,2 ms |

Speichern und Laden erzeugen die Welt neu, um Kacheln gegen den Generator zu differenzieren, und verwenden ein synchronisiertes Schreiben. Bei 1× (50 ms pro Tick) bleibt ein Autosave unter einem Tick.

### A/B Etappe 3 (`ad5c637`) gegen Etappe 4

| Lauf | TPS A | TPS B | Δ TPS | Δ p99 | Δ Instruktionen | Hash |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Jungle, 5.000 Ticks | 894,0 | 905,3 | +1,27 % | +1,55 % | +0,000 % (500 Ticks) | `0x19eb3e87d333006d` (beide) |
| Fortress, Szenario, 20.000 Ticks | 16.005 | 20.233 | +26,42 % | −10,14 % | −5,328 % (2.000 Ticks) | `0xae5ee3d9cc6dbcc2` (beide) |

Bei Jungle sind die Instruktionen identisch; der TPS-Unterschied ist Rauschen. Die absolute Jungle-Rate lag in dieser Sitzung bei ≈900 statt ≈1.450 TPS, ein Beispiel für die Schwankungen zwischen Sitzungen. Fortress gewinnt durch den Kategorie-Index bei unverändertem Verhalten.

## Etappe 5 – Weltphysik

Gemessen am 24.11.2026 auf `zen` (AMD Ryzen 7 9800X3D), Rust 1.97.1, Release-Profil, Seed 42, 20.000 Ticks, Phasenmessung. Alle Werte sind Mediane aus 3 Läufen. Stand `ce54c07` (mit Hysterese der Unpassierbar-Markierung).

| Szenario | TPS | p50 µs | p99 µs | Max µs | Fluid-Chunks schlafend | Wärme-Chunks aktiv | End-Hash |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `dig_in` | 4.413 | 216,03 | 625,84 | 5.920 | 123/144 (85 %) | 0 | `0x661aec897fba0652` |
| `flood` | 4.569 | 190,93 | 510,26 | 4.331 | 123/144 (85 %) | 0 | `0xed6cc7456121d81a` |
| `magma` | 2.135 | 491,79 | 862,19 | 5.271 | 123/144 (85 %) | 2 | `0xfac8b755ad9c607d` |
| `cavein` | 5.750 | 179,08 | 276,18 | 5.869 | 123/144 (85 %) | 0 | `0x350d729d31f3b480` |

Alle vier Läufe bestanden zusätzlich `--verify-determinism --verify-save-load --max-step-allocs 0 --budget-p99-us 10000` (Exit 0).

Phasen (Anteil, p99 µs, ns pro Skalierungseinheit):

| Szenario | `fluids` | `temperature` | `support` | `collapse` | `job_assign` | `connectivity` |
| --- | --- | --- | --- | --- | --- | --- |
| `dig_in` | 76,5 %, 416, 91,9/Fluid-Kachel | 0,2 %, 1,4 | 0,06 %, 3,7, 233/Prüfeinheit | 0,01 %, 0,2 | 12,7 %, 77 | 2,4 %, 71 |
| `flood` | 88,3 %, 445, 83,5 | 0,2 %, 1,5 | 0,04 %, 0,6 | 0,02 %, 0,2 | 5,0 %, 53 | 0,8 %, 45 |
| `magma` | 35,4 %, 443, 90,4 | 59,5 %, 418, 71,3/Wärme-Kachel | 0,02 %, 0,5 | 0,01 %, 0,3 | 2,3 %, 38 | 0,5 %, 47 |
| `cavein` | 88,4 %, 233, 95,0 | 0,3 %, 1,6 | 0,06 %, 0,4 | 0,02 %, 0,2 | 5,6 %, 43 | 0,4 %, 30 |

Schlimmste Ticks (Lauf 1): `dig_in` 10.959 (5,9 ms), `flood` 4.291 (17,4 ms, davon `fluids` 17,35 ms; in den Läufen 2 und 3 nicht unter den 10 schlimmsten, also Systemrauschen), `magma` 2.108 (4,3 ms), `cavein` 5.555 (5,9 ms). Vor der Hysterese lagen die Spitzen in `connectivity` (bis 11,4 ms), weil Wasserstände an der Schwelle die Regionen in jedem Tick neu füllten.

**Kosten der Physik gegenüber Etappe 3/4.** Der Standardlauf `dig_in` fällt von 13.906 auf 4.413 TPS (p99 250 → 626 µs). Ursache ist der dauerhaft fließende Fluss im Osten: Quelle und Mündung halten 21 Chunks wach (≈3.500 aktive Fluid-Kacheln pro Tick). Das Budget p99 ≤ 10.000 µs bleibt mit Faktor 16 eingehalten.

**Hysterese (A/B im selben Lauf, `dig_in`, Mediane aus 3):** vorher 2.827 TPS, p99 1.204 µs, Max 6.672 µs, 242 Flood-Fills; nachher 4.413 TPS, p99 626 µs, Max 5.920 µs.

### Sweep Wassermenge (`scenario_flood.toml`, 5.000 Ticks, Median aus 3)

| `physics.river_width` | Wasser gesamt | aktive Fluid-Kacheln/Tick | wache Chunks | TPS | p99 µs | `fluids` Anteil | `fluids` p99 µs | ns/aktive Kachel |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 16.358 | 3.531 | 42 | 3.503 | 536 | 88,1 % | 428 | 71,2 |
| 3 | 25.297 | 3.761 | 45 | 3.079 | 1.097 | 88,8 % | 491 | 76,7 |
| 6 | 37.785 | 4.038 | 45 | 2.917 | 629 | 89,3 % | 524 | 75,8 |
| 12 | 62.806 | 4.574 | 45 | 2.516 | 683 | 90,6 % | 587 | 78,7 |

Die Wassermenge vervierfacht sich, die Kosten pro aktiver Kachel steigen nur um 10,6 %. Die Kosten folgen den aktiven Kacheln und nicht der Wassermenge, weil ruhendes Wasser schläft. Der leichte Anstieg pro Kachel liegt in `fluids` (mehr Transfers pro Kachel bei breiterem Fluss). Der Sweep über `physics.aquifer_interval=12,6,3,1` ändert die Wassermenge kaum (22.337 → 25.441), ns pro Kachel bleiben bei 71–76.

## Etappe 6 · Schritt 0b – Ausreißer reproduzierbar prüfen

`--repeat <N>` führt denselben Lauf N-mal aus und vergleicht die Tick-Zeiten Tick für Tick. Ein Tick ist ein Ausreißer eines Laufs, wenn er über `--outlier-factor` (Standard 3) × Median aller Ticks dieses Laufs liegt. Reproduzierbar ist er, wenn das in mindestens N−1 Läufen gilt; sonst wird er als einmalig („vermutlich Rauschen“) gemeldet. `--worst-ticks` nutzt bei `--repeat` den Median pro Tick über alle Läufe, die Phasenaufteilung ist ebenfalls der Median pro Phase. Für eine sinnvolle Trennung braucht es N ≥ 3.

Nachprüfung des 17-ms-Ticks aus Etappe 5 (24.11.2026, `zen`, `scenario_flood.toml`, 20.000 Ticks, `--repeat 5 --profile-phases --worst-ticks 10`, Hash `0xed6cc7456121d81a` unverändert):

- Median-Tick der Läufe 195–202 µs, p99 520–549 µs.
- **Tick 4.291 ist in keinem der 5 Läufe auffällig.** Der 17-ms-Tick war Rauschen der Maschine.
- 362 einmalige Ausreißer, der größte bei Tick 4.989 mit 4,6 ms in 1/5 Läufen (Median 440 µs).
- 26 reproduzierbare Ausreißer, alle unter 1,6 ms:
  - Tick 1 (1,58 ms, Wegsuche beim Start),
  - 22 Ticks mit 0,76–1,18 ms, davon 0,54–0,75 ms in `connectivity`: Flood-Fill der Regionen, wenn Wasser eine Schwelle überschreitet und die Topologie ändert,
  - 3 Ticks um 0,6 ms mit Wegsuchen (`path` ≈ 0,3 ms).
