use domain::{FillId, FillIdGenerator, OrderId, Price, Qty, Side};

use crate::{
    book::{
        book_side::BestFill,
        order_book::{CancelledOrder, OrderBook},
    },
    command::Command,
    event::{Event, RejectionReason},
};

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

    /// # Panics
    ///
    /// Panics after `u64::MAX` fills
    fn increment_fill_id(&mut self) -> FillId {
        self.fill_id_generator.next_id()
    }

    fn limit(&mut self, id: OrderId, qty: Qty, side: Side, price: Price) -> Vec<Event> {
        let mut events = vec![];
        let mut qty_left_option = Some(qty);
        while let Some(qty_left) = qty_left_option {
            let Some(best_price) = self.order_book.best_price(side.opposite()) else {
                // other side is empty, we are done
                break;
            };
            if !price_crosses(side, price, best_price) {
                // price is not good enough, we are done
                break;
            }
            let Some(BestFill {
                price: price_fill,
                maker,
                filled,
                remaining: _,
            }) = self.order_book.fill_best(side.opposite(), qty_left)
            else {
                unreachable!("we checked that good price exists");
            };
            let Ok(new_qty_left) = qty_left.checked_sub(filled) else {
                unreachable!("fill_best fills at most qty_left");
            };
            qty_left_option = new_qty_left;
            let fill_id = self.increment_fill_id();
            events.push(Event::Fill {
                maker,
                taker: id,
                taker_side: side,
                filled,
                price: price_fill,
                fill_id,
            });
        }
        if let Some(qty_left) = qty_left_option {
            events.push(self.rest(id, side, price, qty_left));
        }
        events
    }

    #[must_use]
    pub fn apply(&mut self, cmd: Command) -> Vec<Event> {
        match cmd {
            Command::Cancel { id } => self.cancel(id),
            Command::Limit {
                id,
                qty,
                side,
                price,
            } => self.limit(id, qty, side, price),
            Command::Market => todo!(),
        }
    }
}

#[cfg(test)]
mod tests {
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
