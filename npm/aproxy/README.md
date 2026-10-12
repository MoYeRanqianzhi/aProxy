# @meowo/aproxy

Local API proxy with **infinite retries** for agent workloads.

Upstream rate limits, broken streams, timeouts — aProxy catches the request
locally: on failure it retries without limit (exponential backoff, capped and
configurable), keeps streaming responses alive with injected SSE heartbeats,
and replays the successful response byte-for-byte. Your client never notices
the storm upstream; it only notices that the request took a little longer.

## Install

Two packages carry the same proxy and command line; install one of them (both
provide the `aproxy` command):

```sh
npm install -g @meowo/aproxy            # with the desktop window (aproxy desktop)
npm install -g @meowo/aproxy-headless   # without it: servers, containers
```

The right platform binary (windows x64/x86/arm64, linux x64/arm64 with glibc
or musl, macOS arm64/x64) is installed automatically via
`optionalDependencies` — no postinstall downloads, everything comes from the
npm registry.

On Linux the desktop build links WebKitGTK: it needs WebKitGTK 2.40 or later
(`libwebkit2gtk-4.1-0` on Debian/Ubuntu) and glibc 2.35 or later, and without
them it cannot start at all — use `@meowo/aproxy-headless` there. On musl
systems (Alpine) both packages carry the same headless binary.

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
