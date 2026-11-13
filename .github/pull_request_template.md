## What this changes

<!-- A short description. Which issue does it close (Closes #…), or which TODO(guess) marker does it settle? -->

## How it was verified

<!-- The evidence (addresses in the executable, files on the disc, a reference video) and what you checked in the
game: the drop, the menu, the steps. -->

## Checklist

- [ ] This pull request targets the `release/<version>` branch, not `main`
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`
      pass
- [ ] Guesses that remain carry a `TODO(guess):` marker that says what is guessed, what is known and what would
      settle it; markers this change settles are removed
- [ ] Decisions of the port's own (PC input and windows, the launcher, mods, online play) carry a `PORT(choice):`
      marker that says the choice and what the console had instead
- [ ] No game data, extracted files or other copyrighted material in the change
- [ ] I agree that my contribution is licensed under the project's MIT License
