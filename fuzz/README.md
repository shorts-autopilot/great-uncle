# Fuzz targets

Coverage-guided fuzzing for the four parsers that read input from outside the
project: the show document, `.gdtf`, `.mvr` and the command line.

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
cargo fuzz run show_load
cargo fuzz run gdtf_import
cargo fuzz run mvr_import
cargo fuzz run command_parse
```

The corpus under `corpus/` is committed. When a target finds a panic, add its
input to the matching corpus directory and a plain unit test for it, so a stable
`cargo test` catches the regression too. Fuzzing is a release job, not a
per-pull-request gate; the property tests in `crates/engine/tests/properties.rs`
run on every test run and cover the same "never panics" rule.
