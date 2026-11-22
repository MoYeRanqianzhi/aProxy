"""Check Chinese catalog literals against the source of the original CLI: python tests/i18n_origins.py."""

import ast
from functools import cache
import sys
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


@cache
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
            if not re.match(r'(?:b?r#*|b)?"', token):
                continue
            value = normalized(text(token).replace("{{", "{").replace("}}", "}"))
            values.add(value)
            # The severity marker is still emitted by the sink, not by its translation.
            values.add(re.sub(r"^\[(?:ERROR|WARN|INFO|\{\})\]\s+", "", value))
        if name == "src/cli.rs":
            for block in re.findall(r"(?m)^(?: +///[^\n]*(?:\n|$))+", source):
                values.add(normalized(" ".join(line.strip()[3:].strip() for line in block.splitlines())))
    return values


# File/line allowlists refer to immutable baseline sources. Do not exempt whole
# modules: deleting a catalog entry must introduce an unaccounted-for origin.
EXEMPT = {}
for path, lines in {
    "aproxy-ipc/src/decode.rs": "202 204 228 230 295 302",
    "src/proxy/attempts.rs": "362 538 542 543 956 1019 1032 1191 1216 1232 1240 1252 1264 1278",
    "src/proxy/body.rs": "378 384 414",
    "src/proxy/forward_only.rs": "419 431",
    "src/proxy/handler.rs": "93 157 239",
    "src/proxy/inbound.rs": "119 137 159 163",
    "src/proxy/inspect.rs": "26 57 70 106 113 150 177 183",
    "src/proxy/keepalive.rs": "442 448 540 568 574 588",
    "src/proxy/replay.rs": "22 53 58 61",
    "src/proxy/spool.rs": "83 191 257",
    "src/proxy/state.rs": "242",
    "src/proxy/trigger.rs": "37 107 160",
    "src/proxy/upstream.rs": "18",
}.items():
    for line in lines.split():
        EXEMPT[path, int(line)] = 'Data-plane or decoding diagnostic, log or invariant: machine/forensic English, not terminal prose.'

for path, lines in {
    "src/cli.rs": "31 225 279 407 472",
}.items():
    for line in lines.split():
        EXEMPT[path, int(line)] = 'Rustdoc for a type or subcommand field, not displayed by clap.'

for path, lines in {
    "src/commands/install.rs": "92",
}.items():
    for line in lines.split():
        EXEMPT[path, int(line)] = 'Frozen force route: the original bytes are still used.'

for path, lines in {
    "src/commands/install.rs": "188 195",
    "src/daemon.rs": "78 90 1674 1688 1743 1781 1785 1805 1808",
    "src/install/broadcast.rs": "75",
    "src/install/download/mod.rs": "687",
    "src/install/flow.rs": "335 339 683 778 891 919 1140 1160",
    "src/install/restart.rs": "252 257 308 317 341",
    "src/install/skills.rs": "193 195",
    "src/ops/start.rs": "149 220 354 355",
    "src/server.rs": "110 157 224 253 254 283 320 323 338 343 347 372 385 404 433 469",
    "src/settings/mod.rs": "246",
    "src/watchdog.rs": "147 152 155 526 583 586 643 663 677 707 715 756 770 801 828 838 895 916 922 926 944 953 966 983 1019 1041 1106 1128 1136 1168 1180 1200 1208 1212 1279",
}.items():
    for line in lines.split():
        EXEMPT[path, int(line)] = 'Detached process, log or installer event: forensic English, not a terminal message.'

for path, lines in {
    "aproxy-format/src/config.rs": "90",
    "aproxy-format/src/convert.rs": "254 393",
    "aproxy-format/src/dryrun.rs": "358",
    "aproxy-format/src/dryrun/peer.rs": "52 53 99",
    "aproxy-format/src/route.rs": "292 323",
    "aproxy-ipc/src/code.rs": "885",
    "aproxy-ipc/src/wire.rs": "717",
    "src/commands/install.rs": "240 252 361 469 597 675",
    "src/commands/restart.rs": "85",
    "src/commands/restore.rs": "31",
    "src/commands/settings.rs": "223",
    "src/commands/start.rs": "135",
    "src/config/edit.rs": "252",
    "src/config/view.rs": "570",
    "src/daemon.rs": "809 1195 1501",
    "src/desktop.rs": "54 65",
    "src/desktop/forms.rs": "83 85",
    "src/desktop/update.rs": "153 383",
    "src/install/flow.rs": "989",
    "src/install/mod.rs": "155",
    "src/install/skills/manifest.rs": "49 56",
    "src/install/state.rs": "407",
    "src/main.rs": "84 299 301 305",
    "src/ops/restore.rs": "38",
    "src/proxy/errors.rs": "74 200",
    "src/proxy/keepalive.rs": "560",
    "src/proxy/state.rs": "98 106 140",
    "src/server.rs": "95 474 480",
    "src/settings/edit.rs": "101 152",
    "src/spec.rs": "128 184",
}.items():
    for line in lines.split():
        EXEMPT[path, int(line)] = 'Internal invariant, assertion or serialization explanation: not terminal prose.'

