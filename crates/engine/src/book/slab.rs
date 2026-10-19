use thiserror::Error;

// Slot indices are u32; this crate only supports targets where that fits in usize.
const _: () = assert!(usize::BITS >= 32);

fn to_usize(index: u32) -> usize {
    index as usize // lossless: guaranteed by the assert above
}

fn to_u32(index: usize) -> u32 {
    #[expect(
        clippy::expect_used,
        reason = "insert caps len at u32::MAX, so vec.len() always fits"
    )]
    u32::try_from(index).expect("usize -> u32 conversion failed")
}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub(crate) enum SlabError {
    #[error("slot key does not match slab state")]
    StaleSlotKey,
}

#[derive(Debug)]
struct Slot<T> {
    generation: u32,
    entry: Entry<T>,
}

#[derive(Debug)]
enum Entry<T> {
    Occupied(T),
    Vacant { next_free: Option<u32> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlotKey {
    index: u32,
    generation: u32,
}

#[derive(Debug)]
pub(crate) struct Slab<T> {
    vec: Vec<Slot<T>>,
    free_head: Option<u32>,
    len: u32,
}

impl<T> Default for Slab<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Slab<T> {
    #[must_use]
    pub(crate) fn new() -> Self {
        Slab {
            vec: vec![],
            free_head: None,
            len: 0,
        }
    }
    #[must_use]
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Slab {
            vec: Vec::with_capacity(capacity),
            free_head: None,
            len: 0,
        }
    }
    pub(crate) fn insert(&mut self, item: T) -> SlotKey {
        #[expect(clippy::panic, reason = "capacity exhaustion is fail-stop")]
        let Some(new_len) = self.len.checked_add(1) else {
            panic!("slab full: u32 index space exhausted");
        };

        if let Some(index) = self.free_head {
            let Some(slot) = self.vec.get_mut(to_usize(index)) else {
                unreachable!("free head pointing to non existent slot");
            };
            let generation = slot.generation;

            let Entry::Vacant { next_free } = slot.entry else {
                unreachable!("free head pointing to occupied slot");
            };

            slot.entry = Entry::Occupied(item);
            self.free_head = next_free;
            self.len = new_len;
            return SlotKey { index, generation };
        }

        debug_assert_eq!(
            to_usize(self.len),
            self.vec.len(),
            "free list empty, so all slots must be occupied"
        );

        let slot = Slot {
            generation: 0, // creating new slot
            entry: Entry::Occupied(item),
        };

        let index = to_u32(self.vec.len());
        self.vec.push(slot);

        self.len = new_len;

        SlotKey {
            index,
            generation: 0,
        }
    }

    pub(crate) fn remove(&mut self, key: SlotKey) -> Result<T, SlabError> {
        let slot = self
            .vec
            .get_mut(to_usize(key.index))
            .ok_or(SlabError::StaleSlotKey)?;

        if slot.generation != key.generation {
            return Err(SlabError::StaleSlotKey);
        }

        // Validate before mutating: a failed remove must leave the slab untouched.
        if matches!(slot.entry, Entry::Vacant { .. }) {
            return Err(SlabError::StaleSlotKey);
        }

        // Swap the vacant entry in and take ownership of the old one in one step.
        let old_entry = std::mem::replace(
            &mut slot.entry,
            Entry::Vacant {
                next_free: self.free_head,
            },
        );
        slot.generation = slot.generation.wrapping_add(1);
        self.free_head = Some(key.index);

        let Some(new_len) = self.len.checked_sub(1) else {
            unreachable!("len underflow");
        };
        self.len = new_len;

        let Entry::Occupied(item) = old_entry else {
            unreachable!("slot was checked to be occupied above");
        };
        Ok(item)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn len(&self) -> u32 {
        self.len
    }

    pub(crate) fn contains(&self, key: SlotKey) -> bool {
        let Some(slot) = self.vec.get(to_usize(key.index)) else {
            return false;
        };

        if slot.generation != key.generation || !matches!(slot.entry, Entry::Occupied(_)) {
            return false;
        }

        true
    }

    pub(crate) fn get(&self, key: SlotKey) -> Result<&T, SlabError> {
        let slot = self
            .vec
            .get(to_usize(key.index))
            .ok_or(SlabError::StaleSlotKey)?;

        if slot.generation != key.generation {
            return Err(SlabError::StaleSlotKey);
        }

        match &slot.entry {
            Entry::Occupied(item) => Ok(item),
            Entry::Vacant { .. } => Err(SlabError::StaleSlotKey),
        }
    }

    pub(crate) fn get_mut(&mut self, key: SlotKey) -> Result<&mut T, SlabError> {
        let slot = self
            .vec
            .get_mut(to_usize(key.index))
            .ok_or(SlabError::StaleSlotKey)?;

        if slot.generation != key.generation {
            return Err(SlabError::StaleSlotKey);
        }

        match &mut slot.entry {
            Entry::Occupied(item) => Ok(item),
            Entry::Vacant { .. } => Err(SlabError::StaleSlotKey),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forges a key. Tests only: real keys come from `insert`.
    fn key(index: u32, generation: u32) -> SlotKey {
        SlotKey { index, generation }
    }

    /// Checks every structural invariant by walking the internals.
    fn assert_invariants<T>(slab: &Slab<T>) {
        let occupied = slab
            .vec
            .iter()
            .filter(|slot| matches!(slot.entry, Entry::Occupied(_)))
            .count();
        assert_eq!(
            to_usize(slab.len),
            occupied,
            "len must count occupied slots"
        );

        // Walk the free list: every node must be vacant, in range, and visited once.
        let mut visited = vec![false; slab.vec.len()];
        let mut cursor = slab.free_head;
        while let Some(index) = cursor {
            let i = to_usize(index);
            assert!(i < slab.vec.len(), "free list points out of range");
            assert!(!visited[i], "free list has a cycle");
            visited[i] = true;
            cursor = match slab.vec[i].entry {
                Entry::Vacant { next_free } => next_free,
                Entry::Occupied(_) => panic!("free list points at occupied slot"),
            };
        }
        let vacant = slab.vec.len().saturating_sub(occupied);
        let on_free_list = visited.iter().filter(|&&v| v).count();
        assert_eq!(
            on_free_list, vacant,
            "every vacant slot must be on the free list"
        );
    }

    // --- Construction ---

    #[test]
    fn new_slab_is_empty() {
        let slab: Slab<u32> = Slab::new();
        assert!(slab.is_empty());
        assert_eq!(slab.len(), 0);
        assert_invariants(&slab);
    }

    #[test]
    fn default_slab_is_empty() {
        let slab: Slab<u32> = Slab::default();
        assert!(slab.is_empty());
        assert_invariants(&slab);
    }

    #[test]
    fn with_capacity_preallocates_without_creating_slots() {
        let slab: Slab<u32> = Slab::with_capacity(16);
        assert!(slab.is_empty());
        assert!(slab.vec.capacity() >= 16);
        assert_eq!(slab.vec.len(), 0);
    }

    // --- Insert and get ---

    #[test]
    fn inserted_value_is_readable_by_its_key() {
        let mut slab = Slab::new();
        let k = slab.insert("A");
        assert_eq!(slab.get(k), Ok(&"A"));
        assert!(slab.contains(k));
    }

    #[test]
    fn each_insert_returns_a_distinct_key() {
        let mut slab = Slab::new();
        let a = slab.insert("A");
        let b = slab.insert("B");
        let c = slab.insert("C");
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        assert_eq!(slab.get(a), Ok(&"A"));
        assert_eq!(slab.get(b), Ok(&"B"));
        assert_eq!(slab.get(c), Ok(&"C"));
    }

    #[test]
    fn insert_increments_len() {
        let mut slab = Slab::new();
        slab.insert(1);
        slab.insert(2);
        assert_eq!(slab.len(), 2);
        assert!(!slab.is_empty());
        assert_invariants(&slab);
    }

    #[test]
    fn fresh_slots_start_at_generation_zero() {
        let mut slab = Slab::new();
        assert_eq!(slab.insert("A"), key(0, 0));
        assert_eq!(slab.insert("B"), key(1, 0));
    }

    // --- get_mut ---

    #[test]
    fn get_mut_changes_are_visible_through_get() {
        let mut slab = Slab::new();
        let k = slab.insert(10);
        *slab.get_mut(k).unwrap() -= 3;
        assert_eq!(slab.get(k), Ok(&7));
    }

    // --- Remove ---

    #[test]
    fn remove_returns_ownership_of_the_value() {
        let mut slab = Slab::new();
        let k = slab.insert(String::from("order"));
        assert_eq!(slab.remove(k), Ok(String::from("order")));
    }

    #[test]
    fn remove_decrements_len() {
        let mut slab = Slab::new();
        let k = slab.insert(1);
        slab.insert(2);
        slab.remove(k).unwrap();
        assert_eq!(slab.len(), 1);
        assert_invariants(&slab);
    }

    #[test]
    fn removing_last_item_leaves_slab_empty() {
        let mut slab = Slab::new();
        let k = slab.insert(1);
        slab.remove(k).unwrap();
        assert!(slab.is_empty());
        assert_invariants(&slab);
    }

    #[test]
    fn removed_key_is_rejected_by_every_accessor() {
        let mut slab = Slab::new();
        let k = slab.insert(1);
        slab.remove(k).unwrap();
        assert!(!slab.contains(k));
        assert_eq!(slab.get(k), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.get_mut(k), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.remove(k), Err(SlabError::StaleSlotKey));
    }

    #[test]
    fn double_remove_leaves_slab_untouched() {
        let mut slab = Slab::new();
        let k = slab.insert(1);
        let other = slab.insert(2);
        slab.remove(k).unwrap();

        assert_eq!(slab.remove(k), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.len(), 1);
        assert_eq!(slab.get(other), Ok(&2));
        assert_invariants(&slab);
    }

    #[test]
    fn remove_does_not_disturb_neighbours() {
        let mut slab = Slab::new();
        let a = slab.insert("A");
        let b = slab.insert("B");
        let c = slab.insert("C");
        slab.remove(b).unwrap();
        assert_eq!(slab.get(a), Ok(&"A"));
        assert_eq!(slab.get(c), Ok(&"C"));
    }

    // --- Slot reuse ---

    #[test]
    fn freed_slot_is_reused_with_bumped_generation() {
        let mut slab = Slab::new();
        slab.insert("A");
        let b = slab.insert("B");
        slab.insert("C");
        slab.remove(b).unwrap();

        let d = slab.insert("D");
        assert_eq!(d, key(b.index, b.generation + 1));
        assert_eq!(slab.vec.len(), 3, "reuse must not grow the vec");
        assert_invariants(&slab);
    }

    #[test]
    fn free_list_reuses_slots_last_in_first_out() {
        let mut slab = Slab::new();
        let a = slab.insert("A");
        let b = slab.insert("B");
        slab.remove(a).unwrap();
        slab.remove(b).unwrap();

        assert_eq!(slab.insert("X").index, b.index);
        assert_eq!(slab.insert("Y").index, a.index);
        assert_invariants(&slab);
    }

    #[test]
    fn insert_after_free_list_drains_appends_new_slot() {
        let mut slab = Slab::new();
        let a = slab.insert("A");
        slab.remove(a).unwrap();
        slab.insert("B"); // reuses slot 0
        assert_eq!(slab.insert("C"), key(1, 0));
        assert_invariants(&slab);
    }

    // --- Stale keys (the ABA problem generations exist for) ---

    #[test]
    fn stale_key_cannot_reach_the_slots_new_occupant() {
        let mut slab = Slab::new();
        let old = slab.insert("alice");
        slab.remove(old).unwrap();
        let new = slab.insert("bob");
        assert_eq!(old.index, new.index, "precondition: same slot reused");

        assert!(!slab.contains(old));
        assert_eq!(slab.get(old), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.get_mut(old), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.remove(old), Err(SlabError::StaleSlotKey));

        assert_eq!(slab.get(new), Ok(&"bob"), "new occupant untouched");
        assert_eq!(slab.len(), 1);
        assert_invariants(&slab);
    }

    #[test]
    fn key_from_the_future_is_rejected() {
        let mut slab = Slab::new();
        let k = slab.insert(1);
        let future = key(k.index, k.generation + 1);
        assert!(!slab.contains(future));
        assert_eq!(slab.get(future), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.remove(future), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.len(), 1);
    }

    #[test]
    fn out_of_range_key_is_rejected() {
        let mut slab = Slab::new();
        slab.insert(1);
        let k = key(99, 0);
        assert!(!slab.contains(k));
        assert_eq!(slab.get(k), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.get_mut(k), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.remove(k), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.len(), 1);
    }

    #[test]
    fn key_on_empty_slab_is_rejected() {
        let mut slab: Slab<u32> = Slab::new();
        let k = key(0, 0);
        assert!(!slab.contains(k));
        assert_eq!(slab.get(k), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.remove(k), Err(SlabError::StaleSlotKey));
    }

    #[test]
    fn matching_generation_on_vacant_slot_is_rejected() {
        // Unreachable with genuine keys (remove bumps the generation), so forge one
        // to cover the vacancy check on its own.
        let mut slab = Slab::new();
        let k = slab.insert(1);
        slab.remove(k).unwrap();
        let forged = key(k.index, slab.vec[to_usize(k.index)].generation);

        assert!(!slab.contains(forged));
        assert_eq!(slab.get(forged), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.get_mut(forged), Err(SlabError::StaleSlotKey));
        assert_eq!(slab.remove(forged), Err(SlabError::StaleSlotKey));
        assert!(slab.is_empty());
        assert_invariants(&slab);
    }

    // --- Boundaries ---

    #[test]
    fn generation_wraps_instead_of_overflowing() {
        let mut slab = Slab::new();
        let k = slab.insert(1);
        slab.vec[to_usize(k.index)].generation = u32::MAX;

        slab.remove(key(k.index, u32::MAX)).unwrap();
        assert_eq!(slab.insert(2), key(k.index, 0));
        assert_invariants(&slab);
    }

    #[test]
    #[should_panic(expected = "slab full")]
    fn insert_panics_when_index_space_is_exhausted() {
        let mut slab = Slab::new();
        slab.len = u32::MAX; // Simulated: allocating 2^32 slots is not feasible.
        slab.insert(1);
    }

    // --- Model-based churn ---

    #[test]
    fn random_churn_matches_a_reference_model() {
        // Deterministic xorshift so failures reproduce.
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        let mut slab = Slab::new();
        let mut live: Vec<(SlotKey, u64)> = Vec::new();
        let mut dead: Vec<SlotKey> = Vec::new();

        for step in 0..10_000_u64 {
            let roll = next();
            if live.is_empty() || roll % 3 != 0 {
                let k = slab.insert(step);
                assert!(!live.iter().any(|(lk, _)| *lk == k), "live key reissued");
                live.push((k, step));
            } else {
                let i = usize::try_from(next() % live.len() as u64).unwrap();
                let (k, value) = live.swap_remove(i);
                assert_eq!(slab.remove(k), Ok(value));
                dead.push(k);
            }

            if step % 500 == 0 {
                assert_invariants(&slab);
                assert_eq!(slab.len() as usize, live.len());
                for (k, value) in &live {
                    assert_eq!(slab.get(*k), Ok(value));
                }
                for k in &dead {
                    assert!(!slab.contains(*k), "dead key resurrected");
                }
            }
        }
        assert_invariants(&slab);
    }
}
