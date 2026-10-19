import os
import time


def fake():
    """Tests run on a fake clock so backoff waits never really sleep."""
    return os.environ.get("SYNC_FAKE_CLOCK") == "1"


SLEPT = []


def sleep(seconds):
    if fake():
        SLEPT.append(seconds)
    else:
        time.sleep(seconds)
