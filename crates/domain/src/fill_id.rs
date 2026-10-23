/// Identifies one fill. Fill ids are issued by the engine in order, one per fill, so
/// settlement can recognize a fill it has already processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct FillId(u64);

impl FillId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Issues fill ids in order, starting at 1 or after a checkpoint.
#[derive(Debug)]
pub struct FillIdGenerator {
    last_issued: FillId,
}

impl FillIdGenerator {
    #[must_use]
    pub fn fresh() -> Self {
        Self {
            last_issued: FillId(0),
        }
    }

    #[must_use]
    pub fn resume_after(last_fill_id: FillId) -> Self {
        Self {
            last_issued: last_fill_id,
        }
    }

    /// # Panics
    ///
    /// Will panic when out of fill ids (u64 add overflow).
    pub fn next_id(&mut self) -> FillId {
        let current = self.last_issued.get();
        #[allow(clippy::panic)]
        let Some(next) = current.checked_add(1) else {
            panic!("FillIdGenerator ran out of ids");
        };
        let next_fill_id = FillId(next);
        self.last_issued = next_fill_id;
        next_fill_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_generator_issues_one_first() {
        assert_eq!(FillIdGenerator::fresh().next_id(), FillId::new(1));
    }

    #[test]
    fn ids_are_sequential() {
        let mut generator = FillIdGenerator::fresh();
        let ids: Vec<u64> = (0..3).map(|_| generator.next_id().get()).collect();
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn resume_continues_after_checkpoint() {
        let mut generator = FillIdGenerator::resume_after(FillId::new(41));
        assert_eq!(generator.next_id(), FillId::new(42));
    }

    #[test]
    fn resume_does_not_reissue_checkpoint() {
        let checkpoint = FillId::new(41);
        let mut generator = FillIdGenerator::resume_after(checkpoint);
        assert!(generator.next_id() > checkpoint);
    }

    #[test]
    fn issues_max_as_last_id() {
        let mut generator = FillIdGenerator::resume_after(FillId::new(u64::MAX - 1));
        assert_eq!(generator.next_id(), FillId::new(u64::MAX));
    }

    #[test]
    #[should_panic(expected = "ran out of ids")]
    fn panics_after_max_is_issued() {
        let mut generator = FillIdGenerator::resume_after(FillId::new(u64::MAX));
        let _ = generator.next_id();
    }
}
