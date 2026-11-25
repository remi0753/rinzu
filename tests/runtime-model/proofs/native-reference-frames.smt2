; Run with z3 -smt2. The first four obligations must be unsat.
; The detached-wake witness and the two weakened transitions must be sat.
(set-logic ALL)
(declare-datatype Event
  ((Retain) (Install) (Rollback) (Capture) (MergeDuplicate)
   (BeginNotify) (EndNotify) (ReturnSdk) (Detach) (ReturnRuntime)))

; One operation has a single target and at most one affected batch.
; other-leases abstracts the sum over all unmodified lease families.
(declare-const event Event)
(declare-const target Int)
(declare-const peer-target Int)
(declare-const batch Int)
(declare-const peer-batch Int)
(declare-const other-leases Int)
(declare-const runtime-before Bool)
(declare-const attached-before Bool)
(declare-const counts-before (Array Int Int))
(declare-const sdk-leases-before (Array Int Int))
(declare-const physical-users Int)
(declare-const context-readers Int)
(declare-const helper-running Bool)
(declare-const max-reference-count Int)

(define-fun candidate-before () Int
  (ite (or (= event Install) (= event Rollback)) 1 0))
(define-fun candidate-after () Int (ite (= event Retain) 1 0))
(define-fun sdk-before () Int
  (ite (= event MergeDuplicate) 2
    (ite (or (= event Capture) (= event BeginNotify)
             (= event EndNotify) (= event ReturnSdk)) 1 0)))
(define-fun sdk-after () Int
  (ite (or (= event Install) (= event Capture) (= event MergeDuplicate)
           (= event BeginNotify) (= event EndNotify)) 1 0))
(define-fun runtime-after () Bool (and runtime-before (distinct event ReturnRuntime)))
(define-fun attached-after () Bool (and attached-before (distinct event Detach)))
(define-fun context-after () Int
  (ite (not attached-before) 0
    (+ context-readers
       (ite (= event BeginNotify) 1 (ite (= event EndNotify) (- 1) 0)))))
(define-fun delta () Int
  (ite (= event Retain) 1
    (ite (or (= event Rollback) (= event MergeDuplicate)
             (= event ReturnSdk) (= event ReturnRuntime)) (- 1) 0)))

(define-fun count-before () Int (select counts-before target))
(define-fun counts-after () (Array Int Int)
  (store counts-before target (+ count-before delta)))
(define-fun sdk-leases-after () (Array Int Int)
  (store sdk-leases-before batch sdk-after))
(define-fun count-after () Int (select counts-after target))
(define-fun owned-after () Int
  (+ (ite runtime-after 1 0) candidate-after sdk-after other-leases))

(assert (distinct target peer-target))
(assert (distinct batch peer-batch))
(assert (>= other-leases 0))
(assert (>= (select sdk-leases-before peer-batch) 0))
(assert (>= other-leases (select sdk-leases-before peer-batch)))
(assert (>= physical-users 0))
(assert (>= context-readers 0))
(assert (= (select sdk-leases-before batch) sdk-before))
(assert (= count-before
  (+ (ite runtime-before 1 0) candidate-before sdk-before other-leases)))
(assert (=> attached-before runtime-before))
(assert
  (=> (or (= event Retain) (= event Install) (= event Rollback) (= event Capture))
      attached-before))
(define-fun quiescent () Bool
  (and (= physical-users 0) (= context-readers 0) (not helper-running)))
(assert (=> (not attached-before) quiescent))
(assert (=> (= event Detach) (and attached-before quiescent)))
(assert (=> (and attached-before (= event EndNotify)) (> context-readers 0)))
(assert (=> (= event ReturnRuntime) (and runtime-before (not attached-before))))
; Successful live operations have representable atomic counts.
(assert (> max-reference-count 0))
(assert (<= count-before max-reference-count))
(assert (<= count-after max-reference-count))

(echo "Reference conservation for any number of other leases")
(push)
(assert (or (not (= count-after owned-after)) (< count-after 0)))
(check-sat)
(pop)

(echo "An operation leaves other targets and batches unchanged")
(push)
(assert
  (or (not (= (select counts-after peer-target) (select counts-before peer-target)))
      (not (= (select sdk-leases-after peer-batch)
              (select sdk-leases-before peer-batch)))))
(check-sat)
(pop)

(echo "A zero target count cannot coexist with a retained lease")
(push)
(assert (= count-after 0))
(assert (or (> owned-after 0) attached-after (> context-after 0)))
(check-sat)
(pop)

(echo "Detach waits for all physical users and context readers")
(push)
(assert (not attached-after))
(assert (or (> physical-users 0) (> context-after 0) helper-running))
(check-sat)
(pop)

(echo "A detached target can still have SDK notifications and other retained leases")
(push)
(assert (= event BeginNotify))
(assert (not attached-before))
(assert (not runtime-before))
(assert (> other-leases 0))
(assert (> count-after 1))
(check-sat)
(pop)

(echo "Mutant returning a duplicate from a different batch")
(push)
(assert (= event MergeDuplicate))
(assert (> (select sdk-leases-before peer-batch) 0))
(define-fun mutant-sdk () (Array Int Int)
  (store sdk-leases-before peer-batch (- (select sdk-leases-before peer-batch) 1)))
(assert (not (= (select mutant-sdk peer-batch)
                (select sdk-leases-before peer-batch))))
(check-sat)
(pop)

(echo "Mutant double-decrement when merging a duplicate")
(push)
(assert (= event MergeDuplicate))
(assert (not (= (- count-before 2) owned-after)))
(check-sat)
(pop)
