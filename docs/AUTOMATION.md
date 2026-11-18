# Automation server and multi-account sessions

`browser-app` can run as a headless HTTP JSON API that drives Servo Content
Processes. Each **session** owns one Content Process and one stable profile
directory, so several signed-in accounts can be used at the same time.

```
HTTP client (BoardDo, curl, scripts)
    │  GET /health, POST /render, /sessions/…
    ▼
axum (tokio)
    │  mpsc
    ▼
one actor thread per session
    │  Unix IPC (protocol v5)
    ▼
rust-browser --content-process --automation-content
```

## Start

```bash
cargo run -p browser-app -- \
  --automation-server 127.0.0.1:9333 \
  --automation-token "$RUST_BROWSER_AUTOMATION_TOKEN" \
  --automation-data ~/.rust-browser/automation
```

| Flag | Meaning |
|------|---------|
| `--automation-server <addr>` | Listen address. Off-loopback requires a token. |
| `--automation-token <t>` | `Authorization: Bearer <t>`. Also read from `RUST_BROWSER_AUTOMATION_TOKEN`. |
| `--automation-data <dir>` | Root for profiles. Defaults to a temp dir per process. |
| `--automation-max-tabs <n>` | Concurrent render jobs per session (default 4). |
| `--automation-exit-on-stdin-eof` | Exit when the parent process closes stdin. |
| `--content-host-file <path>` | Optional content host binary (sandboxing). |

`/health` and `/render` keep their legacy behaviour and use the `default`
session. Everything under `/sessions/{id}/…` targets an explicit session;
`/sessions` creates and lists sessions.

## Profile layout

```
<automation-data>/content                  default session
<automation-data>/sessions/<id>/content    named session
```

The directory is stable across restarts. Servo writes `cookie_jar.json`,
`auth_cache.json` and `hsts_list.json` there when the Content Process exits
gracefully, so sessions survive server restarts.

## Manual login (Google, reCAPTCHA)

Login is intentionally **not automated**. Sign in once by hand in a visible
window that uses the session profile:

```bash
curl -X POST http://127.0.0.1:9333/sessions/work/login/open \
  -H 'content-type: application/json' \
  -H "authorization: Bearer $TOKEN" \
  -d '{"url":"https://accounts.google.com/"}'
```

The server stops the headless Content Process (one writer per profile), opens
`rust-browser --open <url> --content-dir <session>/content`, and waits until
the window is closed or `timeout_ms` elapses (default 15 min, max 60 min).
Afterwards it restarts the headless session, which loads the saved cookies.
Complete the captcha and any 2FA by hand; nothing is bypassed.

Repeat with a different session id for another account.

## Routes

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/health` | Default session health. |
| `POST` | `/render` | Render a page in the default session. |
| `GET` | `/sessions` | List live sessions. |
| `POST` | `/sessions` | Create/start a session (`{"id":"work"}`, optional). |
| `DELETE` | `/sessions/{id}` | Stop a session and flush its profile. |
| `POST` | `/sessions/{id}/login/open` | Open the visible login window. |
| `GET` | `/sessions/{id}/cookies?url=…` | Export cookies (incl. HttpOnly). |
| `PUT`/`POST` | `/sessions/{id}/cookies` | Import cookies. |
| `POST` | `/sessions/{id}/request` | Raw HTTP with the session cookies. |
| `POST` | `/sessions/{id}/fetch` | `fetch()` inside a page of the session. |
| `POST` | `/sessions/{id}/deepseek/chat` | Send a prompt, wait for the answer, auto-continue. |
| `GET` | `/sessions/{id}/deepseek/status` | Is this session signed in to DeepSeek? |
| `GET` | `/sessions/{id}/deepseek/chats` | List DeepSeek chats. |
| `POST` | `/sessions/{id}/deepseek/chats` | Create a chat (optional `title`). |
| `DELETE` | `/sessions/{id}/deepseek/chats` | Delete chats (`{"chat_ids":["…"]}`). |
| `GET` | `/sessions/{id}/deepseek/chats/{chat_id}` | Read a chat's history. |
| `POST` | `/sessions/{id}/deepseek/chats/{chat_id}/rename` | Rename a chat. |

Session ids are 1–64 chars: letters, digits, `-`, `_`.

### Render

```bash
curl -X POST http://127.0.0.1:9333/render \
  -H 'content-type: application/json' \
  -d '{"url":"https://example.com/","wait_for":"h1","screenshot":true}'
```

### Page fetch (cookies, CSRF, page JS)

Runs `fetch()` from a page in the session, so browser cookies, `SameSite`
rules, CSRF tokens produced by page JS and redirects all behave normally.

```bash
curl -X POST http://127.0.0.1:9333/sessions/work/fetch \
  -H 'content-type: application/json' \
  -H "authorization: Bearer $TOKEN" \
  -d '{
    "url": "https://example.com/api/me",
    "page_url": "https://example.com/",
    "method": "GET"
  }'
```

`page_url` defaults to the request URL's origin. The page is kept in a
persistent tab per session and reused by later fetches.

### Raw HTTP with cookies

Executes the request outside the browser with the exported cookies attached.
Use this for APIs that do not need page JS or when a raw HTTP client is more
convenient.

```bash
curl -X POST http://127.0.0.1:9333/sessions/work/request \
  -H 'content-type: application/json' \
  -H "authorization: Bearer $TOKEN" \
  -d '{
    "url": "https://example.com/api/items",
    "method": "POST",
    "headers": {"content-type": "application/json"},
    "body": "{\"name\":\"demo\"}"
  }'
