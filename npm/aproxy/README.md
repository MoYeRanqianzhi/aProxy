# @meowo/aproxy

Local API proxy with **infinite retries** for agent workloads.

Upstream rate limits, broken streams, timeouts — aProxy catches the request
locally: on failure it retries without limit (exponential backoff, capped and
configurable), keeps streaming responses alive with injected SSE heartbeats,
and replays the successful response byte-for-byte. Your client never notices
the storm upstream; it only notices that the request took a little longer.

## Install

```sh
npm install -g @meowo/aproxy
```

The right platform binary (windows / linux-glibc / linux-musl / macOS, x64 or
arm64) is installed automatically via `optionalDependencies` — no postinstall
downloads, everything comes from the npm registry.

## Quick start

```sh
# 1. Configure the upstream (writes ~/.aproxy/config.toml)
aproxy config --baseurl https://api.anthropic.com --api-key sk-ant-...

# 2. Start (background daemon)
aproxy

# 3. Point your agent's API base URL at the local proxy
#    https://api.anthropic.com  →  http://127.0.0.1:12345
```

## Links

- Full documentation: <https://github.com/MoYeRanqianzhi/aProxy>
- Releases: <https://github.com/MoYeRanqianzhi/aProxy/releases>

## License

MIT
