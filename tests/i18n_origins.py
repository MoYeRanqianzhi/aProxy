"""Check Chinese catalog literals against the source of the original CLI: python tests/i18n_origins.py."""

import ast
from pathlib import Path
import re
import subprocess


BASELINE = "390cb492"
ROOT = Path(__file__).resolve().parents[1]
# Match Rust strings before looking for field names inside a message definition. Raw
# strings have matching hash delimiters; a backslash-newline continues a normal string.
TOKEN = re.compile(r'//[^\n]*|/\*.*?\*/|(?:br|r)(?P<h>\#*)".*?"(?P=h)|b?"(?:\\.|[^"\\])*"|b?\'(?:\\(?:u\{[0-9a-fA-F]+\}|x[\da-fA-F]{2}|.)|[^\'\\\n])\'|\w+|[^\s]', re.S)
FIELD = re.compile(r"\{(?:[A-Za-z_][\w]*|\d+)?(:[^{}]+)?\}")


def text(literal):
    first = literal.index('"')
    if 'r' in literal[:first]:
        return literal[first + 1 : literal.rindex('"')]
    value = re.sub(r"\\\r?\n\s*", "", literal[first:])
    value = re.sub(r"\\u\{([0-9a-fA-F_]+)\}", lambda m: chr(int(m[1], 16)), value)
    return ast.literal_eval(value.replace("\n", r"\n"))


def normalized(value):
    return FIELD.sub(lambda m: "{" + (m[1] or "") + "}", value)


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT).decode("utf-8")


def baseline_texts():
    values = set()
    dirs = ("src", "aproxy-format/src", "aproxy-ipc/src", "aproxy-desktop/src", "aproxy-envelope/src")
    for name in git("ls-tree", "-rz", "--name-only", BASELINE, "--", *dirs).split("\0"):
        if not name.endswith(".rs"):
            continue
        source = git("show", f"{BASELINE}:{name}")
        for m in TOKEN.finditer(source):
            token = m[0]
            if not re.match(r'(?:b?r#*|b)?"', token) or not re.search(r"[\u3400-\u9fff]", token):
                continue
            value = normalized(text(token))
            values.add(value)
            # The severity marker is still emitted by the sink, not by its translation.
            values.add(re.sub(r"^\[(?:ERROR|WARN|INFO|\{\})\]\s+", "", value))
        if name == "src/cli.rs":
            for block in re.findall(r"(?m)^(?: +///[^\n]*(?:\n|$))+", source):
                values.add(normalized(" ".join(line.strip()[3:].strip() for line in block.splitlines())))
    return values


def check():
    baseline = baseline_texts()
    count = 0
    misses = 0
    unexpected = 0
    # These messages had English text from clap or the root command, but no Chinese
    # at baseline. Keep reporting them for review; do not exempt the rest of the module.
    english_only = {"root", "commands", "arguments", "options", "help_button", "help_short", "help_long", "help_command", "usage_template"}
    for path in sorted(p for root in (ROOT, ROOT / "aproxy-format", ROOT / "aproxy-ipc") for p in (root / "src/i18n").glob("*.rs")):
        source = path.read_text(encoding="utf-8")
        tokens = [m for m in TOKEN.finditer(source) if not m[0].startswith(("//", "/*"))]
        name = None
        for i, tok in enumerate(tokens[:-4]):
            if tok[0] == "message" and tokens[i + 1][0] == "!":
                name = tokens[i + 3][0]
            if tok[0] != "zh" or tokens[i + 1][0] != ":" or '"' not in tokens[i + 2][0]:
                continue
            value = text(tokens[i + 2][0]).replace("{{", "{").replace("}}", "}")
            count += 1
            if normalized(value) not in baseline:
                misses += 1
                line = source.count("\n", 0, tok.start()) + 1
                known = path.relative_to(ROOT).as_posix() == "src/i18n/help.rs" and name in english_only
                unexpected += not known
                reason = "no previous Chinese help" if known else "REVIEW"
                print(f"{path.relative_to(ROOT)}:{line} {name}: {value!r} ({reason})")
    print(f"{count} literals checked; {misses} without a baseline match")
    return unexpected != 0


if __name__ == "__main__":
    raise SystemExit(check())
