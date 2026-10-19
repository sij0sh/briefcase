import os
import unittest
from unittest import mock

from payments.orders import Order
from payments.refund import RefundError, refund


def settled(total=100, captured=100, refunded=0):
    return Order("o1", total=total, captured=captured, refunded=refunded, settled=True)


class RefundBalance(unittest.TestCase):
    def test_refund_cannot_exceed_captured_amount(self):
        order = settled(total=100, captured=60)
        with self.assertRaises(RefundError):
            refund(order, 80)

    def test_cumulative_refunds_stay_within_capture(self):
        order = settled(total=100, captured=60, refunded=50)
        with self.assertRaises(RefundError):
            refund(order, 20)


class RefundInvariants(unittest.TestCase):
    def test_full_capture_refund_succeeds(self):
        self.assertEqual(refund(settled(), 100), 100)

    def test_partial_refund_accumulates(self):
        order = settled(captured=60)
        refund(order, 20)
        self.assertEqual(refund(order, 30), 50)


class RefundGuards(unittest.TestCase):
    def test_unsettled_order_is_rejected(self):
        order = Order("o2", total=100, captured=100, settled=False)
        with self.assertRaises(RefundError):
            refund(order, 10)

    def test_disabled_refunds_are_rejected(self):
        with mock.patch.dict(os.environ, {"REFUNDS_ENABLED": "0"}):
            with self.assertRaises(RefundError):
                refund(settled(), 10)
