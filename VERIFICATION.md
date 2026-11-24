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

## Etappe 3 – Zwischenbericht Teil A (Fundament)

Geprüft am Commit `1169dfc` mit `scripts/ci.sh` (Exit 0): `cargo fmt --all -- --check`, striktes Clippy, `cargo test --workspace` in Debug und Release (106 Tests), Dummy- und Jungle-Läufe mit Determinismus-, Save/Load-, Replay- und Allokationsprüfung sowie der `release-checked`-Langlauf über 50.000 Ticks.

- **Referenzen unverändert:** Jungle `0x5317cfd6e2e12024` (Tick 1.000) und `0x19eb3e87d333006d` (Tick 5.000, auch nach Laden des Snapshots bei Tick 2.500), Golden Snapshot lädt. Dummy `0x20f21cbeee53c309` (Golden-Test). Jungle hält `--max-step-allocs 0`.
- **0a Makro-Spalten:** `entity_columns!` erzeugt aus derselben Deklaration Zeilentyp, `row`/`push_row`, `hash_row` und `dump_row`. `StateHash` deckt Integer, `bool`, Tupel, Arrays, `Option`, `Vec` und `EntityId` ab; `state_hash_struct!` erzwingt per vollständiger Destrukturierung, dass kein Feld fehlt. `SparseStorage` hasht, sichert und dumpt nach ID sortiert. Jungle nutzt die generierten Funktionen; ein Test kodiert den Golden Snapshot neu und erhält **byteidentische** Daten. Dafür werden `u16` als vier Bytes gehasht (wie im bisherigen Jungle-Format) und die Spaltenreihenfolge `position, vitals` beibehalten.
- **0b Replays:** Format Version 2 mit längenpräfixierten, FNV-geprüften Datensätzen (Start, Commands pro Tick, Checkpoints, Abschluss). Geschrieben wird während des Laufs, geflusht bei jedem Checkpoint und spätestens alle `--replay-flush-every` Ticks. Ein Test-Panic bei Tick 123 hinterlässt ein Replay mit Abschlussdatensatz `Panic { tick: 123 }`; das Abspielen meldet „simulation panicked at tick 123 … (reproduziert wie aufgezeichnet)“ mit Exit-Code 7. Abgeschnittene und beschädigte Dateien spielen bis zum letzten vollständigen Datensatz ab und melden „unvollständig“. `--replay-max-bytes` beendet die Aufzeichnung mit Warnung und Grenzdatensatz.
- **0c Schlüssel-Diff:** Dumps vergleichen `key<TAB>value` nach Schlüssel. Eine zusätzliche Entität erscheint als genau eine Zeile `only in B: entity:2:0 …`. Zeilen ohne Tab gelten als ganzer Schlüssel.
- **Schritte 1–3:** `ChunkedGrid3` (16×16×8, Morton-Chunks, Versionen, Änderungsliste) stimmt mit einem flachen Modell überein. `Tile` belegt 4 Bytes. Bewegungsregeln sind getestet und auf Zufallswelten symmetrisch. A* liefert auf zufälligen Schicht- und 2D-Welten dieselben Kosten wie Dijkstra; die Warteschlange ist unabhängig von der Einfügereihenfolge. Die gezielte Erreichbarkeitsaktualisierung stimmt nach über 5.000 zufälligen Grab-, Mauer-, Treppen- und Rampenoperationen (vier Seeds) in jedem Schritt mit einer Neuberechnung überein; zwei Mutationen (keine Flutfüllung, keine Vereinigung) lässt der Test scheitern.
- **Abweichung:** Die vorgegebene Heuristik `oktil + 20·|Δz|` ist mit Rampen nicht zulässig: Eine Rampe überwindet einen orthogonalen Schritt und eine Ebene für 20, die Heuristik schätzt 30. Verwendet wird `20·dz + max(0, oktil − 10·dz)`; ein Test belegt das Gegenbeispiel.

## Etappe 3 – Abschlussbericht

`scripts/ci.sh` (um Fortress ergänzt) lief mit Exit 0: fmt, striktes Clippy, je 125 Tests in Debug und Release, Dummy- und Jungle-Läufe, Fortress mit Determinismus, Save/Load (Roundtrips 500 und 7.777), `--max-step-allocs 0` und `--budget-p99-us 10000`, ein hierarchischer Lauf, Fortress-Replay, Snapshot mit ASCII-Ansicht sowie `release-checked`-Langläufe (Jungle 50.000 und Fortress 20.000 Ticks). Benchmark-Zahlen stehen in `BENCHMARKS.md`.

