# Verifikation der Massen-Simulation

`sim-host` definiert den Ausführungsvertrag und die Diagnose-Hooks. `sim-runner` führt Spiele über die Registry aus und misst Latenzen, Phasen, Allokationen und Determinismus. `sim-core` stellt generationale IDs, Dense- und Sparse-Storage, Command- und Intent-Buffer, Kachelgitter, Counting-Sort-Raumindex, Scheduler, zustandslosen Zufall und ganzzahlige Utility-Auswahl bereit. `jungle-sim` definiert Gelände, Tierverhalten, Ressourcen, Fortpflanzung und Tod.

Die Formatprüfung, striktes Clippy sowie alle 51 Tests in Debug und Release waren erfolgreich. Das Testprofil nutzt Optimierungsstufe 2, damit es den vollständigen Populationstest mit drei Seeds und je 10.000 Ticks ausführen kann. Beide Arten überlebten und ihre Populationen schwankten bei jedem Seed.

## Jungle-Release-Lauf

Seed 42, 5.000 Ticks, 50 übereinstimmende Checkpoints und finaler Hash `0x31cc391dc4a2dd1a`. Es wurden 487,52 TPS bei 164,26 ns pro aktiver Entität und Tick gemessen. Die Step-Gesamtzeit betrug 10,256025 s, die Wall-Clock-Zeit 10,559475 s. Tick-Latenzen in µs waren 824,22 (Min), 1.960,29 (p50), 2.926,57 (p95), 5.360,72 (p99) und 9.080,88 (Max). Der Lauf zählte null Step-Allokationen. Auch der separate Lauf über 3.000 Ticks bestand `--max-step-allocs 0` mit null Allokationen insgesamt.

| Phase | p50 µs | p99 µs | Max µs | Step-Anteil |
| --- | --- | --- | --- | --- |
| `commands` | 0,03 | 0,32 | 1,09 | 0,00 % |
| `spatial` | 96,98 | 126,72 | 2.942,55 | 4,78 % |
| `decide` | 1.524,54 | 4.602,54 | 8.324,74 | 77,94 % |
| `resolve` | 93,38 | 125,58 | 2.982,83 | 4,66 % |
| `apply` | 81,30 | 119,77 | 3.055,53 | 3,83 % |
| `environment` | 27,19 | 138,28 | 2.891,91 | 1,77 % |
| `lifecycle` | 144,28 | 186,96 | 4.839,35 | 7,00 % |

| Tick | Pflanzenfresser | Raubtiere |
| --- | --- | --- |
| 0 | 10.000 | 500 |
| 1.000 | 12.815 | 931 |
| 2.000 | 11.903 | 827 |
| 3.000 | 12.213 | 861 |
| 4.000 | 12.465 | 851 |
| 5.000 | 10.274 | 761 |

## Sweep bei 2.048 × 2.048 Kacheln

| Start-Pflanzenfresser | Ø aktive Entitäten | TPS | p50 µs | p99 µs | ns pro Entität und Tick |
| --- | --- | --- | --- | --- | --- |
| 1.000 | 1.380,55 | 1.619,31 | 430,34 | 3.118,95 | 447,32 |
| 10.000 | 21.309,55 | 269,78 | 2.476,96 | 12.038,58 | 173,95 |
| 100.000 | 183.326,45 | 16,25 | 63.126,78 | 115.725,87 | 335,70 |

Die Kosten skalieren über diese drei Punkte nicht linear. Im kleinsten Szenario beansprucht der Pflanzenstreifen 70,91 % der Step-Zeit. Dieser weitgehend von der Weltgröße bestimmte Aufwand verteilt sich im mittleren Szenario auf mehr Tiere. Von der mittleren zur größten Population steigen die Kosten pro aktiver Entität um den Faktor 1,93. Der Anteil von `decide` wächst von 54,56 % auf 77,37 %, entsprechend etwa 95 auf 260 ns Entscheidungszeit pro aktiver Entität und Tick. Bei fester Weltgröße erhöht die Populationsdichte die Anzahl der Kandidaten pro Radiusabfrage und damit den Aufwand für Distanztests, Sortierung und Verhaltensauswahl. Die Messung erfasst die Phasenzeit. Der Beitrag einzelner Operationen innerhalb der Phase wurde nicht getrennt gemessen. Der Hinweis bei Faktor 2 blieb aus, da er die kleinste und größte Einstellung vergleicht (447,32 und 335,70 ns).

Der Dummy-Release-Lauf bestätigte 50 Checkpoints und den finalen Hash `0x57e3662cf9fdc402`. Alle vier vorgeschriebenen Release-Aufrufe wurden erfolgreich abgeschlossen.

## Golden Hashes

| Spiel | Seed | Tick | Hash |
|---|---:|---:|---|
| Dummy | 42 | 1.000 | `0x20f21cbeee53c309` |
| Jungle | 42 | 1.000 | `0x9498cee015167101` |

## Detailentscheidungen

