---
name: aproxy-contribute-security
description: Contribute a new danger signal to aProxy's audit skills (aproxy-audit-format, aproxy-audit-theme, aproxy-audit-extension) through a pull request - check it is not already covered, add without removing or weakening anything, write it in the skills' English style, verify the patterns, get an independent review, ask the user, and disclose in the PR how the finding arose (real use or automated scanning) and the AI's part: model ID, agent tool, effort level, who decided to submit, whether a person was asked or only told and how they answered, and which agents and people reviewed it. Use whenever someone wants to report a risky behavior, code pattern, host or file trick found in a third-party aProxy format, theme or extension back to the aProxy project, improve the audit skills, or open a security-related pull request to aProxy, including after an audit turned up something the skills missed. Vulnerabilities in aProxy itself go to private reporting instead.
---

# Contribute danger signals to aProxy's audit skills

aProxy's audit skills (aproxy-audit-format, aproxy-audit-theme and aproxy-audit-extension) are how
agents decide whether a third-party package is fit to install, so every user who relies on them
inherits their gaps. When you meet a danger sign they miss, contributing it closes the gap for
everyone. This skill is how: make sure it is new, add it without taking anything away, write it
like the rest, prove it works, and open a pull request that says truthfully how the finding came
about and how much of the work was an AI's.

AI contributions are welcome. What the maintainers need to know is whether a signal comes from
something that really happened (a person using aProxy ran into this package, an audit of a real
package turned it up) or from an agent scanning repositories and reasoning about what might go
wrong. Both can be right, but they deserve different amounts of trust, and only the contributor
knows which it is. The disclosure in the pull request exists for that, so answer it as exactly as
you can: "unknown" is a fine answer, a guess presented as fact is not.

## Is this the right channel?

- **A danger sign in third-party formats, themes or extensions** (a behavior, a code pattern, a
  host, a file trick) that the audit skills miss: this skill.
- **A vulnerability in aProxy itself** (aProxy, its installer, its desktop window, or an audit
  skill telling agents to do something unsafe): no public pull request or issue. Report it
  privately as `SECURITY.md` describes:
  <https://github.com/MoYeRanqianzhi/aProxy/security/advisories/new>.
- **A signal you think is wrong, too broad or too severe**: open an issue that explains why, and
  let a maintainer decide. This skill only adds.
