use crate::{
    amount::{BaseAmount, QuoteAmount},
    error::DomainError,
    grid::{Price, Qty},
};

#[derive(Debug, Clone)]
pub struct MarketSpec {
    base_decimals: u8,  // 18 for ETH
    quote_decimals: u8, // 6 for USDC
    tick: u64,          // 0.01 USDC is 10^-2 * 10^6 = 10^4
    lot: u64,           // 0.001 ETH is 10^-3 * 10^18 = 10^15
    k: u64,             // lot * tick / 10^base_decimals
}

impl MarketSpec {
    /// # Errors
    ///
    /// Will return error if provided spec is not valid.
    pub fn new(
        base_decimals: u8,
        quote_decimals: u8,
        tick: u64,
        lot: u64,
    ) -> Result<Self, DomainError> {
        if tick == 0 {
            return Err(DomainError::ZeroTickSize);
        }
        if lot == 0 {
            return Err(DomainError::ZeroLotSize);
        }
        // 10**38 is largest integer that fits into u128
        if base_decimals > 38 {
            return Err(DomainError::DecimalsTooLarge);
        }
        let base = 10_u128;
        let power = base.pow(u32::from(base_decimals));
        let Some(product) = u128::from(tick).checked_mul(u128::from(lot)) else {
            return Err(DomainError::TickLotProductTooLarge);
        };

        #[allow(clippy::arithmetic_side_effects)]
        if product % power != 0 {
            return Err(DomainError::NonIntegralTickLot);
        }

        #[allow(clippy::arithmetic_side_effects)]
        let Ok(k) = u64::try_from(product / power) else {
            return Err(DomainError::TickLotProductTooLarge);
        };
        Ok(MarketSpec {
            base_decimals,
            quote_decimals,
            tick,
            lot,
            k,
        })
    }
    /// # Errors
    ///
    /// Returns [`DomainError::NotionalOverflow`] if price x qty is too large for the given market.
    pub fn notional(&self, price: Price, qty: Qty) -> Result<QuoteAmount, DomainError> {
        let price_ticks = u128::from(price.get());
        let qty_lots = u128::from(qty.get());
        let k = u128::from(self.k);

        // this operation is safe: u64 * u64 -> u128 will never overflow
        #[allow(clippy::arithmetic_side_effects)]
        let product = price_ticks * qty_lots;
        // this operation may overflow when price × qty × K > u128::MAX
        let notional = product
            .checked_mul(k)
            .ok_or(DomainError::NotionalOverflow)?;

        Ok(QuoteAmount::new(notional))
    }

    #[must_use]
    pub fn base_amount(&self, qty: Qty) -> BaseAmount {
        let lot_size = u128::from(self.lot);
        let qty_lots = u128::from(qty.get());

        // this operation will never overflow: u64::MAX * u64::MAX < u128::MAX
        #[allow(clippy::arithmetic_side_effects)]
        let amount = qty_lots * lot_size;

        BaseAmount::new(amount)
    }

    // GETTERS
    #[must_use]
    pub fn base_decimals(&self) -> u8 {
        self.base_decimals
    }

    #[must_use]
    pub fn quote_decimals(&self) -> u8 {
        self.quote_decimals
    }

    #[must_use]
    pub fn lot(&self) -> u64 {
        self.lot
    }

    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ETH_DECIMALS: u8 = 18;
    const USDC_DECIMALS: u8 = 6;
    const TICK_0_01_USDC: u64 = 10_u64.pow(4);
    const TICK_0_05_USDC: u64 = 5 * 10_u64.pow(4);
    const LOT_0_001_ETH: u64 = 10_u64.pow(15);
    const LOT_0_01_ETH: u64 = 10_u64.pow(16);
    const LOT_0_1_ETH: u64 = 10_u64.pow(17);
    /// Largest power of ten that fits in u64.
    const TEN_POW_19: u64 = 10_u64.pow(19);

