from payments import config, ledger, orders


class RefundError(Exception):
    pass


def refund(order, amount):
    if not config.refunds_enabled():
        raise RefundError("refunds are disabled")
    if not ledger.is_settled(order):
        raise RefundError("order is not settled")
    if amount <= 0:
        raise RefundError("amount must be positive")
    if amount > orders.amount_due_basis(order) - order.refunded:
        raise RefundError("amount exceeds the refundable balance")
    order.refunded += amount
    return order.refunded
