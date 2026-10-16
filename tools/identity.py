"""Versioned canonical-json-v1 identity primitives; not a symbol resolver."""
import hashlib
import json
import unicodedata


def normalize(value):
    if value is None or isinstance(value, (bool, int)):
        return value
    if isinstance(value, str):
        return unicodedata.normalize("NFC", value)
    if isinstance(value, list):
        return [normalize(x) for x in value]
    if isinstance(value, dict):
        result = {}
        for key, child in value.items():
            if not isinstance(key, str):
                raise ValueError("Identity object keys must be strings")
            key = normalize(key)
            if key in result:
                raise ValueError("NFC-normalized keys collide")
            result[key] = normalize(child)
        return result
    raise ValueError("Identity values cannot contain floats or arbitrary objects")


def canonical_bytes(value):
    return json.dumps(normalize(value), sort_keys=True, ensure_ascii=False,
                      separators=(",", ":"), allow_nan=False).encode("utf-8")


def digest(value):
    return "sha256:" + hashlib.sha256(canonical_bytes(value)).hexdigest()


def file_digest(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def load_json(path):
    with open(path, encoding="utf-8") as stream:
        return json.load(stream, object_pairs_hook=unique_pairs,
                         parse_constant=lambda x: (_ for _ in ()).throw(ValueError(x)))