1. **Schritt 0:** siehe Zwischenbericht Teil A. Der Golden Snapshot von Jungle wird byteidentisch neu kodiert. Das Panic-Replay reproduziert Tick 123 mit Exit-Code 7. Der Schlüssel-Diff meldet eine zusätzliche Entität als genau eine Zeile.
2. **Fortress-Lauf** (A*, 20.000 Ticks, Median aus 3 Läufen): 13.906 TPS, p50 61,42 µs, p99 249,74 µs, Max 4,1 ms, 0 Step-Allokationen. Den größten Anteil haben `job_assign` (62 %) und `work` (25 %). Die langsamsten Ticks liegen in jedem Lauf woanders und gehen auf Scheduling-Rauschen zurück.
3. **Pfade und Erreichbarkeit:** 1,35 Suchen pro Tick, 64,5 Expansionen pro Suche, Ø 14,35 Pfadknoten, 0 `Unreachable`, 0 `BudgetExceeded`, keine Wartezeit in der Queue. 860 Erreichbarkeits-Aktualisierungen mit 1.115 Verschmelzungen. Nur die 12 Mauerbauten lösten Flutfüllungen aus (Ø 6.241 Kacheln).
4. **A\* gegen hierarchisch:** HPA* braucht 152,6 statt 64,5 Expansionen pro Suche, und `path` wird doppelt so teuer (p99 122 statt 64 µs). Die Wege sind 6,4 % länger (Knoten), Fallbacks gab es keine. **Standard bleibt `astar`:** Im Szenario sind die Wege kurz (≈14 Knoten), deshalb überwiegen die lokalen Start- und Zielsuchen von HPA* dessen Ersparnis. HPA* bleibt als deterministische Option für lange Wege in großen Welten bestehen.
5. **Sweep:** Von 25 Bewohnern auf 96² bis 400 Bewohner auf 192² steigen die Kosten pro Entität und Tick nur um 35 %. `needs` skaliert nicht: Bei 400 Bewohnern reicht die Startnahrung nicht (81 verhungern), und jeder Hungrige durchsucht die freie Nahrung (O(Hungrige × Nahrung)). Flutfüllungen wachsen quadratisch mit der Seitenlänge, sind aber selten.
6. **ASCII bei Tick 20.000** (Ausschnitte). Oberste Tunnel-Ebene z=14 (x 0–63, y 24–62) mit Lager vor dem Berg, Hallen, Tor aus Mauer, Tür und Mauer bei x=16 sowie den Treppen `>` beider Schächte:
```text
level z=14 tick=20000
.oooooooooooo..........::::#####################################
.oooooooooooo.........::::######################################
.oooooooooooo........::::#######################################
.ooooo$oooooo........:::########################################
.oooooooooooo.......:::#########################################
.oooooooooooo......:::#################***######################
.oooooooooooo......:###################*#****###################
.oooooooooooo.....:#################****##**####################
.oooooooooooo.....:#################*###########################
.oooooooooooo....:##############################################
.oooooooooooo....:##############################################
.oooooooooooo...:###############################################
.oooooooooooo...::##############################################
.fooooofooooo..:::##############################################
.foofffff@ooo..:::##############################################
.fffffffffooo..::###############################################
.fffffffffooo..::###############################################
.ffffffffff@o.:::###############################################
.fffffffff@@o.:::#####ooooooooo#################################
.fffffffffff@.::######ooooooooo#################################
..............::######o$ooooooo>################################
..............::######o$$oooooo#################################
..............::######ooooooooo#################################
f...............W.......................########################
ff..@...........+.......................>#######################
ff..............W.......................########################
..............::######.........#################################
..............::######.........#################################
..............::######.........#################################
..............::######.........#################################
..............::######.........#################################
..............::####################################*###########
...............::###################################*###########
...............:###################################**###########
...............:::#################################*############
...............:::#################################*############
```

