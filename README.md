# Fadis Nemotron

Desktop-Agent und Chat für **NVIDIA Nemotron 3.5 Super VL**, geschrieben in Rust. Läuft als normale Mac-App; die DMG ist für Intel-Macs gebaut.

Nemotron arbeitet direkt auf deinem Mac, wie Google Antigravity oder Claude Code: Er sieht deinen Benutzerordner (Downloads, Schreibtisch, Dokumente …), liest und durchsucht Dateien selbst, legt Ordner und Dateien an, ändert sie und führt Befehle aus. Für Programmierprojekte öffnest du den Projektordner, dann arbeitet er dort (Build, Tests, Fehler beheben).

Er kann auch den **Mac bedienen wie ein Mensch**: den Bildschirm ansehen (Screenshots gehen als Bild an das Modell), die Maus bewegen und klicken, tippen, Tastenkürzel drücken, Apps öffnen, im Netz suchen und Webseiten lesen.

Dazu gibt es eine **Android-App**: QR-Code scannen, und alle Chats vom Mac sind auf dem Handy – du gibst Aufträge mit Fotos und Videos, siehst Nemotron live zu und gibst Befehle frei. **Die Android-App steckt in der Mac-App**: beim ersten Start zeigt sie dir einen QR-Code, mit dem das Handy die App direkt vom Mac lädt – nichts muss von GitHub heruntergeladen werden.

## ⬇ Neueste Version herunterladen

