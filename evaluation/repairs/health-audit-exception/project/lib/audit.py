STORE = {"available": True, "records": []}


def record(event):
    """Append an audit record. The store is unavailable during readiness probes."""
    if not STORE["available"]:
        raise RuntimeError("audit store unavailable")
    STORE["records"].append(event)
    return event
