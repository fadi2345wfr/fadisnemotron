# Fadis Nemotron

Desktop-Agent und Chat für **NVIDIA Nemotron 3.5 Super VL**, geschrieben in Rust. Läuft als normale Mac-App; die DMG ist für Intel-Macs gebaut.

Öffnest du einen Projektordner, arbeitet Nemotron darin wie Google Antigravity oder Claude Code: Er liest und durchsucht die Dateien selbst, ändert sie direkt und führt Befehle aus (Build, Tests). Ohne Projektordner ist es ein normaler Chat.

Nemotron 3.5 Super VL ist NVIDIAs offenes Multimodal-Modell (veröffentlicht am 15. Oktober 2026): 120 Milliarden Parameter, davon 12 Milliarden aktiv pro Token, hybride Mamba-Transformer-MoE-Architektur, bis zu 1 Million Token Kontext. Es versteht Text, Bilder, Dokument-Seiten und Videos und kann Werkzeuge aufrufen.

![Agent-Modus: Nemotron liest, ändert und prüft Dateien (Antwort von einem Test-Server)](docs/screenshot.png)

## Agent-Modus

1. **📁 Projekt öffnen** klicken (oder ⌘O, oder den Ordner ins Fenster ziehen).
2. Schreiben, was passieren soll, z. B. „Bau das Projekt und behebe die Fehler“ oder „Füge in `src/app.rs` einen Dunkelmodus-Schalter hinzu“.
3. Nemotron arbeitet in Schritten. Jeder Schritt erscheint als Karte im Chat:
   - 📂 Ordner ansehen, 🔎 Dateien finden, 📄 Datei lesen, 🔍 Inhalte suchen – ohne Nachfrage
   - ✏ Datei bearbeiten, 📝 Datei schreiben – mit Diff (+/− Zeilen)
   - ▶ Befehl ausführen – mit Exit-Code und Ausgabe
4. Rechts in **Änderungen** steht jede geänderte Datei mit **Diff** und **↶ Rückgängig** (setzt die Datei auf den Stand vor Nemotron zurück; neu angelegte Dateien werden gelöscht).

Was Nemotron ohne Nachfrage darf, stellst du rechts ein:

| Einstellung | Dateien ändern | Befehle |
|---|---|---|
| Vor jeder Änderung fragen | fragt, mit Diff-Vorschau | fragt |
| Dateien selbst ändern, vor Befehlen fragen (Standard) | sofort | fragt |
| Alles selbst machen | sofort | sofort |

Unabhängig davon gilt:
- Nur lesende Befehle (`git status`, `git diff`, `git log`, `ls`, `cat` …) laufen immer ohne Nachfrage.
- Riskante Befehle fragen **immer** nach, mit Warnhinweis: `sudo`, `rm -rf` außerhalb des Projekts, `git push`, `git reset --hard`, `git clean`, `curl … | sh`, rekursives `chmod`/`chown`, `dd`, `shutdown` usw.
- Pfade außerhalb des Projektordners (auch über `..` oder symbolische Links) und Dateien in `.git/` sind gesperrt.
- Ein Platzhalter wie `// ... rest of the code ...` beim Überschreiben einer Datei wird abgelehnt, damit nichts verloren geht.
- Befehle laufen im Projektordner in deiner Login-Shell (damit `cargo`, `npm`, `swift` usw. gefunden werden), mit Zeitlimit; **⏹ Stop** oder Esc beendet sie samt Unterprozessen.

Damit Nemotron wirklich ändert statt nur Beispielcode zu zeigen, bekommt er klare Anweisungen; zeigt er trotzdem einen größeren Codeblock ohne Änderung, fordert die App ihn einmal auf, die Änderung selbst einzutragen. Liegt im Projekt eine `AGENTS.md` oder `CLAUDE.md`, liest Nemotron sie als Projektregeln mit.

## Was die App sonst kann

- Antworten erscheinen Wort für Wort; den Denkprozess siehst du live, er klappt zu, sobald die Antwort oder ein Werkzeug kommt.
- Bilder (PNG, JPEG, WebP, GIF, BMP) und Videos (MP4, MOV, WebM) anhängen: per Büroklammer, per Drag & Drop oder als Link. Große Bilder werden auf 1536 px verkleinert, Handyfotos richtig gedreht.
- Markdown mit Tabellen und Code-Hervorhebung, Kopieren, „Neu generieren“, Export als Markdown.
- Funktioniert mit der NVIDIA-Cloud oder einem eigenen Server (vLLM, NIM, SGLang).

## Installation auf dem Mac (Intel)

1. DMG herunterladen:
   - Unter **Actions → macOS DMG (Intel)** den neuesten erfolgreichen Lauf öffnen und unten das Artefakt `FadisNemotron-intel-dmg` laden, oder
   - bei einem Release (Tag `v…`) die Datei `FadisNemotron-<Version>-intel.dmg` laden.
2. DMG öffnen und **Fadis Nemotron** in den Ordner **Programme** ziehen.
3. Erster Start: Die App ist nicht von Apple notariell beglaubigt, deshalb blockiert macOS sie beim ersten Öffnen.
   - macOS 15 und neuer: App einmal öffnen, dann **Systemeinstellungen → Datenschutz & Sicherheit → „Dennoch öffnen“**.
   - macOS 14 und älter: Rechtsklick auf die App → **Öffnen** → **Öffnen**.
   - Alternativ im Terminal: `xattr -dr com.apple.quarantine "/Applications/Fadis Nemotron.app"`

