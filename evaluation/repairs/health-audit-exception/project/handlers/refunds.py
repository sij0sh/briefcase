from lib import audit


def handle_refunds(event):
    audit.record({"handler": "refunds", **event})
    return {"ok": True, "handler": "refunds"}
