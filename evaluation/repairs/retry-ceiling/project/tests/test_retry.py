import unittest

from sync import retry


class Ceiling(unittest.TestCase):
    def test_late_attempts_stay_under_max_delay(self):
        for attempt in range(0, 40):
            self.assertLessEqual(retry.delay(attempt, random=lambda: 1.0), retry.MAX_DELAY)


class Doubling(unittest.TestCase):
    def test_early_attempts_double(self):
        self.assertEqual(retry.delay(1, random=lambda: 1.0), 1.0)
        self.assertEqual(retry.delay(2, random=lambda: 1.0), 2.0)


class Jitter(unittest.TestCase):
    def test_jitter_stays_in_the_upper_half(self):
        for attempt in (1, 5, 30):
            low = retry.delay(attempt, random=lambda: 0.0)
            high = retry.delay(attempt, random=lambda: 1.0)
            self.assertAlmostEqual(low, high / 2)
