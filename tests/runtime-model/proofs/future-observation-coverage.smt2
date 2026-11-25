; Run with z3 -smt2. The first three obligations must be unsat.
; Each weakened transition must admit an uncovered producer.
(set-logic ALL)
(declare-datatype Rule ((Deliver) (Delegate) (Invalidate)
                        (DelegateToChild) (Export) (FailedExport) (ResumeExport)
                        (UncheckedDelegate) (InterruptedWait) (RepeatAwait)))

; Fix any producer that may fail. other-route stands for at least one valid
; receiver record outside the operated route; it covers arbitrary populations.
(declare-const rule Rule)
(declare-const direct Bool)
(declare-const pending Bool)
(declare-const exported Bool)
(declare-const route Bool)
(declare-const other-route Bool)
(declare-const child-valid Bool)
(declare-const invocation-completed Bool)
(declare-const delivery Bool)
(declare-const valid-delegation Bool)
(declare-const export-succeeded Bool)
(declare-const retains-producer Bool)
(declare-const conservative-restore Bool)

(define-fun covered-before () Bool (or direct pending exported route other-route))
(assert covered-before)
(assert (=> (= rule Deliver) delivery))
(assert (=> (= rule Delegate) valid-delegation))
(assert (=> (= rule DelegateToChild)
            (and route invocation-completed child-valid)))
(assert (=> (= rule Export) (and export-succeeded retains-producer)))
(assert (=> (= rule FailedExport) (not (and export-succeeded retains-producer))))
(assert (=> (= rule UncheckedDelegate) (not valid-delegation)))
(assert (=> (= rule InterruptedWait) (not delivery)))
(assert (=> (= rule ResumeExport) exported))

(define-fun direct-after () Bool (or direct (= rule Deliver)))
(define-fun route-after () Bool
  (ite (= rule Delegate) true
    (ite (or (= rule Invalidate) (= rule DelegateToChild) (= rule Export)) false route)))
(define-fun other-route-after () Bool (or other-route (= rule DelegateToChild)))
(define-fun exported-after () Bool
  (ite (= rule ResumeExport) false (or exported (= rule Export))))
(define-fun pending-restored () Bool
  (and (= rule Invalidate) route (not direct)
       (or conservative-restore (not (or other-route exported)))))
(define-fun pending-after () Bool
  (or pending-restored (= rule ResumeExport)
      (and pending (distinct rule Deliver) (distinct rule Delegate) (distinct rule Export))))
(define-fun covered-after () Bool
  (or direct-after pending-after exported-after route-after other-route-after))

(echo "All applicable responsibility transitions preserve coverage")
(push)
(assert (not covered-after))
(check-sat)
(pop)

(echo "Definite observation is established only by actual delivery")
(push)
(assert (not direct))
(assert direct-after)
(assert (not delivery))
(check-sat)
(pop)

(echo "Repeating an await does not revive an invalidated receiver")
(push)
(assert (= rule RepeatAwait))
(assert (not route))
(assert route-after)
(check-sat)
(pop)

(echo "Mutant forgets the only revoked route")
(push)
(assert (= rule Invalidate))
(assert (not (or direct pending exported route-after other-route-after)))
(check-sat)
(pop)

(echo "Mutant transfers pending responsibility on a failing return")
(push)
; Model a failed transfer that drops the pending obligation without creating X.
(assert (= rule FailedExport))
(assert pending)
(assert (not (or direct-after exported-after route-after other-route-after)))
(check-sat)
(pop)

(echo "Mutant delegation drops pending before checking the receiver")
(push)
(assert (= rule UncheckedDelegate))
(assert pending)
(assert (not (or direct-after exported-after route-after other-route-after)))
(check-sat)
(pop)

(echo "Mutant counts an interrupted wait as direct observation")
(push)
(assert (= rule InterruptedWait))
(assert (not direct))
(define-fun mutant-direct () Bool (or direct-after (= rule InterruptedWait)))
(assert mutant-direct)
(assert (not delivery))
(check-sat)
(pop)
