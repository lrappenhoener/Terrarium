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
- 6 für einen abweichenden Zustand nach Save/Load. Dieser Code gilt für eine unmittelbare Abweichung nach dem Laden und für eine spätere Abweichung im fortgesetzten Lauf.

## Etappe 2 · Arithmetikprüfung

- Bedürfnisse und Biomasse bleiben durch Sättigung und Zuteilungsgrenzen innerhalb ihrer Wertebereiche. Alter, Tick und Langlaufzähler sättigen. Generationen und Ereignisfolgen verwenden geprüfte Nachfolger.
- Positionen sind durch die validierte Weltgröße bis 4.096 und Nachbarschaftsschritte von höchstens zwei Kacheln begrenzt. Distanzquadrate und Staffelung verwenden verbreiterte Integer. Chunk- und Slot-Indizes folgen den geprüften Abmessungen und Kapazitäten. Counting-Sort benutzt Präfixsummen der lebenden Slots und streut jede ID genau einmal in ihren Bereich.
- Die Zweierpotenz-Streifen berechnen den modularen Startversatz mit expliziter Wrapping-Subtraktion und Maske. Beide Varianten wählen dieselben logischen Kacheln.
- `release-checked` führt den Jungle-Langlauf zusätzlich mit aktiven Überlaufprüfungen aus.

## Etappe 2 · Snapshots, Replays und genaue Abweichungssuche

Formatprüfung, striktes Clippy sowie je 74 Tests in Debug und Release sind grün. `scripts/ci.sh` prüft auch den 5.000-Tick-Lauf mit kombiniertem Determinismus- und Save/Load-Vergleich, den Snapshot-Neustart, das Beispiel-Replay, die Ausreißerliste und den erwarteten Dummy-Fehler. Der geprüfte Langlauf erreicht 50.000 Ticks bei 500 übereinstimmenden Checkpoints mit aktiven Overflow-Checks.

Die Referenzen sind unverändert. Jungle erreicht bei Tick 1.000 `0x5317cfd6e2e12024` und bei Tick 5.000 `0x19eb3e87d333006d`, auch vom Snapshot bei Tick 2.500 aus. Dummy behält seinen Golden Hash `0x20f21cbeee53c309`. Der Neustart erfüllt `--max-step-allocs 0`. Für Jungle wurde kein Verlust des logischen Zustands beobachtet. Die Prüfungen umfassen Umsortierungsgrenzen, aktive Trächtigkeit und Dürre. Die Golden Fixture reproduziert `0xcd31ad5b0c3262ff` bei Tick 500 und `0x22ed95e00104e9ba` nach weiteren 500 Ticks.

### Nachweis der Abweichungssuche

`cargo test -p sim-runner --lib bisect -- --nocapture` meldete in beiden Pflichttests:

```text
determinism mismatch at tick 437: 0xae3874868a8703cb vs 0x8f3dad7d7f97b9aa; checkpoint interval (400, 500]; differing subsystems: b
line 2: A b entity=17 value=0 | B b entity=17 value=1

save/load verification at tick 437: exact mismatch 317 ticks after reload at 120 in checkpoint interval (400, 500]; differing subsystems: b
line 2: A b entity=17 value=0 | B b entity=17 value=1
```

### Format und Betriebsentscheidungen

- Snapshot und Replay teilen eine Envelope mit Magic, Little-Endian-Version und Headerlänge, JSON-Header und FNV-1a über die Nutzdaten. Die Build-ID kombiniert den Git-Commit mit dem Dirty-Status versionierter Dateien. Speichern ersetzt die Zieldatei nach einem synchronisierten Schreiben über eine temporäre Datei.
- Jungle verwendet Postcard für die versionierten Nutzdaten und JSON für die vollständige wirksame Config darin. Wasserentfernungen, Belegung, Index, dichte Slots und Puffer werden neu aufgebaut. Die Statusreferenzen verwenden generationale IDs. [Postcard-Wire-Format](https://docs.rs/crate/postcard/1.2.0)
- Replays bewahren die zusammengeführte TOML-Eingabe bzw. JSON-Eingabe und markieren reine Defaults explizit. JSON bleibt erhalten, damit Typen und Werte wie unsigned 64-Bit-Zahlen oberhalb des TOML-Integerbereichs lesbar bleiben. Command-Gruppen verwenden nach Möglichkeit direkt Postcard und fallen für selbstbeschreibende Serde-Typen auf JSON oder TOML zurück.
- Der Runner verfeinert festgestellte Abweichungen automatisch. `--bisect` aktiviert zusätzlich die Snapshot-Anker von Lauf A. Reproduziert ein Anker die Abweichung nicht, etwa wegen eines verlorenen versteckten Zustands, beginnt der Vergleich am ursprünglichen Start. Die Dateinamen lauten `tick-<TICK>-a.txt` und `tick-<TICK>-b.txt`. Der Textreport enthält bis zu 20 abweichende Zeilen.
- Der Worst-Tick-Report hält einen vorreservierten Heap der langsamsten Samples. Er begrenzt den Speicherbedarf auf die angeforderte Anzahl. Ticknummern bezeichnen den Zustand nach dem Step.

### Nächste Prioritäten für den Produktiveinsatz

1. Versionsmigrationen mit archivierten Fixtures pro unterstütztem Release und wiederherstellbaren, versionierten Speicherständen.
2. Artefaktgrößen begrenzen und Replay-Logs streamen, damit lange Produktionsaufzeichnungen und die Fehlersuche weniger Arbeitsspeicher brauchen.
3. Dumps nach stabilen Schlüsseln abgleichen und Laufzeitmessungen mit CPU-Isolation und Hardwarezählern ergänzen.

Der Jungle-Standard-Snapshot bei Tick 2.500 misst **1.075.429 Bytes**. Im Median aus drei Läufen dauerte Speichern **12,10 ms** und Laden inklusive Rekonstruktion **15,63 ms**. Historische Zuordnung, Ausreißer und Messmethode stehen in [BENCHMARKS.md](BENCHMARKS.md).
