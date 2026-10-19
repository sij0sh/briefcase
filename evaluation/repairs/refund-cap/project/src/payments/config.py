import os


def refunds_enabled():
    """Refunds can be switched off per deployment during reconciliation."""
    return os.environ.get("REFUNDS_ENABLED", "1") == "1"
