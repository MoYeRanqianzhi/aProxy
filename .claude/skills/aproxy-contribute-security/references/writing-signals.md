# How the audit skills are written

An addition should read as if the same author wrote the whole catalog. These are the conventions
the three audit skills follow.

## Contents

- [Language](#language)
- [Entries in signals.md](#entries-in-signalsmd)
- [Patterns](#patterns)
- [Severity](#severity)
- [Rules in SKILL.md](#rules-in-skillmd)
- [standard.md](#standardmd)

## Language

- English, plain, American spelling ("behavior", "color").
- Instructions are imperative ("Search", "Read", "Rate"); judgments are statements ("Critical.").
- Give the reason in a clause, so the auditor can judge cases the text did not foresee: "...
  Critical, because the request goes there with the user's key and prompt."
- No capitals for emphasis and no MUST: the severity word carries the weight.
- Wrap lines at about 100 characters, like the existing files.
- Write for the agent doing an audit, not about the history of the catalog or who added what.

## Entries in signals.md

A section looks like this:

````markdown
## 9. Evading review

Rules 2 and 3.

```
# Python
pattern
pattern
# JavaScript
pattern
```

- <the case>: <Severity>. <Why, in a clause.>
- <another case>: <Severity>.
````

- The title says what the code does ("Where data goes", "Watching the user"), not a technique's
  jargon.
- The line under the title names the `SKILL.md` rules the section applies.
- The bullets say how to judge what a pattern finds: the harmful cases with their severity and,
  for a new pattern, what an ordinary hit looks like.
- Lists of hosts or names: in backticks, grouped by what they are, the most telling first.
- Examples are minimal and defanged: `example.invalid` for hosts that are not themselves the point,
  no working payloads, no real secrets.
- Every catalog has a Contents list. A new section gets a line there, at the end.

## Patterns

- Written for ripgrep, which uses Rust's regex syntax: `(?i)` inline for case-insensitive matching,
  `\x{200B}` for a Unicode character, `\b` for word boundaries.
- Ripgrep does not support lookaround unless run as `rg -P`. Put such patterns in a group whose
  comment line says so, e.g. `# Enumerating the environment (rg -P)`.
- One pattern per line; related alternatives on one line with `|`. Group lines by language with
  `# Language` comment lines when they differ by language.
- Aim at the dangerous construct, not at everything near it: a pattern that hits most ordinary
  code makes auditors skim. A broad pattern is acceptable when its bullets say how to tell the
  harmful hits apart.

## Severity

The four words and what they mean are defined in each `SKILL.md` under "Severity and verdict":

| Severity | Meaning |
|---|---|
| Critical | evidence of harm or deception |
| High | a risk the user cannot assess, or power far beyond the purpose |
| Medium | a weakness worth knowing that is not harmful by itself |
| Low | a departure from the standard with no security effect |

Rate the harmful case, not the pattern: the same pattern can be normal in one place and Critical
in another, and the bullets say which is which. Name the rule the rating rests on.

## Rules in SKILL.md

Numbered, starting with a bold phrase that states the principle, followed by the reasoning and the
severities. A new rule goes at the end with the next number, so existing references keep pointing
at the right rule.

## standard.md

Tables and lists of what conforms. A new non-conforming file type, field or location gets a new
row or bullet; existing rows stay as they are.
