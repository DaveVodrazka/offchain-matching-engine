use domain::{FillIdGenerator, OrderId, Price, Qty, Side};

use crate::{
    book::{
        book_side::BestFill,
        order_book::{CancelledOrder, OrderBook},
    },
    command::Command,
    event::{Event, RejectionReason},
};

/// The matching engine for one market.
///
/// [`apply`](Self::apply) is a pure function of the engine's state and the command: no
/// clock, no randomness, no IO. Replaying the same commands on a fresh engine produces the
/// same events, so the state equals the replay of the command log.
pub struct Engine {
    order_book: OrderBook,
    fill_id_generator: FillIdGenerator,
}

fn price_crosses(side: Side, price_wanted: Price, best_price: Price) -> bool {
    match side {
        Side::Ask => price_wanted <= best_price,
        Side::Bid => price_wanted >= best_price,
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            order_book: OrderBook::new(),
            fill_id_generator: FillIdGenerator::fresh(),
        }
    }
}

impl Engine {
    /// An engine with an empty book whose first fill id is 1.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn cancel(&mut self, id: OrderId) -> Vec<Event> {
        let Some(CancelledOrder { price, qty, side }) = self.order_book.cancel(id) else {
            return vec![Event::Rejected {
                id,
                reason: RejectionReason::UnknownOrder,
            }];
        };
        vec![Event::Cancelled {
            id,
            qty,
            side,
            price,
        }]
    }

    fn rest(&mut self, id: OrderId, side: Side, price: Price, qty: Qty) -> Event {
        self.order_book.rest(side, id, price, qty);
        Event::Rested {
            id,
            side,
            price,
            qty,
        }
    }

    /// Matches a taker against the opposite side, best price first, one maker per step,
    /// until the taker is filled, the opposite side is empty, or the next price is beyond
    /// `limit` (`None`: no limit).
    ///
    /// Returns one `Fill` per maker and the unfilled quantity (`None`: fully filled).
    ///
    /// # Panics
    ///
    /// Panics after `u64::MAX` fills, when fill ids run out.
    fn match_taker(
        &mut self,
        id: OrderId,
        side: Side,
        limit: Option<Price>,
        qty: Qty,
    ) -> (Vec<Event>, Option<Qty>) {
        let opposite = side.opposite();
        let mut events = vec![];
        let mut qty_left = Some(qty);
        while let Some(wanted) = qty_left {
            let Some(best_price) = self.order_book.best_price(opposite) else {
                break; // no liquidity
            };
            if limit.is_some_and(|limit| !price_crosses(side, limit, best_price)) {
                break; // next price is beyond the limit
            }
            let Some(BestFill {
                price,
                maker,
                filled,
                remaining: _,
            }) = self.order_book.fill_best(opposite, wanted)
            else {
                unreachable!("best_price returned Some, so the opposite side is not empty");
            };
            let Ok(left) = wanted.checked_sub(filled) else {
                unreachable!("fill_best fills at most `wanted`");
            };
            qty_left = left;
            events.push(Event::Fill {
                maker,
                taker: id,
                taker_side: side,
                filled,
                price,
                fill_id: self.fill_id_generator.next_id(),
            });
        }
        (events, qty_left)
    }

    fn limit(&mut self, id: OrderId, side: Side, price: Price, qty: Qty) -> Vec<Event> {
        let (mut events, leftover) = self.match_taker(id, side, Some(price), qty);
        if let Some(qty) = leftover {
            events.push(self.rest(id, side, price, qty));
        }
        events
    }

    fn market(&mut self, id: OrderId, side: Side, qty: Qty) -> Vec<Event> {
        let (mut events, leftover) = self.match_taker(id, side, None, qty);
        if let Some(qty) = leftover {
            events.push(Event::Expired { id, qty, side });
        }
        events
    }

    /// Applies one command and returns its events, in order: the fills, best price first
    /// and oldest maker first within a price, then the taker's `Rested` or `Expired`
    /// remainder, if any. Every command produces at least one event.
    ///
    /// # Panics
    ///
    /// Panics if a `Limit` order would rest under an id that is already resting. The
    /// sequencer issues unique ids, so a duplicate is a bug upstream. Also panics after
    /// `u64::MAX` fills, when fill ids run out.
    #[must_use = "the events are the only record of what the command did"]
    pub fn apply(&mut self, cmd: Command) -> Vec<Event> {
        match cmd {
            Command::Cancel { id } => self.cancel(id),
            Command::Limit {
                id,
                qty,
                side,
                price,
            } => self.limit(id, side, price, qty),
            Command::Market { id, qty, side } => self.market(id, side, qty),
        }
    }
}

