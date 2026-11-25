# config.toml reference: upstream, listening address and limits

Part of [config-toml.md](../config-toml.md), which lists the other parts. `base_url`, `listen_addr`, `api_key`, headers, the outgoing proxy, size limits and timeouts.

## base_url

The upstream API root. aProxy appends the client's full request path and query unchanged, so
`base_url` plus the path the client sends must form the real upstream URL: if the client's base
URL is `http://127.0.0.1:12345/v1` and it calls `/v1/chat/completions`, set
`base_url = "https://api.example.com"`, not `.../v1`.

- Must start with `http://` or `https://` (any case) and must not contain `?` or `#`, which would
  turn the appended path into part of the query.
- A trailing `/` is removed.
- It may come from settings.json `defaults.base_url`. Credentials in `defaults` are tied to that
  value: an instance whose own `base_url` names another target does not inherit them (see
  [Precedence](editing.md#precedence)).

## listen_addr

The `host:port` the instance listens on. The default `127.0.0.1:12345` is reachable only from this
machine.

- The port is required; without one, start fails with `config_invalid_value` on `listen_addr`
  (`listen_addr has no port or the port is not valid`).
- Every instance needs its own port. A second instance on a port that another aProxy instance
  already uses is refused, even with a different host part.
- Port `0` lets the OS choose. The instance reports the actual address, and that port is pinned in
  its desired-state record: `restart`, `restore` and watchdog respawns return to the same port
  while the config still says `0`. Once the config names a fixed port, the config wins and the
  pinned port is discarded. Clients need a fixed address, so use `0` only for throwaway tests.
- A non-loopback address (`0.0.0.0`, a LAN IP) lets every host that can reach the port use the
  upstream through the proxy, with any key the instance injects. `aproxy start`, the startup log and
  `aproxy doctor` warn with a message containing `不是回环地址` ("is not a loopback address"). It
  also switches off the default Host check (see [allowed_hosts](storage-and-access.md#allowed_hosts--allowed_origins)).

## api_key

Injects a key into every forwarded request: sets `Authorization: Bearer <key>` and
`x-api-key: <key>`, replacing whatever the client sent in those headers. Setting both serves
OpenAI-style and Anthropic-style upstreams and keeps a stale client key from reaching the upstream
through the other header. Use it when the client cannot hold the key or you want one place to
change it. For an upstream that expects the key in another header, such as Gemini's
`x-goog-api-key`, use `override_headers` instead. It may come from settings.json `defaults.api_key`;
that value is inherited only by instances whose `base_url` names the same target as
`defaults.base_url` ([Precedence](editing.md#precedence)).

## extra_headers / override_headers

Tables of header name to value.

```toml
extra_headers    = { "http-referer" = "https://example.com", "x-title" = "my-agent" }
override_headers = { "user-agent" = "my-agent/1.0" }
```

- `extra_headers` adds a header only when the client did not send it (names compare
  case-insensitively): use it for defaults the client may override.
- `override_headers` always sets the header, replacing the client's value: use it to force a value.
- Order of application: `api_key`, then `override_headers`, then `extra_headers`. An
  `Authorization` entry in `override_headers` therefore beats `api_key`, and `extra_headers` never
  replaces a header that `api_key` set.
- An `accept-encoding` entry in `override_headers` reaches the upstream only on requests that get
  no keepalive. Keepalive requests always go upstream with `accept-encoding: identity`, because
  heartbeats cannot be inserted into a compressed stream. When your entry is shadowed this way, the
  log shows a warn at start: `override_headers 里的 accept-encoding 对保活适用的请求不生效`
  ("accept-encoding in override_headers does not apply to keepalive requests").
- An entry whose name or value is not a legal HTTP header (a space in the name, a line break in the
  value) is skipped without any message; verify the upstream receives a header that matters. Do not
  list one header twice with different capitalization: which entry wins is unspecified.
- CLI: `aproxy config set extra_headers.KEY VALUE` and `aproxy config set override_headers.KEY
  VALUE` add entries one at a time; `aproxy config unset extra_headers` and `aproxy config unset
  override_headers` empty both tables. Quote a header name that contains dots, as in
  `extra_headers."x.y"`.

## proxy / proxy_username / proxy_password

The outbound proxy for requests to the upstream. Unset, aProxy honors the proxy variables the
daemon inherited from the shell that started it (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`,
`NO_PROXY`). Set, every upstream request goes through this proxy and those variables are ignored.

- Schemes: `http`, `https`, `socks4`, `socks4a`, `socks5`, `socks5h`. Use `socks5h` when host names
  must be resolved by the proxy.
- Credentials can be embedded (`socks5://user:pass@127.0.0.1:1080`) or given separately.
  `proxy_username` (with `proxy_password`) replaces embedded credentials; `proxy_password` without
  `proxy_username` is ignored. Username or password without `proxy` fails start.
- Downloads by `aproxy install` do not use this; they use settings.json
  `update.download_proxy`.
- `aproxy config unset proxy` clears the URL; also `unset proxy_username` and `unset
  proxy_password` when they were set.

## max_request_mb

The largest request body accepted, in MB (default 128, or settings.json
`defaults.max_request_mb`). A larger body gets `413` `x-aproxy-error: request_too_large` (`details.limit_bytes`) and
is not forwarded. `0` = no limit, which lets one request grow memory or disk use without bound.
Raise it for long agent sessions with images, only when you see that 413. Also enforced with
`delivery = "forward"`.

## max_response_mb

The largest upstream response aProxy buffers, in MB (default 256). A larger response is a
deterministic failure and is not retried: the client gets `502` `x-aproxy-error:
response_too_large` (`details.limit_bytes`), or the same error as the SSE frame if heartbeat
headers were already sent (behaviors.md, After the commit). Raise it for instances that relay large
files. `0` is treated as 1 MB, not as unlimited. No effect with `delivery = "forward"`.

## timeouts

Limits on talking to the upstream. Each timeout counts as a network error and is retried; each has
its own failure code in logs and in `status`'s latest error.

```toml
[timeouts]
connect_secs = 30      # TCP/TLS connection
first_byte_secs = 300  # from sending the request until the response headers arrive
read_idle_secs = 300   # gap between two reads of the response body
```

- `connect_secs` (default 30): time allowed to establish the TCP/TLS connection. On expiry the
  attempt fails with `upstream_connect_timeout`.
- `first_byte_secs` (default 300): time from sending the request (connection setup and upload of
  the request body included) until the upstream sends its response headers. On expiry the attempt
  fails with `upstream_first_byte_timeout`. Set it above the longest time an upstream can stay
  silent before its headers while healthy: queueing before the first token, or a
  `"stream": false` request that sends nothing until the whole answer is ready.
- `read_idle_secs` (default 300): the longest gap between two reads of the response body. It is
  re-armed on every read, so it does not cap the total duration of a response. On expiry the
  attempt fails with `upstream_read_idle_timeout`.
- `0` = no limit for that timeout. There is no limit on total request time. None of these affects
  the client's own timeouts (clients.md).
