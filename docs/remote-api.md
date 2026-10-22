# Fernzugriff-Protokoll (Version 1)

Die Mac-App bietet, wenn **Fernzugriff** eingeschaltet ist, eine kleine HTTP/JSON-Schnittstelle an. Die Android-App steuert darüber den Chat und den Agenten auf dem Mac. Standard-Port: `7878`.

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

Fehler haben einen passenden Status (`400`, `401`, `404`, `409`, `413`, `503`) und den Body `{"error": "Text für den Nutzer"}`.

## Endpunkte

### `GET /api/v1/ping`

```json
{"ok": true, "app": "Fadis Nemotron", "version": "0.3.0", "protocol": 1, "name": "MacBook"}
```

### `GET /api/v1/state?after=<version>&wait=<sekunden>`

Liefert den ganzen sichtbaren Zustand. Ist `after` gleich der aktuellen Version, wartet der Server bis zu `wait` Sekunden (höchstens 30) auf eine Änderung (Long-Polling) und antwortet dann in jedem Fall mit dem aktuellen Zustand.

```json
{
  "version": 42,
  "busy": true,
  "model": "nvidia/nemotron-3.5-super-vl-120b-a12b",
  "thinking": true,
  "approval": "AutoEdits",
  "workspace": {"name": "proj", "path": "/Users/fadi/proj"},
  "notice": {"text": "README.md wiederhergestellt.", "error": false},
  "changes": [{"path": "README.md", "added": 1, "removed": 1, "created": false}],
  "messages": [
    {"role": "user", "text": "Ändere die README", "attachments": ["foto.jpg"]},
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

- `workspace` ist `null`, wenn kein Projektordner offen ist (reiner Chat).
- `approval`: `AskAll`, `AutoEdits` oder `AutoAll`.
- `status`: `streaming`, `done`, `cancelled`, `failed` (dann steht der Grund in `error`).
- Werkzeug-`state`: `planned`, `awaiting_approval`, `running`, `done`, `declined`, `not_run`, `interrupted` (lief beim Stopp noch; kann teilweise ausgeführt sein). Bei `awaiting_approval` enthält `diff` die geplante Änderung und `warning` ggf. den Grund, warum ein Befehl immer nachgefragt wird.
- Lange Texte werden gekürzt: `reasoning` auf die letzten 6000 Zeichen, `output` auf die letzten 4000, `diff` auf die ersten 8000.

### `POST /api/v1/send`

```json
{"text": "Bau das Projekt und behebe die Fehler", "images": [{"name": "foto.jpg", "data": "<base64 JPEG/PNG>"}]}
```

Wie Tippen und Senden in der Mac-App. `409`, wenn gerade eine Antwort läuft. Höchstens 20 MB pro Anfrage.

### `POST /api/v1/stop`

Stoppt die laufende Antwort (wie der Stop-Knopf).

### `POST /api/v1/decide`

```json
{"call_id": "chatcmpl-tool-1", "decision": "allow"}
```

`decision`: `allow`, `allow_for_run` (für diesen Auftrag alles erlauben) oder `deny`.

### `POST /api/v1/new_chat` · `POST /api/v1/retry`

Neuer Chat bzw. letzte Antwort neu generieren.

### `POST /api/v1/settings`

```json
{"approval": "AskAll", "thinking": false}
```

Beide Felder sind optional.

### `POST /api/v1/undo`

```json
{"path": "README.md"}
```

Setzt eine geänderte Datei auf den Stand vor Nemotron zurück. `409`, wenn die Datei inzwischen anderweitig verändert wurde (am Mac bestätigen) oder gerade eine Antwort läuft.

### `GET /api/v1/diff?path=README.md`

```json
{"path": "README.md", "diff": "--- a/README.md\n…"}
```