#[cfg(test)]
mod tests {
    use domain::FillId;

    use super::*;

    fn order_id(n: u64) -> OrderId {
        OrderId::new(n)
    }

    fn qty(n: u64) -> Qty {
        Qty::try_new(n).expect("test qty must be non-zero")
    }

    fn price(n: u64) -> Price {
        Price::try_new(n).expect("test price must be non-zero")
    }

    fn limit(id: OrderId, side: Side, price: Price, qty: Qty) -> Command {
        Command::Limit {
            id,
            qty,
            side,
            price,
        }
    }
    fn rested(id: OrderId, side: Side, price: Price, qty: Qty) -> Event {
        Event::Rested {
            id,
            qty,
            side,
            price,
        }
    }
    fn fill(
        fill_id: FillId,
        maker: OrderId,
        taker: OrderId,
        taker_side: Side,
        price: Price,
        filled: Qty,
    ) -> Event {
        Event::Fill {
            maker,
            taker,
            taker_side,
            filled,
            price,
            fill_id,
        }
    }

    fn cancel(id: OrderId) -> Command {
        Command::Cancel { id }
    }

    fn cancelled(id: OrderId, side: Side, price: Price, qty: Qty) -> Event {
        Event::Cancelled {
            id,
            qty,
            price,
            side,
        }
    }

    fn market(id: OrderId, side: Side, qty: Qty) -> Command {
        Command::Market { id, qty, side }
    }

    fn expired(id: OrderId, side: Side, qty: Qty) -> Event {
        Event::Expired { id, qty, side }
    }

    fn unknown_order(id: OrderId) -> Event {
        Event::Rejected {
            id,
            reason: RejectionReason::UnknownOrder,
        }
    }

    /// Builds an engine from orders that must each rest without matching.
    fn engine_with(orders: &[Command]) -> Engine {
        let mut engine = Engine::new();
        for &order in orders {
            let events = engine.apply(order);
            assert!(
                matches!(events.as_slice(), [Event::Rested { .. }]),
                "setup order {order:?} must rest, got {events:?}"
            );
        }
        engine
    }

    // --- Resting ---

