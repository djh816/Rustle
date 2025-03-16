Rustle is minimal, cross platform, graphical Reddit client written in Rust.

Screenshots:
![Screenshot 1](./assets/screen1.png)
![Screenshot 2](./assets/screen2.png)

Dependencies:
• Latest version of RustUp and cargo
• Linux may require libssl-dev or openssl-devel

Build Instructions:
```bash
cargo build --release
```

Optionally bundle as a macOS .app:
```bash
cargo bundle --target aarch64-apple-darwin --release
```