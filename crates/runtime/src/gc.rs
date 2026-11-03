//! Domain-local allocation pacing. Collect only after publishing typed roots;
//! tiny heaps batch temporary values, larger heaps budget for their live graph.

pub const MIN_ALLOCATION_BUDGET: usize = 256 * 1024;

#[derive(Debug)]
pub struct GcPolicy {
    allocated: usize,
    budget: usize,
    stress: bool,
}

impl Default for GcPolicy {
    fn default() -> Self {
        Self {
            allocated: 0,
            budget: MIN_ALLOCATION_BUDGET,
            stress: std::env::var_os("RINZU_GC_STRESS").is_some_and(|value| value == "1"),
        }
    }
}

impl GcPolicy {
    pub fn enable_stress(&mut self) {
        self.stress = true;
    }

    #[must_use]
    pub const fn is_stress(&self) -> bool {
        self.stress
    }

    pub fn allocated(&mut self, bytes: usize) {
        self.allocated = self.allocated.saturating_add(bytes);
    }

    #[must_use]
    pub fn collection_due(&self) -> bool {
        self.stress || self.allocated >= self.budget
    }

    pub fn collected(&mut self, live_bytes: usize, root_bytes: usize) {
        self.budget = MIN_ALLOCATION_BUDGET.max(live_bytes.saturating_add(root_bytes));
        self.allocated = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_pressure_live_graph_roots_and_stress_are_separate() {
        let mut policy = GcPolicy {
            allocated: 0,
            budget: MIN_ALLOCATION_BUDGET,
            stress: false,
        };
        policy.allocated(MIN_ALLOCATION_BUDGET - 1);
        assert!(!policy.collection_due());
        policy.allocated(1);
        assert!(policy.collection_due());
        policy.collected(2 * MIN_ALLOCATION_BUDGET, MIN_ALLOCATION_BUDGET);
        policy.allocated(3 * MIN_ALLOCATION_BUDGET - 1);
        assert!(!policy.collection_due());
        policy.allocated(1);
        assert!(policy.collection_due());
        policy.collected(0, 0);
        assert!(!policy.collection_due());
        policy.enable_stress();
        assert!(policy.collection_due());
    }

    #[test]
    fn pressure_arithmetic_saturates_without_wrapping() {
        let mut policy = GcPolicy {
            allocated: 0,
            budget: MIN_ALLOCATION_BUDGET,
            stress: false,
        };
        policy.collected(usize::MAX, 1);
        policy.allocated(usize::MAX - 1);
        assert!(!policy.collection_due());
        policy.allocated(10);
        assert!(policy.collection_due());
    }
}
