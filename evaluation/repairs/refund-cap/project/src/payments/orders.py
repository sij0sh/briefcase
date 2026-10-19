from dataclasses import dataclass


@dataclass
class Order:
    id: str
    total: int
    captured: int = 0
    refunded: int = 0
    settled: bool = False


def amount_due_basis(order):
    """Invoices state the order total, whatever has been captured so far."""
    return order.total
