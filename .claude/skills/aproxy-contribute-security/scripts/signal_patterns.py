#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Run the danger-signal patterns of aProxy's audit skills.

  check                 compile every pattern in the three catalogs
  match FILE...         list the existing patterns that match the files (the duplicate check)
  try PATTERN PATH...   show what one pattern matches in files or directories (true and false hits)

The catalogs are the code blocks without a language tag in
aproxy-audit-*/references/signals.md; lines starting with "#" inside them are comments. By default
the script reads the catalogs that sit next to its own skill: in an aProxy checkout that is
.claude/skills/, where a contributor's changes are. --skills DIR points somewhere else.

The patterns are written for ripgrep. Python's re reads them the same way except for "\\x{HHHH}"
escapes, which are translated here; lookaround, which plain ripgrep rejects, is accepted by both
re and `rg -P`. Files are read as data, at most 8 MiB each, and never executed. Run the script with
python -I from outside any directory you are checking, so nothing in it can be imported.

  python -I .claude/skills/aproxy-contribute-security/scripts/signal_patterns.py check
  python -I .claude/skills/aproxy-contribute-security/scripts/signal_patterns.py match /tmp/excerpt.js
  python -I .claude/skills/aproxy-contribute-security/scripts/signal_patterns.py try '\\bfetch\\s*\\(' aproxy-desktop/ui/src
"""
import argparse
import glob
import os
import re
import sys

LIMIT = 8 << 20
FENCE = re.compile(r'^\s*```(.*)$')
HEX = re.compile(r'\\x\{([0-9A-Fa-f]{1,6})\}')
SKIP_DIRS = {'.git', 'node_modules', 'target', 'dist', '.worktrees'}


def skills_dir(arg):
    """The directory holding the aproxy-audit-* skills: --skills, or the parent of this skill."""
    if arg:
        return arg
    here = os.path.dirname(os.path.abspath(__file__))
    return os.path.dirname(os.path.dirname(here))


def catalogs(root):
    paths = sorted(glob.glob(os.path.join(root, 'aproxy-audit-*', 'references', 'signals.md')))
    if not paths:
        sys.exit(f'no aproxy-audit-*/references/signals.md under {root}; pass --skills DIR')
    return paths


def patterns(path):
    """Yield (line number, section title, pattern) for each pattern line of a catalog."""
    section, inblock, tagged = '', False, False
    with open(path, encoding='utf-8') as f:
        for number, line in enumerate(f, 1):
            fence = FENCE.match(line)
            if fence:
                if inblock:
                    inblock = False
                else:
                    inblock, tagged = True, bool(fence.group(1).strip())
                continue
            if not inblock and line.startswith('## '):
                section = line[3:].strip()
                continue
            text = line.strip()
            if inblock and not tagged and text and not text.startswith('#'):
                yield number, section, text


def compile_pattern(text):
    """Compile a ripgrep pattern with Python's re: only \\x{HHHH} needs translating."""
    return re.compile(HEX.sub(lambda m: '\\U%08x' % int(m.group(1), 16), text))


def read_lines(path):
    """A file's lines as text; undecodable bytes become U+FFFD instead of stopping the search."""
    with open(path, 'rb') as f:
        data = f.read(LIMIT + 1)
    if len(data) > LIMIT:
        print(f'{path}: larger than 8 MiB; only the first 8 MiB was searched', file=sys.stderr)
        data = data[:LIMIT]
    return data.decode('utf-8', errors='replace').splitlines()


def files_under(paths):
    """The files named, and every file below the directories named."""
    for path in paths:
        if os.path.isdir(path):
            for dirpath, dirnames, filenames in os.walk(path):
                dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
                for name in sorted(filenames):
                    yield os.path.join(dirpath, name)
        else:
            yield path


def around(line, match, width=80):
    """The match with some context; minified code puts whole programs on one line."""
    start = max(0, match.start() - width)
    return line[start:match.end() + width].strip()


def first_hit(rx, samples):
    for path, lines in samples:
        for number, line in enumerate(lines, 1):
            found = rx.search(line)
            if found:
                return path, number, line, found
    return None


def cmd_check(args):
    total = errors = 0
    for catalog in catalogs(skills_dir(args.skills)):
        for number, _section, text in patterns(catalog):
            total += 1
            try:
                compile_pattern(text)
            except re.error as e:
                errors += 1
                print(f'{catalog}:{number}: {e}: {text}')
    print(f'{total} patterns checked, {errors} errors')
    return 1 if errors else 0


def cmd_match(args):
    samples = [(path, read_lines(path)) for path in files_under(args.files)]
    matched = 0
    for catalog in catalogs(skills_dir(args.skills)):
        skill = os.path.basename(os.path.dirname(os.path.dirname(catalog)))
        for number, section, text in patterns(catalog):
            try:
                rx = compile_pattern(text)
            except re.error:
                continue
            hit = first_hit(rx, samples)
            if hit:
                matched += 1
                path, line_number, line, found = hit
                print(f'{skill} signals.md:{number} [{section}] {text}')
                print(f'    {path}:{line_number}: {around(line, found)}')
    print(f'{matched} existing patterns match' if matched else 'no existing pattern matches')
    return 0


def cmd_try(args):
    try:
        rx = compile_pattern(args.pattern)
    except re.error as e:
        print(f'the pattern does not compile: {e}')
        return 1
    lines_hit = files_hit = 0
    for path in files_under(args.paths):
        count = 0
        for number, line in enumerate(read_lines(path), 1):
            found = rx.search(line)
            if found:
                count += 1
                if count <= args.show:
                    print(f'{path}:{number}: {around(line, found)}')
        if count:
            files_hit += 1
            lines_hit += count
            if count > args.show:
                print(f'{path}: {count - args.show} more')
    print(f'{lines_hit} matching lines in {files_hit} files')
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--skills', help='the directory holding the aproxy-audit-* skills')
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('check', help='compile every pattern in the catalogs')
    match = commands.add_parser('match', help='existing patterns that match these files')
    match.add_argument('files', nargs='+')
    trial = commands.add_parser('try', help='what one pattern matches')
    trial.add_argument('pattern')
    trial.add_argument('paths', nargs='+')
    trial.add_argument('--show', type=int, default=5,
                       help='matching lines to print per file (default 5)')
    args = parser.parse_args()
    return {'check': cmd_check, 'match': cmd_match, 'try': cmd_try}[args.command](args)


if __name__ == '__main__':
    sys.exit(main())