Tiefste Schacht-Ebene z=2 (x 30–70, y 37–59): Halle mit Mauer- und Türreihe bei x=46, der Raum am Schachtfuß ist mit Steinen belegt (die `StairUp`-Kachel bei (40,48) liegt unter einem Stein):
```text
level z=2 tick=20000
#########################################
#########################################
#########################################
##################oooooooooooooooo#######
##################oooooooooooooooo#######
##################oooooooooooooooo#######
##################oooooooooooooooo#######
##################oooooooooooooooo#######
############oooooooooooooooooooooo#######
############oooooWoooooooooooooooo#######
############oooooWoooooooooooooooo#######
###########oooooo+oooooooooooooooo#######
############oooooWoooooooooooooooo#######
############oooooWoooooooooooooooo#######
############oooooooooooooooooooooo#######
##################oooooooooooooooo#######
##################oooooooooooooooo#######
###########*######oooooooooooooooo#######
###########*######oooooooooooooooo#######
########*****#####oooooooooooooooo#######
#########**#*############################
#########################################
#########################################
```

7. **Referenzen:** Jungle `0x5317cfd6e2e12024` (Tick 1.000) und `0x19eb3e87d333006d` (Tick 5.000) sind unverändert, der Jungle-Golden-Snapshot lädt, Dummy bleibt bei `0x20f21cbeee53c309`. Fortress-Golden-Hashes (Seed 42, Szenario, 2.000 Ticks): A* `0x0647c8823c28e430`, hierarchisch `0x000538f6ea0f8d1e`. Golden Snapshot `fortress-v1-small.bin`: `0x4b5fe3168d2245db` bei Tick 300, `0x87e72f66415229d4` bei Tick 601. Szenario bei Tick 20.000: `0xae5ee3d9cc6dbcc2`.
8. **Gefundene und behobene Fehler:**
   - Arbeiter verhungerten an der Inspektionsgrenze, weil verschüttete und unbearbeitbare Jobs die nächsten Kandidaten belegten. Der Index enthält jetzt nur bearbeitbare Jobs.
   - Baustellen warteten ewig auf liegende Steine. Steine auf Baustellen werden jetzt mit Vorrang weggeräumt, notfalls in die Nähe statt ins Lager.
   - Ablegen beim Aufgabenabbruch überbelegte Lagerplätze; die Invariante schlug zu Recht an. Abgelegt wird jetzt auf einem sicheren Nachbarfeld.
   - `nearest_open` prüfte bei wenigen Jobs O(r⁴) Zellen (2 ms pro Tick). Jetzt werden nur Schalen durchlaufen, kleine Indizes werden direkt gescannt.
   - Der Platzhaltertausch von `Assigner` allokierte einmal pro Tick.
   - `new()` panickte, wenn das Embark-Feld außerhalb der Welt lag. Jetzt kommt ein Config-Fehler.
   - Der Mauertest prüfte die Erreichbarkeit im selben Tick wie den Bau; sie wird aber erst zu Beginn des nächsten Ticks aktualisiert.
   - Shuffle-, Save/Load- und Erreichbarkeitstests fanden keine Abweichung. Mutationen der Erreichbarkeit (fehlende Flutfüllung, fehlende Vereinigung) lassen den Pflichttest scheitern.
9. **Eigene Entscheidungen und Abweichungen:**
   - Die Heuristik ist `20·dz + max(0, oktil − 10·dz)`, weil die vorgegebene mit Rampen überschätzt.
   - Rampen verbinden nur orthogonale Nachbarn. Graben ist orthogonal oder vertikal über passende Treppen möglich.
   - Die Pfad-Queue ordnet nach (Einreichungstick, ID), damit hohe IDs nicht verhungern. Die letzte Suche eines Ticks darf das Budget um höchstens ein Suchlimit überschreiten.
   - `AStar<N>` ist über den Knotentyp statt über die Topologie generisch. Kandidatensuchen sind auf `job_inspect_limit` (64) Prüfungen begrenzt.
   - Der Berg ist an feste Koordinaten gebunden: Größere Welten im Sweep bekommen mehr Ebene, und das Szenario bleibt gültig.
   - Snapshots speichern nur die Kacheln, die von der neu generierten Welt abweichen (39,7 kB bei Tick 20.000).
   - HPA* speichert die Kosten zwischen den Eingängen eines Chunks; die konkrete Verfeinerung wird chunk-lokal und deterministisch neu gesucht. Diagonal- und Rampenübergänge zwischen Chunks sind keine Eingänge, dafür greift der A*-Fallback.
   - Die Karawane bringt pro Bewohner eine Ration, mindestens 10, alle 1.000 Ticks.
   - Diagnosezähler werden nicht gespeichert; die logischen Zähler (gegraben, gebaut, Tote …) werden gespeichert und gehasht.
   - Das alte Replay-Format v1 wird nicht mehr gelesen.