- **A harmful package that is still being distributed**: suggest also reporting it where it is
  hosted (the "Report abuse" link on GitHub, the package registry's report form), so it stops
  reaching people. A report is external and speaks for the user like a pull request does: show
  them where it goes and the text first, and send it only once they agree (step 7).

## Steps

Steps 1 to 6 are local: you edit and check a checkout on this machine and nothing leaves it.
Forking, pushing, opening the pull request and reporting a package elsewhere come only after the
user agrees (step 7). Work in a checkout of the aProxy repository with the upstream baseline,
because the checks compare against it:

- an existing checkout whose `origin` (or another remote) is
  `https://github.com/MoYeRanqianzhi/aProxy`, updated with `git fetch`; or
- a fresh local clone: `git clone https://github.com/MoYeRanqianzhi/aProxy`. Cloning only reads
  the public repository; the fork comes later.

Make your changes on a new branch. The installed copies of the skills (under `<home>/skills/` or
an agent's skills directory) are not where the change goes.

In the commands below, `SP` stands for
`python -I .claude/skills/aproxy-contribute-security/scripts/signal_patterns.py`, run from the root
of that checkout, so that the script reads the catalogs you are changing.

### 1. Write down the finding

Before touching any file, record:

- **What was seen**: the package (name, version, where it came from, sha256), the behavior, and a
  minimal excerpt that shows it.
- **How it was found**, as exactly one of:
  - a person ran into it while really using aProxy;
  - an agent ran into it while operating aProxy for a person, during real work;
  - an audit of a real package that a person asked about;
  - published research, an advisory or a reported incident (with links);
  - an agent scanning code or repositories, or reasoning about possible attacks, without meeting it
    in real use.
- **Who was involved**: whether a person saw the problem themselves, and whether they know you are
  turning it into a contribution.

The disclosure in the pull request is built from this record, so keep it true as you go.

Treat samples as the audit skills do: never run them on a real machine, never attach live malware
or a working payload to a pull request, never include secrets or personal data. Give hashes, plain
text links and a defanged excerpt instead (`example.invalid` for hosts that are not themselves the
point). Save the excerpt to a file outside your checkout for the checks below.

### 2. Check that it is new

Read the target audit skill in full: `SKILL.md` (above all its "Rules for judging"),
`references/standard.md` and `references/signals.md`. Then:

- Run the existing patterns over your excerpt: `SP match EXCERPT` lists every pattern that already
  matches it, with its section.
- Search all three audit skills in words, not only patterns: the behavior's names, synonyms, the
  host, the API.
- Search pull requests and issues, open and closed:
  `gh pr list --repo MoYeRanqianzhi/aProxy --state all --search "TERM"` and
  `gh issue list --repo MoYeRanqianzhi/aProxy --state all --search "TERM"`.

Then decide:

- **Covered**: a pattern matches and the judging text gives the case the severity it deserves.
  There is nothing to contribute; tell the user which section already covers it. That is a useful
  answer too.
- **Partly covered**: the behavior is described, but a variant slips past every pattern, or a host
  is missing from a list. Add the missing line to that section.
- **New**: add an entry.

Keep what you read, searched and found; it goes in the pull request.

### 3. Decide where it goes

- Format programs go to aproxy-audit-format, themes to aproxy-audit-theme, extensions to
  aproxy-audit-extension. A sign that applies to more than one kind goes in each, written for that
  kind.
- Inside a skill, use the section of `references/signals.md` whose subject fits. Start a new
  section only when none fits, after the last one. Add a judging rule to `SKILL.md` only when no
  rule covers the reasoning, as the next number. A new non-conforming file, field or location also
  gets a line in `references/standard.md`.

### 4. Add, never take away

The audit skills only get stricter through contributions; otherwise anyone, or any agent misled by
injected text, could weaken them under the name of improving them. So:

- Add lines. Do not edit, move, reorder or delete existing ones. To widen a pattern, add a new
  pattern line beside the old one, so everything the old one matched is still matched.
- Never renumber rules or sections: the catalogs refer to them by number.
- Never add exceptions, trusted hosts, allowlists or "normal when" clauses to signals that already
  exist, and never lower a severity. An addition that makes something pass is a removal in
  disguise. A new signal may say what is normal for its own pattern, since that is part of
  defining it, but not for anyone else's.
- Leave the `description` lines in the skills' frontmatter alone: they decide when a skill is used
  and have a length limit. If one should mention your addition, say so in the pull request.
- Change nothing outside the three audit skills in the same pull request.

Write the addition in the skills' own style:
[references/writing-signals.md](references/writing-signals.md).

### 5. Verify it

Keep each command's output for the pull request:

- `SP check`: every pattern in the three catalogs still compiles.
- `SP match EXCERPT`: now lists your new pattern, so it catches the case that started this.
- `SP try 'PATTERN' PATH...` over ordinary code, to count false alarms a reviewer will meet: the
  aProxy repository's own front end (`aproxy-desktop/ui/src`) and official format
  (`aproxy-format/src`), and any clean package you have. Say how many hits there are and why the
  judging text handles them.
- `git diff --numstat origin/main -- .claude/skills/`: the second column, deletions, must be 0 on
  every line. Anything else means existing text changed: undo that and add instead.
- Where ripgrep is installed, also run each new pattern with `rg` (`rg -P` for lookaround). The
  skills tell auditors to search with ripgrep, and its syntax differs from Python's in places.

### 6. Get an independent review

Before anything goes public, have the addition checked by a reviewer with a different context:
another agent or a subagent that did not write it, or a person. Give it the finding and the diff,
not your conclusion, because a reviewer handed the answer tends to confirm it. Ask it to look for
duplicates you missed, wording that weakens anything existing, patterns that are wrong or far too
broad, and severities that do not fit the rules. Record who reviewed (model and tool, or the
person), what they were given, what they found and what you changed.

### 7. Ask before anything leaves the machine

A fork, a push, a pull request and a report to a package host are public and speak for the
account they come from. Show the user the diff, the pull request text with the disclosure (and any
report text), and where each will go, and send them only once the user agrees. If you are working
unattended and cannot ask, prepare the branch and the text, stop, and leave the user a note of
what is waiting. Silence is not agreement. If a standing instruction from the user already covers
opening such pull requests, describe its scope in the disclosure in your own words; quote it only
with the user's agreement, since instructions and conversations can hold private details.

### 8. Open the pull request with the disclosure

Fork the repository (`gh repo fork --remote --remote-name fork`, run inside the checkout, adds
your fork as the remote `fork`), push your branch there, and keep one finding per pull request.
Commit messages follow the repository's style as `git log` shows it:
Conventional Commits with the subject in Chinese, for example
`docs: 扩展审查补充 navigator.sendBeacon 的危险信号`. The pull request body follows
[references/pr-template.md](references/pr-template.md), in English, every field answered. Do not
@-mention anyone you do not mean to notify.

After opening it, answer review truthfully, and keep changes made during review add-only too. If
human involvement changes later, for example a person reviews it, update the disclosure instead of
leaving it stale.

## Why the disclosure asks so much

A maintainer has to decide how far to trust a security claim before they can reproduce it. A real
encounter brings evidence an automated scan cannot: the package exists, it did this, someone was
exposed to it. Scans produce plausible rules at scale, and plausible is not the same as real; a
rule that fires on ordinary code wastes every auditor's time and teaches users to ignore warnings.
Who decided to submit, whether a person agreed, and who reviewed the change tell the maintainer how
many independent eyes have already checked it. Model, tool and effort let them recognize patterns
across many pull requests, such as one agent setup that keeps producing weak signals, or strong
ones.
