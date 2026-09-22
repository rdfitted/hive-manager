# Strict path token fixture

## Tasks

- [ ] T1: Harvest authentication path (inputs: src/auth.rs)
- [ ] T2: Ignore configuration identifiers (inputs: config.max_size, settings.retry_limit, config.toml)
- [ ] T3: Reject doubled separator (inputs: file:src//pass.rs)
- [ ] T4: Reject dot component (inputs: file:src/./pass.rs)
- [ ] T5: Reject leading separator (inputs: file:/src/pass.rs)
- [ ] T6: Reject drive designator (inputs: file:C:src/pass.rs)
- [ ] T7: Preserve raw declared example (inputs: file:src/missing.rs:10-20)