```

- `body` (UTF-8) or `body_base64` (binary), never both.
- `follow_redirects` defaults to `true`.
- Text responses come back in `body`; binary responses in `body_base64`.
- Bodies are capped at 5 MiB (`truncated` marks a cut response).
- A user-supplied `Cookie` header wins over the exported cookies.
- Cookies are filtered by domain, path, `Secure` and expiry before sending.

### Cookie import/export

```bash
curl 'http://127.0.0.1:9333/sessions/work/cookies?url=https%3A%2F%2Fexample.com%2F' \
  -H "authorization: Bearer $TOKEN"

curl -X PUT http://127.0.0.1:9333/sessions/work/cookies \
  -H 'content-type: application/json' \
  -H "authorization: Bearer $TOKEN" \
  -d '{
    "url": "https://example.com/",
    "cookies": [{"name":"sid","value":"…","domain":".example.com","path":"/","secure":true,"http_only":true}]
  }'
```

## DeepSeek plugin

`chat.deepseek.com` is driven through the real UI, not by re-implementing its
private PoW or streaming protocol. The plugin lives in
`crates/browser-automation/src/deepseek.rs` and owns a dedicated page tab per
session, separate from `/fetch`.

1. Sign in once, by hand, in the session profile:

   ```bash
   curl -X POST http://127.0.0.1:9333/sessions/work/login/open \
     -H 'content-type: application/json' \
     -d '{"url":"https://chat.deepseek.com/sign_in"}'
   ```

   Complete Google / captcha / 2FA in the visible window and close it.

2. Check the session:

   ```bash
   curl http://127.0.0.1:9333/sessions/work/deepseek/status
   # {"logged_in":true,"url":"https://chat.deepseek.com/","chat_id":null,"token_found":true}
   ```

3. Send a prompt and get the full answer:

   ```bash
   curl -X POST http://127.0.0.1:9333/sessions/work/deepseek/chat \
     -H 'content-type: application/json' \
     -d '{"prompt":"Explain Rust ownership in two paragraphs"}'
   ```

   ```json
   {
     "chat_id": "…",
     "chat_url": "https://chat.deepseek.com/a/chat/s/…",
     "answer": "…",
     "continued": 0,
     "thinking_enabled": true,
     "search_enabled": false,
     "messages": [{"id":"…","role":"user","text":"…","status":"FINISHED"}],
     "elapsed_ms": 18342
   }
   ```

   To continue an existing chat, pass `"chat_id"`. `timeout_ms` defaults to
   180 s (max 900 s), `settle_ms` to 2 s, `max_continues` to 10.

   The composer switches can be set per request:

   ```bash
   curl -X POST http://127.0.0.1:9333/sessions/work/deepseek/chat \
     -H 'content-type: application/json' \
     -d '{"prompt":"What changed in Rust 1.85?","thinking":true,"search":true}'
   ```

   `thinking` is DeepSeek's "DeepThink" (reasoning) switch, `search` is web
   search (`deep_thinking` / `smart_search` are accepted as aliases). `true`
   turns the switch on, `false` turns it off, and omitting it leaves the
   composer exactly as the page has it. The plugin drives the real switch
   chips (`.ds-toggle-button`) through their React handler and waits for
   `aria-pressed` to change before typing, so the outgoing request carries
   the flags. The result reports the switches that were actually on in
   `thinking_enabled` / `search_enabled`.

   Long answers that hit DeepSeek's length limit show a **Continue** button.
   The button ignores untrusted clicks, so the plugin invokes the button's
   React `onClick` handler with a trusted-looking native event; this runs
   DeepSeek's own `continueCompletion()` and keeps the UI stream in sync. The
   plugin repeats until the answer settles. Matching is locale-independent
   (`Continue`, `继续生成`, `Продолжить`, …).

4. Chat management:

   ```bash
   # list (pinned first, then newest)
   curl 'http://127.0.0.1:9333/sessions/work/deepseek/chats?limit=50'

   # create, optionally with a title
   curl -X POST http://127.0.0.1:9333/sessions/work/deepseek/chats \
     -H 'content-type: application/json' -d '{"title":"Research"}'

   # read history
   curl http://127.0.0.1:9333/sessions/work/deepseek/chats/CHAT_ID

   # rename
   curl -X POST http://127.0.0.1:9333/sessions/work/deepseek/chats/CHAT_ID/rename \
     -H 'content-type: application/json' -d '{"title":"New title"}'

   # delete one or more
   curl -X DELETE http://127.0.0.1:9333/sessions/work/deepseek/chats \
     -H 'content-type: application/json' -d '{"chat_ids":["CHAT_ID"]}'
   ```

All plugin calls run inside the `chat.deepseek.com` page, so the WAF cookie,
`ds_session_id`, and the bearer token persisted by the site are used exactly
as the web app uses them. The plugin never stores or logs those secrets
itself. Chat management is available only while the session is signed in.

Omitting `chat_id` always opens a fresh chat page, so a prompt never lands in
an unrelated open chat. Failures come back as `{"error": "…"}` with a 4xx/5xx
status; read-only calls (`status`, chat list, history) are retried once if the
page is replaced by a full reload.

## Security notes

- The automation Content Process runs with `--automation-content`: no frame
  streaming, and requests to loopback / private networks are rejected both by
  the API and inside the content process. Raw `/request` calls reject literal
  loopback / private URLs up front; treat the server as a trusted local client
  (bind to loopback and use a token).
- Session profiles contain live session tokens. Directories are created with
  mode `0700`; still, protect `<automation-data>` like a password store.
- Bind to loopback or set a token. The server refuses to listen on a public
  address without one.
- reCAPTCHA and Google Sign-In are never automated or bypassed. Login is a
  human step in a real browser window.