| | Datei |
|---|---|
| **Mac (Intel)** | [FadisNemotron-Intel.dmg](https://github.com/fadi2345wfr/fadisnemotron/releases/download/neueste/FadisNemotron-Intel.dmg) – öffnen und in „Programme“ ziehen. **Enthält die Android-App.** |
| Android einzeln | [FadisNemotron.apk](https://github.com/fadi2345wfr/fadisnemotron/releases/download/neueste/FadisNemotron.apk) – nur nötig, wenn du die Mac-App nicht zur Hand hast |

Die Links zeigen immer auf den neuesten Build (Release [„Neueste Version“](https://github.com/fadi2345wfr/fadisnemotron/releases/tag/neueste), auf der Startseite rechts unter **Releases**). Die Android-App in der DMG ist genau die APK desselben Builds.

Nemotron 3.5 Super VL ist NVIDIAs offenes Multimodal-Modell (veröffentlicht am 15. Oktober 2026): 120 Milliarden Parameter, davon 12 Milliarden aktiv pro Token, hybride Mamba-Transformer-MoE-Architektur, bis zu 1 Million Token Kontext. Es versteht Text, Bilder, Dokument-Seiten und Videos und kann Werkzeuge aufrufen.

![Agent-Modus: Nemotron liest, ändert und prüft Dateien (Antwort von einem Test-Server)](docs/screenshot.png)

## So arbeitet Nemotron

1. Schreiben, was passieren soll, z. B. „Was liegt in meinem Downloads-Ordner? Sortiere es nach Art.“ oder „Leg auf dem Schreibtisch einen Ordner ‚Rechnungen‘ an und verschiebe alle PDFs mit ‚Rechnung‘ im Namen hinein.“
   - Ohne Projekt arbeitet Nemotron in deinem **Benutzerordner** (rechts steht 🏠 Benutzerordner).
   - Für Programmierprojekte **📁 Projekt öffnen** klicken (oder ⌘O, oder den Ordner ins Fenster ziehen), dann z. B. „Bau das Projekt und behebe die Fehler“.
   - macOS fragt beim ersten Zugriff auf Schreibtisch, Dokumente oder Downloads einmal nach: **Erlauben**. Hast du es abgelehnt: **Systemeinstellungen → Datenschutz & Sicherheit → Dateien und Ordner → Fadis Nemotron**.
2. Grenzen setzt du im Chat: Sagst du „schau nur in den Ordner X“ oder „fass Datei Y nicht an“, hält sich Nemotron daran.
3. Nemotron arbeitet in Schritten. Jeder Schritt erscheint als Karte im Chat:
   - 📂 Ordner ansehen, 🔎 Dateien finden, 📄 Datei lesen, 🔍 Inhalte suchen – ohne Nachfrage
   - ✏ Datei bearbeiten, 📝 Datei schreiben – mit Diff (+/− Zeilen)
   - ▶ Befehl ausführen – mit Exit-Code und Ausgabe
   - 🌐 im Netz suchen und Webseiten lesen, 🖱️ den Bildschirm ansehen und den Mac mit Maus und Tastatur bedienen – das fragt im Standard-Modus jedes Mal nach
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
- Nemotron arbeitet im Arbeitsordner: ohne Projekt dein ganzer Benutzerordner, mit Projekt dieser Ordner. Was außerhalb liegt (z. B. `/Applications`), erreicht er, wenn du einen Ordner darüber öffnest.
- Ein Platzhalter wie `// ... rest of the code ...` beim Überschreiben einer Datei wird abgelehnt, damit nichts verloren geht.
- Befehle laufen im Arbeitsordner mit der Umgebung deiner Shell (inklusive `~/.zshrc`, damit `cargo`, `npm` über nvm, `python` über pyenv usw. gefunden werden), mit Zeitlimit. **⏹ Stop**, Esc oder das Beenden der App beendet sie samt Unterprozessen. Sehr lange Ausgaben werden gekürzt (Anfang und Ende bleiben).

**Lange Aufgaben:** Je länger ein Gespräch wird (viele gelesene Dateien), desto langsamer und schlechter antwortet der NVIDIA-Server – bis hin zu „timeout: receive response“. Deshalb schneidet die App nichts ab, sondern wechselt selbst den Chat: Ab etwa 60 000 Token (oder wenn NVIDIA bei einem längeren Gespräch nicht antwortet) schreibt Nemotron eine Übergabe-Notiz (Aufgabe, Erledigtes, Gefundenes, nächste Schritte), die App legt einen neuen Chat an – darin dein Auftrag samt Anhängen und die Notiz – und Nemotron macht dort von allein weiter. Der alte Chat bleibt unverändert in der Liste. Antwortet der Server auch für die Notiz nicht, schreibt die App ein Protokoll der letzten Schritte. Höchstens dreimal pro Auftrag; danach werden alte Werkzeugausgaben gekürzt. Kurze Aussetzer (Timeout, 429, 5xx) werden bis zu zweimal erneut versucht.

**Chats** werden automatisch gespeichert und stehen links in der Chatliste (➕ Neuer Chat, anklicken zum Wechseln, 🗑 zum Löschen). Dieselbe Liste siehst du auf dem Handy.

Damit Nemotron wirklich ändert statt nur Beispielcode zu zeigen, bekommt er klare Anweisungen; zeigt er trotzdem einen größeren Codeblock ohne Änderung, fordert die App ihn einmal auf, die Änderung selbst einzutragen. Liegt im Projekt eine `AGENTS.md` oder `CLAUDE.md`, liest Nemotron sie als Projektregeln mit.

## Der Mac wie von einem Menschen bedient

Nemotron bekommt auf Wunsch ein Bild vom Bildschirm (**Screenshot**), sieht darauf, wo etwas ist, und klickt, tippt oder drückt Tasten – dann macht er wieder einen Screenshot und prüft, ob es geklappt hat. So bedient er auch Programme ohne Schnittstelle (Safari, Einstellungen, Finder, andere Apps). Alles läuft lokal über macOS-Funktionen (Screenshot, CoreGraphics, `osascript`); Text, den er tippt, wird nie als Skript ausgeführt.

Damit macOS das erlaubt, gibst du der App einmal Rechte. Fehlt eines, sagt Nemotron dir genau welches und öffnet die Einstellungen:

| Recht | Wofür | Wo |
|---|---|---|
| **Bedienungshilfen** | Maus, Tastatur | Systemeinstellungen → Datenschutz & Sicherheit → Bedienungshilfen |
| **Bildschirm- & Systemaudioaufnahme** | Screenshots | Systemeinstellungen → Datenschutz & Sicherheit → Bildschirm- & Systemaudioaufnahme (danach App neu öffnen) |
| **Automation** („System Events“) | Tastenkürzel, Tippen | beim ersten Mal fragt macOS selbst |

Nach einem Update der App muss der Schalter bei Bedienungshilfen manchmal einmal aus- und wieder eingeschaltet werden, weil die App nicht von Apple signiert ist.

Webseiten, Dateien und Screenshots behandelt Nemotron als Material, nicht als Anweisung: Steht auf einer Seite „lösche alles“, tut er es nicht. Kaufen, Löschen, Nachrichten an andere und Passwörter macht er nur, wenn du es ausdrücklich verlangst.

## Vom Handy aus (Android)

Die Android-App ist eine zweite Oberfläche der Mac-App – alles läuft über deinen Mac:

- **Alle Chats synchron:** Die Chatliste (☰ oben links) ist dieselbe wie auf dem Mac. Ein Chat, den du am Handy öffnest oder neu anlegst, ist auch am Mac offen – und umgekehrt.
- **Fotos und Videos anhängen:** Videos verkleinert das Handy vorher auf 720p, damit sie schnell übertragen werden und NVIDIA sie annimmt (höchstens 100 MB).
- Denken, Werkzeuge und Antworten live sehen, Befehle freigeben oder ablehnen, stoppen, neu generieren, Änderungen als Diff ansehen und rückgängig machen, Freigabe-Modus und Denkmodus umschalten.

### Installieren – direkt vom Mac

Beim ersten Start der Mac-App öffnet sich das Fenster **Fernzugriff fürs Handy**:

1. **Schritt 1 · App aufs Handy:** Mit der Kamera des Android-Handys den ersten QR-Code scannen (Handy und Mac im selben WLAN). Die Seite öffnet sich, **Fadis Nemotron laden** tippen, die Datei öffnen. Android fragt einmal, ob Installationen aus dieser Quelle erlaubt sind: **Zulassen**. Voraussetzung: Android 8.0 oder neuer. Sobald das Handy die App geladen hat, steht im Fenster ein Haken.
2. **Schritt 2 · Koppeln:** App öffnen, **QR-Code scannen** tippen und den zweiten Code scannen. Alternativ Adresse und Schlüssel von Hand eintippen (steht unter „Technik“).
3. Fertig. Im Fenster steht, wann das Handy zuletzt verbunden war; der grüne Punkt am 📱 zeigt, dass der Fernzugriff an ist. Das Fenster erreichst du jederzeit über **📱**.

macOS fragt beim ersten Mal, ob „Fadis Nemotron“ eingehende Verbindungen annehmen darf: **Erlauben** (bzw. Zugriff aufs lokale Netzwerk erlauben). Die Installationsseite braucht keinen Schlüssel und liefert nur die App aus; alles andere verlangt den Schlüssel.

Ein Android-Handy lässt sich nicht ohne Zutun des Nutzers aus der Ferne bespielen, deshalb bleibt es bei „scannen, laden, installieren“. Das Programm per Kabel (adb) zu installieren geht auch, ist aber nicht eingebaut.

**Unterwegs:** Im eigenen WLAN klappt es sofort. Für unterwegs [Tailscale](https://tailscale.com) auf Mac und Handy installieren und bei beiden mit demselben Konto anmelden: Die Verbindung ist dann verschlüsselt und funktioniert über Mobilfunk, ohne Einstellungen am Router. Danach einmal neu scannen, damit das Handy die Tailscale-Adresse kennt. **Keine Portfreigabe im Router einrichten** – die Verbindung selbst ist unverschlüsselt.

**Im Hintergrund:** Solange der Fernzugriff an ist, hält die App den Mac wach (`caffeinate`; abschaltbar im Fernzugriff-Fenster), das Fenster darf minimiert und der Bildschirm dunkel sein. Arbeitet Nemotron, bleibt auch der Bildschirm an, damit er ihn sehen kann.

**Deckel zuklappen:** Ein zugeklapptes MacBook schläft sonst immer ein – das kann keine App verhindern. Dafür gibt es im Fernzugriff-Fenster den Knopf **Auch bei zugeklapptem Deckel weiterlaufen**: Er fragt nach deinem Passwort und stellt `pmset -a disablesleep 1` ein (rückgängig mit **Wieder normal schlafen lassen**). Bitte nur nutzen, solange der Mac Luft hat – im Rucksack wird er warm. Ohne Passwort geht es im Clamshell-Modus (Netzteil, externer Bildschirm und Tastatur).

**Sicherheit:** Jede Anfrage braucht den Schlüssel aus dem QR-Code. **Neuen Schlüssel erzeugen** im Fernzugriff-Fenster sperrt alle gekoppelten Handys aus; **Fernzugriff an** abschalten beendet den Zugang sofort. Riskante Befehle fragen auch übers Handy immer nach. Das Protokoll ist in [docs/remote-api.md](docs/remote-api.md) beschrieben.

## Was die App sonst kann

- Antworten erscheinen Wort für Wort; den Denkprozess siehst du live, er klappt zu, sobald die Antwort oder ein Werkzeug kommt.
- Bilder (PNG, JPEG, WebP, GIF, BMP) und Videos (MP4, MOV, WebM) anhängen: per Büroklammer, per Drag & Drop oder als Link. Große Bilder werden auf 1536 px verkleinert, Handyfotos richtig gedreht.
- Markdown mit Tabellen und Code-Hervorhebung, Kopieren, „Neu generieren“, Export als Markdown.
- Funktioniert mit der NVIDIA-Cloud oder einem eigenen Server (vLLM, NIM, SGLang).

## Installation auf dem Mac (Intel)

1. [FadisNemotron-Intel.dmg](https://github.com/fadi2345wfr/fadisnemotron/releases/download/neueste/FadisNemotron-Intel.dmg) laden (immer die neueste Version).
2. DMG öffnen und **Fadis Nemotron** in den Ordner **Programme** ziehen.
3. Erster Start: Die App ist nicht von Apple notariell beglaubigt, deshalb blockiert macOS sie beim ersten Öffnen.
   - macOS 15 und neuer: App einmal öffnen, dann **Systemeinstellungen → Datenschutz & Sicherheit → „Dennoch öffnen“**.
   - macOS 14 und älter: Rechtsklick auf die App → **Öffnen** → **Öffnen**.
   - Alternativ im Terminal: `xattr -dr com.apple.quarantine "/Applications/Fadis Nemotron.app"`

Voraussetzung: macOS 10.15 (Catalina) oder neuer.

## API-Schlüssel

1. Kostenlosen Schlüssel auf [build.nvidia.com/settings/api-keys](https://build.nvidia.com/settings/api-keys) erstellen (beginnt mit `nvapi-`). In der App führt dich ein Link auf dem Startbildschirm und in den Einstellungen direkt dorthin.
2. In der App **⚙** öffnen, Schlüssel eintragen, **Verbindung testen**, **Speichern**.

Die Einstellungen liegen in `~/Library/Application Support/de.fadi.FadisNemotron/settings.json`, die Chats im Ordner `chats/` daneben – beides nur für deinen Benutzer lesbar. Ist das Feld leer, nimmt die App `NVIDIA_API_KEY` aus der Umgebung.

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

- **Videos:** MP4, 1–2 Minuten. Große Dateien gehen als Base64 mit jeder Nachricht mit. Wenn der Server sie ablehnt, das Video kürzen oder als öffentlichen Link anhängen (🔗). Vom Handy kommen Videos schon verkleinert (720p).
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

`.github/workflows/macos-dmg.yml` läuft bei jedem Pull Request und Push auf `main` und hat zwei Aufgaben nacheinander:

1. `apk` ruft `.github/workflows/android-apk.yml` auf: Unit-Tests der Android-App, Release-APK bauen, Signatur prüfen.
2. `dmg` holt genau diese APK, prüft Formatierung, Clippy und Tests der Mac-App, baut die Intel-DMG **mit der APK im App-Bundle** (`Contents/Resources/FadisNemotron.apk`, Skript `scripts/build-dmg.sh`, in CI Pflicht) und prüft sie: Architektur x86_64, Signatur, Icon, APK enthalten und identisch.

Beide Dateien landen als Artefakte am Lauf und, nach jedem erfolgreichen Build, unter festen Namen im Release „Neueste Version“ (Tag `neueste`, Skript `scripts/publish-latest.sh`), daher die festen Download-Links oben. Ein Tag wie `v0.4.0` hängt DMG und APK zusätzlich an ein eigenes Release.

Lokal: `APK=android/app/build/outputs/apk/release/app-release.apk ./scripts/build-dmg.sh` baut die DMG mit der Android-App darin; ohne `APK` verweist die Mac-App auf GitHub.

### Android-App selbst bauen

Mit JDK 21 und dem Android SDK (Plattform 36):

```sh
cd android
./gradlew testDebugUnitTest assembleRelease   # APK in app/build/outputs/apk/release/
```

Die APK wird mit dem Schlüssel `android/app/fadis.keystore` signiert, der im Repository liegt. So lässt sich jede neue Version über die alte installieren. Für eine Veröffentlichung im Play Store bräuchte es einen eigenen, geheimen Schlüssel.

## Aufbau

| Datei | Inhalt |
|---|---|
| `src/main.rs` | Fenster, Icon, Wahl des Renderers |
| `src/app.rs` | Zustand und Abläufe: Chat, Chatliste, Arbeitsordner, Freigaben, Änderungsliste, Einstellungen |
| `src/chats.rs` | Chats speichern, laden und löschen (samt Fotos und Videos) |
| `src/widgets.rs` | Darstellung: Nachrichten, Werkzeug-Karten, Diffs, Anhänge |
| `src/agent.rs` | Agent-Schleife: Modell fragen, Werkzeuge ausführen, Ergebnisse zurückgeben; System-Prompt |
| `src/tools.rs` | Werkzeuge im Arbeitsordner, Befehlsregeln |
| `src/api.rs` | Chat-Completions-API, Streaming (SSE), Werkzeugaufrufe, Fehlermeldungen |
| `src/attachments.rs` | Bilder aufbereiten, Videos prüfen, Data-URLs |
| `src/config.rs` | Einstellungen laden und speichern |
| `src/remote.rs` | Fernzugriff fürs Handy: kleiner HTTP-Server, Schlüssel, Kopplung, Installationsseite mit der Android-App |
| `src/computer.rs` | Netz und Bildschirm: Websuche, Webseiten lesen, Screenshot, Maus, Tastatur |
| `src/awake.rs` | Mac wach halten (`caffeinate`), Schlaf bei zugeklapptem Deckel ein-/ausschalten |
| `docs/remote-api.md` | Protokoll zwischen Mac-App und Android-App |
| `android/` | Android-App (Kotlin, Jetpack Compose) |
| `scripts/build-dmg.sh` | App-Bundle und DMG |

Grenzen dieser Version: Es gibt keinen eingebauten Editor oder Browser wie in Antigravity, die Mac-App ist nicht notariell beglaubigt, die Android-App gibt es nur als APK (nicht im Play Store) und eine iPhone-App gibt es nicht (sie müsste neu geschrieben und mit einem Apple-Entwicklerkonto signiert werden).
