# Dashboard: visuelles Konzept (Vorschlag)

Stand 2026-11-01. Rein optisch; Funktionen, Datenmodell und API bleiben. Bezug: `src/web/style.css`, `src/web/index.html`, `src/web/app.js`; Designsystem SPECTRUM (`~/.claude/skills/spectrum-design/DESIGN.md`).

## 1. Diagnose: warum es „naja“ aussieht

Die Tokens stimmen, die Hierarchie fehlt. Konkret:

1. **Drei flache Chrome-Zeilen.** Header, Section-Strip und Tab-Zeile sind drei Reihen aus 0.68rem-Mono-Uppercase. Nichts ist groß, nichts ist klein. Das Auge findet keinen Einstieg. Die Stat-Zeile `76 OPEN 3 RUNNING 53 SHARED FILES` ist das wichtigste Signal und zugleich das kleinste Element der Seite.
2. **Rot ohne Bedeutung.** Auf dem Board trägt fast jede Karte zwei rote Badges (`9 overlaps`, `with running L-38, L-54, L-56`). Bei 40 roten Badges heißt Rot nichts mehr. Dazu der 3px-Prioritätsstreifen links (critical = rot) und rote Pips. Alarm-Müdigkeit auf dem ersten Blick.
3. **Linien statt Tonstufen.** `border-left` auf Karten und Track-Zeilen, `border-top` zwischen Swimlane-Zellen, Logbook-Zeilen und Track-Zeilen. Verstößt gegen die No-Line-Regel und wirkt wie generisches Kanban.
4. **Karten liegen zu hoch.** Karten sind `surface-container-high` auf `surface-container-low`-Spalten auf `lowest`-Canvas: drei Stufen Kontrast für eine Karte. Dadurch flimmert die Fläche, nichts ist ruhig.
5. **Pills für alles.** Tabs, Rollenfilter, Group-by-Schalter und Logbook-Filter sind alle dieselbe Pill. Navigation und Zustand sind optisch nicht unterscheidbar. Laut Corner-Doctrine drücken Pills *Zustand* aus, nicht Struktur.
6. **Textwände.** Activity: 30 Zeilen `E-xx ← E-yy · engine, nobody on it` als Fließtext mit vier Farben. Tracks: die „Jetzt“-Spalte ist ein grüner Absatz. Keine Spalten, keine Ausrichtung, keine Tabellenzahlen.
7. **Fonts fehlen.** Loopback-only, keine externen Fonts, also fällt Space Grotesk und Manrope auf `system-ui` zurück. Die dritte SPECTRUM-Säule (Editorial Precision) ist damit komplett weg. Nur die Mono-Schrift greift.
8. **Leere Rechtshälften.** Review, Questions, Leases sind linksbündige Listen mit `max-width: 960px` und nichts rechts davon. Leases laufen horizontal über (Progress-`pre` ohne Umbruch).

## 2. Leitidee: ein Instrument, keine Seitensammlung

Airlock zeigt, *wer gerade in der Schleuse ist* und *was dabei kollidiert*. Das ist auf jedem Tab relevant, also gehört es über die Tabs, nicht hinter einen davon.

```
[A] AIRLOCK  STUDIO // v0.1                                 project openshr   ◉ LIVE
──────────────────────────────────────────────────────────────── (lowest → surface)
 76 open   3 running   1 in review   3 open questions   53 shared files
                                          IN THE AIRLOCK NOW
                                          ◉ L-38  Campaign captures…   lead-1   157 min
                                          ◉ L-54  Figure records…      lead-1   187 min
                                          ◉ L-56  Trace probe format…  lead-1   714 min
──────────────────────────────────────────────────────────────── (surface)
 TRACKS  ACTIVITY  BOARD  STAGES  FILES  REVIEW 1  QUESTIONS 3  LEASES 3     [filters]
 ──────
 (view)
```

Das **Hero-Band** ist das eine mutige Element. Alles darunter wird ruhiger, damit es trägt.

### 2.1 Hero-Band (ersetzt Section-Strip)

- Links: Stat-Lockup nach DESIGN.md §5.4. Zahl `data-lg` (2.25rem Mono, `tnum`), Label `body` gedämpft. Farbe nur, wo sie Bedeutung hat: `running` primary, `open questions` secondary, alles andere `on-surface`.
- Rechts: **Lease-Reihen** der laufenden Tasks. Pulse-Dot (echter SPECTRUM-Pulse mit zwei Ringen, §6.10), ID, Titel abgeschnitten, Session in secondary, Restminuten rechtsbündig, darunter eine 2px-Naht als Restzeit-Balken (primary; tertiary unter 15 min, also im letzten Viertel eines 60-Minuten-Lease-Fensters). Das ist der Linear-Progress aus §6.11, kein Pill.
- Hintergrund `surface-container-lowest`; der Übergang zu `surface` trennt Hero und Inhalt. Keine Linie.
- `● LIVE` wandert in den Header rechts neben den Projektnamen. Der Header bleibt sonst wie er ist (`STUDIO // v0.1` unverändert).

