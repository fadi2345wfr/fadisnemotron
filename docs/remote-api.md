# Fernzugriff-Protokoll (Version 2)

Die Mac-App bietet, wenn **Fernzugriff** eingeschaltet ist, eine kleine HTTP/JSON-Schnittstelle an. Die Android-App ist darüber eine zweite Oberfläche derselben App: Sie zeigt dieselben Chats, denselben Agenten und dieselben Freigaben wie der Mac. Standard-Port: `7878`.

Version 2 ergänzt gegenüber Version 1: gespeicherte Chats (`chat`, `chats`, `open_chat`, `delete_chat`), Uploads für Fotos und Videos (`upload`, `send.uploads`) und `workspace.home`.

## Sicherheit

- Jede Anfrage braucht den Header `Authorization: Bearer <token>`. Den Schlüssel erzeugt die Mac-App (32 Zeichen, zufällig). Ohne oder mit falschem Schlüssel: `401`.
- Die Verbindung ist reines HTTP. Gedacht ist sie für das eigene WLAN oder für [Tailscale](https://tailscale.com) (verschlüsselt, funktioniert unterwegs). **Keine Portfreigabe im Router.**
- Fernzugriff ist standardmäßig aus und lässt sich jederzeit abschalten. „Neuen Schlüssel erzeugen“ sperrt alle gekoppelten Geräte aus.

## Kopplung

Die Mac-App zeigt einen QR-Code mit diesem JSON:

```json
{"fadis": 1, "name": "MacBook", "urls": ["http://192.168.1.20:7878", "http://100.101.2.3:7878"], "token": "…"}
```

`urls` enthält alle IPv4-Adressen des Macs (Tailscale-Adressen `100.64.0.0/10` zuerst). Das Handy probiert sie der Reihe nach mit `GET /api/v1/ping` und merkt sich die erste, die antwortet. Alternativ gibt man Adresse und Schlüssel von Hand ein.

## Fehler

Fehler haben einen passenden Status (`400`, `401`, `404`, `408`, `409`, `411`, `413`, `431`, `500`, `503`) und den Body `{"error": "Text für den Nutzer"}`.

Jede Verbindung trägt genau eine Anfrage (`Connection: close`). Anfragen mit Body (`POST`) müssen `Content-Length` angeben, `Transfer-Encoding` gibt es nicht (`411`). Der Schlüssel wird geprüft, bevor ein Body gelesen wird. Kopfzeilen dürfen zusammen 16 KB nicht überschreiten (`431`), der Kopf muss innerhalb von 10 Sekunden ankommen (`408`).

## Endpunkte

### `GET /api/v1/ping`

```json
{"ok": true, "app": "Fadis Nemotron", "version": "0.4.0", "protocol": 2, "name": "MacBook"}
```

Das Handy prüft `protocol`. Ist es kleiner als 2, zeigt es „Bitte die Mac-App aktualisieren“.

### `GET /api/v1/state?after=<version>&wait=<sekunden>`

Liefert den ganzen sichtbaren Zustand. Ist `after` gleich der aktuellen Version, wartet der Server bis zu `wait` Sekunden (höchstens 30) auf eine Änderung (Long-Polling) und antwortet dann in jedem Fall mit dem aktuellen Zustand.

```json
{
  "version": 42,
  "busy": true,
  "model": "nvidia/nemotron-3.5-super-vl-120b-a12b",
  "thinking": true,
  "approval": "AutoEdits",
  "workspace": {"name": "proj", "path": "/Users/fadi/proj", "home": false},
  "notice": {"text": "README.md wiederhergestellt.", "error": false},
  "chat": {"id": "c-1792658712-3f9a1c", "title": "Ändere die README"},
  "chats": [
    {"id": "c-1792658712-3f9a1c", "title": "Ändere die README", "updated": 1792658712, "messages": 2},
    {"id": "c-1792605781-b7c1e0", "title": "Ordner auf dem Schreibtisch anlegen", "updated": 1792605781, "messages": 6}
  ],
  "changes": [{"path": "README.md", "added": 1, "removed": 1, "created": false}],
  "messages": [
    {"role": "user", "text": "Ändere die README", "attachments": ["foto.jpg", "clip.mp4"]},
    {
      "role": "assistant",
      "status": "streaming",
      "error": null,
      "finish_reason": null,
      "notices": [],
      "steps": [
        {
          "reasoning": "Ich lese zuerst …",
          "content": "Markdown-Text",
          "tools": [
            {
              "id": "chatcmpl-tool-1",
              "name": "edit_file",
              "title": "Bearbeitet: README.md",
              "state": "done",
              "ok": true,
              "output": "Updated README.md (+1 -1 lines, 2 lines total).",
              "diff": "--- a/README.md\n+++ b/README.md\n…",
              "added": 1,
              "removed": 1,
              "warning": null
            }
          ]
        }
      ]
    }
  ]
}
```

- `workspace` ist der Arbeitsordner des Agenten. Ohne geöffnetes Projekt ist es der Benutzerordner des Macs, dann ist `home` `true`. Nemotron kann auch außerhalb des Arbeitsordners im Benutzerordner lesen und (nach Freigabe) schreiben, z. B. in Downloads oder auf dem Schreibtisch.
- `chat` ist der Chat, der gerade auf dem Mac (und damit auch auf dem Handy) offen ist; `messages` gehört zu ihm. `chats` listet alle gespeicherten Chats, neueste zuerst, höchstens 200. `updated` ist eine Unix-Zeit in Sekunden. Ein neuer, noch leerer Chat hat den Titel `"Neuer Chat"`.
- `approval`: `AskAll`, `AutoEdits` oder `AutoAll`.
- `status`: `streaming`, `done`, `cancelled`, `failed` (dann steht der Grund in `error`).
- Werkzeug-`state`: `planned`, `awaiting_approval`, `running`, `done`, `declined`, `not_run`, `interrupted` (lief beim Stopp noch; kann teilweise ausgeführt sein). Bei `awaiting_approval` enthält `diff` die geplante Änderung und `warning` ggf. den Grund, warum nachgefragt wird (z. B. „ändert eine Datei außerhalb des Projektordners“ oder „läuft mit Administratorrechten“).
- Lange Texte werden gekürzt: `reasoning` auf die letzten 6000 Zeichen, `output` auf die letzten 4000, `diff` auf die ersten 8000.
- Unbekannte Felder und Werte soll der Client ignorieren bzw. auf einen Standard zurückfallen lassen.

### `POST /api/v1/upload?name=<dateiname>`

Lädt ein Foto oder Video hoch, **als rohe Bytes im Body** (kein JSON, kein Base64), z. B. mit `Content-Type: video/mp4` oder `image/jpeg`. `Content-Length` ist Pflicht. Höchstens 30 MB für Fotos und 100 MB für Videos; ist die angekündigte Länge größer, antwortet der Mac sofort mit `413`, ohne den Body zu lesen. Nimmt der Mac gerade schon sehr viele Daten an, antwortet er mit `503` (kurz warten, noch einmal senden).

```json
{"id": "up-7f3a9c", "kind": "video", "size": 14680064}
```

- Die Art erkennt der Mac an der Dateiendung von `name` (Bilder: png, jpg, jpeg, gif, webp, bmp; Videos: mp4, m4v, mov, webm), sonst am `Content-Type` (`image/…`, `video/…`). Unbekannt: `400`.
- Bilder werden wie in der Mac-App verkleinert (längste Seite 1536 px); ist die Datei kein lesbares Bild: `400`. Videos werden unverändert übernommen. Das Handy verkleinert Videos deshalb vorher (720p, MP4/H.264).
- Ein Upload bleibt 30 Minuten gültig und wird mit dem nächsten `send`, der ihn nennt, verbraucht.

### `POST /api/v1/send`

```json
{"text": "Was passiert in dem Video?", "uploads": ["up-7f3a9c"], "images": [{"name": "foto.jpg", "data": "<base64 JPEG/PNG>"}]}
```

Wie Tippen und Senden in der Mac-App, im offenen Chat. `uploads` und `images` sind optional (`images` ist die Form aus Version 1 und nur für kleine Bilder gedacht). `409`, wenn gerade eine Antwort läuft; `404`, wenn ein Upload unbekannt oder abgelaufen ist. Höchstens 20 MB pro Anfrage.

### `POST /api/v1/stop`

Stoppt die laufende Antwort (wie der Stop-Knopf).

### `POST /api/v1/decide`

```json
{"call_id": "chatcmpl-tool-1", "decision": "allow"}
```

`decision`: `allow`, `allow_for_run` (für diesen Auftrag alles erlauben) oder `deny`.

### `POST /api/v1/new_chat`

Legt einen neuen, leeren Chat an und öffnet ihn (auf Mac und Handy). `409`, wenn gerade eine Antwort läuft; `500`, wenn der bisherige Chat nicht gespeichert werden konnte (er bleibt dann offen).

### `POST /api/v1/open_chat`

```json
{"id": "c-1792605781-b7c1e0"}
```

Öffnet einen gespeicherten Chat. `404` bei unbekannter `id`, `409`, wenn gerade eine Antwort läuft.

### `POST /api/v1/delete_chat`

```json
{"id": "c-1792605781-b7c1e0"}
```

Löscht einen Chat endgültig. Ist es der offene Chat, öffnet der Mac den neuesten übrigen (oder einen neuen, leeren). `404` bei unbekannter `id`, `409`, wenn der offene Chat gelöscht werden soll, während gerade eine Antwort läuft. Andere Chats lassen sich auch während einer Antwort löschen.

### `POST /api/v1/retry`

Letzte Antwort neu generieren.

### `POST /api/v1/settings`

```json
{"approval": "AskAll", "thinking": false}
```

Beide Felder sind optional.

### `POST /api/v1/undo`

```json
{"path": "README.md"}
```

Setzt eine geänderte Datei auf den Stand vor Nemotron zurück. `path` ist der Pfad wie in `changes`. `409`, wenn die Datei inzwischen anderweitig verändert wurde (am Mac bestätigen) oder gerade eine Antwort läuft.

### `GET /api/v1/diff?path=README.md`

```json
{"path": "README.md", "diff": "--- a/README.md\n…"}
```
