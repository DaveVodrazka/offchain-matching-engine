use domain::{FillId, OrderId, Price, Qty, Side};

/// Why a command was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    /// The order isn't resting: it never existed, was already cancelled, or was fully
    /// filled. A cancel racing a fill ends here.
    UnknownOrder,
}

/// An outcome of a command. Each event is self-contained: a consumer can act on it without
/// asking the engine about the order.
///
/// Every taker order ends in exactly one way: fully filled (`Fill`s only), with a `Rested`
/// remainder, or with an `Expired` remainder. Its fills plus that remainder equal its
/// quantity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A trade between one resting maker and the incoming taker, at the maker's price.
    /// `taker_side` gives the direction: `Bid` means the taker bought. `fill_id` is unique
    /// and increases by one per fill, so settlement can recognize a fill it already saw.
    Fill {
        maker: OrderId,
        taker: OrderId,
        taker_side: Side,
        filled: Qty,
        price: Price,
        fill_id: FillId,
    },
    /// The unfilled part of a limit order was added to the book. Book change: +`qty` at
    /// `price` on `side`.
    Rested {
        id: OrderId,
        qty: Qty,
        side: Side,
        price: Price,
    },
    /// A resting order was removed from the book. `qty` is what it still had unfilled.
    /// Book change: -`qty` at `price` on `side`.
    Cancelled {
        id: OrderId,
        qty: Qty,
        side: Side,
        price: Price,
    },
    /// The unfilled remainder of a taker that never rested, such as what a market order
    /// couldn't fill. `qty` is that remainder; on an empty book it's the whole order. Not a
    /// book change: the remainder was never on the book.
    Expired { id: OrderId, qty: Qty, side: Side },
    /// The command was not applied and changed nothing.
    Rejected {
        id: OrderId,
        reason: RejectionReason,
    },
}
