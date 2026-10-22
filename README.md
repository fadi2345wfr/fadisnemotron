# Fadis Nemotron

Desktop-Chat für **NVIDIA Nemotron 3.5 Super VL**, geschrieben in Rust. Läuft als normale Mac-App, die DMG ist für Intel-Macs gebaut.

Nemotron 3.5 Super VL ist NVIDIAs offenes Multimodal-Modell (veröffentlicht am 15. Oktober 2026): 120 Milliarden Parameter, davon 12 Milliarden aktiv pro Token, hybride Mamba-Transformer-MoE-Architektur, bis zu 1 Million Token Kontext. Es versteht Text, Bilder, Dokument-Seiten und Videos.

![Oberfläche mit einer Beispielantwort eines Test-Servers](docs/screenshot.png)

## Was die App kann

- Chat mit Nemotron, die Antwort erscheint Wort für Wort.
- Denkmodus an/aus. Den Denkprozess des Modells siehst du live, er klappt zu, sobald die Antwort kommt.
- Bilder (PNG, JPEG, WebP, GIF, BMP) und Videos (MP4, MOV, WebM) anhängen: per Büroklammer, per Drag & Drop ins Fenster oder als Link.
- Große Bilder werden automatisch auf 1536 px verkleinert, Handyfotos richtig gedreht, Transparenz auf Weiß gelegt.
- Antworten mit Markdown, Tabellen und Code-Hervorhebung, Kopieren-Knopf, „Neu generieren“.
- Chat als Markdown-Datei exportieren.
- Funktioniert mit der NVIDIA-Cloud oder mit einem eigenen Server (vLLM, NIM, SGLang).

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
2. In der App **⚙ Einstellungen** öffnen, Schlüssel eintragen, **Verbindung testen**, **Speichern**.

Die Einstellungen liegen in `~/Library/Application Support/de.fadi.FadisNemotron/settings.json`, nur für deinen Benutzer lesbar. Ist das Feld leer, nimmt die App `NVIDIA_API_KEY` aus der Umgebung.

Standardwerte, wie NVIDIA sie empfiehlt:

| Einstellung | Wert |
|---|---|
| Server | `https://integrate.api.nvidia.com/v1` |
| Modell | `nvidia/nemotron-3.5-super-vl-120b-a12b` |
| Temperatur / Top-p | 1.0 / 0.95 |
| Max. Tokens | 16384 (Denken und Antwort zusammen) |

### Eigener Server

Läuft Nemotron bei dir mit vLLM, trag als Server z. B. `http://localhost:8000/v1` ein und als Modell den Namen aus `--served-model-name` (in NVIDIAs Anleitung `nemotron-3.5-super`). Ein Schlüssel ist dann meist nicht nötig. Damit der Denkprozess getrennt ankommt, braucht vLLM `--reasoning-parser nemotron_v3`; ohne ihn trennt die App die `<think>`-Blöcke selbst ab.

## Tipps

- **Videos:** MP4, 1–2 Minuten. Große Dateien gehen als Base64 mit jeder Nachricht mit. Wenn der Server sie ablehnt, das Video als öffentlichen Link anhängen (🔗).
- **PDFs:** Das Modell nimmt keine PDFs. Seiten als PNG exportieren (Vorschau → Exportieren) und die Bilder anhängen; bis zu 32 Seiten pro Anfrage.
- **Leere oder abgeschnittene Antwort:** Das Denken verbraucht Tokens. „Max. Tokens“ erhöhen oder den Denkmodus ausschalten.
- **Tastenkürzel:** Enter senden, Shift+Enter neue Zeile, Esc stoppt die Antwort, ⌘N neuer Chat, ⌘, Einstellungen.
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

`.github/workflows/macos-dmg.yml` läuft bei jedem Pull Request und Push auf `main`: Formatierung, Clippy, Tests, dann Intel-DMG bauen, einhängen und prüfen (Architektur x86_64, Signatur, Icon). Die DMG landet als Artefakt am Lauf. Ein Tag wie `v0.1.0` hängt sie zusätzlich an ein GitHub-Release.

## Aufbau

| Datei | Inhalt |
|---|---|
| `src/main.rs` | Fenster, Icon, Wahl des Renderers |
| `src/app.rs` | Oberfläche (egui): Chat, Anhänge, Einstellungen, Export |
| `src/api.rs` | Anfragen an die Chat-Completions-API, Streaming (SSE), Fehlermeldungen |
| `src/attachments.rs` | Bilder aufbereiten, Videos prüfen, Data-URLs |
| `src/config.rs` | Einstellungen laden und speichern |
| `scripts/build-dmg.sh` | App-Bundle und DMG |

Grenzen dieser Version: Chats werden nicht automatisch gespeichert (nur Export), Tool-Calling ist noch nicht eingebaut, und die App ist nicht notariell beglaubigt.
