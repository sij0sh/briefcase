"""Deterministic, independently authored task fixtures with executable outcomes.

Each fixture is a small repository, a task text with no path hints, and a
separate answer key: required evidence paths, an executable check and a
reference patch. The check must fail on the fixture and pass with the patch,
so a verified outcome means behavior, not gold-patch equality. Variants of one
family differ in names and layout; whole repositories are held out by a stable
hash of their identity, never by individual paths.
"""
import hashlib
import json
import random

WORDS = ("amber", "basil", "cedar", "delta", "ember", "fjord", "garnet", "harbor", "indigo", "juniper",
         "kestrel", "lumen", "maple", "nectar", "onyx", "pollen", "quartz", "raven", "sorrel", "tundra")
NOUNS = ("order", "invoice", "parcel", "ticket", "account", "voucher", "shipment", "batch")
FILLER = ("please", "carefully", "review", "existing", "behaviour", "around", "general", "module", "structure",
          "keeping", "everything", "consistent", "while", "preserving", "current", "outputs", "elsewhere")

PRELUDE = "import sys\nsys.path.insert(0, sys.argv[1])\n"


def split(fixture_id, held_out_share=4):
    """Repository-level split; 1 in `held_out_share` repositories are held out."""
    return "held_out" if int(hashlib.sha256(fixture_id.encode()).hexdigest(), 16) % held_out_share == 0 else "dev"


def noise(rng, package, count):
    return {f"{package}/{rng.choice(WORDS)}_{n:02d}.py": f"def helper_{n}(value):\n    return value + {n}\n" for n in range(count)}


def hidden_caller(rng):
    package, sub, noun = rng.choice(WORDS), rng.choice(WORDS) + "_flows", rng.choice(NOUNS)
    fn = f"{noun}_total"
    pricing = f"def {fn}(quantity, price):\n    return quantity * price\n"
    files = {f"{package}/__init__.py": "", f"{package}/{sub}/__init__.py": "", f"{package}/pricing.py": pricing,
             f"{package}/{sub}/checkout.py": f"from ..pricing import {fn} as charge\n\n\ndef checkout(items):\n    return sum(charge(q, p) for q, p in items)\n",
             **noise(rng, package, 12)}
    check = PRELUDE + (f"from {package}.pricing import {fn}\nfrom {package}.{sub}.checkout import checkout\n"
                       f"assert checkout([(2, 3), (1, 4)]) == 10\ntry:\n    {fn}(-1, 5)\nexcept ValueError:\n    pass\nelse:\n    raise SystemExit('negative quantity accepted')\n")
    patch = {f"{package}/pricing.py": f"def {fn}(quantity, price):\n    if quantity < 0:\n        raise ValueError('quantity')\n    return quantity * price\n"}
    task = f"Make `{fn}` reject negative quantities with ValueError without breaking checkout totals."
    return files, task, [f"{package}/pricing.py", f"{package}/{sub}/checkout.py"], check, patch


def config_default(rng):
    package, key = rng.choice(WORDS), rng.choice(("retry_limit", "batch_size", "page_size"))
    defaults = {key: 3, "region": rng.choice(WORDS)}
    files = {"config/defaults.json": json.dumps(defaults, indent=2) + "\n", f"{package}/__init__.py": "",
             f"{package}/settings.py": "import json\nfrom pathlib import Path\n\n\ndef load():\n    return json.loads((Path(__file__).resolve().parents[1] / 'config/defaults.json').read_text())\n",
             **noise(rng, package, 10)}
    check = PRELUDE + f"from {package}.settings import load\nassert load()['{key}'] == 5, load()\n"
    patch = {"config/defaults.json": json.dumps({**defaults, key: 5}, indent=2) + "\n"}
    task = f"The default {key.replace('_', ' ')} should be 5 instead of 3."
    return files, task, ["config/defaults.json", f"{package}/settings.py"], check, patch


def contract_rename(rng):
    package, old, new = rng.choice(WORDS), rng.choice(("user_id", "owner_id")), rng.choice(("account_id", "member_id"))
    files = {"schema/event.json": json.dumps({"required": [old, "kind"]}) + "\n", f"{package}/__init__.py": "",
             f"{package}/emit.py": f"def emit(identity):\n    return {{'{old}': identity, 'kind': 'created'}}\n",
             f"{package}/consume.py": f"def consume(event):\n    return 'seen ' + str(event['{old}'])\n", **noise(rng, package, 8)}
    check = PRELUDE + (f"import json, pathlib\nfrom {package}.emit import emit\nfrom {package}.consume import consume\n"
                       f"event = emit(7)\nassert '{new}' in event and '{old}' not in event\nassert consume(event) == 'seen 7'\n"
                       f"assert json.loads((pathlib.Path(sys.argv[1]) / 'schema/event.json').read_text())['required'][0] == '{new}'\n")
    patch = {"schema/event.json": json.dumps({"required": [new, "kind"]}) + "\n",
             f"{package}/emit.py": f"def emit(identity):\n    return {{'{new}': identity, 'kind': 'created'}}\n",
             f"{package}/consume.py": f"def consume(event):\n    return 'seen ' + str(event['{new}'])\n"}
    task = f"Rename the event field `{old}` to `{new}` in the contract, its producer and its consumer."
    return files, task, ["schema/event.json", f"{package}/emit.py", f"{package}/consume.py"], check, patch


