use domain::{OrderId, Price, Qty, Side};
use engine::Command;
use proptest::prelude::*;

fn side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Bid), Just(Side::Ask)]
}

/// A limit order before it gets an id: `(side, price, qty)`. Prices sit in a narrow band
/// so that bids and asks often cross.
fn limit_shape() -> impl Strategy<Value = (Side, u64, u64)> {
    (side(), 95u64..=105, 1u64..=10)
}

/// A sequence of limit orders with unique, increasing ids starting at 1.
///
/// Ids are assigned after generation, so when proptest shrinks the sequence by dropping
/// orders, the remaining ones are renumbered and stay unique.
pub(crate) fn commands() -> impl Strategy<Value = Vec<Command>> {
    prop::collection::vec(limit_shape(), 1..20).prop_map(|shapes| {
        #[allow(clippy::expect_used)]
        shapes
            .into_iter()
            .zip(1..)
            .map(|((side, price, qty), id)| Command::Limit {
                id: OrderId::new(id),
                qty: Qty::try_new(qty).expect("qty range starts at 1"),
                side,
                price: Price::try_new(price).expect("price range starts at 95"),
            })
            .collect()
    })
}
