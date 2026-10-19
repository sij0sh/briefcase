import unittest

from handlers import health, invoices, orders, refunds, users
from lib import audit


class AuditedHandlers(unittest.TestCase):
    def setUp(self):
        audit.STORE.update(available=True, records=[])

    def test_user_actions_are_audited(self):
        for handle in (orders.handle_orders, refunds.handle_refunds, users.handle_users, invoices.handle_invoices):
            handle({"id": 1})
        self.assertEqual(len(audit.STORE["records"]), 4)


class HealthProbe(unittest.TestCase):
    def tearDown(self):
        audit.STORE.update(available=True, records=[])

    def test_health_answers_while_audit_store_is_down(self):
        audit.STORE["available"] = False
        try:
            answer = health.handle_health({})
        except RuntimeError as error:
            self.fail(f"health probe depends on the audit store: {error}")
        self.assertEqual(answer["ok"], True)
