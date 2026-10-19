from lib import audit


def handle_users(event):
    audit.record({"handler": "users", **event})
    return {"ok": True, "handler": "users"}