10. **Vorbereitung für Etappe 4 (Flüssigkeiten, Temperatur, Einstürze):**
    - `ChunkedGrid3` braucht zusätzliche Kanäle pro Kachel (Füllstand, Temperatur) mit eigenen Versionszählern, damit Strömung nicht Erreichbarkeit und Weg-Caches invalidiert.
    - Aktive Chunk-Mengen und Dirty-Regionen, damit Zellautomaten nur wache Chunks rechnen. Dafür eine feste, layoutunabhängige Update-Reihenfolge (Morton-Chunks, Doppelpuffer oder Schachbrett-Phasen).
    - Gesammelte Kachel-Änderungen mit Konfliktauflösung, wenn mehrere Systeme dieselbe Kachel ändern.
    - Für Einstürze eine Stützberechnung als abgeleitete Struktur mit Union-Find wie bei der Erreichbarkeit, inklusive Test gegen vollständige Neuberechnung.
    - Bewegungsregeln mit Kosten oder Sperren durch Flüssigkeit.
    - Chunk-Diff-Snapshots, die den bereits vorhandenen Diff gegen die generierte Welt chunkweise ablegen.

## Etappe 4 – Viewer und Sandbox: Abschlussbericht

`scripts/ci.sh` lief mit Exit 0. Geprüft wurden fmt, striktes Clippy und je 289 Tests in Debug und Release (zusammen). Dazu kamen Dummy-, Jungle- und Fortress-Läufe mit Determinismus, Save/Load, Replays und `--max-step-allocs 0`, ein hierarchischer Lauf, Build und Tests des Viewers ohne Fenster sowie die `release-checked`-Langläufe. `cargo build --release -p fortress-viewer` war erfolgreich.

0. **Schritt 0**
   - **Gegenstandssuchen:** Nahrung, Steine für Baustellen und Belegungsprüfungen laufen über `sim_core::index::CategoryIndex` (Chunk × Kategorie, intrusive Listen ohne Allokation). Die Abfrage liefert das Minimum von `(Distanz, ID)` wie der vollständige Scan; ein Pflichttest vergleicht nach zufälligen Änderungen (4 Seeds) mit Brute Force. Die Lagerzählung und die Belegungsliste der Arbeitsphase sind Momentaufnahmen und keine Suchen; sie bleiben lineare Durchläufe pro Tick. Die Fortress-Golden-Hashes sind unverändert: A* `0x0647c8823c28e430`, hierarchisch `0x000538f6ea0f8d1e`, Golden Snapshot unverändert.
   - **Sweep ohne Verhungern:** siehe `BENCHMARKS.md`. Ursache der 81 Toten war ein Fehler: Spawn-Plätze auf unerreichbaren Klippen. Danach gibt es 0 Tote, und `needs` kostet konstant 5–6 ns pro Bewohner und Tick. `initial_food_per_resident` (Standard 0) skaliert die Startnahrung; die Karawane war bereits pro Bewohner konfigurierbar.
   - **„797 gegen 742“:** 797 TPS gehörten zum neueren Stand. Mit `scripts/bench-ab.sh` (5 abwechselnde Paare und Callgrind) ergaben sich −0,85 % TPS bei +0,076 % Instruktionen, also keine Regression.
   - **Generator-Fingerabdruck:** Nutzlast v2 mit `GENERATOR_VERSION` und Hash aller Generator-Eingaben. Bei Abweichung meldet das Laden „Weltgenerator geändert: Version 7 → 1“ (Test). v1-Snapshots laden mit Warnung über den neuen `sim-host`-Hook `load_warnings`.
   - **Lange Wege:** HPA* halbiert p99 (236 gegen 470 µs) und senkt die Spitzen von 2,96 auf 1,08 ms. Die Wege sind dabei 4,4 % teurer. Empfehlung: hierarchisch ab mehrchunkigen Wegen über etwa 100 Knoten. Der Standard bleibt A*.
   - **HPA-Hash-Plausibilität:** Die Subsystem-Hashes des hierarchischen Laufs sind unauffällig (`world 9e7c7670a709397d`, `residents b095222b177e4688`, `items 0180dacac8126d09`, `jobs b257712cfae35175`, `meta 330b7001f851fba1`). `world` ist in beiden Modi identisch, Tick 1.999 hat `0xecc32486df1cfee3`. Die drei führenden Nullen sind Zufall (Wahrscheinlichkeit 1/4.096).
   - **CI:** `.github/workflows/ci.yml` (Linux, Windows, macOS ARM; Hash-Vergleich der Plattformen; nächtliche `release-checked`-Läufe). actionlint meldet 0 Fehler (shellcheck war nicht installiert). Ohne Remote konnte der Workflow nicht ausgeführt werden; die Anleitung steht in `README.md`.
