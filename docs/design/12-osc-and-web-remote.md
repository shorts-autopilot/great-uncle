# 12 · OSC and web remote

**Rank** 12 · **Order** 14 · **Size** M · **No format change** · **Depends on** 01 (executor state), 06 (blackout)

## Problem

The desk has exactly one input surface: a keyboard attached to the machine. A
tablet walking the stage, a show-control system, or a lighting network's cue
button all need to press `Go` without being the console. The project already
speaks open standards (GDTF, MVR later); OSC is the standard for this, and a
browser page is the cheapest remote that needs no install.

## Behaviour when this is done

```
openlight --osc-port 9000 --remote-port 8080
OSC listening on 0.0.0.0:9000; remote at http://127.0.0.1:8080 (token in ~/.openlight/remote.token)
```

```
/osc/openlight/executor/1/go          -> Go Executor 1
/osc/openlight/executor/1/fader 0.75  -> Executor 1 At 75
/osc/openlight/blackout 1             -> Blackout
/osc/openlight/fixture/5/at 0.5       -> Fixture 5 At 50
```

The browser page shows executor buttons, the grand master and a blackout
button, and prints the engine's reply for each press — over the same command
line the keyboard uses.

## Design

### New crate

```
crates/remote/
  src/lib.rs        Remote::start(engine: Arc<Mutex<Engine>>, config) -> Remote
  src/osc.rs        message/bundle decode + encode, address parsing
  src/http.rs       tiny HTTP/1.1 server, SSE stream, POST /command
  src/mapping.rs    address/value -> command line
  src/token.rs      token file, constant-time compare
```

The engine is reached only through `Arc<Mutex<Engine>>` and
`Engine::execute`, the same handle the console holds. No lighting logic, no
second state, no second listing.

### OSC

- Implemented from scratch (OSC 1.0 is a small binary format: padded
  null-terminated address, comma type tag, big-endian `i`/`f`/`s`/`b`):
  ~250 lines with careful padding, which is cheaper than a dependency for one
  protocol.
- Listener: one `UdpSocket` thread, 64 KiB buffer, parses bundles (with the
  `#bundle` tag and 64-bit timetag; timetags in the future are **ignored**, not
  scheduled — scheduling belongs to the engine tick and item 15).
- Address scheme, versioned by prefix so a future schema can coexist:

  | Address | Value | Command |
  |---|---|---|
  | `/openlight/executor/<n>/go` | — | `Go Executor <n>` |
  | `/openlight/executor/<n>/back` | — | `Back Executor <n>` |
  | `/openlight/executor/<n>/fader` | float 0..1 | `Executor <n> At <pct>` |
  | `/openlight/executor/<n>/flash` | int 0/1 | `Executor <n> Flash[ Off]` |
  | `/openlight/sequence/<n>/go` | — | `Go Sequence <n>` |
  | `/openlight/fixture/<n>/at` | float 0..1 | `Fixture <n> At <pct>` |
  | `/openlight/group/<n>/at` | float 0..1 | `Group <n> At <pct>` |
  | `/openlight/grand` | float 0..1 | `Master Grand At <pct>` |
  | `/openlight/blackout` | int 0/1 | `Blackout[ Off]` |
  | `/openlight/command` | string | the line, verbatim |

- Values are clamped and rounded to whole percent when building the command, so
  a float jitter of 0.7499999 does not produce `74.99999%` in the journal.
- Unknown addresses are ignored (OSC has no error channel) but counted and
  shown in `Status` as `osc: 3 unmatched`.
- No replies by default. `/openlight/command` with a string sends no reply
  either; feedback is the web page's job, because OSC feedback addresses are a
  schema of their own and not needed to press Go.

**The mapping is the design.** Every address compiles to command text, so the
remote inherits validation, locking (item 06), journaling (item 04) and undo
semantics for free. The test asserts the exact string for every address, so the
mapping cannot drift from the grammar.

### Web remote

- `TcpListener` + a hand-written HTTP/1.1 subset: one request per connection
  (`GET /`, `GET /events`, `POST /command`), `Connection: close`, no keep-alive,
  no chunked encoding, bounded header size (8 KiB) and body size (4 KiB).
- `GET /` serves one embedded HTML page (~120 lines, no framework, no external
  assets): executor buttons from `GET /state`, a command input, a log of
  replies.