    fn eth_usdc(tick: u64, lot: u64) -> Result<MarketSpec, DomainError> {
        MarketSpec::new(ETH_DECIMALS, USDC_DECIMALS, tick, lot)
    }

    fn price(ticks: u64) -> Price {
        Price::try_new(ticks).expect("test price must be non-zero")
    }

    fn qty(lots: u64) -> Qty {
        Qty::try_new(lots).expect("test qty must be non-zero")
    }

    /// K observed through the public API: the notional of 1 tick × 1 lot.
    fn k_of(spec: Result<MarketSpec, DomainError>) -> u128 {
        spec.expect("spec should be valid")
            .notional(price(1), qty(1))
            .expect("1 tick × 1 lot cannot overflow")
            .get()
    }

    // --- construction: valid specs and their K ---

    #[test]
    fn eth_usdc_with_0_001_lot_has_k_10() {
        assert_eq!(k_of(eth_usdc(TICK_0_01_USDC, LOT_0_001_ETH)), 10);
    }

    #[test]
    fn eth_usdc_with_0_1_lot_has_k_1000() {
        assert_eq!(k_of(eth_usdc(TICK_0_01_USDC, LOT_0_1_ETH)), 1_000);
    }

    #[test]
    fn non_power_of_ten_tick_is_accepted() {
        // 50_000 * 10^16 / 10^18 = 500
        assert_eq!(k_of(eth_usdc(TICK_0_05_USDC, LOT_0_01_ETH)), 500);
    }

    #[test]
    fn base_with_fewer_decimals_than_quote_is_accepted() {
        // USDC/ETH: tick 0.001 ETH, lot 0.001 USDC -> 10^15 * 10^3 / 10^6 = 10^12
        let spec = MarketSpec::new(USDC_DECIMALS, ETH_DECIMALS, LOT_0_001_ETH, 1_000);
        assert_eq!(k_of(spec), 10_u128.pow(12));
    }

    #[test]
    fn rejects_zero_tick() {
        assert_eq!(
            eth_usdc(0, LOT_0_001_ETH).err(),
            Some(DomainError::ZeroTickSize)
        );
    }

    #[test]
    fn rejects_zero_lot() {
        assert_eq!(
            eth_usdc(TICK_0_01_USDC, 0).err(),
            Some(DomainError::ZeroLotSize)
        );
    }

    #[test]
    fn rejects_non_integral_tick_lot() {
        // 1 atomic USDC * 1 wei / 10^18 is a fraction
        assert_eq!(eth_usdc(1, 1).err(), Some(DomainError::NonIntegralTickLot));
    }

    #[test]
    fn rejects_off_by_one_tick_that_breaks_divisibility() {
        // 10_001 * 10^15 / 10^18 = 10.001
        assert_eq!(
            eth_usdc(TICK_0_01_USDC + 1, LOT_0_001_ETH).err(),
            Some(DomainError::NonIntegralTickLot)
        );
    }

    #[test]
    fn accepts_k_equal_to_u64_max() {
        let spec = MarketSpec::new(0, USDC_DECIMALS, u64::MAX, 1);
        assert_eq!(k_of(spec), u128::from(u64::MAX));
    }

    #[test]
    fn rejects_k_above_u64_max() {
        // 10^19 * 10^19 / 10^18 = 10^20 > u64::MAX, and divisible, so only the
        // K range check can reject it.
        assert_eq!(
            eth_usdc(TEN_POW_19, TEN_POW_19).err(),
            Some(DomainError::TickLotProductTooLarge)
        );
    }

    #[test]
    fn accepts_38_decimals() {
        // 10^19 * 10^19 / 10^38 = 1
        let spec = MarketSpec::new(38, USDC_DECIMALS, TEN_POW_19, TEN_POW_19);
        assert_eq!(k_of(spec), 1);
    }