### 2.2 Navigation: Tabs sind Struktur, Filter sind Zustand

- Tabs als Mono-Uppercase-Text mit **2px-primary-Unterstrich** unter dem aktiven Tab (Positionsanker wie Bottom-Nav in §5.6). Zähler (`REVIEW 1`) in `on-surface-dim`, beim aktiven Tab primary.
- Filter bleiben Pills (Zustand) und sitzen rechts in derselben Zeile. Die Track- und Prioritäts-Selects werden Pills mit `▾`. Auf Tabs ohne Filter ist die rechte Seite leer.
- Auf schmalen Breiten ist die Filtergruppe zu breit (im Mock sichtbar). Vorschlag für die Umsetzung: Track/Prio/Group in ein „Filter ▾“-Popover; Rollen bleiben sichtbar.

### 2.3 Farbbudget

| Signal | Farbe | Wo |
|---|---|---|
| läuft gerade | primary + Pulse | Lease-Reihen, Karten-ID, Spaltenkopf „In progress“, Track-Bar-Segment |
| echte Kollision (Overlap *mit laufendem* Task) | error, Badge `⚠ running L-38, L-54` | Karte, Files-Zelle |
| Overlap mit nicht laufenden Tasks | neutral, Badge `⧉ 9` | Karte |
| Touch-Liste fehlt | neutral mit Ghost-Rand, `? no touch list` | Karte |
| blockiert durch | neutral, `← L-56` | Karte |
| Session | secondary | überall, wo ein Session-Name steht |
| gedimmte Zähler, Nullwerte, Platzhalter | `on-surface-dim` (#555555, neues Token) | Tab-Zähler, Mini-Stats mit 0, `idle` |
| Priorität | Pips in Prio-Farbe (critical rot, high orange, normal cyan, low grau), **kein** Streifen, **kein** Chip auf der Karte | Karte; Chip nur im Track-Kopf und Drawer |
| Kurs-Warnung, Lease fast abgelaufen, `nobody on it` bei high/critical | tertiary | Tracks, Hero, Activity |

Effekt auf dem openshr-Board: Rot erscheint auf 3 bis 6 Karten statt auf 40. Dann heißt Rot wieder „hier kollidiert gerade etwas“.

### 2.4 Flächen

- Canvas `surface` (#131313), Header und Hero `lowest`.
- Spalten, Panels, Wells: `surface-container-low`.
- Karten: `surface-container`, hover `high`. Radius `md`. Keine Streifen, keine Schatten.
- Laufende Karte: Ghost-Ring primary 45% innen (`inset 0 0 0 1px`). Gewählte Karte: 2px secondary.
- Listenzeilen (Logbook, Blocker, Sessions): **keine `border-top`**, stattdessen Zebra `surface-container-low` / `lowest` oder nur Abstand.

### 2.5 Typografie

- Space Grotesk 600 für Track-Namen (1.25rem) und Drawer-Titel (1.4rem). Manrope für Kartentitel und Fließtext. JetBrains Mono für IDs, Zeiten, Zähler, Sessions.
- Tabular numerals global (ist schon gesetzt).
- **Entscheidung nötig:** Fonts als woff2 in die Binary (`include_bytes!`, ausgeliefert unter `/fonts/…`, Latin-Subset ≈ 4 × 20 KB, alle drei OFL/Apache) oder bei System-Fonts bleiben. Ohne die Fonts bleibt ein Drittel des Konzepts Theorie. Empfehlung: einbetten.

## 3. Pro Tab

**Tracks.** Dreispaltig: `Prio-Chip + Name (display) + Beschreibung` | `Bar + Mini-Stat-Lockup (Zahl fett, Label gedämpft, Nullwerte gedimmt)` | `Jetzt`. Rechts laufende Tasks als Reihe `◉ ID Titel… Session · min`, sonst `idle` gedimmt plus „last done“. Zeilen abwechselnd `low` auf `surface`, keine Linien. Kurs-Warnung in tertiary unter der Bar.

**Board.** Wie in 2.3/2.4. Spaltenköpfe: Label + Zähler als Lockup, „In progress“ in primary. Karte: `ID + Pips` / Titel / Meta `role · track · session · min` / Tags. Group-by-Track: Trackköpfe als linke Rail mit Chip + Name, Zellen ohne `border-top`.

**Stages.** Dieselben Karten. Pfeile nur sichtbar, wenn eine Karte gewählt ist oder beim Hover; Standard `outline-variant` 25%, hot = secondary. Die Hinweiszeile oben wird ein Empty-State-Satz im Mono-Label, nicht ein Kasten.

**Files.** Matrix bleibt. Zellen: Treffer neutral (`surface-container-high` Punkt), Rot nur wenn ein *laufender* Task die Datei hält, Grün wenn der gewählte Task sie hält. Datei-Zähler rechts neben dem Pfad in `on-surface-variant`, Rot nur über einem Schwellwert (z. B. ≥ 10 offene Tasks). Spaltenköpfe sticky.

**Activity.** Drei Zonen. Oben links „Needs attention“ als Panel mit Glyph-Spalte (`✕` error, `▲` tertiary), Session secondary, Text, Zeit rechts Mono. Oben rechts „Claimed“ und „Blocked · waiting on“ als ausgerichtete Zebra-Tabelle; darunter „Sessions“ als Tabelle (`Session Rolle MCP CLI REST zuletzt`), deren Zeilen zugleich der Session-Filter des Logbooks sind (gewählte Zeile mit secondary-Ring); `nobody on it` gedimmt, tertiary nur bei high/critical-Track. Logbook darunter mit Filter-Pills, Zeilen ohne Linien, Fehlerzeile mit rotem Hauch.

**Review und Questions.** Zweispaltig nach §5.5 Split-Panel: links die Liste (ID, Topic/Datei, Fragesteller, Alter, kompakt), rechts der gewählte Eintrag mit Antwortfeld. Die 960px-Spalte mit leerem Rest entfällt. Formulare: Textarea ohne Rand, Caret primary, Fokus = Ghost-Rand primary + Glow.

**Leases.** Dieselben Lease-Reihen wie im Hero, nur mit vollem Titel, Progress-Notiz umbrochen in einem `pre`-Block auf `lowest`, und den Buttons (`Release`, `Force release`) als Ghost-Buttons rechts. Kein horizontaler Überlauf mehr.

**Drawer.** Glas (`surface-container-high` 80% + 16px Blur), sticky Kopf mit `CLOSE`, Titel in Space Grotesk, Read/Touch als zwei Spalten, Overlap-Tags neutral, Kollisionen rot. `DO` und `VERIFY` als `pre` auf `lowest`.

## 4. Was nicht Teil des Konzepts ist

- Keine neuen Funktionen, keine Änderung an Routen, API oder Datenformen.
- Keine Animationen außer Pulse, Hover (120 ms) und Drawer (250 ms). Kein Einblenden von Karten.
- Keine Illustrationen in Empty States.

## 5. Umsetzung in Schritten (je ein PR)

1. **Shell:** Header, Hero-Band, Tab-Unterstrich, Filter-Cluster. Fonts einbetten (falls ja).
2. **Karten und Farbbudget:** Board, Stages, Group-by-Track. Linien raus.
3. **Tracks und Activity:** Tabellenlayout, Zebra, Glyph-Spalte.
4. **Files, Leases, Drawer.**
5. **Review und Questions** als Split-Panel (einziger Schritt mit Layoutänderung über CSS hinaus).

Jeder Schritt wird mit `airlock serve` gegen openshr geprüft; `node --check src/web/app.js` vor jedem Commit.

## 6. Entscheidungen (Peter, 2026-11-01)

1. Fonts einbetten: **ja** (woff2 in der Binary, `/fonts/…`).
2. Review/Questions als Split-Panel: **ja**.
3. Hero-Band: **immer sichtbar**, begrenzt auf drei Lease-Reihen, „+N more“ darüber hinaus.

Nav-Zeile darf nie breiter als der Viewport werden: Tabs scrollen in sich, Filter-Cluster bricht um (Schritt 1). Unter ~1100px wandern Track/Prio/Group in ein „Filter ▾“-Popover (Schritt 2, zusammen mit den Board-Filtern).

## 7. Mock

Wegwerf-Mock mit echten openshr-Daten (Tracks, Board, Activity), nicht im Repo:
`/tmp/claude-1000/-home-peter-Projects-airlock/5a729418-6d39-45f9-a0ab-4a78994ecebf/scratchpad/mock.html`
(lädt Fonts von Google, nur für den Mock).
