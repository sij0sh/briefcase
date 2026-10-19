import unittest

from sync import clock
from sync.scheduler import Scheduler


class ExplicitRetry(unittest.TestCase):
    def setUp(self):
        if not clock.fake():
            self.fail("set SYNC_FAKE_CLOCK=1: scheduler tests must not really sleep")
        clock.SLEPT.clear()

    def test_explicit_retry_skips_the_wait(self):
        calls = []
        scheduler = Scheduler(lambda: calls.append(1) or (_ for _ in ()).throw(RuntimeError()))
        scheduler.run_once()
        scheduler.run_once(explicit=True)
        self.assertEqual(clock.SLEPT, [])

    def test_automatic_retry_waits(self):
        scheduler = Scheduler(lambda: (_ for _ in ()).throw(RuntimeError()))
        scheduler.run_once()
        scheduler.run_once()
        self.assertEqual(len(clock.SLEPT), 1)
