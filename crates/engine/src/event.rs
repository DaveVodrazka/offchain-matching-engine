use domain::{FillId, OrderId, Price, Qty, Side};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    UnknownOrder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Fill {
        maker: OrderId,
        taker: OrderId,
        taker_side: Side,
        filled: Qty,
        price: Price,
        fill_id: FillId,
    },
    Rested {
        id: OrderId,
        qty: Qty,
        side: Side,
        price: Price,
    },
    Cancelled {
        id: OrderId,
        qty: Qty,
        side: Side,
        price: Price,
    },
    Expired {
        id: OrderId,
        qty: Qty,
        side: Side,
    },
    Rejected {
        id: OrderId,
        reason: RejectionReason,
    },
}