    #[test]
    fn ask_on_empty_book_rests_and_cancels_in_full() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.apply(limit(order_id(1), Side::Ask, price(100), qty(10))),
            vec![rested(order_id(1), Side::Ask, price(100), qty(10))]
        );
        assert_eq!(
            engine.apply(cancel(order_id(1))),
            vec![cancelled(order_id(1), Side::Ask, price(100), qty(10))]
        );
    }

    #[test]
    fn bid_on_empty_book_rests_in_full() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.apply(limit(order_id(1), Side::Bid, price(100), qty(10))),
            vec![rested(order_id(1), Side::Bid, price(100), qty(10))]
        );
    }

    #[test]
    fn bid_one_tick_below_best_ask_rests_without_filling() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(101), qty(5))]);
        assert_eq!(
            engine.apply(limit(order_id(2), Side::Bid, price(100), qty(7))),
            vec![rested(order_id(2), Side::Bid, price(100), qty(7))]
        );
    }

    #[test]
    fn ask_one_tick_above_best_bid_rests_without_filling() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Bid, price(100), qty(5))]);
        assert_eq!(
            engine.apply(limit(order_id(2), Side::Ask, price(101), qty(7))),
            vec![rested(order_id(2), Side::Ask, price(101), qty(7))]
        );
    }

    // --- Matching ---

    #[test]
    fn crossing_bid_fills_at_maker_price_and_rests_remainder_at_limit() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(100), qty(10))]);
        assert_eq!(
            engine.apply(limit(order_id(2), Side::Bid, price(102), qty(15))),
            vec![
                fill(
                    FillId::new(1),
                    order_id(1),
                    order_id(2),
                    Side::Bid,
                    price(100),
                    qty(10)
                ),
                rested(order_id(2), Side::Bid, price(102), qty(5)),
            ]
        );
    }

    #[test]
    fn crossing_ask_fills_at_maker_price_and_rests_remainder_at_limit() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Bid, price(100), qty(10))]);
        assert_eq!(
            engine.apply(limit(order_id(2), Side::Ask, price(98), qty(15))),
            vec![
                fill(
                    FillId::new(1),
                    order_id(1),
                    order_id(2),
                    Side::Ask,
                    price(100),
                    qty(10)
                ),
                rested(order_id(2), Side::Ask, price(98), qty(5)),
            ]
        );
    }

    #[test]
    fn ask_at_best_bid_fills_exactly_and_does_not_rest() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Bid, price(100), qty(6))]);
        assert_eq!(
            engine.apply(limit(order_id(2), Side::Ask, price(100), qty(6))),
            vec![fill(
                FillId::new(1),
                order_id(1),
                order_id(2),
                Side::Ask,
                price(100),
                qty(6)
            )]
        );
        // Neither order is left on the book.
        assert_eq!(
            engine.apply(cancel(order_id(1))),
            vec![unknown_order(order_id(1))]
        );
        assert_eq!(
            engine.apply(cancel(order_id(2))),
            vec![unknown_order(order_id(2))]
        );
    }

    #[test]
    fn bid_sweeps_asks_lowest_first_and_stops_at_its_limit() {
        let mut engine = engine_with(&[
            limit(order_id(1), Side::Ask, price(101), qty(4)),
            limit(order_id(2), Side::Ask, price(100), qty(3)),
            limit(order_id(3), Side::Ask, price(102), qty(5)),
        ]);
        assert_eq!(
            engine.apply(limit(order_id(4), Side::Bid, price(101), qty(10))),
            vec![
                fill(
                    FillId::new(1),
                    order_id(2),
                    order_id(4),
                    Side::Bid,
                    price(100),
                    qty(3)
                ),
                fill(
                    FillId::new(2),
                    order_id(1),
                    order_id(4),
                    Side::Bid,
                    price(101),
                    qty(4)
                ),
                rested(order_id(4), Side::Bid, price(101), qty(3)),
            ]
        );
        // The ask beyond the limit is untouched.
        assert_eq!(
            engine.apply(cancel(order_id(3))),
            vec![cancelled(order_id(3), Side::Ask, price(102), qty(5))]
        );
    }

    #[test]
    fn ask_sweeps_bids_highest_first() {
        let mut engine = engine_with(&[
            limit(order_id(1), Side::Bid, price(99), qty(4)),
            limit(order_id(2), Side::Bid, price(100), qty(3)),
        ]);
        assert_eq!(
            engine.apply(limit(order_id(3), Side::Ask, price(99), qty(5))),
            vec![
                fill(
                    FillId::new(1),
                    order_id(2),
                    order_id(3),
                    Side::Ask,
                    price(100),
                    qty(3)
                ),
                fill(
                    FillId::new(2),
                    order_id(1),
                    order_id(3),
                    Side::Ask,
                    price(99),
                    qty(2)
                ),
            ]
        );
        assert_eq!(
            engine.apply(cancel(order_id(1))),
            vec![cancelled(order_id(1), Side::Bid, price(99), qty(2))]
        );
    }

    #[test]
    fn makers_at_one_price_fill_oldest_first() {
        let mut engine = engine_with(&[
            limit(order_id(1), Side::Ask, price(100), qty(3)),
            limit(order_id(2), Side::Ask, price(100), qty(4)),
        ]);
        assert_eq!(
            engine.apply(limit(order_id(3), Side::Bid, price(100), qty(5))),
            vec![
                fill(
                    FillId::new(1),
                    order_id(1),
                    order_id(3),
                    Side::Bid,
                    price(100),
                    qty(3)
                ),
                fill(
                    FillId::new(2),
                    order_id(2),
                    order_id(3),
                    Side::Bid,
                    price(100),
                    qty(2)
                ),
            ]
        );
        assert_eq!(
            engine.apply(cancel(order_id(2))),
            vec![cancelled(order_id(2), Side::Ask, price(100), qty(2))]
        );
    }

    #[test]
    fn partially_filled_maker_keeps_its_place_in_the_queue() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(100), qty(10))]);
        assert_eq!(
            engine.apply(limit(order_id(2), Side::Bid, price(100), qty(4))),
            vec![fill(
                FillId::new(1),
                order_id(1),
                order_id(2),
                Side::Bid,
                price(100),
                qty(4)
            )]
        );
        assert_eq!(
            engine.apply(limit(order_id(3), Side::Ask, price(100), qty(5))),
            vec![rested(order_id(3), Side::Ask, price(100), qty(5))]
        );
        assert_eq!(
            engine.apply(limit(order_id(4), Side::Bid, price(100), qty(7))),
            vec![
                fill(
                    FillId::new(2),
                    order_id(1),
                    order_id(4),
                    Side::Bid,
                    price(100),
                    qty(6)
                ),
                fill(
                    FillId::new(3),
                    order_id(3),
                    order_id(4),
                    Side::Bid,
                    price(100),
                    qty(1)
                ),
            ]
        );
    }

    #[test]
    fn fill_ids_continue_across_commands_and_only_fills_use_them() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(100), qty(2))]);
        assert_eq!(
            engine.apply(limit(order_id(2), Side::Bid, price(100), qty(1))),
            vec![fill(
                FillId::new(1),
                order_id(1),
                order_id(2),
                Side::Bid,
                price(100),
                qty(1)
            )]
        );
        let _ = engine.apply(limit(order_id(3), Side::Ask, price(105), qty(1)));
        let _ = engine.apply(cancel(order_id(3)));
        let _ = engine.apply(cancel(order_id(99)));
        assert_eq!(
            engine.apply(limit(order_id(4), Side::Bid, price(100), qty(1))),
            vec![fill(
                FillId::new(2),
                order_id(1),
                order_id(4),
                Side::Bid,
                price(100),
                qty(1)
            )]
        );
    }

    // --- Market ---

    #[test]
    fn market_on_empty_book_expires_in_full() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.apply(market(order_id(1), Side::Bid, qty(5))),
            vec![expired(order_id(1), Side::Bid, qty(5))]
        );
    }

    #[test]
    fn market_sell_expires_when_only_asks_rest() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(100), qty(5))]);
        assert_eq!(
            engine.apply(market(order_id(2), Side::Ask, qty(3))),
            vec![expired(order_id(2), Side::Ask, qty(3))]
        );
        // The resting ask on the market order's own side is untouched.
        assert_eq!(
            engine.apply(cancel(order_id(1))),
            vec![cancelled(order_id(1), Side::Ask, price(100), qty(5))]
        );
    }

    #[test]
    fn market_buy_sweeps_asks_lowest_first_at_any_price() {
        let mut engine = engine_with(&[
            limit(order_id(1), Side::Ask, price(10_000), qty(1)),
            limit(order_id(2), Side::Ask, price(100), qty(2)),
            limit(order_id(3), Side::Ask, price(500), qty(3)),
        ]);
        assert_eq!(
            engine.apply(market(order_id(4), Side::Bid, qty(6))),
            vec![
                fill(
                    FillId::new(1),
                    order_id(2),
                    order_id(4),
                    Side::Bid,
                    price(100),
                    qty(2)
                ),
                fill(
                    FillId::new(2),
                    order_id(3),
                    order_id(4),
                    Side::Bid,
                    price(500),
                    qty(3)
                ),
                fill(
                    FillId::new(3),
                    order_id(1),
                    order_id(4),
                    Side::Bid,
                    price(10_000),
                    qty(1)
                ),
            ]
        );
    }

    #[test]
    fn market_sell_exhausts_bids_highest_first_and_expires_the_rest() {
        let mut engine = engine_with(&[
            limit(order_id(1), Side::Bid, price(99), qty(2)),
            limit(order_id(2), Side::Bid, price(100), qty(3)),
        ]);
        assert_eq!(
            engine.apply(market(order_id(3), Side::Ask, qty(8))),
            vec![
                fill(
                    FillId::new(1),
                    order_id(2),
                    order_id(3),
                    Side::Ask,
                    price(100),
                    qty(3)
                ),
                fill(
                    FillId::new(2),
                    order_id(1),
                    order_id(3),
                    Side::Ask,
                    price(99),
                    qty(2)
                ),
                expired(order_id(3), Side::Ask, qty(3)),
            ]
        );
    }

    #[test]
    fn market_partially_fills_a_maker_and_does_not_expire() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(100), qty(10))]);
        assert_eq!(
            engine.apply(market(order_id(2), Side::Bid, qty(4))),
            vec![fill(
                FillId::new(1),
                order_id(1),
                order_id(2),
                Side::Bid,
                price(100),
                qty(4)
            )]
        );
        assert_eq!(
            engine.apply(cancel(order_id(1))),
            vec![cancelled(order_id(1), Side::Ask, price(100), qty(6))]
        );
    }

    #[test]
    fn expired_remainder_never_rests() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(100), qty(2))]);
        assert_eq!(
            engine.apply(market(order_id(2), Side::Bid, qty(5))),
            vec![
                fill(
                    FillId::new(1),
                    order_id(1),
                    order_id(2),
                    Side::Bid,
                    price(100),
                    qty(2)
                ),
                expired(order_id(2), Side::Bid, qty(3)),
            ]
        );
        assert_eq!(
            engine.apply(cancel(order_id(2))),
            vec![unknown_order(order_id(2))]
        );
        // No bid is left to match, so even the lowest ask rests.
        assert_eq!(
            engine.apply(limit(order_id(3), Side::Ask, price(1), qty(1))),
            vec![rested(order_id(3), Side::Ask, price(1), qty(1))]
        );
    }

    // --- Cancel ---

    #[test]
    fn cancel_of_unknown_id_is_rejected() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.apply(cancel(order_id(7))),
            vec![unknown_order(order_id(7))]
        );
    }

    #[test]
    fn second_cancel_of_same_order_is_rejected() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Bid, price(100), qty(5))]);
        assert_eq!(
            engine.apply(cancel(order_id(1))),
            vec![cancelled(order_id(1), Side::Bid, price(100), qty(5))]
        );
        assert_eq!(
            engine.apply(cancel(order_id(1))),
            vec![unknown_order(order_id(1))]
        );
    }

    #[test]
    fn cancel_of_fully_filled_maker_is_rejected() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(100), qty(5))]);
        let _ = engine.apply(limit(order_id(2), Side::Bid, price(100), qty(5)));
        assert_eq!(
            engine.apply(cancel(order_id(1))),
            vec![unknown_order(order_id(1))]
        );
    }

    #[test]
    fn cancel_of_partially_filled_maker_removes_only_the_unfilled_qty() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(100), qty(10))]);
        let _ = engine.apply(limit(order_id(2), Side::Bid, price(100), qty(4)));
        assert_eq!(
            engine.apply(cancel(order_id(1))),
            vec![cancelled(order_id(1), Side::Ask, price(100), qty(6))]
        );
    }

    #[test]
    fn rested_remainder_of_a_taker_can_be_cancelled() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(100), qty(4))]);
        let _ = engine.apply(limit(order_id(2), Side::Bid, price(101), qty(10)));
        assert_eq!(
            engine.apply(cancel(order_id(2))),
            vec![cancelled(order_id(2), Side::Bid, price(101), qty(6))]
        );
        // The bid side is empty again, so an ask at the old bid price rests.
        assert_eq!(
            engine.apply(limit(order_id(3), Side::Ask, price(101), qty(1))),
            vec![rested(order_id(3), Side::Ask, price(101), qty(1))]
        );
    }

    #[test]
    fn cancelled_order_no_longer_matches() {
        let mut engine = engine_with(&[limit(order_id(1), Side::Ask, price(100), qty(5))]);
        let _ = engine.apply(cancel(order_id(1)));
        assert_eq!(
            engine.apply(limit(order_id(2), Side::Bid, price(100), qty(5))),
            vec![rested(order_id(2), Side::Bid, price(100), qty(5))]
        );
    }

    // --- Determinism ---

    #[test]
    fn same_commands_produce_same_events() {
        let commands = [
            limit(order_id(1), Side::Ask, price(101), qty(4)),
            limit(order_id(2), Side::Ask, price(100), qty(3)),
            limit(order_id(3), Side::Bid, price(98), qty(6)),
            limit(order_id(4), Side::Bid, price(101), qty(5)),
            cancel(order_id(3)),
            limit(order_id(5), Side::Ask, price(97), qty(8)),
            cancel(order_id(1)),
            cancel(order_id(42)),
            limit(order_id(6), Side::Bid, price(95), qty(4)),
            market(order_id(7), Side::Ask, qty(6)),
            market(order_id(8), Side::Bid, qty(2)),
        ];
        let run = || {
            let mut engine = Engine::new();
            commands
                .iter()
                .map(|&command| engine.apply(command))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}
