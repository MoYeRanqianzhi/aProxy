# The official aproxy-format binary: roles, chains and tools

Working configurations built from the official roles. Read it when a user wants protocol
conversion, key rotation, routing, a retry policy or adaptive delivery and you would rather
configure than write code. Every role file and chain below was accepted by `aproxy-format check`.

## Parts

This reference is split by topic into the files below (paths relative to this file). Open only the part the task needs; a reference elsewhere to `examples.md, "<section>"` means the part that holds that section.

| File | Covers | Sections |
|---|---|---|
| [examples/setup.md](examples/setup.md) | Installing `aproxy-format`, wiring a role into config.toml, and the `init` starter scenarios. | [Install](examples/setup.md#install) · [How a role is wired in](examples/setup.md#how-a-role-is-wired-in) · [Starter scenarios (`init`)](examples/setup.md#starter-scenarios-init) · [claude-relay: one protocol converter](examples/setup.md#claude-relay-one-protocol-converter) · [key-rotation: several keys for one relay](examples/setup.md#key-rotation-several-keys-for-one-relay) · [openrouter: a channel plus conversion](examples/setup.md#openrouter-a-channel-plus-conversion) · [adaptive-relay: true streaming when it is safe](examples/setup.md#adaptive-relay-true-streaming-when-it-is-safe) |
| [examples/roles.md](examples/roles.md) | Every official role and its options. | [Role reference](examples/roles.md#role-reference) · [convert](examples/roles.md#convert) · [keys](examples/roles.md#keys) · [route](examples/roles.md#route) · [policy](examples/roles.md#policy) · [adaptive](examples/roles.md#adaptive) · [rewrite](examples/roles.md#rewrite) · [heartbeat](examples/roles.md#heartbeat) |
| [examples/recipes.md](examples/recipes.md) | Routing then converting per destination, a full chain, and the known conversion limits. | [Mixed channels: route, then convert per destination](examples/recipes.md#mixed-channels-route-then-convert-per-destination) · [A full chain](examples/recipes.md#a-full-chain) · [Known conversion limits](examples/recipes.md#known-conversion-limits) |
| [examples/tools.md](examples/tools.md) | `check`, `try` and `translate`. | [Tools: check, try, translate](examples/tools.md#tools-check-try-translate) · [check](examples/tools.md#check) · [try](examples/tools.md#try) · [translate](examples/tools.md#translate) |
