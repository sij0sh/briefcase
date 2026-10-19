def is_settled(order):
    """Only a settled capture can be refunded; pending captures may still fail."""
    return order.settled