    #[test]
    fn rejects_39_decimals() {
        assert_eq!(
            MarketSpec::new(39, USDC_DECIMALS, TEN_POW_19, TEN_POW_19).err(),
            Some(DomainError::DecimalsTooLarge)
        );
    }

    #[test]
    fn rejects_max_decimals_without_panicking() {
        assert_eq!(
            MarketSpec::new(u8::MAX, USDC_DECIMALS, TICK_0_01_USDC, LOT_0_001_ETH).err(),
            Some(DomainError::DecimalsTooLarge)
        );
    }

    // --- notional ---

    #[test]
    fn notional_of_1_5_eth_at_3000_25_is_4500_375_usdc() {
        let spec = eth_usdc(TICK_0_01_USDC, LOT_0_001_ETH).unwrap();
        // 1.5 ETH = 1_500 lots, 3000.25 USDC = 300_025 ticks
        let notional = spec.notional(price(300_025), qty(1_500)).unwrap();
        assert_eq!(notional.get(), 4_500_375_000);
    }

    #[test]
    fn notional_is_independent_of_lot_size() {
        let fine = eth_usdc(TICK_0_01_USDC, LOT_0_001_ETH).unwrap();
        let coarse = eth_usdc(TICK_0_01_USDC, LOT_0_1_ETH).unwrap();
        // the same 1.5 ETH expressed on two grids
        assert_eq!(
            fine.notional(price(300_025), qty(1_500)),
            coarse.notional(price(300_025), qty(15)),
        );
    }

    #[test]
    fn notional_at_max_price_and_qty_fits_when_k_is_1() {
        // K = 1 -> notional = price × qty, which always fits in u128
        let spec = MarketSpec::new(0, USDC_DECIMALS, 1, 1).unwrap();
        let notional = spec.notional(price(u64::MAX), qty(u64::MAX)).unwrap();
        assert_eq!(notional.get(), u128::from(u64::MAX).pow(2));
    }

    #[test]
    fn notional_overflow_returns_err() {
        // (2^64 - 1)^2 × 10 > u128::MAX
        let spec = eth_usdc(TICK_0_01_USDC, LOT_0_001_ETH).unwrap();
        assert_eq!(
            spec.notional(price(u64::MAX), qty(u64::MAX)),
            Err(DomainError::NotionalOverflow)
        );
    }

    // --- base_amount ---

    #[test]
    fn base_amount_of_1_500_lots_is_1_5_eth() {
        let spec = eth_usdc(TICK_0_01_USDC, LOT_0_001_ETH).unwrap();
        assert_eq!(
            spec.base_amount(qty(1_500)).get(),
            1_500_000_000_000_000_000
        );
    }

    #[test]
    fn base_amount_is_independent_of_lot_size() {
        let fine = eth_usdc(TICK_0_01_USDC, LOT_0_001_ETH).unwrap();
        let coarse = eth_usdc(TICK_0_01_USDC, LOT_0_1_ETH).unwrap();
        // the same 1.5 ETH expressed on two grids
        assert_eq!(fine.base_amount(qty(1_500)), coarse.base_amount(qty(15)));
    }

    #[test]
    fn base_amount_at_max_qty_does_not_overflow() {
        // (2^64 - 1) × 10^17 < (2^64 - 1)^2 < 2^128
        let spec = eth_usdc(TICK_0_01_USDC, LOT_0_1_ETH).unwrap();
        assert_eq!(
            spec.base_amount(qty(u64::MAX)).get(),
            u128::from(u64::MAX) * u128::from(LOT_0_1_ETH)
        );
    }

    #[test]
    fn base_amount_at_max_qty_and_max_lot_does_not_overflow() {
        // the worst case of the proof in `base_amount`: (2^64 - 1)^2 < 2^128
        let spec = MarketSpec::new(0, USDC_DECIMALS, 1, u64::MAX).unwrap();
        assert_eq!(
            spec.base_amount(qty(u64::MAX)).get(),
            u128::from(u64::MAX).pow(2)
        );
    }
}
