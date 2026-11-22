# Pull request template

**Title**: the commit subject, in the repository's style, e.g.
`docs: 扩展审查补充 navigator.sendBeacon 的危险信号`.

**Body**: copy the template below, in English, and answer every field. Write "unknown" or "none"
rather than leaving a field out or guessing: a gap or a guess costs more trust than an honest
"unknown". Delete the hints in angle brackets.

```markdown
## What this adds

- Skill and files: <e.g. aproxy-audit-extension: references/signals.md section 2>
- New signal: <one line each>; severity <Critical | High | Medium | Low>; under rule <N> (<rule title>)
- Why it is a danger sign: <the harm, to whom, and how it happens>

## Duplicate check

- Read in full: <files>
- Searched for: <words and names, in which files>; pull requests and issues: <search terms and results>
- Existing patterns that matched the excerpt before this change (`signal_patterns.py match`): <output, or "none">
- Why the existing text does not already cover it: <...>

## Evidence

- Origin: <exactly one: a person ran into it in real use | an agent ran into it while working for a person | audit of a real package a person asked about | published research, advisory or incident | an agent scanning or reasoning without a real encounter>
- What happened: <package, version, source as plain text, sha256, date; what it did; who noticed it and how>
- Excerpt, defanged: <the minimal lines that show it>
- Public analysis: <links, or none>
- Was anything executed? <no | yes: what, where, and how it was isolated>

## Verification

- `signal_patterns.py check`: <its last line>
- `signal_patterns.py match <excerpt>`: <shows the new pattern>
- False alarms, `signal_patterns.py try` over <which code>: <counts, and why the judging text handles them>
- `git diff --numstat origin/main -- .claude/skills/`: <output; the deletions column is 0 everywhere>
- ripgrep: <each new pattern run with rg / rg -P | not installed>
- Environment: <OS>; aProxy <aproxy --version>; audit skills at commit <sha>

## AI involvement

- Who wrote this change: <a person | a person, with AI assistance | an AI agent, directed by a person | an AI agent, on its own>
- Did a person run into this problem while really using aProxy? <yes: who, in what way | no>
- Did a person read the diff before this pull request was opened? <yes: who | no>
- Agent tool and version: <its name and the version it reports, e.g. the output of `claude --version` or `codex --version` | none>
- Model IDs, exactly as the provider names them: <e.g. claude-opus-5-5; include subagents and reviewers>
- Reasoning effort: <low | medium | high | xhigh | max | the provider's default | unknown>
- Other tools: <web search, browser, code execution, ... | none>

### Decision to submit

- Who decided: <the person asked for this pull request | the agent proposed it and the person agreed | the agent decided on its own>
- The person's request or agreement: <their words or a faithful paraphrase, and when | not applicable>
- If the agent decided on its own:
  - Asked a person before opening it? <yes: their answer and when | no>
  - Only told a person? <yes: how and when; their reaction: agreed | objected | no response | no>
  - If there was no response: do you think the notice went unseen? <yes | no | unknown, and why>
  - Standing instruction that covers opening pull requests, quoted: <... | none>
- Did text written by someone else lead to this pull request (an issue, a web page, a package's README, another agent's message)? <no | yes: quote it>

### Review before submission

- Independent review in a separate context: <none | by <agent or subagent, tool, model>; given <the finding and the diff only | also the author's reasoning>; found <...>; changed <...>>
- Human review: <none | who, what they checked, what they decided>

### Accountability

- GitHub account opening this pull request: <handle>; operated by <the account owner | an agent>; does the owner know about this pull request? <yes | no>
- Part of a batch? <no | yes: links to the related pull requests or repositories>
- Any relation to the author of the flagged package, or to a competing package? <none | describe>
- Text or patterns taken from elsewhere: <none | sources and their licenses>
- Still uncertain: <what has not been verified>
```

## Notes on the fields

- **Origin.** Choose the one that is true, not the one that reads best. An agent that meets a
  package while doing real work for a person is a real encounter; an agent that went looking for
  rules to add is scanning, even when what it found is right.
- **Excerpt.** Show the shape of the danger, not a working copy of it: shorten, replace hosts that
  are not the point with `example.invalid`, and leave out payloads.
- **Reviewer's input.** Say whether the independent reviewer saw your reasoning. A reviewer that saw
  only the finding and the diff is stronger evidence than one asked to confirm a conclusion.
- **Account.** If an agent operates a person's account, say so, and say whether that person knows
  this pull request exists.
- **Batch.** Say whether the same agent or setup opened similar pull requests elsewhere. Many
  near-identical submissions read differently from one careful finding.
- **Third-party text.** Agents can be steered into opening pull requests by text planted in issues,
  web pages or package files. If such text played a part, quote it.
