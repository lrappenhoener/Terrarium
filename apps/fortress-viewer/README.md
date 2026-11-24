# Fortress-Viewer

Grafischer Client und Sandbox für `fortress-sim`: Draufsicht auf eine Ebene, Werkzeuge per Maus, Untersuchen, Geschwindigkeit, Speichern und Laden. Der Viewer verändert den Simulationszustand nie direkt; er liest über `fortress_sim::view` und schickt `FortressCommand`s. Jede Sitzung wird als Replay in `sessions/<zeitstempel>.simr` aufgezeichnet und lässt sich im Runner reproduzieren:

```sh
cargo run --release -p sim-runner -- --replay sessions/session-<zeit>.simr
```

## Start

```sh
cargo run --release -p fortress-viewer                       # Startbildschirm
cargo run --release -p fortress-viewer -- --seed 42 \
    --scenario games/fortress-sim/data/scenario_dig_in.toml  # direkt mit Szenario
cargo run --release -p fortress-viewer -- --load sessions/quicksave.snap
```

Weitere Argumente: `--config <toml>`, `--speed pause|1|2|5|max`, `--base-tps <n>` (Standard 20 Ticks/s bei 1×), `--level <z>`, `--session-dir <dir>`, `--screenshot-at-tick <tick> <png>` mit `--exit-after-screenshot` (pausiert die Simulation genau beim Tick), `--exit-at-tick <tick>` (gibt TPS und p99 aus). Unter Linux lädt `miniquad` zur Laufzeit `libGL`, `libX11`, `libXi` und `libxkbcommon`.

## Bedienung

| Eingabe | Wirkung |
| --- | --- |
| W A S D, Pfeiltasten | Kamera verschieben |
| rechte oder mittlere Maustaste ziehen | Kamera verschieben |
| Mausrad | Zoom zum Mauszeiger |
| Bild↑ / `<` | Ebene höher |
| Bild↓ / `>` | Ebene tiefer |
| Leertaste | Pause umschalten |
| N | Einzelschritt (pausiert) |
| 0 / 1 / 2 / 3 / 4 | Pause / 1× / 2× / 5× / Maximum |
| G | Werkzeug Graben, M wechselt Modus: Graben, Treppe hoch+runter, runter, hoch, Rampe |
| Z | Werkzeug Lagerzone |
| B | Werkzeug Bauen, M wechselt Mauer, Boden, Treppe, Tür |
| C | Werkzeug Aufheben (Designationen und Lagerzonen entfernen) |
| I | Werkzeug Untersuchen |
| linke Maustaste ziehen | Rechteck für das Werkzeug; Ebenenwechsel während des Ziehens erweitert es über mehrere Ebenen |
| Klick mit Untersuchen | Bewohner, Gegenstand oder Kachel anzeigen; Bewohner werden ausgewählt |
| F | ausgewähltem Bewohner folgen |
| Esc | Auswahl aufheben |
| F1 / F2 / F3 / F4 | Overlays Jobs / Pfad / Erreichbarkeits-Regionen (Debug) / Chunk-Änderungen (Debug) |
| F6 / F7 / F8 | Overlays Temperatur-Heatmap / aktive Chunks (blau Flüssigkeit, rot Wärme) / ausstehende Stützprüfungen (einsturzgefährdet, Debug) |
| F10 | Overlay Felder und Bäume (Standard an): Bäume dunkelgrün, Feldfrüchte hellgrün, reif gelb, Feldparzellen braun umrandet |
| K / H / T | Werkzeuge Werkstatt platzieren (Klick setzt die Ecke) / Feld anlegen / Bäume fällen; M wechselt Werkstattart bzw. Feldfrucht |
| O | Auftragsliste ein/aus |
| R / X | Auftrag (5 ×) für das nächste Rezept der untersuchten Werkstatt / neuesten Auftrag aufheben |
| L / P | Kategoriefilter / Priorität der untersuchten Lagerzone weiterschalten |
| F5 / F9 | Speichern / Laden (`sessions/quicksave.snap`) |
| Strg+Q | Beenden |

Wasser (blau) und Magma (orange) erscheinen immer, ab einer Kachelgröße von 14 Pixeln mit Ziffer für den Stand 1–7.

Autosave schreibt alle 6.000 Ticks (5 Minuten bei 1×) `sessions/<sitzung>-autosave.snap`. Speicherdauer und Größe erscheinen im HUD.

## Architektur

- `session.rs`: Der Simulations-Thread besitzt die Simulation exklusiv. Pro Tick holt er Commands aus einem Kanal, wendet sie beim nächsten `step` an, zeichnet sie im Replay auf und speichert oder lädt zwischen zwei Ticks.
- `frame.rs`: Er veröffentlicht höchstens alle 15 ms ein Frame-Update in ein Postfach mit genau einem Platz. Geänderte Chunks (über Chunk-Versionen) sammeln sich, Entitätslisten werden ersetzt, und Puffer werden zwischen den Threads getauscht statt neu angelegt.
- Der UI-Thread hält eine Spiegelkopie der Welt, aktualisiert nur geänderte Chunks und zeichnet unabhängig vom Tick-Takt.
- `camera.rs`, `tools.rs`, `script.rs` sind fensterunabhängig und unit-getestet. `render.rs` und `main.rs` enthalten alles, was `macroquad` betrifft.
- **Grafikbibliothek:** `macroquad` 0.4 ohne Standard-Features (kein Audio). Gründe: kurze Kompilierzeit, einfache API, ausreichend für eine Kachelansicht. Alternativen: `bevy` (deutlich schwerer, ECS widerspricht der eigenen Datenhaltung), `wgpu` direkt (viel Eigencode), `egui`/`eframe` (gut für Panels, schwächer für Kachelgrafik).

Das Untersuchen-Panel (I, Klick) zeigt auf einer Kachel Werkstatt (Art, Bauzustand, Auftrag), Lagerzone (Priorität, Filter, Behälter), Pflanzen (Wachstum) und Gegenstände mit Menge, Material, Qualität und Abnutzung samt Behälterinhalt. Werkstätten erscheinen violett mit Namen, halbtransparent solange sie im Bau sind. Beispiel: `docs/industry-tick12000-z14.png` (Industrie-Szenario, Tick 12.000).
