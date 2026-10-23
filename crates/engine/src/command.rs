use domain::{OrderId, Price, Qty, Side};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Limit {
        id: OrderId,
        qty: Qty,
        side: Side,
        price: Price,
    },
    Market,
    Cancel {
        id: OrderId,
    },
}
