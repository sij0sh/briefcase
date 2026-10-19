from lib import audit


def handle_invoices(event):
    audit.record({"handler": "invoices", **event})
    return {"ok": True, "handler": "invoices"}
