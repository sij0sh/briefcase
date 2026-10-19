import unittest

from payments.invoice import render
from payments.orders import Order


class InvoiceBasis(unittest.TestCase):
    def test_invoice_states_order_total_before_full_capture(self):
        order = Order("o3", total=100, captured=40, settled=True)
        self.assertIn("due 100", render(order))