for path, lines in {
    "src/process.rs": "82 147 152 155 235 238 244",
}.items():
    for line in lines.split():
        EXEMPT[path, int(line)] = 'Low-level process identity evidence: this OS/ledger path is fixed English; human termination outcomes use the typed process catalog.'

for path, lines in {
    "src/install/skills.rs": "187",
}.items():
    for line in lines.split():
        EXEMPT[path, int(line)] = 'Redaction sentinel is machine evidence, not prose.'

for path, lines in {
    "aproxy-format/src/adaptive.rs": "96",
    "aproxy-format/src/config.rs": "35 46 51 102 104 107",
    "aproxy-format/src/convert.rs": "121 129 136 144 150",
    "aproxy-format/src/heartbeat.rs": "44 47",
    "aproxy-format/src/keys.rs": "153 156 161 166 175 184",
    "aproxy-format/src/lib.rs": "93 103 105 308 310 313 315 319 322 329",
    "aproxy-format/src/policy.rs": "60 155 163",
    "aproxy-format/src/rewrite.rs": "33 42 45 48 51 57 61 66",
    "aproxy-format/src/route.rs": "22 236 253 257 260 264",
    "aproxy-format/src/translate.rs": "27",
}.items():
    for line in lines.split():
        EXEMPT[path, int(line)] = 'Worker-only protocol or role-load detail: fixed English; offline tools expose only the safe classification.'

for path, lines in {
    "src/install/skills/archive.rs": "324 334 338",
}.items():
    for line in lines.split():
        EXEMPT[path, int(line)] = 'Hidden package verifier: fixed English.'

def production_tokens(source):
    tokens = [m for m in TOKEN.finditer(source) if not m[0].startswith(("//", "/*"))]
    pairs, stack = {}, []
    for i, m in enumerate(tokens):
        if m[0] in {"(", "{", "["}:
            stack.append(i)
        elif m[0] in {")", "}", "]"}:
            pairs[stack.pop()] = i
    i = 0
    while i < len(tokens):
        m = tokens[i]
        if m[0] == "#" and i + 1 in pairs and tokens[i + 1][0] == "[":
            end = pairs[i + 1]
            attr = "".join(t[0] for t in tokens[i + 2:end])
            if attr in {"test", "cfg(test)"} or attr.endswith("::test") or attr.startswith("cfg(all(test,"):
                i = end + 1
                while i < len(tokens) and tokens[i][0] not in {"{", ";"}:
                    i = pairs.get(i, i) + 1
                i = pairs.get(i, i) + 1
                continue
        yield m
        i += 1


def reverse_inventory(catalog):
    dirs = ("src", "aproxy-ipc/src", "aproxy-format/src")
    frozen = {"src/install/force.rs", "aproxy-ipc/src/home.rs", "aproxy-ipc/src/ledger.rs"}
    accounted = set()
    for path in git("ls-tree", "-rz", "--name-only", BASELINE, "--", *dirs).split("\0"):
        if not path.endswith(".rs") or path in frozen or "tests" in Path(path).parts or Path(path).stem in {"tests", "archive_tests"}:
            continue
        source = git("show", f"{BASELINE}:{path}")
        rows = [(m.start(), text(m[0])) for m in production_tokens(source)
                if re.match(r'(?:b?r#*|b)?"', m[0]) and re.search(r"[\u3400-\u9fff]", m[0])]
        if path == "src/cli.rs":
            for m in re.finditer(r"(?m)^(?P<i> *)///[^\n]*(?:\n(?P=i)///[^\n]*)*", source[:source.index("/// 构造实际解析")]):
                rows.append((m.start(), " ".join(l.strip()[3:].strip() for l in m[0].splitlines())))
        for offset, raw in rows:
            line = 1 + source.count("\n", 0, offset)
            key = normalized(raw.replace("{{", "{").replace("}}", "}"))
            bare = re.sub(r"^\[(?:ERROR|WARN|INFO|\{\})\]\s+", "", key)
            reason = "catalog" if key in catalog or bare in catalog else EXEMPT.get((path, line))
            if reason and reason != "catalog": accounted.add((path, line))
            yield path, line, raw, reason
    for path, line in sorted(EXEMPT.keys() - accounted):
        yield path, line, "stale exemption", None


