from payments import orders


def render(order):
    return f"Order {order.id}: due {orders.amount_due_basis(order)}, refunded {order.refunded}"
