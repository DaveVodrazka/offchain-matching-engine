use crate::error::DomainError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Price(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Qty(u64);

impl Price {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
    /// # Errors
    ///
    /// Will return `Err` if created from 0 value.
    pub const fn try_new(value: u64) -> Result<Self, DomainError> {
        if value == 0_u64 {
            return Err(DomainError::ZeroPrice);
        }
        Ok(Self(value))
    }
}
impl Qty {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
    /// # Errors
    ///
    /// Will return `Err` if created from 0 value.
    pub const fn try_new(value: u64) -> Result<Self, DomainError> {
        if value == 0_u64 {
            return Err(DomainError::ZeroQuantity);
        }
        Ok(Self(value))
    }
    /// # Errors
    ///
    /// Will return `Err` if subtraction underflows.
    pub fn checked_sub(self, rhs: Self) -> Result<Option<Self>, DomainError> {
        match self.0.checked_sub(rhs.0) {
            Some(0) => Ok(None),
            Some(v) => Ok(Some(Qty(v))),
            None => Err(DomainError::QuantityUnderflow),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qty(lots: u64) -> Qty {
        Qty::try_new(lots).expect("test qty must be non-zero")
    }

    // --- Price construction ---

    #[test]
    fn price_rejects_zero() {
        assert_eq!(Price::try_new(0), Err(DomainError::ZeroPrice));
    }

    #[test]
    fn price_accepts_one() {
        assert_eq!(Price::try_new(1).map(Price::get), Ok(1));
    }

    #[test]
    fn price_accepts_max() {
        assert_eq!(Price::try_new(u64::MAX).map(Price::get), Ok(u64::MAX));
    }

    // --- Qty construction ---

    #[test]
    fn qty_rejects_zero() {
        assert_eq!(Qty::try_new(0), Err(DomainError::ZeroQuantity));
    }

    #[test]
    fn qty_accepts_one() {
        assert_eq!(Qty::try_new(1).map(Qty::get), Ok(1));
    }

    #[test]
    fn qty_accepts_max() {
        assert_eq!(Qty::try_new(u64::MAX).map(Qty::get), Ok(u64::MAX));
    }

    // --- Qty::checked_sub ---

    #[test]
    fn partial_fill_leaves_remainder() {
        assert_eq!(qty(10).checked_sub(qty(3)), Ok(Some(qty(7))));
    }

    #[test]
    fn fill_one_lot_short_of_full_leaves_one_lot() {
        assert_eq!(qty(10).checked_sub(qty(9)), Ok(Some(qty(1))));
    }

    #[test]
    fn exact_fill_returns_none_instead_of_zero_qty() {
        assert_eq!(qty(10).checked_sub(qty(10)), Ok(None));
    }

    #[test]
    fn overfill_by_one_lot_is_underflow() {
        assert_eq!(
            qty(10).checked_sub(qty(11)),
            Err(DomainError::QuantityUnderflow)
        );
    }

    #[test]
    fn exact_fill_at_max_returns_none() {
        assert_eq!(qty(u64::MAX).checked_sub(qty(u64::MAX)), Ok(None));
    }

    #[test]
    fn subtracting_max_from_one_is_underflow() {
        assert_eq!(
            qty(1).checked_sub(qty(u64::MAX)),
            Err(DomainError::QuantityUnderflow)
        );
    }
}
