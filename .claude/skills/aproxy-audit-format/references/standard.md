# What a well-behaved format program looks like

What a conforming format looks like. Each departure is a finding (SKILL.md, step 4); how severe
depends on what it exposes. The protocol itself, frame by frame, is in the aproxy-format skill
(`references/protocol.md`); this file covers what matters for the user's data.

## Contents

- [The [[format]] entry](#the-format-entry)
- [Stages and body access](#stages-and-body-access)
- [Protocol promises about data](#protocol-promises-about-data)
- [Credentials](#credentials)
- [Files and data](#files-and-data)
- [Control](#control)
- [Network and other programs](#network-and-other-programs)
- [Releases](#releases)

## The [[format]] entry

| Key | Conforming |
|---|---|
| `command` | a fixed path to the program, or a bare name found in `<home>/bin/` or a system or package-manager directory; not a downloads or temporary folder, not a directory other users can write to |
| `args` | the program's own subcommand and files; no inline code (`python -c`, `node -e`, `powershell -Command`, `sh -c`, `cmd /c`) and no URLs to fetch code from |
| `env` | the variables the program's documentation names, carrying only the secrets its job needs |
| `process` | `spawn` or `persistent`, as the documentation says |
| `stages` | what the job needs (next section) |

## Stages and body access

The official aproxy-format roles are the reference. Compare a third-party program with the role
closest to its job:

| Job | Official role | Stages | `request_body` |
|---|---|---|---|
| choose or rotate keys | keys | `request`, `retry` | `none`; `full` only for sticky selection by conversation |
| route by model, fail over between channels | route | `request`, `retry`; `response` with model aliases | `full` |
| convert between protocols | convert | `request`, `retry`, `response`, `stream` | `full` |
| decide whether to retry | policy | `decide` | `none` |
| learn which requests can be forwarded | adaptive | `request`, `decide`, `notify` | `digest` for `request` and `notify`, `none` for `decide` |
| rewrite models, headers or fields | rewrite | `request`, `retry` | `none` for headers only, `full` for body fields |
| style keep-alive beats | heartbeat | `heartbeat` | (no request body) |

`request_body` is a privacy contract: a program that only votes never sees the conversation.

## Protocol promises about data

- **Reports** describe what was done, in `summary` and `actions`, never credentials, header values
  or body content. A key is reported by its id (`key_select` with `key_id`).
- **`state`** is at most 64 KiB, private to the program and the request: ids, never keys.
- **stderr** carries diagnostics. aProxy replaces the secrets it knows about before writing it to
  the instance log, which does not cover everything a client sends: never log headers or bodies.
- **The hello reply** is honest: `stages` lists every stage the program implements, and `name` and
  `version` are the program's own (the official binary answers with the role name, such as `keys`,
  and its own version).
- **Heartbeat replies** are keep-alive bytes: comments or the protocol's own ping events, never text,
  tool calls, usage or the end of a response.
- **Replies** come one per message, in order, within the advertised limits; a `persistent` worker
  exits at end of input.

## Credentials

- The program's own secrets come from environment variables its configuration names. Values are
  never written in its files: the official roles' files name variables, and the values live in the
  environment.
- Credentials seen in request headers are used only to build the request the program returns,
  never stored, logged or sent anywhere else.
- A format process inherits the instance's whole environment. It reads the variables its
  documentation names and never enumerates the rest.

## Files and data

- **Configuration files** are the program's own. The official roles read them once at start and
  never reload them: changes take effect when the format or the instance restarts. A third-party
  program may reload instead, and documents which it does.
- **Data** the program keeps on disk (state shared between workers and the control process,
  caches, what it has learned) goes in the data directory aProxy gives it, `APROXY_FORMAT_DATA`,
  where aProxy provides one; without it, the documentation says where the program writes.
- **Nothing else**: no files in the home's root, in `<home>/bin/`, beside the user's configuration
  files or in the user's own folders. Request content goes on disk only when the job needs it,
  only in the data directory, and documented.

## Control

- A program offers control by naming `control.args` in its hello reply; aProxy starts the same
  command with those arguments as a separate process, which speaks the same framing (hello, then
  one reply per message).
- Its methods belong to the program's own job (status, switches, parameters) and are documented.
- When a change made through control takes effect (written to its configuration and applied on
  restart, shared through a file in its data directory, or held in memory) is the program's
  choice, and is documented.

## Network and other programs

None, unless the documentation names the host, the program and the reason. No downloaded code and
no self-update: the user reviews and installs each version.

## Releases

Source on GitHub; release assets built by a workflow from the tagged commit, with sha256 digests
and, at best, build attestations; release notes that name the commit.
