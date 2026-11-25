# Contributing to Parsley

Thanks for helping. This guide gets you from a fresh checkout to a first pull request.

## 1. Set up (Windows)

- Windows 10 or 11, x64
- [Rust](https://rustup.rs) with the `x86_64-pc-windows-msvc` target and the Visual Studio C++ build tools that rustup asks for
- Microsoft Edge WebView2 Runtime (already on most Windows 11 PCs), for the desktop window
- Node.js, only if you are packaging a release

```powershell
git clone https://github.com/Primitivelabai/parsley
cd parsley
```

## 2. Run the tests

```powershell
cargo test --locked
```

All tests should pass before you change anything. The Windows test suite includes checks for Windows-specific code. On macOS or Linux, the platform-neutral tests still run.

## 3. Try a fictional case before changing code

The Rewind and comparison commands below use fictional events so you can try your change with sample data.

```powershell
cargo run --locked --bin parsley -- demo demo.html
cargo run --locked --bin parsley -- demo-compare comparison.html
cargo run --locked --bin parsley-desktop
```

Open the `.html` file in a browser. In the desktop app, choose **Open the example**. Each output must be a new file. Parsley won't overwrite one.

## 4. Make your change

- Keep pull requests small and about one thing.
- Use synthetic data in tests and examples. Fictional apps are named `demo-*.exe`, and the fictional events live in `incidents::demo()`.
- Add or update a test for the behaviour you change.
- Run `cargo test --locked` again, and `cargo fmt` before you commit.

## 5. Open a pull request

Say what changed, how you checked it (the test or demo command you ran) and link the issue it closes.

## Never paste real logs

Raw Windows records can contain user names, PC names, folder paths, app names, serial numbers and network details. Use a fictional case. If only a real log shows the problem, cut it down to the fewest events and replace those details with placeholders such as `[user]`, `[pc-name]` and `[path]`.

## Where to start

Issues labelled [`good first issue`](https://github.com/Primitivelabai/parsley/labels/good%20first%20issue) are small, with clear acceptance criteria and a way to check the result. Found a bug? Use the [bug report form](https://github.com/Primitivelabai/parsley/issues/new?template=bug_report.yml).