1. **Architektur:** `sim-io` übernimmt Container, Replays und Configs aus dem Runner; Formate, Meldungen und Exit-Codes sind unverändert. `fortress_sim::view` ist die rein lesende Schnittstelle; ein Pflichttest fragt sie in jedem Tick vollständig ab, und alle Hashes bleiben gleich. Im Viewer besitzt ein eigener Simulations-Thread die Simulation. Er wendet Commands an dem Tick an, an dem sie ausgeführt werden, zeichnet jede Sitzung als Replay auf und veröffentlicht zusammengeführte Chunk-Änderungen in ein Postfach mit einem Platz. Der UI-Thread spiegelt die Welt und zeichnet mit `macroquad`. Bedienung: `apps/fortress-viewer/README.md`.
2. **Overhead:** Die reine Step-Zeit im Viewer entspricht headless (19.658 gegen 19.831 TPS, p99 168 gegen 161 µs). Die Wanduhr-TPS liegen bei 10.742 gegen 11.090.
3. **Speichern:** 25–40 kB. Speichern dauert 15–23 ms, Laden 33 ms, ein Autosave 21–23 ms (alle 6.000 Ticks).
4. **Viewer-Replay im Runner:** Zwei Integrationstests (Skript aus UI-Aktionen bzw. Laden mitten in der Sitzung mit eingebettetem Snapshot) werden vom Runner mit `Replay OK` und gleichem End-Hash reproduziert. Ein echtes Fenster-Replay über 20.000 Ticks spielte im Runner mit `Replay OK` und `0xae5ee3d9cc6dbcc2` ab.
5. **Referenzen:** Jungle `0x5317cfd6e2e12024` und `0x19eb3e87d333006d`, Dummy `0x20f21cbeee53c309`, Fortress-Golden-Hashes beider Modi und alle Golden Snapshots sind unverändert. Der 20.000-Tick-Lauf des Szenarios bleibt `0xae5ee3d9cc6dbcc2`.
6. **Screenshots:** `apps/fortress-viewer/docs/tick5000-z14.png` (Tunnel-Ebene) und `tick5000-z5.png` (tiefe Halle), erstellt mit `--screenshot-at-tick 5000 … --exit-after-screenshot` unter Wayland/Xwayland.
7. **Entscheidungen und Abweichungen**
   - Replays mit Abschlussdatensatz `Finished` enden für die Wiedergabe beim Tick dieses Datensatzes, weil Client-Sitzungen ihr Ende beim Start nicht kennen.
   - `sim-host` erhält `min_snapshot_version` und `load_warnings` mit Defaults (CHANGELOG).
   - Das Lange-Wege-Szenario braucht eine eigene Config (48 Ebenen), weil die Standardwelt unter dem Tunnel nur 14 Ebenen hat.
   - Neue Fehlerkorrekturen ohne Hash-Änderung im Standardszenario:
     - Spawn-Plätze müssen erreichbar sein.
     - Ablagen auf vollen Lagern suchen weiter.
     - Lagerzonen über gestapelten Kacheln werden abgelehnt.
     - Spawns auf Lagerplätzen beachten deren Kapazität.
   - Das Postfach ersetzt einen Kanal mit fester Länge: Es fasst Updates zusammen und tauscht Puffer.
   - Der Text nutzt die in `macroquad` eingebaute Schrift.
