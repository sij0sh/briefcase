import copy

from settings import schema

REDACTED = "<redacted>"


def redact(settings):
    exported = copy.deepcopy(settings)
    for field in schema.SECRET_FIELDS:
        if exported.get(field):
            exported[field] = REDACTED
    return exported
