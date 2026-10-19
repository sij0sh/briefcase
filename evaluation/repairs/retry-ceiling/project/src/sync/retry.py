import random as _random

BASE_DELAY = 0.5
MAX_DELAY = 30.0


def delay(attempt, random=_random.random):
    """Doubling backoff with jitter drawn from the upper half of the ceiling."""
    ceiling = BASE_DELAY * 2 ** attempt
    return ceiling / 2 + (ceiling / 2) * random()
