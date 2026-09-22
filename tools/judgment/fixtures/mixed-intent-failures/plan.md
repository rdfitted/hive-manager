# Mixed intent failure fixture

## Tasks

- [ ] T1: Count repeated declared and harvested failures once per event (inputs: file:missing/declared.rs:10-20) (outputs: file:missing/declared.rs:10-20) (acceptance: missing/harvested.rs:30-40 remains unresolved)
- [ ] T2: Keep a passing declared intent (inputs: file:src/pass.rs)
- [ ] T3: Keep a passing harvested-only intent (acceptance: src/pass.rs remains covered)
- [ ] T4: Keep an ambiguous declared intent visible (inputs: file:auth.rs)