- Der Allocator nutzt eine LIFO-Free-List und legt einen Slot bei erschöpfter Generation still. Komponenten prüfen die Generation beim Zugriff. Command-Aufrufer vergeben einen eindeutigen Ordinalwert je Quelle. Intent-Aufrufer geben je Quelle und Konfliktschlüssel höchstens eine Absicht ab. Kleinere Prioritätswerte gewinnen. Jungle kehrt den Hungerwert für Angriffe um.
- Die Welt kombiniert ganzzahliges Patch-Rauschen und Wasserstellen auf einem versetzten Raster. Lokale Bewegungen nutzen vier Nachbarn und vorberechnete Wasserdistanzen. Ziele müssen beim Entscheiden frei sein. Gras kann auch von Nachbarfeldern aufgenommen werden und wird in ID-Reihenfolge aufgeteilt. Angriffe wirken auf das beim Entscheiden sichtbare Ziel und sättigen den Sieger beim Fang.
- Herdenbindungen und Jagdziele werden bei der nächsten Entscheidung erneut validiert. Geburten nutzen begehbare Plätze im Umfeld des Elternteils. Die Geburtenrate wird zusätzlich durch Populationsgrenzen und Partner in Sichtweite gebremst. Externe Spawns suchen von einem zufälligen Startfeld aus zyklisch nach freien Kacheln.
- Der Raumindex wird am Tick-Anfang für Entscheidungen und nach dem Lebenszyklus für einen konsistenten Endzustand aufgebaut. Eine neue Dürre ersetzt die vorherige. Während einer Dürre verlieren fällige Kacheln Biomasse. Die Populationszählung und der Zufallsstream für externe Spawns gehen in den Meta-Hash ein. Der Gesamt-Hash kombiniert die vier Subsystem-Hashes in fester Reihenfolge.
- Der CLI-Einstieg bindet den globalen Allocator aus `allocation-counter` ein. Er erfasst Allokationen und Reallokationen im ausführenden Thread während `step_profiled`. Report-Summen enthalten die Warm-up-Ticks. Der harte Grenzwert gilt ab dem darauf folgenden Tick. Die Phasensonde nutzt feste Scratch-Arrays für jeweils bis zu 64 Phasen und Verschachtelungsebenen. Messreihen werden beim ersten Auftreten außerhalb der Step-Zeitmessung reserviert. [allocation-counter](https://docs.rs/allocation-counter/0.8.1/allocation_counter/)
- `--set` akzeptiert TOML-Literale und wendet Overrides in Flag-Reihenfolge an. Dummy lehnt jetzt ebenfalls unbekannte Config-Felder ab. Die Simulationslogik und der Golden Hash sind erhalten. Sweeps verwenden dieselben Seeds und mitteln die aktive Population über die Hash-Checkpoints. Optionalen Ausgabedateien wird im Sweep ein Laufindex angehängt.
- Traces beginnen am Initialzustand, schreiben anschließend an Intervallgrenzen und gegebenenfalls am finalen Tick. Opaque IDs werden sortiert und dedupliziert. Der erste fehlende Zustand erzeugt genau einen `null`-Eintrag.
- Beim Einrichtungsschritt war `.git` in der Sandbox schreibgeschützt. Initialisierung und lokale Commits erfolgten mit der dafür freigegebenen Ausführung.

## Prioritäten für Burg- oder Stadtsimulationen

1. Deterministische Wegeplanung mit reservierbaren Wegen und Zielen, damit viele Agenten Engstellen gemeinsam nutzen können.
2. Ein wiederverwendbarer Job- und Ressourcenplaner für Arbeitsaufträge, Bestände, Transporte und Reservierungen.
3. Persistenz mit versionierten Checkpoints und Replay sowie ein Datenlayout für größere, hierarchisch organisierte Welten.

## CLI-Exit-Codes

- 0 für einen erfolgreichen Lauf.
- 1 für Eingabe- und allgemeine Fehler.
- 2 für Abweichungen beim Determinismusvergleich.
- 3 für verletzte Invarianten.
- 4 für ein überschrittenes Step-Allokationslimit.
- 5 für ein überschrittenes p99-Budget.

## Etappe 2 · Arithmetikprüfung

- Bedürfnisse und Biomasse bleiben durch Sättigung und Zuteilungsgrenzen innerhalb ihrer Wertebereiche. Alter, Tick und Langlaufzähler sättigen. Generationen und Ereignisfolgen verwenden geprüfte Nachfolger.
- Positionen sind durch die validierte Weltgröße bis 4.096 und Nachbarschaftsschritte von höchstens zwei Kacheln begrenzt. Distanzquadrate und Staffelung verwenden verbreiterte Integer. Chunk- und Slot-Indizes folgen den geprüften Abmessungen und Kapazitäten. Counting-Sort benutzt Präfixsummen der lebenden Slots und streut jede ID genau einmal in ihren Bereich.
- Die Zweierpotenz-Streifen berechnen den modularen Startversatz mit expliziter Wrapping-Subtraktion und Maske. Beide Varianten wählen dieselben logischen Kacheln.
- `release-checked` führt den Jungle-Langlauf zusätzlich mit aktiven Überlaufprüfungen aus.
