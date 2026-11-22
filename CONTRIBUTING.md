# Contributing to hiruzen

Bug reports, fixes and new ideas are welcome.

## Reporting a problem

Run `hiruzen issue`: it opens a new GitHub issue with your version, system
and shell filled in. Or open one from the
[issue templates](https://github.com/farhanmalhi/hiruzen-cli/issues/new/choose).

For a tool that gives no suggestions or wrong ones, include the output of
`hiruzen complete "<what you typed>"`. Adding `HIRUZEN_DEBUG=1` in front
shows which commands the tool's spec ran and why its code failed.

Report a security problem privately, as described in
[SECURITY.md](SECURITY.md), not in a public issue.

## Sending a change

1. Fork the repository and create a branch for your change.
2. Make the change, with a test for it where one is practical.
3. Run the checks below; CI runs the same ones on Ubuntu, macOS and Windows.
4. Add a line under `Unreleased` in [CHANGELOG.md](CHANGELOG.md) if users
   would notice the change.
5. Open a pull request that says what the change does and why.

Completion specs for most tools come from
[@withfig/autocomplete](https://github.com/withfig/autocomplete). A wrong or
missing subcommand or option is best fixed there, so every tool that uses
the library benefits; hiruzen picks it up when its spec archive is updated.

## Development setup

```sh
cargo build                              # debug build
cargo run                                # start a session with the debug build
cargo test                               # unit tests, including parsing every embedded spec
cargo clippy --locked -- -D warnings     # lints; CI fails on any warning
```

CI builds, tests and lints every push to `main` and every pull request on
Ubuntu, macOS and Windows, so changes must pass on all three.

### Source layout

| Path | Contents |
| --- | --- |
| `src/main.rs` | Command-line parsing and the `init` and `complete` commands |
| `src/engine/` | Tokenizer, spec walker, generators (live context), history ranking |
| `src/specs.rs` | Spec model and the embedded archive reader |
| `src/term/` | Screen model, dropdown layout, prompt markers, the pseudo-terminal session |
| `src/shell.rs`, `shell/` | Per-shell launch logic and integration scripts |
| `specs/specs.bin` | The embedded spec archive (generated, committed) |
| `tools/specgen/` | Converts the Fig spec library into `specs/specs.bin` |
| `tools/helpgen/` | Builds a spec from a tool's `--help` output, for tools Fig does not cover (used for `specs/claude.json`) |
| `specs/hiruzen.json`, `specs/claude.json` | Specs embedded in addition to the Fig library |
| `claude-plugin/`, `.claude-plugin/` | The Claude Code plugin and its marketplace entry |

### Updating the specs

`specs/specs.bin` is generated from
[@withfig/autocomplete](https://github.com/withfig/autocomplete) and committed,
so building hiruzen does not need Node. To regenerate it you need Node 22.15
or newer:

```sh
cd tools/specgen
npm install
npm run build
```

To move to a newer spec library, change the `@withfig/autocomplete` version in
`tools/specgen/package.json` first. Run `cargo test` afterwards: it checks that
every spec in the new archive still parses.

### Releasing

Releases are built by [dist](https://axodotdev.github.io/cargo-dist/); its
settings are in `dist-workspace.toml`, and `.github/workflows/release.yml` is
generated from them (run `dist generate` after changing the settings).

1. Set the new `version` in `Cargo.toml` and run `cargo build` to update
   `Cargo.lock`.
2. Move the entries under `Unreleased` in `CHANGELOG.md` to a new
   `## [x.y.z] - date` heading. That section becomes the release notes.
3. Commit, then tag and push: `git tag v0.2.0 && git push origin main v0.2.0`.
4. Publish to crates.io: `cargo publish`. This needs a crates.io token
   (`cargo login`). The package carries `specs/specs.bin`, so it is about
   8 MiB of the 10 MiB crates.io allows; `cargo package --list` shows what
   goes in.

The tag starts the release workflow. It builds every target, creates the
GitHub Release with the archives, checksums and installer scripts, and pushes
the Homebrew formula to `farhanmalhi/homebrew-tap`. The tag must match the `version` in
`Cargo.toml`, or the workflow stops with nothing to release. A version with a
suffix, such as `0.2.0-rc.1` tagged `v0.2.0-rc.1`, makes a prerelease and
leaves the tap alone, which is the way to try the workflow out; it takes its
release notes from the `0.2.0` section of the changelog.

When the release workflow succeeds, the Scoop workflow
(`.github/workflows/scoop.yml`) points `bucket/hiruzen.json` at the new
release and commits it to `main`, so pull before your next change. It can
also be run by hand from the Actions tab.

The website, <https://hiruzen.farhanmalhi.com/>, is kept in a separate
repository. When a release changes behaviour the site describes (keys,
settings, what is sent over the network, supported shells), update the site
in the same week.

## License

By contributing you agree that your contribution is licensed under the
[MIT licence](LICENSE) that covers the project.
