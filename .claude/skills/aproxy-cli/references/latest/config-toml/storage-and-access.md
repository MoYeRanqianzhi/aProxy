# config.toml reference: disk cache, access control, logs and history

Part of [config-toml.md](../config-toml.md), which lists the other parts. `disk_cache`, `allowed_hosts` / `allowed_origins`, `log_file`, `history`.

## disk_cache

Whether request and response bodies larger than 1 MiB spill to temporary files under
`<APROXY_HOME>/spool/<pid>-<hash>/` instead of staying in memory. On (the built-in default, or
settings.json `defaults.disk_cache`), memory per request stays flat whatever the body size; off,
every body is held in RAM. Bodies under 1 MiB stay in
memory either way, so leave it on unless disk writes are unwanted. When a disk write or read fails,
the request ends with `507` `x-aproxy-error: local_storage_failed` (`details.op` is
`write_request`, `read_request` or `write_response`), or the same error as the SSE frame if
heartbeat headers were already sent, rather than falling back to memory.
No effect with `delivery = "forward"` (behaviors.md, "Disk cache (spool)").

## allowed_hosts / allowed_origins

Inbound checks that stop web pages in a local browser from using the proxy, and the key it injects.
CLI clients send no `Origin` header and reach the proxy as `127.0.0.1:PORT`, so they need neither
field. A refused request gets a local 403 that names the field to change and is logged as warn; it
is never forwarded, never gets a key injected and is never retried.

`allowed_hosts`, a Host header check against DNS rebinding:

- Active when `listen_addr` is a loopback address (`127.0.0.0/8`, `::1`, `localhost`) or when the
  list is non-empty. A non-loopback listener with an empty list does no Host check, because LAN and
  container clients use many different names.
- When active, it accepts `localhost`, `127.0.0.1`, `[::1]`, the host part of `listen_addr`
  (except `0.0.0.0` and `[::]`), and the list entries. Entries add to these names; they do not
  replace them.
- Compared case-insensitively, ignoring ports on both sides. Write host names only
  (`"host.docker.internal"`); a URL entry is reduced to its host.
- Add the name a container or another machine uses to reach the proxy.

`allowed_origins`, an Origin header check:

- A request carrying an `Origin` header is refused unless it matches an entry. Only browsers and
  Electron or WebView apps send `Origin`.
- Entries match case-insensitively and ignore a trailing `/`; write them as the browser sends them,
  scheme and port included: `"http://localhost:5173"`.
- Applies whatever `listen_addr` is.
- Desktop or web clients such as Cherry Studio or Open WebUI send `Origin`; add theirs, which
  `details.origin` carries and the error message quotes.

For both: an entry `"*"` turns that check off. An empty list means the built-in policy, not "allow
all" or "deny all".

```toml
allowed_hosts   = ["host.docker.internal"]
allowed_origins = ["http://localhost:5173"]
```

A refused request returns `403` with `x-aproxy-error: inbound_host_rejected`
(`details.host`) or `inbound_origin_rejected` (`details.origin`), and is logged as warn with
`入站请求被拒绝…` ("inbound request rejected"); it is never forwarded, never gets a key injected and
is never retried. The error body is the standard error JSON (behaviors.md, After the commit).

## log_file

The path of the instance's daemon log. Unset (the default), every start, restarts included, writes
a new file with a random name in `<APROXY_HOME>/logs/`. The name carries no port, so get the path
from `aproxy status` or `aproxy logs` rather than guessing it.

- Precedence: `--log-file PATH` (a `start` option, that run) > `log_file` > random name. There is no settings.json
  default; each instance chooses its own.
- `~` expands to the user's home directory; a relative path is resolved against `<APROXY_HOME>`,
  never against the working directory, which differs between `start`, `restart` and watchdog
  restarts. Missing parent directories are created.
- Set it for a stable path, or to keep a log after the instance stops: aProxy deletes `.log` files
  in `<APROXY_HOME>/logs/` that no running or restorable instance refers to whenever it lists
  instances. Files outside that directory are never deleted by aProxy.
- A custom file is appended to across restarts, but emptied at start when it is over 2 MiB, and
  while running when it grows past settings.json `logs.rotate_mb`.
- Foreground instances (`--foreground`) log to the console and ignore it.

## history

```toml
[history]
body_budget_mb = 64
```

Each instance keeps a history of the requests it handled, in memory only: the last 1000 requests
(method, path, model, every attempt and why it was retried, outcome, timing), and for as many of
the most recent ones as fit, the request body, the body sent upstream on each attempt and each
response body. `body_budget_mb` caps the memory those bodies take together (default 64).

- When the bodies exceed the budget the oldest requests lose their bodies first; their records
  stay. A single body larger than the whole budget is not kept at all.
- `0` keeps no bodies. The records themselves are always kept; they are small (a few KiB each, at
  most 64 KiB).
- Lower it, or set it to `0`, when the instance must stay small or when request and response
  content should not sit in the instance's memory. Raise it to look further back when requests
  are large: 64 MiB holds the bodies of about thirty requests of 2 MiB each.
- The instance's memory grows by up to the budget plus about 16 MiB of reusable buffers, and
  stays there; it is not returned while the instance runs.
- Bodies are stored decoded (a gzip or brotli response is kept as text). Values aProxy knows to be
  secret - `api_key`, the proxy password, secret header values, secret `[[format]]` arguments and
  environment values - are replaced before a body or header is stored. Other secrets that travel
  inside a body (a token pasted into a prompt, a key some tool printed) are not recognised and are
  kept as they are.
- The history is lost when the instance stops or restarts. Nothing is written to disk.
- This version has no command that prints the history; it is read over the instance's control
  channel (ops `requests`, `request`, `body`, `subscribe`), which only the user who started the
  instance can open.
