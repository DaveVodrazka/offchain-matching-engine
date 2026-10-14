#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct QuoteAmount(u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct BaseAmount(u128);

impl QuoteAmount {
    #[must_use]
    pub const fn get(self) -> u128 {
        self.0
    }

    pub(crate) const fn new(amount: u128) -> Self {
        QuoteAmount(amount)
    }
}
impl BaseAmount {
    #[must_use]
    pub const fn get(self) -> u128 {
        self.0
    }

    pub(crate) const fn new(amount: u128) -> Self {
        BaseAmount(amount)
    }
}
