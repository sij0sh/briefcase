from lib import audit


def handle_orders(event):
    audit.record({"handler": "orders", **event})
    return {"ok": True, "handler": "orders"}
