; NLS05 for v2/v3. Fix any pinned reader and one reused carrier address.
; The source of its load ranges over writes from arbitrary generations.
(set-logic ALL)
(declare-datatype Write ((Initialize) (Register) (Prefix) (Exchange)))
(declare-const source Write)
(declare-const generation Int)
(declare-const source-generation Int)
(declare-const first-position Int)
(declare-const next-generation-position Int)
(declare-const source-position Int)
(declare-const check-start-of-pin Bool)
(declare-const check-end-of-pin Bool)
(declare-const load-prefix Bool)
(declare-const release-write Bool)
(declare-const acquire-read Bool)

; Positions describe consecutive storage lifetimes at one address.
; They are an encoding of the pin, independent of the happens-before
; positions used below for operations on a live atomic object.
(assert (< first-position next-generation-position))
(assert (=> (< source-generation generation) (< source-position first-position)))
(assert (=> (> source-generation generation)
            (>= source-position next-generation-position)))
(assert (=> (= source-generation generation)
            (and (<= first-position source-position)
                 (< source-position next-generation-position))))
(assert (=> check-start-of-pin (<= first-position source-position)))
(assert (=> check-end-of-pin (< source-position next-generation-position)))

; The initialization writes Empty and successful registration writes a
; target, whose representation differs from DONE. The diagnostic store
; writes 1 to base.done, a separate location from notification.
(define-fun done () Bool
  (ite load-prefix (= source Prefix) (= source Exchange)))
(assert done)

(declare-const payload-write Int)
(declare-const prefix-write Int)
(declare-const final-access Int)
(declare-const read-done Int)
(declare-const copy-payload Int)
(assert (< payload-write prefix-write final-access))
(assert (< read-done copy-payload))

; A same-generation release write that supplies an acquire load orders
; the reader after that write. No wake or helper join supplies an edge.
(assert (=> (and (= source-generation generation) release-write acquire-read)
            (< (ite (= source Prefix) prefix-write final-access) read-done)))

(define-fun pinned-notification () Bool
  (and check-start-of-pin check-end-of-pin (not load-prefix)))

(echo "DONE on the pinned notification slot comes from this generation's final exchange")
(push)
(assert pinned-notification)
(assert (or (distinct source-generation generation) (distinct source Exchange)))
(check-sat)
(pop)

(echo "The final carrier access happens before every admitted v3 payload copy")
(push)
(assert pinned-notification)
(assert release-write)
(assert acquire-read)
(assert (>= final-access copy-payload))
(check-sat)
(pop)

(echo "A pinned reader may acquire and copy without any notification or join")
(push)
(assert pinned-notification)
(assert release-write)
(assert acquire-read)
(check-sat)
(pop)

(echo "Mutant allowing access outside the start of the pin admits stale DONE")
(push)
(assert check-end-of-pin)
(assert (not check-start-of-pin))
(assert (not load-prefix))
(assert (< source-generation generation))
(check-sat)
(pop)

(echo "Mutant allowing access outside the end of the pin admits a replacement generation")
(push)
(assert check-start-of-pin)
(assert (not check-end-of-pin))
(assert (not load-prefix))
(assert (> source-generation generation))
(check-sat)
(pop)

(echo "Mutant treating the diagnostic prefix as DONE permits early retirement or copy")
(push)
(assert check-start-of-pin)
(assert check-end-of-pin)
(assert load-prefix)
(assert release-write)
(assert acquire-read)
(assert (< copy-payload final-access))
(check-sat)
(pop)

(echo "Mutant omitting release permits an unordered payload copy")
(push)
(assert pinned-notification)
(assert (not release-write))
(assert acquire-read)
(assert (< copy-payload payload-write))
(check-sat)
(pop)

(echo "Mutant omitting acquire permits an unordered payload copy")
(push)
(assert pinned-notification)
(assert release-write)
(assert (not acquire-read))
(assert (< copy-payload payload-write))
(check-sat)
(pop)
