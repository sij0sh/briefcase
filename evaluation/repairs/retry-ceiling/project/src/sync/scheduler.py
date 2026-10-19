from sync import clock, retry


class Scheduler:
    def __init__(self, job):
        self.job, self.failures = job, 0

    def run_once(self, explicit=False):
        # An explicit retry skips the backoff wait; automatic retries wait.
        if self.failures and not explicit:
            clock.sleep(retry.delay(self.failures))
        try:
            self.job()
        except Exception:
            self.failures += 1
            return False
        self.failures = 0
        return True
