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