8. **Beobachtungen beim Zuschauen** (nicht behoben)
   - Sobald das Lager voll ist, bleiben Steine dauerhaft in Tunneln und Hallen liegen (z=5 ist vollständig belegt), und Hunderte Transportjobs stehen offen, ohne bearbeitbar zu sein.
   - Untätige Bewohner sammeln sich im Lager und an der Lagerzone in der Halle; es gibt keine Freizeit- oder Rückzugsaktion.
   - Bewohner arbeiten große Grabflächen von einer Kante her ab und stehen dabei in Reihen auf derselben Kachelspalte (geteilte Kacheln).
   - Nahrung der Karawane stapelt sich auf wenigen Randkacheln.
   - Gebaute Mauern lassen sich nur über den Generatorvergleich von Naturfels unterscheiden; das Material verrät es nicht.

## Etappe 5 – Weltphysik: Abschlussbericht

`scripts/ci.sh` lief mit Exit 0: fmt, striktes Clippy, 317 Tests (Debug und Release zusammen), Dummy-, Jungle- und Fortress-Läufe sowie neu `scenario_flood`, `scenario_magma` und `scenario_cavein` über je 20.000 Ticks mit `--verify-determinism --verify-save-load --profile-phases --worst-ticks 10 --max-step-allocs 0 --budget-p99-us 10000`. `dig_in` lief mit denselben Optionen ebenfalls mit Exit 0.

1. **Regeln**
   - **Flüssigkeiten:** Sie liegen in einem eigenen `ChunkedGrid3<u8>`. Bits 0–6 enthalten Einheiten (16 pro Stand, höchstens 112 = Stand 7), Bit 7 markiert Magma.
     - Jede Kachel schlägt auf Basis des Phasenbeginns vor: zuerst nach unten fallen, sonst zu Nachbarn mit mindestens 2 Einheiten weniger. Die Restverteilung rotiert mit `(tick + x + y) % 4`.
     - Ziele werden in der Reihenfolge (Priorität, Quelle) bis zur Startkapazität bedient, Reaktionen danach.
     - Wasser auf Magma ergibt Obsidian (Magma-Kachel wird fest, Wasser verbraucht).
     - Quellen sind Aquiferflächen (`wet_sandstone`, alle 6 Ticks) und der Flusskopf. Die Mündung ist eine Senke. Einzelne Einheiten verdunsten nach zustandslosem Zufall.
   - **Topologie:** Die Markierung „unpassierbar“ im Kachel-Flag ist die einzige Kopplung an Bewegung. Sie wird gesetzt bei Wasser ≥ 4, Magma ≥ 1 oder Temperatur ≥ 80 und erst unterhalb der Schwelle wieder gelöscht (Hysterese: Wasser < 3, Temperatur < 70).
   - **Bewohner:** Wer ab Stand 4 untergetaucht ist, ertrinkt nach 150 Ticks; Bewohner fliehen aus Wasser und Hitze (`Task::Flee`).
   - **Temperatur:** Die Umgebungstemperatur folgt der Tiefe (`10 + (Tiefe − 1 − z)`). Gespeichert wird nur die Energieabweichung in aktiven Chunks, in gepoolten Doppelpuffern.
     - Der Fluss ist paarweise antisymmetrisch: `(T_p − T_q) · min(k) / 16` mit Ganzzahlabschneidung.
     - Freiliegendes Magma speist Energie ein; kleine Abweichungen (≤ 2) kehren zur Umgebung zurück.
     - Ab 80 Grad nehmen Bewohner Schaden. Ab 100 Grad kocht Wasser (1 Einheit pro Tick), ab 180 Grad entzündet sich Nahrung.
   - **Stütze:** Elemente sind feste Kacheln und Brückenböden; Anker liegen auf z ≤ 1.
     - Graben, Bauen und Einstürze stellen Prüfungen in eine `BudgetQueue` (20.000 Einheiten pro Tick, Suchgrenze 4.096, Wiederholung mit doppelter Grenze).
     - Ungestützte Kacheln fallen spaltenweise und werden zu Steinen am Landepunkt. Bewohner auf dem Fallweg sterben, Gegenstände werden mitgerissen.
