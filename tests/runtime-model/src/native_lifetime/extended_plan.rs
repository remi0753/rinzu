use super::cohort::{Owner, append_call};
use super::{Binding, CohortMode, Decision};
use std::sync::LazyLock;
fn plan(mode: CohortMode, controlled: bool, polling: bool, binding: Binding) -> Vec<Owner> {
    let mut p = Vec::new();
    append_call(&mut p, 0, 2, false);
    p.push(Owner::Registered(0));
    if controlled {
        p.push(Owner::Control(0, 0, Decision::Cancelled));
    }
    append_call(&mut p, 1, 2, mode == CohortMode::Early);
    if mode != CohortMode::Deferred {
        p.push(Owner::Registered(1));
    }
    if controlled && mode != CohortMode::Early {
        p.extend([
            Owner::Control(1, 0, Decision::Cancelled),
            Owner::Control(1, 1, Decision::TimedOut),
        ]);
    }
    if mode != CohortMode::Early {
        p.push(Owner::Publish(1));
    }
    p.extend([Owner::Wait(1), Owner::Retire(1)]);
    if polling && (binding == Binding::CJoined || mode != CohortMode::Deferred) {
        p.push(Owner::Poll);
    }
    p.extend([Owner::Wait(0), Owner::Retire(0)]);
    if polling && binding == Binding::RustChannel && mode == CohortMode::Deferred {
        p.push(Owner::Poll);
    }
    append_call(&mut p, 2, 2, false);
    p.push(Owner::Registered(2));
    p.extend([
        Owner::Late(0, Decision::Cancelled),
        Owner::Late(1, Decision::TimedOut),
        Owner::Alias,
        Owner::Status,
        Owner::DropAliases,
        Owner::Wait(2),
        Owner::Retire(2),
        Owner::End,
    ]);
    p
}
static PLANS: LazyLock<[Vec<Owner>; 16]> = LazyLock::new(|| {
    std::array::from_fn(|i| {
        let (b, m, p) = match i / 2 {
            0 => (Binding::CDirect, CohortMode::Early, false),
            1 => (Binding::CJoined, CohortMode::Early, true),
            j => (
                Binding::RustChannel,
                [CohortMode::Early, CohortMode::Deferred, CohortMode::Mixed][(j - 2) / 2],
                (j - 2) % 2 == 1,
            ),
        };
        plan(m, i % 2 == 1, p, b)
    })
});
pub(super) fn owner(
    binding: Binding,
    mode: CohortMode,
    polling: bool,
    controlled: bool,
    pc: usize,
) -> Owner {
    let profile = match binding {
        Binding::CDirect => 0,
        Binding::CJoined => 1,
        Binding::RustChannel => {
            2 + match mode {
                CohortMode::Early => 0,
                CohortMode::Deferred => 2,
                CohortMode::Mixed => 4,
                CohortMode::Registered => unreachable!(),
            } + usize::from(polling)
        }
    };
    PLANS[profile * 2 + usize::from(controlled)][pc]
}
