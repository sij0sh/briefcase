from settings.export import REDACTED


def save(stored, incoming):
    """A redacted value means "unchanged": clients never hold the real secret."""
    merged = dict(stored)
    for field, value in incoming.items():
        if isinstance(value, dict):
            merged[field] = save(stored.get(field, {}), value)
        elif value == REDACTED:
            continue
        else:
            merged[field] = value
    return merged