2. **Szenarien:** TPS, p50, p99, schlimmste Ticks, Phasen und Schlafanteil stehen in `BENCHMARKS.md` („Etappe 5“). Kurz: `dig_in` 4.413 TPS, p99 626 µs; `flood` 4.569 TPS, 510 µs; `magma` 2.135 TPS, 862 µs; `cavein` 5.750 TPS, 276 µs. 85 % der Fluid-Chunks schlafen, alle Läufe haben 0 Step-Allokationen.
   - **Ereignisse:**
     - `flood`: 3 Ertrunkene.
     - `magma`: 112 Einheiten Magma zu Obsidian, 207 Einheiten verkocht, 3 Hitzetote, 1 Ertrunkener.
     - `cavein`: 1 Einsturz mit 16 Kacheln, 1 Toter.
     - `dig_in`: Sickerwasser im Raum auf z=8, 6 Ertrunkene.
3. **Nachweise** (Tests in `games/fortress-sim/src/tests.rs` bzw. `crates/sim-core`)
   - **Schlafen ist verhaltensneutral:** `sleeping_chunks_do_not_change_behavior` vergleicht 3.500 Magma-Ticks mit Fluiden, Wärme, Sieden und Obsidian; die Hashes sind in jedem Tick identisch.
   - **Erhaltung:** Die Bilanz `fluids_are_conserved_for_ten_thousand_flood_ticks` (Zählung = Summe = Anfang + Quellen − Senken − Verdunstung − Umwandlung − Verkochtes) stimmt in jedem Tick. Die Invariante prüft dies auch im Runner.
   - **Energiebilanz:** `heat_balances_exactly_and_returns_to_ambient`.
   - **Stütze:** `targeted_checks_match_full_recomputation` (gezielt = vollständig, 4 Seeds × 600 Runden).
   - **Einsturz:** `an_undermined_slab_caves_in_and_becomes_stone`.
   - **Shuffle und Golden:** Shuffle-Test und Golden-Hash pro Szenario (`flood_…`, `magma_…`, `cavein_is_deterministic_shuffle_neutral_and_golden`).
   - **Save/Load an Übergängen:** `save_load_survives_flowing_water_heat_and_sleep_transitions` und `save_load_survives_a_cave_in` laden bei jedem Schlaf- oder Weckwechsel, bei ausstehenden Stützprüfungen und während des Einsturzes neu.
   - **Erreichbarkeit:** `a_flooded_room_hides_jobs_until_it_is_pumped_out`: Ein Grabjob in der gefluteten Halle bleibt 300 Ticks offen und wird nach dem Abpumpen erledigt.
   - **Allokationen:** `phases_do_not_allocate_after_warmup` misst jetzt auch `hazards`, `fluids`, `temperature`, `support` und `collapse` im Magma-Szenario.
4. **Sweep:** Bei `river_width` 1 → 12 vervierfacht sich die Wassermenge (16.358 → 62.806), die aktiven Kacheln steigen von 3.531 auf 4.574. Die Kosten pro aktiver Kachel wachsen in `fluids` nur von 71 auf 79 ns, die Phase skaliert also mit den aktiven Kacheln und nicht mit dem Wasser.
5. **Goldens und Version:** Snapshot-Version 3 (Mindestversion 3), Nutzlast `FORTSNP3`, `GENERATOR_VERSION` 2.
   - **Golden Hashes:**
     - A* `0x6eccbc989aa504b0`
     - hierarchisch `0xfa068482edbf7def`
     - `flood` (2.500 Ticks) `0xc961da72edd69363`
     - `magma` (2.500) `0x89290525a6c6150a`
     - `cavein` (1.500) `0xd0de6349237be0be`
   - **Golden Snapshot:** neu `tests/fixtures/fortress-v3-small.bin` mit `0x7eca89da59972239` bei Tick 300 und `0x4a2b634c2f3e3429` bei Tick 601.
   - **Alte Snapshots:** v1/v2 werden mit „Weltgenerator geändert: Version 1 → 2“ abgelehnt, weil die Welt ohne Physik-Generator nicht rekonstruierbar ist (dokumentierter Bruch, CHANGELOG).
   - **Unverändert:** Jungle `0x5317cfd6e2e12024` (Tick 1.000) und `0x19eb3e87d333006d` (Tick 5.000), Dummy `0x20f21cbeee53c309`.
