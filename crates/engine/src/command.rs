use domain::{OrderId, Price, Qty, Side};

/// An instruction to the engine. Order ids are assigned upstream by the sequencer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Buy (`Bid`) or sell (`Ask`) up to `qty` at `price` or better. Whatever doesn't fill
    /// immediately rests on the book at `price`.
    Limit {
        id: OrderId,
        qty: Qty,
        side: Side,
        price: Price,
    },
    /// Buy or sell up to `qty` at any price. Whatever doesn't fill immediately expires;
    /// a market order never rests.
    Market { id: OrderId, qty: Qty, side: Side },
    /// Remove the resting order `id` from the book.
    Cancel { id: OrderId },
}
