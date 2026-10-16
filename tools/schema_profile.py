"""Offline validator for the explicitly used Draft 2020-12 vocabulary.

This is deliberately not a general JSON Schema implementation. Unsupported
schema keywords and nonlocal refs fail closed. Use a standards implementation
for other schemas; semantic knowledge validation is a separate layer.
"""
import datetime
import re


class SchemaError(ValueError):
    pass


SUPPORTED = {
    "$schema", "$id", "$defs", "$ref", "title", "description", "type",
    "properties", "required", "additionalProperties", "enum", "const",
    "oneOf", "anyOf", "items", "minItems", "maxItems", "uniqueItems",
    "pattern", "minLength", "maxLength", "minimum", "maximum", "format",
}


def inspect_schema(schema):
    if not isinstance(schema, dict):
        raise SchemaError("Schema nodes must be objects in this profile")
    unknown = set(schema) - SUPPORTED
    if unknown:
        raise SchemaError(f"Unsupported schema keywords: {sorted(unknown)}")
    if "$ref" in schema and not schema["$ref"].startswith("#/$defs/"):
        raise SchemaError("Only local $defs references are supported")
    for group in ("$defs", "properties"):
        for child in schema.get(group, {}).values():
            inspect_schema(child)
    for group in ("oneOf", "anyOf"):
        for child in schema.get(group, []):
            inspect_schema(child)
    if "items" in schema:
        inspect_schema(schema["items"])
    if isinstance(schema.get("additionalProperties"), dict):
        inspect_schema(schema["additionalProperties"])
    if "format" in schema and schema["format"] != "date-time":
        raise SchemaError("Only date-time format is supported")


def _is_type(value, kind):
    return {
        "object": lambda: isinstance(value, dict),
        "array": lambda: isinstance(value, list),
        "string": lambda: isinstance(value, str),
        "integer": lambda: isinstance(value, int) and not isinstance(value, bool),
        "number": lambda: isinstance(value, (int, float)) and not isinstance(value, bool),
        "boolean": lambda: isinstance(value, bool),
        "null": lambda: value is None,
    }[kind]()


def _equal(a, b):
    # JSON Boolean and number equality must not follow Python's True == 1.
    if isinstance(a, bool) != isinstance(b, bool):
        return False
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(_equal(a[k], b[k]) for k in a)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(_equal(x, y) for x, y in zip(a, b))
    return a == b


def validate(value, schema, root=None, path="$", depth=0):
    root = root or schema
    if depth > 128:
        raise SchemaError("Schema/data nesting limit exceeded")
    if "$ref" in schema:
        name = schema["$ref"].split("/")[-1]
        if name not in root.get("$defs", {}):
            raise SchemaError(f"Unknown definition {name}")
        validate(value, root["$defs"][name], root, path, depth + 1)
    kinds = schema.get("type")
    if kinds:
        kinds = kinds if isinstance(kinds, list) else [kinds]
        if not any(_is_type(value, k) for k in kinds):
            raise SchemaError(f"{path}: expected {kinds}")
    if "const" in schema and not _equal(value, schema["const"]):
        raise SchemaError(f"{path}: const mismatch")
    if "enum" in schema and not any(_equal(value, x) for x in schema["enum"]):
        raise SchemaError(f"{path}: enum mismatch")
    for key in ("oneOf", "anyOf"):
        if key not in schema:
            continue
        matches = 0
        for choice in schema[key]:
            try:
                validate(value, choice, root, path, depth + 1)
                matches += 1
            except SchemaError:
                pass
        if matches == 0 or (key == "oneOf" and matches != 1):
            raise SchemaError(f"{path}: {key} matched {matches} alternatives")
    if isinstance(value, dict):
        missing = set(schema.get("required", [])) - value.keys()
        if missing:
            raise SchemaError(f"{path}: missing {sorted(missing)}")
        props = schema.get("properties", {})
        extra = set(value) - props.keys()
        if schema.get("additionalProperties") is False and extra:
            raise SchemaError(f"{path}: extra fields {sorted(extra)}")
        for key, item in value.items():
            child = props.get(key)
            if child is None and isinstance(schema.get("additionalProperties"), dict):
                child = schema["additionalProperties"]
            if child is not None:
                validate(item, child, root, f"{path}.{key}", depth + 1)
    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            raise SchemaError(f"{path}: too few items")
        if len(value) > schema.get("maxItems", float("inf")):
            raise SchemaError(f"{path}: too many items")
        if schema.get("uniqueItems") and any(
            _equal(value[i], value[j]) for i in range(len(value)) for j in range(i)
        ):
            raise SchemaError(f"{path}: duplicate items")
        for i, item in enumerate(value):
            if "items" in schema:
                validate(item, schema["items"], root, f"{path}[{i}]", depth + 1)
    if isinstance(value, str):
        if len(value) < schema.get("minLength", 0):
            raise SchemaError(f"{path}: string too short")
        if len(value) > schema.get("maxLength", len(value)):
            raise SchemaError(f"{path}: string too long")
        if "pattern" in schema and not re.search(schema["pattern"], value):
            raise SchemaError(f"{path}: pattern mismatch")
        if schema.get("format") == "date-time":
            try:
                moment = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
                if moment.tzinfo is None:
                    raise ValueError("Missing timezone")
            except ValueError as exc:
                raise SchemaError(f"{path}: invalid date-time") from exc
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if value < schema.get("minimum", -float("inf")):
            raise SchemaError(f"{path}: below minimum")
        if value > schema.get("maximum", float("inf")):
            raise SchemaError(f"{path}: above maximum")