- `GET /state` returns JSON (`{"executors":[{"number":1,"state":"cue 2"}],
  "blackout":false,"blind":false,"lock":true,"grand":0.75}`), built from the
  same listing/status code the shell uses.
- `POST /command` with a JSON body `{"line":"Go Executor 1"}` executes and
  returns `{"lines":["…"],"error":null}` with HTTP 200 even for a command error
  (the error is the payload, not a transport failure).
- `GET /events` is **Server-Sent Events**: `data: <state json>\n\n` on every
  revision change, plus a comment heartbeat every 15 s. SSE was chosen over
  WebSocket because it needs no handshake hash, no framing and no upgrade path —
  ~40 lines instead of ~200 — and it is enough for one-way state push.
- **Security.** A 32-byte random token, generated on first start and stored in
  `~/.openlight/remote.token` (mode 0600) or the path from
  `--remote-token-file`. Every request except `GET /` (the static page, which
  contains no data) requires `Authorization: Bearer <token>`; comparison is
  constant time. Default bind is `127.0.0.1`; `--remote-bind 0.0.0.0` is opt-in
  and prints a warning. `POST` requires `Content-Type: application/json` and an
  `Origin` that is either absent (curl) or the server's own host — a browser
  page on another site cannot drive the desk.
- **Rate limit.** A token bucket per connection: 50 commands/s, burst 100;
  excess is dropped with a `429` and counted in `Status`. A loop of OSC or HTTP
  commands must not be able to starve the engine tick (which holds the same
  mutex), so the remote dispatch is a bounded `sync_channel` of 256 handled by
  one thread; when full, the remote drops with a warning rather than blocking a
  socket read under the engine lock.

### Configuration and status

`--osc-port`, `--remote-port`, `--remote-bind`, `--remote-token-file`,
`--no-remote`. A `Remote` section in `Status` lists ports, the bound address,
and counters (`osc: 412 accepted, 3 unmatched`, `http: 12 clients, 0 rejected`).
Failures to bind are startup errors, not silent.

## Implementation plan

1. `osc.rs` codec + tests (padding, bundles, type tags, truncated packets).
2. `mapping.rs` + exhaustive mapping test.
3. `Remote::start` with the UDP thread and the bounded dispatch channel.
4. `http.rs`: request parser, router, `/`, `/state`, `/command`, SSE, token,
   origin and rate limit; tests with a real `TcpStream` on port 0.
5. The embedded page (plain HTML + `fetch` + `EventSource`).
6. Wire flags in `crates/console`, add the `Status` section, update README.

## Tests

- **osc** — round-trip of `i`, `f`, `s`, `b` and a bundle; a packet with a bad
  size is rejected, not panicking; an address with a trailing slash is
  unmatched.
- **mapping** — the exact command for all ten addresses; out-of-range floats
  clamp; `int` other than 0/1 is rejected.
- **http** — missing token → 401, wrong token → 401, correct token → 200; a
  foreign `Origin` → 403; oversized body → 413; the 51st command in a second →
  429; SSE sends a state event when the engine revision changes and closes
  cleanly on client disconnect.
- **remote** — 500 commands through the channel never block the sender and the
  engine ends in the expected state; lock (item 06) refuses remote commands
  exactly as it refuses typed ones.
- **console** — flags parse; a port already in use is a startup error naming
  the port.

## Boundaries

- **Inbound only.** No OSC feedback/query addresses, no OSCQuery, no mDNS
  discovery.
- **No TLS.** The token over plain HTTP is for a trusted show network; the docs
  say so and the default bind is loopback.
- No WebSocket, no REST beyond the three endpoints, no authentication accounts.
- No OSC scheduling by timetag (item 15 owns external time).

## Risks

- **A remote is a second writer.** Mitigated structurally: the engine mutex is
  the only writer, commands are queued and applied on one thread, and the rate
  limit is in front of the queue.
- **Token handling.** The token is a file, never printed in logs, and the
  embedded page does not contain it; the page asks the user to paste it once and
  keeps it in `sessionStorage`.
- **Hand-written HTTP.** A subset parser is a liability; it is bounded (8 KiB
  headers, 4 KiB body), rejects anything outside the subset, and is tested with
  malformed input rather than being trusted.
