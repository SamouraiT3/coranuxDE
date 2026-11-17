# Coranux

Coranux is an original desktop environment for **Arch Linux x86_64 and its
derivatives**, built around a different vision of what a desktop environment
can be. The project is still in beta: bugs may occur, and its features and
appearance are not final. The long-term goal is to make the environment fully
customizable.

Coranux is compatible with LightDM and its Wayland session. It can also be
started from a TTY.

## Build and run from Rust source

Install Rust and Cargo, then build all components from the repository root:

```sh
cargo build --release --locked --bins
```

From an active TTY, start the session:

```sh
./target/release/coranux_session
```

## Run with the downloaded package

Install the provided Arch Linux package (or a compatible package for your Arch
derivative), then start Coranux with:

```sh
coranuxDE
```

With LightDM, select the Coranux session on the login screen.

## Bugs and contributions

Bug reports help move the project forward. If you run into a problem, open an
issue with your system details, what you did, and what happened. I’ll use the
report to investigate and fix it.

Contributions are welcome: ideas, feedback, bug reports, and code contributions
can all help Coranux grow.

## License

The code is released under the [MIT License](LICENSE). You may use, modify,
and fork it, including for commercial purposes. Official Coranux releases will
remain freely available from this repository.

The Coranux name and visual identity are not covered by the code license. A
fork distributed under another name must not imply that it is an official
release. See the [name and branding guidelines](docs/TRADEMARKS.md).
