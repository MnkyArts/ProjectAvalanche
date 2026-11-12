# Contributing to Project Avalanche

Thanks for helping. Project Avalanche is a native PC port of SSX (2012): the game's formats and behaviour are
decoded from the retail Xbox 360 disc and re-implemented in Rust. The most useful contributions are:

- **Bug reports** with the steps, the build and the log ([report a bug](https://github.com/MnkyArts/ProjectAvalanche/issues/new?template=bug_report.yml)).
- **Accuracy reports**: something that behaves differently from the original game, with a reference such as a
  video of the Xbox 360 or PS3 version ([report a difference](https://github.com/MnkyArts/ProjectAvalanche/issues/new?template=accuracy.yml)).
- **Decoding a `TODO(guess)`**: the code marks every place where the port approximates instead of doing exactly
  what the game does. Each marker says what is guessed, what is known and what would settle it. Findings and
  pull requests that settle one are very welcome ([share a finding](https://github.com/MnkyArts/ProjectAvalanche/issues/new?template=decoding.yml)).

Questions and help with the setup: the [Discord server](https://discord.com/invite/XrFGQEfwdJ).

## Ground rules

- **No game data.** Never attach or commit the ISO, extracted files, the executable, dumps of it or other
  copyrighted material, not even small parts. Screenshots, short videos and addresses are fine.
- **Evidence over guesses.** Say how you know: the function or data address in the Xbox 360 executable, the file on
  the disc, a measurement on the original hardware. If a change still has to approximate, it keeps or adds a
  `TODO(guess)` marker (below).
- Be kind and constructive; the [Code of Conduct](CODE_OF_CONDUCT.md) applies everywhere in the project.

## Building and checking

Build instructions are in the [README](README.md#build-from-source). Before you open a pull request, run in
`Engine/`:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The CI's **fast check** runs these on Linux (with `cargo nextest`) and the movie decoder's sandbox. The full
Windows + Linux build, the dedicated server and the downloads run in the merge queue, once per merge; add the
`full-ci` label to a pull request to run that full matrix on the pull request itself. Draft pull requests run no
CI until they are marked ready for review.

## Code conventions

- Rust 2024, formatted by `rustfmt` (`Engine/rustfmt.toml`, width 120); clippy clean with `-D warnings`.
- Parsers read from `&[u8]` with bounds checks and return errors: no panics or unwraps on data from the disc.
- Name things after what the data is; unknown fields stay in the structure as `unk_*`.
- **Guess markers**: every guess, approximation, port choice or stand-in for undecoded game behaviour gets a comment
  at the code site, readable on its own:

  ```rust
  // TODO(guess): <what is guessed>; <what is known: addresses, names, numbers, what was tried>; settle: <what would settle it>
  ```

  A change that decodes the behaviour removes the marker.
- The published code carries no comments except these markers and the command-line help texts. Explain your change
  in the pull request's description instead; reviewers keep what matters.

## Pull requests

- Target the `release/<version>` branch (the next version), not `main`: `main` stays the last release.
- One topic per pull request; link the issue or name the marker it settles.
- Describe what you changed, how you verified it (the checks, and what you saw in the game) and what is left open.
- Pull requests are squash-merged; the title becomes the commit's subject, so make it a short sentence.

**How changes land.** The code here is published as snapshots of the project's working tree. A merged pull request
is carried into that tree by the maintainers, so it stays in every later snapshot; the snapshot commits that follow
contain it again together with the other changes of the time. The `release/<version>` branch receives the snapshots
and the merged pull requests; at release it is merged into `main` and tagged. The full matrix runs on the merge
queue and on every snapshot.

## License

By contributing you agree that your contribution is licensed under the project's [MIT License](LICENSE).
