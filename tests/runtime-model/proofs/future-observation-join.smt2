; Fix any producer token and receiver owner in the Rinzu 0.5 analysis.
; "present" is receiver_population membership, and "covers" is membership
; in the owner's covers set. A missing receiver contributes no cover.
(set-logic QF_UF)
(declare-const left-present Bool)
(declare-const right-present Bool)
(declare-const left-covers Bool)
(declare-const right-covers Bool)
(declare-const left-pending Bool)
(declare-const right-pending Bool)

(assert (=> left-pending left-present))
(assert (=> right-pending right-present))

; Projection of State::join and State::join_receivers. The pending and
; receiver producer maps preserve keys from both inputs.
(define-fun joined-present () Bool (or left-present right-present))
(define-fun candidate () Bool (or left-covers right-covers))
(define-fun joined-covers () Bool
  (and candidate (=> left-present left-covers) (=> right-present right-covers)))
(define-fun joined-pending () Bool (or left-pending right-pending))

; A witness that the owner fails to cover an applicable alternative persists
; across any number of joins by induction on this disjunction.
(define-fun uncovered-alternative () Bool
  (or (and left-present (not left-covers)) (and right-present (not right-covers))))

(echo "Joining preserves exactly the represented uncovered alternatives")
(push)
(assert (distinct (and joined-present (not joined-covers)) uncovered-alternative))
(check-sat)
(pop)

(echo "An unresolved responsibility survives joining even if the other path has cover")
(push)
(assert left-pending)
(assert right-covers)
(assert (not joined-pending))
(check-sat)
(pop)

(echo "A path where the producer is absent permits coverage on the applicable path")
(push)
(assert left-present)
(assert left-covers)
(assert (not right-present))
(assert (not right-covers))
(assert joined-covers)
(check-sat)
(pop)

(echo "Mutant using the candidate union grants coverage on an uncovered path")
(push)
(assert candidate)
(assert uncovered-alternative)
(check-sat)
(pop)

(echo "Mutant intersecting pending keys loses an unresolved responsibility")
(push)
(assert left-pending)
(assert (not (and left-pending right-pending)))
(check-sat)
(pop)