def check():
    baseline = baseline_texts()
    catalog = set()
    count = 0
    misses = 0
    unexpected = 0
    # These messages had English text from clap or the root command, but no Chinese
    # at baseline. Keep reporting them for review; do not exempt the rest of the module.
    english_only = {"root", "commands", "arguments", "options", "help_button", "help_short", "help_long", "help_command", "usage_template"}
    # These variants came from the English-only parsers, semantic validator, registry
    # and doctor checks. Continue rejecting unreviewed additions, even in the same file.
    diagnostic_drafts = set("""
forward_default_unguarded forward_response_buffered AlwaysTrigger BodyMatcher RetryLimit
MissingCommand EmptyCommand BatchCommand FormatName MissingStages EmptyStages AdmitStage EmptyPool
BreakerCooldown StageConflict PersistentStage TomlSyntax Trigger Regex HttpStatus UnknownStage
DuplicateFormat RetryChain TooLarge Negative OneOf UnknownKey ExpectedString ExpectedNullableString
ExpectedBoolean ExpectedUnsigned ExpectedObject ExpectedStrings ExpectedInteger ExpectedIntegers
ExpectedStringTable ExpectedTable ExpectedTables SettingsObject HeartbeatMinimum StaleMinimum
UiObject DefaultsBase DefaultsProxy JsonSyntax NewerSchema Schema Language DownloadChain BadAlias
EmptyAliasPath InstanceDefault TomlValue MissingBase BaseScheme BaseQuery ListenPort ProxyUrl
ProxyScheme ProxyHost ProxyMissing KeyHeader HeaderName HeaderValue EmptyHeartbeat HeartbeatLine
HeartbeatEvent Override Verbatim Withheld NoStoredSecret ReadFile UnreadableRecord SharedHeader
HangDelay RestartsDisabled MissingDir MissingConfig UnreadableConfig FilePermissions SharedPort
WithAliases OwnedHome OtherHome ExposedKey ExposedProxy RecordRead RecordParse RecordSchema
InstallMissing InstallRead
""".split())
    ipc_drafts = set("""identity unverified_refusal subscription_ack peer_pid pipe_owner run_changed
incomplete not_json missing_v missing_op args_object required invalid unsupported_version unknown_op
unknown_arg invalid_state idle_condition not_found busy""".split())
    for path in sorted(p for root in (ROOT, ROOT / "aproxy-format", ROOT / "aproxy-ipc") for p in (root / "src/i18n").glob("*.rs")):
        source = path.read_text(encoding="utf-8")
        tokens = [m for m in TOKEN.finditer(source) if not m[0].startswith(("//", "/*"))]
        name = None
        declarative = False
        for i, tok in enumerate(tokens[:-4]):
            if tok[0] == "reasons" and tokens[i + 1][0] == "!":
                declarative = True
            if declarative and tok[0][0].isupper() and tokens[i - 1][0] in {"{", ";"}:
                name = tok[0]
            if tok[0] == "message" and tokens[i + 1][0] == "!":
                name = tokens[i + 3][0]
            if tok[0] != "zh" or tokens[i + 1][0] != ":" or '"' not in tokens[i + 2][0]:
                continue
            value = text(tokens[i + 2][0]).replace("{{", "{").replace("}}", "}")
            catalog.add(normalized(value))
            count += 1
            if normalized(value) not in baseline:
                misses += 1
                line = source.count("\n", 0, tok.start()) + 1
                known = path.relative_to(ROOT).as_posix() == "src/i18n/help.rs" and name in english_only
                diagnostic = path.relative_to(ROOT).as_posix() == "aproxy-ipc/src/i18n/diagnostic.rs" and name in diagnostic_drafts
                diagnostic |= path.relative_to(ROOT).as_posix() == "aproxy-ipc/src/i18n/ipc.rs" and name in ipc_drafts
                diagnostic |= path.relative_to(ROOT).as_posix() == "src/i18n/startup.rs" and name == "spec_read"
                diagnostic |= path.relative_to(ROOT).as_posix() == "src/i18n/start.rs" and name in {"ready", "formats"}
                diagnostic |= path.relative_to(ROOT).as_posix() == "src/i18n/stop.rs" and name == "generation"
                diagnostic |= path.relative_to(ROOT).as_posix() == "src/i18n/restart.rs" and name in {"starting", "starting_state", "changed_state"}
                diagnostic |= path.relative_to(ROOT).as_posix() == "src/i18n/format_fault.rs" and name in {"open", "spawn", "io", "died", "timeout", "protocol", "invalid", "stages", "handshake"}
                diagnostic |= path.relative_to(ROOT).as_posix() == "src/i18n/install_reason.rs" and name in {"panic_at", "no_terminal", "saving", "exit_status", "no_status"}
                diagnostic |= path.relative_to(ROOT).as_posix() == "src/i18n/data.rs" and name in {"connect", "connect_timeout", "first_byte", "idle", "interrupted", "status", "error_body", "host", "origin", "request_size", "response_size", "aborted", "upgrade", "decode", "decision", "format_error"}
                unexpected += not (known or diagnostic)
                reason = "no previous Chinese help" if known else "English-only diagnostic" if diagnostic else "REVIEW"
                print(f"{path.relative_to(ROOT)}:{line} {name}: {value!r} ({reason})")
    print(f"{count} literals checked; {misses} without a baseline match")
    rows = list(reverse_inventory(catalog))
    for path, line, raw, reason in rows:
        if not reason or "--inventory" in sys.argv:
            print(f"{path}:{line}: {raw!r} -> {reason or 'UNACCOUNTED'}")
    missing = sum(not reason for _, _, _, reason in rows)
    print(f"{len(rows)} baseline origins; {missing} unaccounted for")
    return unexpected != 0 or missing != 0


if __name__ == "__main__":
    raise SystemExit(check())