Voraussetzung: macOS 10.15 (Catalina) oder neuer.

## API-Schlüssel

1. Kostenlosen Schlüssel auf [build.nvidia.com](https://build.nvidia.com/settings/api-keys) erstellen (beginnt mit `nvapi-`).
2. In der App **⚙** öffnen, Schlüssel eintragen, **Verbindung testen**, **Speichern**.

Die Einstellungen liegen in `~/Library/Application Support/de.fadi.FadisNemotron/settings.json`, nur für deinen Benutzer lesbar. Ist das Feld leer, nimmt die App `NVIDIA_API_KEY` aus der Umgebung.

Standardwerte:

| Einstellung | Wert |
|---|---|
| Server | `https://integrate.api.nvidia.com/v1` |
| Modell | `nvidia/nemotron-3.5-super-vl-120b-a12b` |
| Temperatur / Top-p | 1.0 / 0.95 (NVIDIA-Empfehlung) |
| Max. Tokens | 32768 je Schritt (Denken und Antwort zusammen; NVIDIA-Cloud erlaubt bis 131072) |
| Max. Schritte | 60 Anfragen pro Nachricht, danach pausiert der Agent („weiter“ schreiben) |

### Eigener Server

Läuft Nemotron bei dir mit vLLM, trag als Server z. B. `http://localhost:8000/v1` ein und als Modell den Namen aus `--served-model-name` (in NVIDIAs Anleitung `nemotron-3.5-super`). Für den Agent-Modus braucht vLLM `--enable-auto-tool-choice --tool-call-parser qwen3_coder --reasoning-parser nemotron_v3`. Fehlen die Parser, liest die App Werkzeugaufrufe und `<think>`-Blöcke trotzdem aus dem Text.

## Tipps

- **Videos:** MP4, 1–2 Minuten. Große Dateien gehen als Base64 mit jeder Nachricht mit. Wenn der Server sie ablehnt, das Video als öffentlichen Link anhängen (🔗).
- **PDFs:** Das Modell nimmt keine PDFs. Seiten als PNG exportieren und die Bilder anhängen.
- **Leere oder abgeschnittene Antwort:** Das Denken verbraucht Tokens. „Max. Tokens“ erhöhen oder den Denkmodus ausschalten.
- **Tastenkürzel:** Enter senden, Shift+Enter neue Zeile, Esc stoppt, ⌘N neuer Chat, ⌘O Projekt öffnen, ⌘, Einstellungen.
- **Start mit Projekt:** `open -a "Fadis Nemotron" --args ~/Projekte/meins` öffnet den Ordner gleich als Projekt.
- **Alter Mac ohne Metal oder VM:** `FADIS_RENDERER=glow open -a "Fadis Nemotron"` nutzt OpenGL statt Metal.

## Selbst bauen

Auf einem Mac mit [Rust](https://rustup.rs):

```sh
./scripts/build-dmg.sh                               # Intel-DMG in dist/
TARGET=aarch64-apple-darwin ./scripts/build-dmg.sh   # Apple-Silicon-DMG
```

Das Skript baut das Release-Binary, legt `Fadis Nemotron.app` mit Icon und `Info.plist` an, signiert ad hoc und erzeugt die DMG samt SHA-256-Prüfsumme.

Zum Entwickeln auf jedem System:

```sh
cargo run
cargo test
```

Unter Linux braucht egui zur Laufzeit `libxkbcommon-x11`.

### GitHub Actions

`.github/workflows/macos-dmg.yml` läuft bei jedem Pull Request und Push auf `main`: Formatierung, Clippy, Tests, dann Intel-DMG bauen, einhängen und prüfen (Architektur x86_64, Signatur, Icon). Die DMG landet als Artefakt am Lauf. Ein Tag wie `v0.2.0` hängt sie zusätzlich an ein GitHub-Release.

## Aufbau

| Datei | Inhalt |
|---|---|
| `src/main.rs` | Fenster, Icon, Wahl des Renderers |
| `src/app.rs` | Zustand und Abläufe: Chat, Projektordner, Freigaben, Änderungsliste, Einstellungen |
| `src/widgets.rs` | Darstellung: Nachrichten, Werkzeug-Karten, Diffs, Anhänge |
| `src/agent.rs` | Agent-Schleife: Modell fragen, Werkzeuge ausführen, Ergebnisse zurückgeben; System-Prompt |
| `src/tools.rs` | Werkzeuge im Projektordner, Pfad-Sperre, Befehlsregeln |
| `src/api.rs` | Chat-Completions-API, Streaming (SSE), Werkzeugaufrufe, Fehlermeldungen |
| `src/attachments.rs` | Bilder aufbereiten, Videos prüfen, Data-URLs |
| `src/config.rs` | Einstellungen laden und speichern |
| `scripts/build-dmg.sh` | App-Bundle und DMG |

Grenzen dieser Version: Chats werden nicht automatisch gespeichert (nur Export), es gibt keinen eingebauten Editor oder Browser wie in Antigravity, und die App ist nicht notariell beglaubigt.