6. **Gefundene und behobene Fehler**
   - Bei ganzen Ständen blieb Wasser an Gefällen stehen; daher 16 Einheiten pro Stand.
   - Die Reaktionsreihenfolge konnte Masse erzeugen. Abhilfe: zwei Durchläufe und Begrenzung auf den aktuellen Quellstand.
   - Schlafneutralität scheiterte bei Tick 1.892, weil Kachel-Änderungen und Aquiferquellen schlafende Chunks erst im Folgetick weckten. Beide wecken jetzt vor `prepare`.
   - Bauen auf Wasser schloss Flüssigkeit in Fels ein. Bauen wartet jetzt auf eine trockene Kachel, der Flusskopf füllt nur offene Kacheln.
   - Hitze tötete alle 25 Bewohner. Heiße Kacheln sind jetzt unpassierbar, eingeschlossene Bewohner machen einen Schritt hinaus.
   - Wasser verkochte vor dem Magma, sodass kein Obsidian entstand; daher `BOIL_UNITS` = 1.
   - Die Stützprüfung lief an bereits entschiedenen Nachbarn weiter, und Bauen konnte ungestützte Elemente erzeugen (`note_added`).
   - Die Stützprüfung allokierte in jedem Tick (`mem::take` auf den Suchpuffer). Gefunden über `--max-step-allocs 0`.
   - Schwellen-Flattern: Wasserstände an der Schwelle schalteten die Markierung in jedem Tick um, sodass `connectivity` Spitzen bis 11 ms hatte. Die Hysterese halbiert p99.
7. **Entscheidungen und Abweichungen**
   - **Druckmodell: keines.** Wasser fällt und gleicht sich seitlich aus, steigt aber nicht in kommunizierenden Röhren oder U-Bögen auf. Druck bräuchte eine begrenzte Suche nach der Pegelhöhe verbundener Körper; das passt nicht in das lokale Transfermodell und wurde zurückgestellt.
   - Fluide stehen in einem eigenen Gitter statt im `Tile`, damit Fluidänderungen keine Chunk-Versionen der Topologie auslösen. Der Viewer bekommt eigene Fluid-Versionen.
   - Wärme wird nur als Abweichung gespeichert. Chunks ohne Abweichung geben ihren Pool-Platz frei. Ist der Pool erschöpft (192 Chunks), wird nicht weiter erwärmt; das wird gezählt (`heat_pool_exhausted`, bisher 0).
   - **Snapshot:** gespeichert werden Fluid- und Energie-Differenzen zur generierten Welt, beide Bilanzen und die Stütz-Warteschlange. Aktive Mengen, Aquifer- und Wärmequellen werden beim Laden neu aufgebaut; laut Neutralitätstest ist das ohne Einfluss. Die Rotation ergibt sich aus dem Tick.
   - Anker sind z ≤ 1, weil das Magmameer auf z=0 liegt.
   - Herabfallendes Material wird zu Steinen statt zu fallenden Kacheln. Flüssigkeiten in eingestürzten Kacheln werden verdrängt (`entomb`).
   - Ein Toter durch Einschließen zählt als `Collapse`.
   - Pumpen gibt es nur im Test (`pump`, gezählt als Senke); ein spielbares Pumpen fehlt.
   - **Kosten:** Der Fluss läuft dauernd und hält 21 Chunks wach. Der Standardlauf sinkt damit von 13.906 auf 4.413 TPS (innerhalb des Budgets). Abschalten mit `physics.river = false`.
8. **Was für Feuer, Gas und Dampf fehlt**
   - **Gasgitter:** Gas braucht ein drittes Feld mit Dichte pro Kachel und Ausbreitung in alle sechs Richtungen, Auftrieb statt Fallen. Die Infrastruktur (`ActiveChunkSet`, Doppelpuffer, Bilanz) ist dafür vorhanden.
   - **Dampf:** Verkochtes Wasser verschwindet heute als Bilanzposten `boiled`. Mit Gas würde es Dampf, der aufsteigt, abkühlt und kondensiert; die Bilanz wäre dann über zwei Felder geschlossen.
   - **Feuer:** Brennstoff und Brennzustand als Material- bzw. Kacheleigenschaft (heute entzündet sich nur Nahrung und wird zerstört), dazu Sauerstoff aus dem Gasfeld, Wärmeabgabe über `add_energy` und Rauch als Gas. Die Ausbreitung lässt sich über die vorhandene zustandslose Zufallsquelle pro Kachel regeln.
   - **Holz und Bauteile:** Brennbare Bauteile und Brücken, die bei Brand einstürzen (Kopplung an `note_removed`).
   - **Druck:** Für Gas und Wasser gleichermaßen fehlt ein Druckmodell (siehe 7).