def late_identifier(rng):
    package, ident = rng.choice(WORDS), f"scale_{rng.choice(NOUNS)}_weight"
    words = [rng.choice(FILLER) for _ in range(45)]
    files = {f"{package}/__init__.py": "", f"{package}/weights.py": f"def {ident}(value):\n    return value * 2\n",
             **{f"docs/{w}_{n}.md": f"{w} {' '.join(words[n:n + 8])}\n" for n, w in enumerate(words[:30])}}
    check = PRELUDE + f"from {package}.weights import {ident}\nassert {ident}(4) == 12\n"
    patch = {f"{package}/weights.py": f"def {ident}(value):\n    return value * 3\n"}
    task = " ".join(words) + f" and finally make `{ident}` triple its input."
    return files, task, [f"{package}/weights.py"], check, patch


def distractor_scale(rng):
    package, topic = rng.choice(WORDS), rng.choice(("ledger", "rounding", "settlement"))
    files = {f"notes/{topic}_{n:04d}.md": f"{topic} note {n}\n" for n in range(1100)}
    files.update({f"{package}/__init__.py": "", f"{package}/money.py": f"# {topic} cents conversion\ndef to_cents(amount):\n    return int(amount * 100)\n"})
    check = PRELUDE + f"from {package}.money import to_cents\nassert to_cents(1.15) == 115, to_cents(1.15)\n"
    patch = {f"{package}/money.py": f"# {topic} cents conversion\ndef to_cents(amount):\n    return round(amount * 100)\n"}
    task = f"Fix the {topic} cents conversion: 1.15 becomes 114 instead of 115."
    return files, task, [f"{package}/money.py"], check, patch


def deep_chain(rng):
    package = rng.choice(WORDS)
    names = [f"stage_{rng.choice(WORDS)}_{n}" for n in range(5)]
    files = {f"{package}/__init__.py": ""}
    for n, name in enumerate(names):
        body = f"from .{names[n + 1]} import run as next_run\n\n\ndef run(value):\n    return next_run(value)\n" if n < 4 else "def run(value):\n    return value - 1\n"
        files[f"{package}/{name}.py"] = body
    check = PRELUDE + f"from {package}.{names[0]} import run\nassert run(10) == 10, run(10)\n"
    patch = {f"{package}/{names[4]}.py": "def run(value):\n    return value\n"}
    task = f"`{names[0]}.run` returns one less than its input; it should pass values through unchanged."
    return files, task, [f"{package}/{names[0]}.py", f"{package}/{names[4]}.py"], check, patch


def duplicate_chunks(rng):
    package, term = rng.choice(WORDS), rng.choice(("throttle", "backoff", "quota"))
    files = {f"vendor/{term}_{n:02d}.txt": (f"{term} " * 16 + "\n") * 220 for n in range(60)}
    files.update({f"{package}/__init__.py": "", f"{package}/limits.py": f"# {term} window\nWINDOW = 10\n\n\ndef allowed(count):\n    return count < WINDOW\n"})
    check = PRELUDE + f"from {package}.limits import allowed\nassert allowed(10) and not allowed(11)\n"
    patch = {f"{package}/limits.py": f"# {term} window\nWINDOW = 10\n\n\ndef allowed(count):\n    return count <= WINDOW\n"}
    task = f"The {term} window should allow exactly the window count, not one fewer."
    return files, task, [f"{package}/limits.py"], check, patch


FAMILIES = {f.__name__: f for f in (hidden_caller, config_default, contract_rename, late_identifier,
                                     distractor_scale, deep_chain, duplicate_chunks)}


def corpus(variants=4, seed=0, families=None):
    fixtures = []
    for family in families or FAMILIES:
        for variant in range(variants):
            rng = random.Random(f"{seed}:{family}:{variant}")
            files, task, required, check, patch = FAMILIES[family](rng)
            fixture_id = f"{family}-{seed}-{variant}"
            fixtures.append({"id": fixture_id, "family": family, "split": split(fixture_id), "task": task,
                             "files": files, "answer_key": {"required_paths": required, "check": check, "patch": patch}})
    return fixtures
